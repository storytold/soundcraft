//! Renders longer than one offline buffer holds are refused, rather than cut short and kept.

use serde_json::{Value, json};
use soundcraft_audio_io::{AudioBuffer, FileFormat};
use soundcraft_dsp::offline::MAX_OFFLINE_SAMPLES;
use soundcraft_engine::Engine;
use soundcraft_mix::MAX_RENDER_SAMPLES;
use soundcraft_model::{ChannelFormat, Clip, ClipId, Session, TrackKind};

/// A stereo track "Talk" with one clip `frames` long. Its source is one second: the commands
/// refuse on the clip's length before they render anything.
fn long_clip(frames: usize) -> (Engine, ClipId) {
    let mut s = Session::default();
    let t = s.add_track(TrackKind::Audio, ChannelFormat::Stereo, Some("Talk"));
    let buf = AudioBuffer { sample_rate: 48_000, channels: vec![vec![0.25; 48_000]; 2] };
    let src = soundcraft_engine::io::add_source(&mut s, "talk", buf, None, FileFormat::Wav);
    let id = s.new_clip_id();
    soundcraft_engine::edit::place_clip(&mut s, t, Clip::audio(id, "Talk", src, 0, 0, frames as i64));
    (Engine::new(s), id)
}

fn refused(e: &mut Engine, cmd: &str, params: &Value) {
    let before = serde_json::to_value(e.session()).unwrap();
    let err = e.execute(cmd, params).expect_err(cmd).to_string();
    assert!(err.contains("is too long to"), "{cmd}: {err}");
    assert_eq!(serde_json::to_value(e.session()).unwrap(), before, "{cmd} changed the session");
}

#[test]
fn clips_longer_than_one_render_are_refused() {
    let (mut e, id) = long_clip(MAX_RENDER_SAMPLES / 2 + 1);
    let dir = std::env::temp_dir().join(format!("soundcraft-long-renders-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    refused(&mut e, "clip.gain_render", &json!({"clips": [id.0]}));
    refused(&mut e, "clip.effects_render", &json!({"clips": [id.0]}));
    refused(&mut e, "audiosuite.process", &json!({"process": "reverse", "clips": [id.0]}));
    refused(&mut e, "track.commit", &json!({"tracks": ["Talk"]}));
    refused(&mut e, "file.export_clips", &json!({"dir": dir.to_string_lossy(), "clips": [id.0]}));
    assert!(!dir.exists(), "Export Clips started writing before it refused");
}

#[test]
fn audiosuite_refuses_results_longer_than_one_render() {
    let (mut e, id) = long_clip(MAX_OFFLINE_SAMPLES / 4 + 1);
    let fourfold = [("time_stretch", json!({"ratio": 4.0})), ("pitch_shift", json!({"semitones": 24.0})), ("varispeed", json!({"speed": 0.25}))];
    for (process, params) in fourfold {
        refused(&mut e, "audiosuite.process", &json!({"process": process, "params": params, "clips": [id.0]}));
    }
}

#[test]
fn speed_ramp_refuses_mono_clips_longer_than_one_render() {
    // A mono clip may be longer than one offline buffer, so the speed ramp must refuse it rather than cut it short.
    for (frames, selection) in [(MAX_OFFLINE_SAMPLES + 1, 0), (MAX_OFFLINE_SAMPLES * 2 / 3 + 1, 1)] {
        let mut s = Session::default();
        let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, Some("Talk"));
        let buf = AudioBuffer { sample_rate: 48_000, channels: vec![vec![0.25; 48_000]] };
        let src = soundcraft_engine::io::add_source(&mut s, "talk", buf, None, FileFormat::Wav);
        let id = s.new_clip_id();
        soundcraft_engine::edit::place_clip(&mut s, t, Clip::audio(id, "Talk", src, 0, 0, frames as i64));
        let mut e = Engine::new(s);
        refused(&mut e, "audiosuite.process", &json!({"process": "speed_ramp", "params": {"selection": selection}, "clips": [id.0]}));
    }
}

#[test]
fn signal_generator_refuses_a_selection_longer_than_one_render() {
    let (mut e, _) = long_clip(48_000);
    let end = (MAX_RENDER_SAMPLES / 2 + 1) as i64;
    e.execute("edit.select", &json!({"tracks": ["Talk"], "start": 0, "end": end})).unwrap();
    refused(&mut e, "audiosuite.process", &json!({"process": "signal_generator"}));
}
