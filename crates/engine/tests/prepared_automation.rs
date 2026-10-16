//! Prepared automation retains the existing command, snapshot and callback interpolation contracts.

use serde_json::json;
use soundcraft_audio_io::{AudioBuffer, BitDepth, EncodeOptions, FileFormat};
use soundcraft_engine::{Engine, io};
use soundcraft_mix::{MixEngine, PreparedSession, render_range};
use soundcraft_model::{AutoParam, AutomationLane, ChannelFormat, Clip, Insert, Session, SourceId, TrackId, TrackKind};
use soundcraft_time::Range;
use std::path::PathBuf;
use std::sync::Arc;

const FRAMES: usize = 257;
const AUDIO_POINTS: [(i64, f32); 3] = [(0, -6.0), (73, -18.0), (197, -3.0)];
const MASTER_POINTS: [(i64, f32); 3] = [(0, -3.0), (97, -12.0), (257, 0.0)];

fn fixture() -> (Engine, TrackId, TrackId, SourceId) {
    let mut session = Session::default();
    let audio = session.add_track(TrackKind::Audio, ChannelFormat::Stereo, Some("Prepared source"));
    let source = io::add_source(
        &mut session,
        "Binary fraction DC",
        AudioBuffer { sample_rate: 48_000, channels: vec![vec![0.5; FRAMES], vec![-0.25; FRAMES]] },
        None,
        FileFormat::Wav,
    );
    let clip = session.new_clip_id();
    let track = session.track_mut(audio).unwrap();
    track.playlist_mut().unwrap().clips.push(Clip::audio(clip, "Binary fraction DC", source, 0, 0, FRAMES as i64));
    track.mixer.pan = vec![-1.0, 1.0];
    track.mixer.volume_db = -6.0;
    let master = session.add_track(TrackKind::Master, ChannelFormat::Stereo, Some("Legacy master"));
    session.track_mut(master).unwrap().mixer.volume_db = -3.0;
    session.edit.selected_tracks = vec![audio, master];
    (Engine::new(session), audio, master, source)
}

fn write_points(engine: &mut Engine, track: TrackId, points: &[(i64, f32)]) {
    for &(at, value) in points {
        let result = engine.execute("automation.set_point", &json!({"track": track.0, "param": "volume", "at": at, "value": value})).unwrap();
        assert_eq!(result["at"], at);
        assert_eq!(result["value"].as_f64().unwrap() as f32, value);
    }
}

// Independent legacy reference: declared points, f32 dB interpolation, then interpolation
// between endpoint gains. Ordinary strips use the last included sample; the legacy Master
// uses the next-sample endpoint and the full callback denominator. No lane/mixer/DSP reader
// computes this expectation, and different schedules each have their own reference.
fn db_at(points: &[(i64, f32)], at: i64, default: f32) -> f32 {
    let Some(&(first_at, first_value)) = points.first() else { return default };
    if at <= first_at {
        return first_value;
    }
    for pair in points.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        if at < b.0 {
            return a.1 + (b.1 - a.1) * ((at - a.0) as f32 / (b.0 - a.0) as f32);
        }
    }
    points.last().unwrap().1
}

fn gain(db: f32) -> f32 {
    if db <= -144.0 { 0.0 } else { 10f32.powf(db / 20.0) }
}

fn reference(block: usize, audio: &[(i64, f32)], master: &[(i64, f32)]) -> Vec<Vec<f32>> {
    let mut left = Vec::with_capacity(FRAMES);
    let mut right = Vec::with_capacity(FRAMES);
    for start in (0..FRAMES).step_by(block) {
        let n = block.min(FRAMES - start);
        let a0 = gain(db_at(audio, start as i64, -6.0));
        let a1 = gain(db_at(audio, (start + n - 1) as i64, -6.0));
        let m0 = gain(db_at(master, start as i64, -3.0));
        let m1 = gain(db_at(master, (start + n) as i64, -3.0));
        for frame in 0..n {
            let a = if n == 1 { a0 } else { a0 + (a1 - a0) * (frame as f32 / (n - 1) as f32) };
            let m = m0 + (m1 - m0) * (frame as f32 / n as f32);
            left.push((0.5 * a) * m);
            right.push((-0.25 * a) * m);
        }
    }
    vec![left, right]
}

fn render_pair(pair: &PreparedSession<'_>, block: usize) -> Vec<Vec<f32>> {
    let mut mix = MixEngine::new(pair.source().sample_rate.as_f64() as f32, block);
    let mut output = vec![Vec::new(), Vec::new()];
    for start in (0..FRAMES).step_by(block) {
        let n = block.min(FRAMES - start);
        let mut buffer = vec![vec![0.0; n]; 2];
        mix.render_prepared(pair, start as i64, n, &mut buffer);
        for (channel, samples) in output.iter_mut().zip(buffer) {
            assert_eq!(samples.len(), n);
            channel.extend(samples);
        }
    }
    output
}

fn check_pcm(actual: &[Vec<f32>], expected: &[Vec<f32>]) {
    assert_eq!(actual.len(), 2);
    assert_eq!(expected.len(), 2);
    for (channel, expected) in actual.iter().zip(expected) {
        assert_eq!(channel.len(), FRAMES);
        assert_eq!(expected.len(), FRAMES);
        for (frame, (sample, wanted)) in channel.iter().zip(expected).enumerate() {
            assert!(sample.is_finite());
            assert!((sample - wanted).abs() <= 2e-7, "frame {frame}: {sample} != {wanted}");
        }
    }
    assert!(actual.iter().flatten().any(|sample| sample.abs() > 0.02), "reference must be audible");
}

fn check_session(session: &Session, audio: &[(i64, f32)], master: &[(i64, f32)]) {
    let document = session.to_json().unwrap();
    let pair = PreparedSession::borrowed(session);
    assert!(pair.is_indexed());
    for block in [1, 7, 64, 96] {
        let expected = reference(block, audio, master);
        check_pcm(&render_pair(&pair, block), &expected);
        check_pcm(&render_range(session, Range::new(0, FRAMES as i64), block), &reference(block.clamp(16, 16_384), audio, master));
    }
    assert_eq!(session.to_json().unwrap(), document);
}

#[test]
fn existing_point_commands_and_history_keep_ordinary_and_master_callback_laws() {
    let (mut engine, audio, master, source) = fixture();
    write_points(&mut engine, audio, &AUDIO_POINTS);
    write_points(&mut engine, master, &MASTER_POINTS);
    let original = engine.session_arc();
    let retained = PreparedSession::owned(Arc::clone(&original));
    assert!(Arc::ptr_eq(retained.owned_source().unwrap(), &original));
    assert!(retained.is_indexed());
    let buffer = Arc::clone(original.pool.get(source).unwrap());
    let history = engine.undo_history().len();
    check_session(engine.session(), &AUDIO_POINTS, &MASTER_POINTS);

    engine.execute("automation.set_point", &json!({"track": audio.0, "param": "volume", "at": 73, "value": -9.0})).unwrap();
    let changed = [(0, -6.0), (73, -9.0), (197, -3.0)];
    assert_eq!(engine.undo_history().len(), history + 1);
    assert!(!Arc::ptr_eq(&original, &engine.session_arc()));
    assert!(Arc::ptr_eq(engine.session().pool.get(source).unwrap(), &buffer));
    check_session(engine.session(), &changed, &MASTER_POINTS);
    for block in [7, 64] {
        check_pcm(&render_pair(&retained, block), &reference(block, &AUDIO_POINTS, &MASTER_POINTS));
    }

    engine.execute("edit.undo", &json!({})).unwrap();
    assert_eq!(engine.undo_history().len(), history);
    assert!(engine.can_redo());
    check_session(engine.session(), &AUDIO_POINTS, &MASTER_POINTS);
    engine.execute("edit.redo", &json!({})).unwrap();
    assert_eq!(engine.undo_history().len(), history + 1);
    assert!(!engine.can_redo());
    check_session(engine.session(), &changed, &MASTER_POINTS);
    assert_eq!(buffer.buffer.channels, vec![vec![0.5; FRAMES], vec![-0.25; FRAMES]]);
}

#[test]
fn existing_mode_commands_keep_disabled_and_first_empty_lane_reader_policy() {
    let (mut engine, audio, master, _) = fixture();
    write_points(&mut engine, audio, &AUDIO_POINTS);
    write_points(&mut engine, master, &MASTER_POINTS);
    for id in [audio, master] {
        engine.session_mut().track_mut(id).unwrap().lane_mut(&AutoParam::Volume).enabled = false;
    }
    for mode in ["read", "touch", "latch", "touch/latch", "trim", "write", "off"] {
        engine.execute("mix.automation_mode", &json!({"tracks": [audio.0, master.0], "mode": mode})).unwrap();
        let reads = !matches!(mode, "write" | "off");
        check_session(engine.session(), if reads { &AUDIO_POINTS } else { &[] }, if reads { &MASTER_POINTS } else { &[] });
    }
    engine.execute("mix.automation_mode", &json!({"tracks": [audio.0, master.0], "mode": "read"})).unwrap();
    engine.session_mut().track_mut(audio).unwrap().automation.insert(0, AutomationLane::new(AutoParam::Volume));
    check_session(engine.session(), &[], &MASTER_POINTS);
}

struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        let nonce = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let path = std::env::temp_dir().join(format!("soundcraft-prepared-engine-{}-{nonce}", std::process::id()));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn native_save_and_open_keep_curves_source_samples_and_legacy_prepared_pcm() {
    let (mut engine, audio, master, source) = fixture();
    write_points(&mut engine, audio, &AUDIO_POINTS);
    write_points(&mut engine, master, &MASTER_POINTS);
    let original_audio = engine.session().pool.get(source).unwrap().buffer.channels.clone();
    let original_curves = [engine.session().track(audio).unwrap().automation.clone(), engine.session().track(master).unwrap().automation.clone()];
    let scratch = Scratch::new();
    let path = scratch.0.join("Prepared.scraft");
    engine.execute("session.save_as", &json!({"path": path})).unwrap();
    assert!(path.is_file());
    let saved = std::fs::read(&path).unwrap();
    let mut reopened = Engine::default();
    let report = reopened.execute("session.open", &json!({"path": path})).unwrap();
    assert!(report["missing"].as_array().unwrap().is_empty());
    assert_eq!(reopened.session().track(audio).unwrap().kind, TrackKind::Audio);
    assert_eq!(reopened.session().track(master).unwrap().kind, TrackKind::Master);
    assert_eq!(reopened.session().track(audio).unwrap().automation, original_curves[0]);
    assert_eq!(reopened.session().track(master).unwrap().automation, original_curves[1]);
    assert_eq!(reopened.session().pool.get(source).unwrap().buffer.channels, original_audio);
    check_session(reopened.session(), &AUDIO_POINTS, &MASTER_POINTS);
    assert_eq!(std::fs::read(&path).unwrap(), saved);
}

#[test]
fn streamed_float32_bounce_and_two_pass_normalization_keep_the_legacy_reference() {
    let (mut engine, audio, master, _) = fixture();
    write_points(&mut engine, audio, &AUDIO_POINTS);
    write_points(&mut engine, master, &MASTER_POINTS);
    let snapshot = engine.session_arc();
    let document = engine.session().to_json().unwrap();
    let scratch = Scratch::new();
    let options = EncodeOptions { format: FileFormat::Wav, bit_depth: BitDepth::Float32, dither: false, bwf: None };
    for normalize in [false, true] {
        let path = scratch.0.join(if normalize { "Normalized.wav" } else { "Raw.wav" });
        let mut job = io::Bounce::new(Arc::clone(&snapshot), &path, Range::new(0, FRAMES as i64), &options, None, normalize, false).unwrap();
        assert_eq!(job.progress(), 0.0);
        let mut progress = vec![0.0];
        let result = loop {
            if let Some(done) = job.step(|| false).unwrap() {
                break done;
            }
            progress.push(job.progress());
            assert!(progress.len() < 10, "bounded synthetic bounce must finish");
        };
        assert!(progress.is_sorted());
        assert_eq!(*progress.last().unwrap(), 1.0);
        if normalize {
            assert_eq!(progress, vec![0.0, 0.5, 0.5, 1.0]);
        } else {
            assert_eq!(progress, vec![0.0, 1.0]);
        }
        let mut expected = reference(1024, &AUDIO_POINTS, &MASTER_POINTS);
        if normalize {
            let peak = expected.iter().flatten().fold(0.0f32, |peak, sample| peak.max(sample.abs()));
            assert!(peak > 0.02);
            let factor = 10f32.powf(-0.1 / 20.0) / peak;
            for sample in expected.iter_mut().flatten() {
                *sample *= factor;
            }
        }
        let bytes = std::fs::read(&path).unwrap();
        let (_, decoded) = soundcraft_audio_io::decode(&bytes, Some("wav")).unwrap();
        assert_eq!(decoded.sample_rate, 48_000);
        assert_eq!(result.bytes, bytes.len() as u64);
        check_pcm(&decoded.channels, &expected);
        assert!(!scratch.0.join(if normalize { "Normalized.wav.part" } else { "Raw.wav.part" }).exists());
    }
    assert!(Arc::ptr_eq(&snapshot, &engine.session_arc()));
    assert_eq!(engine.session().to_json().unwrap(), document);
}

#[test]
fn unknown_builtin_insert_keeps_deterministic_dry_fallback_without_host_discovery() {
    let (mut engine, audio, master, source) = fixture();
    write_points(&mut engine, audio, &AUDIO_POINTS);
    write_points(&mut engine, master, &MASTER_POINTS);
    let mut missing = Insert::new("soundcraft-test-unavailable-prepared-builtin");
    missing.params.insert("Exact fixture key".into(), 0.75);
    missing.set_state_bytes(b"Deterministic missing built-in state");
    assert!(soundcraft_dsp::create(&missing.plugin).is_none());
    assert!(!missing.plugin.contains(':'));
    engine.session_mut().track_mut(audio).unwrap().mixer.inserts[0] = Some(missing.clone());
    let document = engine.session().to_json().unwrap();
    let buffer = Arc::clone(engine.session().pool.get(source).unwrap());
    check_session(engine.session(), &AUDIO_POINTS, &MASTER_POINTS);
    assert_eq!(engine.session().track(audio).unwrap().mixer.inserts[0], Some(missing));
    assert_eq!(engine.session().to_json().unwrap(), document);
    assert!(Arc::ptr_eq(engine.session().pool.get(source).unwrap(), &buffer));
    assert_eq!(buffer.buffer.channels, vec![vec![0.5; FRAMES], vec![-0.25; FRAMES]]);
}
