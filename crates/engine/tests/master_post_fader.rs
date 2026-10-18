//! Master commands, Undo and native persistence with an independent nonlinear PCM reference.
use serde_json::json;
use soundcraft_audio_io::{AudioBuffer, FileFormat};
use soundcraft_engine::{Engine, io};
use soundcraft_model::{AutoParam, ChannelFormat, Clip, Session, TrackKind};
use soundcraft_time::Range;
use std::sync::Arc;

const LEN: usize = 512;
const MASTER: &str = "Limiter master";

fn fixture() -> Engine {
    let mut s = Session::default();
    let audio = s.add_track(TrackKind::Audio, ChannelFormat::Stereo, Some("Original"));
    let source = io::add_source(
        &mut s,
        "Original binary-fraction synthetic",
        AudioBuffer { sample_rate: 48_000, channels: vec![vec![0.75; LEN], vec![-0.375; LEN]] },
        None,
        FileFormat::Wav,
    );
    let clip = s.new_clip_id();
    s.track_mut(audio).unwrap().playlist_mut().unwrap().clips.push(Clip::audio(clip, "Original", source, 0, 0, LEN as i64));
    s.edit.selected_tracks = vec![audio];
    s.edit.selected_clips = vec![clip];
    let mut e = Engine::new(s);
    let created = e.execute("track.new", &json!({"name":MASTER,"kind":"master","format":"stereo"})).unwrap();
    let master = e.session().tracks.iter().find(|t| t.kind == TrackKind::Master).unwrap();
    assert_eq!(created["tracks"], json!([master.id.0]));
    assert_eq!(master.name, MASTER);
    e.execute("mix.master_insert_order", &json!({"track":MASTER,"order":"post_fader"})).unwrap();
    e.execute(
        "mix.insert",
        &json!({"track":MASTER,"slot":"A","plugin":"compressor","params":{
            "threshold":-6,"attack":0.01,"release":5,"knee":0,"makeup":0,
            "limiter":1,"lookahead":0,"sc_hpf_on":0,"mix":100
        }}),
    )
    .unwrap();
    e.mark_clean();
    e
}
fn render(e: &Engine) -> Vec<Vec<f32>> {
    let out = soundcraft_mix::render_range(e.session(), Range::new(0, LEN as i64), 37);
    assert_eq!(out.len(), 2);
    assert!(out.iter().all(|ch| ch.len() == LEN));
    assert!(out.iter().flatten().all(|x| x.is_finite()));
    assert!(out.iter().flatten().any(|x| x.abs() > 0.05));
    out
}
// No mixer/DSP helper is used to derive this expectation.
fn reference(db: f64, bypass: bool) -> Vec<Vec<f32>> {
    let gain = 10f64.powf(db / 20.0);
    let level = 0.75 * gain;
    let target = (20.0 * level.log10() + 6.0).max(0.0) * (1.0 - 1.0 / 1000.0);
    let coefficient = (-1.0f64 / (0.01 * 0.001 * 48_000.0)).exp();
    let mut reduction = 0.0;
    let mut left = vec![0.0; LEN];
    let mut right = vec![0.0; LEN];
    for (left, right) in left.iter_mut().zip(&mut right) {
        reduction = target + (reduction - target) * coefficient;
        let attenuation = if bypass { 1.0 } else { 10f64.powf(-reduction / 20.0) };
        *left = (0.75 * gain * attenuation) as f32;
        *right = (-0.375 * gain * attenuation) as f32;
    }
    vec![left, right]
}
fn check(actual: &[Vec<f32>], expected: &[Vec<f32>]) {
    assert_eq!(actual.len(), 2);
    assert_eq!(expected.len(), 2);
    for (actual, expected) in actual.iter().zip(expected) {
        assert_eq!(actual.len(), LEN);
        assert_eq!(expected.len(), LEN);
        for (n, (&a, &e)) in actual.iter().zip(expected).enumerate() {
            assert!(a.is_finite() && e.is_finite());
            assert!((a - e).abs() < 3e-6, "frame {n}: {a} vs {e}");
        }
    }
}

#[test]
fn master_fader_gesture_has_one_undo_and_rendering_preserves_engine_state() {
    let mut e = fixture();
    let before = render(&e);
    check(&before, &reference(0.0, false));
    let document = e.session().to_json().unwrap();
    let history = e.undo_history();
    e.execute_merged("mix.volume", &json!({"track":MASTER,"db":-3}), "master-fader").unwrap();
    e.execute_merged("mix.volume", &json!({"track":MASTER,"db":-6}), "master-fader").unwrap();
    e.end_merge();
    assert_eq!(e.undo_history().len(), history.len() + 1);
    assert!(e.is_dirty());
    let after = render(&e);
    check(&after, &reference(-6.0, false));
    assert!(before.iter().flatten().zip(after.iter().flatten()).any(|(a, b)| (a - b).abs() > 0.1));
    let snapshot = e.session_arc();
    let state = (e.revision, e.is_dirty(), e.undo_history(), e.journal.clone(), e.clipboard.clone(), e.path.clone());
    assert_eq!(render(&e), after);
    assert!(Arc::ptr_eq(&snapshot, &e.session_arc()));
    assert_eq!(state, (e.revision, e.is_dirty(), e.undo_history(), e.journal.clone(), e.clipboard.clone(), e.path.clone()));
    assert!(e.undo());
    assert_eq!(e.session().to_json().unwrap(), document);
    assert_eq!(render(&e), before);
    assert!(!e.is_dirty());
    assert!(e.redo());
    assert_eq!(render(&e), after);
    assert_eq!(e.session().edit.selected_clips, snapshot.edit.selected_clips);
}

#[test]
fn master_bypass_and_slot_move_commands_round_trip_document_and_pcm() {
    let mut e = fixture();
    let before = render(&e);
    let document = e.session().to_json().unwrap();
    let history = e.undo_history();
    e.execute("mix.insert_bypass", &json!({"track":MASTER,"slot":"A","value":true})).unwrap();
    let bypassed = render(&e);
    check(&bypassed, &reference(0.0, true));
    assert!(before.iter().flatten().zip(bypassed.iter().flatten()).any(|(a, b)| (a - b).abs() > 0.1));
    assert_eq!(e.undo_history().len(), history.len() + 1);
    assert!(e.undo());
    assert_eq!(e.session().to_json().unwrap(), document);
    assert_eq!(render(&e), before);
    e.execute("mix.insert_move", &json!({"track":MASTER,"from":"A","to":"J"})).unwrap();
    let master = e.session().tracks.iter().find(|t| t.kind == TrackKind::Master).unwrap();
    assert!(master.mixer.inserts[0].is_none());
    assert!(master.mixer.inserts[9].is_some());
    assert_eq!(render(&e), before);
    assert!(e.undo());
    assert_eq!(e.session().to_json().unwrap(), document);
    assert_eq!(render(&e), before);
}

#[test]
fn master_volume_lanes_and_inserts_survive_native_and_copy_reopen() {
    struct Temp(std::path::PathBuf);
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let nonce = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
    let path = std::env::temp_dir().join(format!("soundcraft-master-copy-{}-{nonce}", std::process::id()));
    std::fs::create_dir(&path).unwrap();
    let temp = Temp(path);
    let mut e = fixture();
    e.execute("mix.volume", &json!({"track":MASTER,"db":-3})).unwrap();
    for at in [0, LEN as i64 - 1] {
        e.execute("automation.set_point", &json!({"track":MASTER,"param":"volume","at":at,"value":-6})).unwrap();
    }
    let master = e.session().tracks.iter().find(|t| t.kind == TrackKind::Master).unwrap().id;
    // Retain the established reader compatibility even when the stored flag is false.
    e.session_mut().track_mut(master).unwrap().lane_mut(&AutoParam::Volume).enabled = false;
    let expected = render(&e);
    check(&expected, &reference(-6.0, false));
    let native = temp.0.join("Original/Master.scraft");
    io::save_session(&mut e, &native.to_string_lossy()).unwrap();
    let snapshot = e.session_arc();
    let state = (e.revision, e.is_dirty(), e.undo_history(), e.journal.clone(), e.clipboard.clone(), e.path.clone());
    let mut opened = Engine::default();
    assert!(io::open_session(&mut opened, &native.to_string_lossy()).unwrap().is_empty());
    assert_eq!(render(&opened), expected);
    assert_eq!(opened.session().track(master), snapshot.track(master));
    assert!(!opened.is_dirty());
    assert!(!opened.can_undo());
    let copy = temp.0.join("Native Copy/Master.scraft");
    io::save_copy(&e, &copy.to_string_lossy()).unwrap();
    assert!(io::open_session(&mut opened, &copy.to_string_lossy()).unwrap().is_empty());
    assert_eq!(render(&opened), expected);
    assert_eq!(opened.session().track(master), snapshot.track(master));
    let (bytes, _) = io::session_bundle(e.session()).unwrap();
    assert!(io::open_session_bytes(&mut opened, "Master.scraftz", &bytes).unwrap().is_empty());
    assert_eq!(render(&opened), expected);
    assert_eq!(opened.session().track(master), snapshot.track(master));
    assert_eq!(opened.session().outputs, snapshot.outputs);
    assert!(Arc::ptr_eq(&snapshot, &e.session_arc()));
    assert_eq!(state, (e.revision, e.is_dirty(), e.undo_history(), e.journal.clone(), e.clipboard.clone(), e.path.clone()));
}
