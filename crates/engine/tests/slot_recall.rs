//! Recall Inserts / Recall Sends on a slot (#639): `track.load_preset` with `only` and `from` puts
//! the preset's inserts (or sends) on one track, starting at the clicked slot, as Pro Tools does.

use serde_json::{Value, json};
use soundcraft_engine::Engine;
use soundcraft_model::{ChannelFormat, TrackId};

fn tracks(e: &mut Engine) -> (TrackId, TrackId) {
    let ids = e.execute("track.new", &json!({"count": 2})).unwrap()["tracks"].clone();
    let id = |v: &Value| TrackId(v.as_u64().unwrap());
    (id(&ids[0]), id(&ids[1]))
}

fn insert(e: &mut Engine, t: TrackId, slot: usize, plugin: &str) {
    e.execute("mix.insert", &json!({"track": t.0, "slot": slot, "plugin": plugin})).unwrap();
}

fn inserts(e: &Engine, t: TrackId) -> Vec<Option<String>> {
    e.session().track(t).unwrap().mixer.inserts.iter().map(|i| i.as_ref().map(|i| i.plugin.clone())).collect()
}

fn sends(e: &Engine, t: TrackId) -> Vec<Option<f32>> {
    e.session().track(t).unwrap().mixer.sends.iter().map(|s| s.as_ref().map(|s| s.level_db)).collect()
}

/// Saves a preset from `src` and returns its path.
fn save(e: &mut Engine, src: TrackId, name: &str) -> String {
    let path = std::env::temp_dir().join(format!("sc-slot-recall-{}-{name}.json", std::process::id())).to_string_lossy().into_owned();
    e.execute("track.save_preset", &json!({"path": path, "track": src.0})).unwrap();
    path
}

/// The preset from the Pro Tools test: A, B, empty C, D.
fn preset(e: &mut Engine, src: TrackId, name: &str) -> String {
    insert(e, src, 0, "compressor");
    insert(e, src, 1, "gain");
    insert(e, src, 3, "eq_7band");
    save(e, src, name)
}

fn s(x: &str) -> Option<String> {
    Some(x.to_string())
}

#[test]
fn inserts_shift_to_the_clicked_slot_and_keep_gaps() {
    let mut e = Engine::default();
    let (src, dst) = tracks(&mut e);
    e.execute("mix.volume", &json!({"tracks": [src.0], "db": -7.0})).unwrap();
    let path = preset(&mut e, src, "shift");
    insert(&mut e, dst, 2, "expander_gate");
    insert(&mut e, dst, 3, "de_esser");
    insert(&mut e, dst, 4, "trim");
    let volume = e.session().track(dst).unwrap().mixer.volume_db;
    e.execute("track.load_preset", &json!({"path": path, "tracks": [dst.0], "only": "inserts", "from": 2})).unwrap();
    // Preset A -> C, B -> D (replacing what was there), empty C leaves E alone, D -> F.
    assert_eq!(inserts(&e, dst), vec![None, None, s("compressor"), s("gain"), s("trim"), s("eq_7band"), None, None, None, None]);
    assert_eq!(e.session().track(dst).unwrap().mixer.volume_db, volume, "only the inserts are recalled");
    let _ = std::fs::remove_file(&path);
}

#[test]
fn inserts_past_slot_j_are_dropped() {
    let mut e = Engine::default();
    let (src, dst) = tracks(&mut e);
    let path = preset(&mut e, src, "overflow");
    e.execute("track.load_preset", &json!({"path": path, "tracks": [dst.0], "only": "inserts", "from": 7})).unwrap();
    assert_eq!(inserts(&e, dst), vec![None, None, None, None, None, None, None, s("compressor"), s("gain"), None]);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn sends_shift_and_leave_the_inserts_alone() {
    let mut e = Engine::default();
    let (src, dst) = tracks(&mut e);
    e.session_mut().add_bus("FX", ChannelFormat::Stereo);
    e.execute("mix.send", &json!({"track": src.0, "slot": 0, "bus": "FX", "level_db": -3.0})).unwrap();
    e.execute("mix.send", &json!({"track": src.0, "slot": 2, "bus": "FX", "level_db": -9.0})).unwrap();
    insert(&mut e, src, 0, "compressor");
    let path = save(&mut e, src, "sends");
    e.execute("mix.send", &json!({"track": dst.0, "slot": 1, "bus": "FX", "level_db": -1.0})).unwrap();
    e.execute("track.load_preset", &json!({"path": path, "tracks": [dst.0], "only": "sends", "from": "b"})).unwrap();
    assert_eq!(sends(&e, dst), vec![None, Some(-3.0), None, Some(-9.0), None, None, None, None, None, None]);
    assert_eq!(inserts(&e, dst), vec![None; 10]);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn a_slot_recall_is_one_undo_step() {
    let mut e = Engine::default();
    let (src, dst) = tracks(&mut e);
    let path = preset(&mut e, src, "undo");
    insert(&mut e, dst, 0, "trim");
    let before = inserts(&e, dst);
    e.execute("track.load_preset", &json!({"path": path, "tracks": [dst.0], "only": "inserts", "from": 0})).unwrap();
    assert_ne!(inserts(&e, dst), before);
    e.execute("edit.undo", &json!({})).unwrap();
    assert_eq!(inserts(&e, dst), before);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn bad_only_or_slot_is_an_error() {
    let mut e = Engine::default();
    let (src, dst) = tracks(&mut e);
    let path = preset(&mut e, src, "bad");
    assert!(e.execute("track.load_preset", &json!({"path": path, "tracks": [dst.0], "only": "clips"})).is_err());
    assert!(e.execute("track.load_preset", &json!({"path": path, "tracks": [dst.0], "only": "inserts", "from": 10})).is_err());
    assert!(e.execute("track.load_preset", &json!({"path": path, "tracks": [dst.0], "from": "c"})).is_err(), "not a full recall");
    let _ = std::fs::remove_file(&path);
}
