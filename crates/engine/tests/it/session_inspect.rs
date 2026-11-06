use serde_json::{Value, json};
use soundcraft_engine::Engine;

fn inspect(e: &mut Engine) -> Value {
    e.execute("session.inspect", &json!({})).expect("session.inspect")
}

fn new_track(e: &mut Engine) -> Value {
    e.execute("track.new", &json!({})).expect("track.new")["tracks"][0].clone()
}

fn track<'a>(v: &'a Value, id: &Value) -> &'a Value {
    v["tracks"].as_array().and_then(|ts| ts.iter().find(|t| &t["id"] == id)).unwrap_or_else(|| panic!("track {id} in {v}"))
}

#[test]
fn inspect_reports_tracks_clips_mixer_and_markers() {
    let mut e = soundcraft_engine::demo::demo_engine();
    let v = inspect(&mut e);
    let tracks = v["tracks"].as_array().expect("tracks");
    assert!(!tracks.is_empty());
    for t in tracks {
        assert!(t["clips"].is_array(), "{t}");
        for key in ["volume_db", "pan", "mute", "solo", "inserts", "sends"] {
            assert!(t.get(key).is_some(), "mixer field {key} missing from {t}");
        }
    }
    assert!(tracks.iter().any(|t| !t["clips"].as_array().unwrap().is_empty()), "the demo has clips");
    assert!(v["markers"].is_array(), "{v}");
}

#[test]
fn a_fresh_session_has_no_undo_or_redo_step() {
    let mut e = Engine::default();
    let v = inspect(&mut e);
    assert_eq!(v["undo"], Value::Null, "{v}");
    assert_eq!(v["redo"], Value::Null, "{v}");
}

#[test]
fn undo_names_the_last_edit() {
    let mut e = Engine::default();
    let t = new_track(&mut e);
    e.execute("mix.volume", &json!({"track": t, "db": -6.0})).unwrap();
    let v = inspect(&mut e);
    assert_eq!(v["undo"], json!("Set Volume"), "{v}");
    assert_eq!(v["redo"], Value::Null, "{v}");
}

#[test]
fn undo_moves_the_step_label_to_redo() {
    let mut e = Engine::default();
    let t = new_track(&mut e);
    e.execute("mix.volume", &json!({"track": t, "db": -6.0})).unwrap();
    let before = inspect(&mut e);
    assert!(e.undo());
    let after = inspect(&mut e);
    assert_eq!(after["redo"], before["undo"], "{after}");
    assert_ne!(after["undo"], before["undo"], "{after}");
    assert!(e.redo());
    let again = inspect(&mut e);
    assert_eq!(again["undo"], before["undo"], "{again}");
    assert_eq!(again["redo"], Value::Null, "{again}");
}

#[test]
fn inspecting_does_not_change_the_session_or_history() {
    let mut e = soundcraft_engine::demo::demo_engine();
    let history = e.undo_history().len();
    let first = inspect(&mut e);
    let second = inspect(&mut e);
    assert_eq!(first, second);
    assert_eq!(e.undo_history().len(), history, "an inspection is not an undo step");
}

#[test]
fn comparing_inspections_shows_a_mixer_change() {
    let mut e = Engine::default();
    let t = new_track(&mut e);
    let before = inspect(&mut e);
    e.execute("mix.volume", &json!({"track": t, "db": -6.0})).unwrap();
    let after = inspect(&mut e);
    assert_ne!(track(&before, &t)["volume_db"], track(&after, &t)["volume_db"]);
    assert_eq!(track(&after, &t)["volume_db"].as_f64(), Some(-6.0));
    assert_eq!(track(&before, &t)["pan"], track(&after, &t)["pan"], "untouched fields stay equal");
}

#[test]
fn comparing_inspections_shows_a_new_marker() {
    let mut e = Engine::default();
    let before = inspect(&mut e);
    e.execute("markers.add", &json!({"name": "Verse", "at": 48_000})).unwrap();
    let after = inspect(&mut e);
    let count = |v: &Value| v["markers"].as_array().map_or(0, Vec::len);
    assert_eq!(count(&after), count(&before) + 1, "{after}");
    let m = after["markers"].as_array().unwrap().iter().find(|m| m["name"] == "Verse").expect("the new marker");
    assert_eq!(m["start"], json!(48_000), "{m}");
    assert_ne!(after["undo"], before["undo"], "the marker is the last undo step");
}

#[test]
fn comparing_inspections_shows_a_new_track() {
    let mut e = Engine::default();
    let before = inspect(&mut e);
    let t = new_track(&mut e);
    let after = inspect(&mut e);
    assert_eq!(after["tracks"].as_array().unwrap().len(), before["tracks"].as_array().unwrap().len() + 1);
    assert!(track(&after, &t)["clips"].as_array().unwrap().is_empty(), "a new track has no clips");
}
