//! `session.inspect` reports what the README promises: tracks, clips, mixer, markers and the last
//! undo and redo labels, so two inspections taken around an edit show what the edit changed.

use serde_json::{Value, json};
use soundcraft_engine::Engine;

/// The engine's current `session.inspect` reply, with default (summary) detail.
fn inspect(e: &mut Engine) -> Value {
    e.execute("session.inspect", &json!({})).expect("session.inspect")
}

/// The engine's current `session.inspect` reply with `{"detail": "full"}`.
fn inspect_full(e: &mut Engine) -> Value {
    e.execute("session.inspect", &json!({"detail": "full"})).expect("session.inspect full")
}

/// Asserts the README's promise on one reply: typed mixer fields and clips on every track, typed
/// markers, and history labels that are each a string or null.
fn assert_reports_session_state(v: &Value) {
    let tracks = v["tracks"].as_array().expect("tracks");
    assert!(!tracks.is_empty());
    for t in tracks {
        assert!(t["clips"].is_array(), "{t}");
        assert!(t["volume_db"].is_number(), "volume_db in {t}");
        let pan = t["pan"].as_array().expect("pan is one value per channel");
        assert!(!pan.is_empty() && pan.iter().all(Value::is_number), "pan in {t}");
        assert!(t["mute"].is_boolean(), "mute in {t}");
        assert!(t["solo"].is_boolean(), "solo in {t}");
        assert!(t["inserts"].is_array(), "inserts in {t}");
        assert!(t["sends"].is_array(), "sends in {t}");
    }
    assert!(tracks.iter().any(|t| !t["clips"].as_array().unwrap().is_empty()), "the demo has clips");
    let markers = v["markers"].as_array().expect("markers");
    assert!(!markers.is_empty(), "the demo has markers");
    for m in markers {
        assert!(m["id"].is_u64(), "id in {m}");
        assert!(m["number"].is_u64(), "number in {m}");
        assert!(m["name"].is_string(), "name in {m}");
        assert!(m["start"].is_u64(), "start in {m}");
        assert!(m["end"].is_u64(), "end in {m}");
    }
    for key in ["undo", "redo"] {
        let label = v.get(key).unwrap_or_else(|| panic!("{key} missing from {v}"));
        assert!(label.is_string() || label.is_null(), "{key} in {v}");
    }
}

/// The README text with line breaks and indentation folded to single spaces.
fn readme() -> String {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../README.md");
    std::fs::read_to_string(path).expect("README.md").split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Adds one track with default parameters and returns its id.
fn new_track(e: &mut Engine) -> Value {
    e.execute("track.new", &json!({})).expect("track.new")["tracks"][0].clone()
}

/// The track with this id in an inspection; panics with the whole reply when it is absent.
fn track<'a>(v: &'a Value, id: &Value) -> &'a Value {
    v["tracks"].as_array().and_then(|ts| ts.iter().find(|t| &t["id"] == id)).unwrap_or_else(|| panic!("track {id} in {v}"))
}

/// The README describes `session.inspect` as current state plus history labels, not a change report.
#[test]
fn readme_describes_inspect_as_current_state() {
    let text = readme();
    let sentence = [
        "`session.inspect` returns the session's current state (tracks, clips, mixer, markers)",
        "and the last undo and redo step labels",
    ];
    assert!(text.contains(&sentence.join(" ")), "README sentence about session.inspect");
    assert!(!text.contains("reports exactly what changed"), "the old change-report wording is back");
}

/// Every track carries its clips and mixer fields, and the session lists its markers.
#[test]
fn inspect_reports_tracks_clips_mixer_and_markers() {
    let mut e = soundcraft_engine::demo::demo_engine();
    assert_reports_session_state(&inspect(&mut e));
}

/// Full detail reports the same state, and adds each insert's parameters.
#[test]
fn full_detail_reports_the_same_state_with_insert_params() {
    let mut e = soundcraft_engine::demo::demo_engine();
    e.execute("mix.volume", &json!({"track": "Kick", "db": -6.0})).unwrap();
    let summary = inspect(&mut e);
    let full = inspect_full(&mut e);
    assert_reports_session_state(&full);
    assert_eq!(full["undo"], json!("Set Volume"), "{full}");
    assert_eq!(full["undo"], summary["undo"]);
    assert_eq!(full["redo"], summary["redo"]);
    assert_eq!(full["markers"], summary["markers"]);
    let inserts = |v: &Value| v["tracks"].as_array().unwrap().iter().flat_map(|t| t["inserts"].as_array().unwrap().clone()).collect::<Vec<_>>();
    let (s, f) = (inserts(&summary), inserts(&full));
    assert!(!f.is_empty(), "the demo has inserts");
    assert_eq!(s.len(), f.len());
    assert!(s.iter().all(|i| i["params"].is_null()), "summary leaves params out");
    assert!(f.iter().all(|i| !i["params"].is_null()), "full includes params");
}

/// With no edits yet, both history labels are null.
#[test]
fn a_fresh_session_has_no_undo_or_redo_step() {
    let mut e = Engine::default();
    let v = inspect(&mut e);
    assert_eq!(v.get("undo"), Some(&Value::Null), "{v}");
    assert_eq!(v.get("redo"), Some(&Value::Null), "{v}");
}

/// The undo label is the label of the command that ran last.
#[test]
fn undo_names_the_last_edit() {
    let mut e = Engine::default();
    let t = new_track(&mut e);
    e.execute("mix.volume", &json!({"track": t, "db": -6.0})).unwrap();
    let v = inspect(&mut e);
    assert_eq!(v["undo"], json!("Set Volume"), "{v}");
    assert_eq!(v["redo"], Value::Null, "{v}");
}

/// Undo moves the step's label to redo, and redo moves it back.
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

/// Inspecting is read-only: two calls in a row agree and add no undo step.
#[test]
fn inspecting_does_not_change_the_session_or_history() {
    let mut e = soundcraft_engine::demo::demo_engine();
    let history = e.undo_history().len();
    let first = inspect(&mut e);
    let second = inspect(&mut e);
    assert_eq!(first, second);
    assert_eq!(e.undo_history().len(), history, "an inspection is not an undo step");
}

/// A volume change shows up in that track's `volume_db` and leaves its other fields alone.
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

/// An added marker appears with its name and start, and becomes the last undo step.
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

/// A new track appears in the list, with no clips.
#[test]
fn comparing_inspections_shows_a_new_track() {
    let mut e = Engine::default();
    let before = inspect(&mut e);
    let t = new_track(&mut e);
    let after = inspect(&mut e);
    assert_eq!(after["tracks"].as_array().unwrap().len(), before["tracks"].as_array().unwrap().len() + 1);
    assert!(track(&after, &t)["clips"].as_array().unwrap().is_empty(), "a new track has no clips");
}
