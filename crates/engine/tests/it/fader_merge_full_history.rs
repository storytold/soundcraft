use serde_json::json;
use soundcraft_engine::Engine;
use soundcraft_model::{ChannelFormat, TrackKind};

fn engine_with_full_history(limit: usize) -> (Engine, u64) {
    let mut e = Engine::default();
    let t = e.session_mut().add_track(TrackKind::Audio, ChannelFormat::Mono, None).0;
    e.history_limit = limit;
    for i in 0..limit + 1 {
        e.execute("mix.pan", &json!({"track": t, "pan": 0.1 * (i as f64 + 1.0)})).unwrap();
    }
    assert_eq!(e.undo_history().len(), limit);
    (e, t)
}

#[test]
fn a_fader_gesture_is_one_undo_step_when_the_history_is_full() {
    let (mut e, t) = engine_with_full_history(3);
    for db in [-1.0, -2.0, -3.0, -4.0, -5.0] {
        e.execute_merged("mix.volume", &json!({"track": t, "db": db}), "fader").unwrap();
    }
    e.end_merge();
    let volume = |e: &Engine| e.session().tracks.iter().find(|x| x.id.0 == t).map(|x| x.mixer.volume_db);
    assert_eq!(volume(&e), Some(-5.0));
    // The gesture costs one step; it must not push further real edits out of the history.
    assert_eq!(e.undo_history().len(), 3, "history: {:?}", e.undo_history());
    assert_eq!(e.undo_history().iter().filter(|l| l.as_str() == "Set Volume").count(), 1, "history: {:?}", e.undo_history());
    assert!(e.undo());
    assert_eq!(volume(&e), Some(0.0), "one undo should undo the whole fader gesture; history: {:?}", e.undo_history());
}

#[test]
fn a_zero_history_limit_keeps_no_undo_entry_for_a_gesture() {
    let mut e = Engine::default();
    let t = e.session_mut().add_track(TrackKind::Audio, ChannelFormat::Mono, None).0;
    e.history_limit = 0;
    for db in [-1.0, -2.0, -3.0] {
        e.execute_merged("mix.volume", &json!({"track": t, "db": db}), "fader").unwrap();
    }
    e.end_merge();
    assert!(e.undo_history().is_empty(), "history: {:?}", e.undo_history());
}
