//! A track preset's sends find their bus by name when recalled, since bus ids are per session. As
//! in Pro Tools, a bus the session doesn't have is made with the preset's name and format.

use serde_json::{Value, json};
use soundcraft_engine::Engine;
use soundcraft_model::{ChannelFormat, Route};

fn preset_path(name: &str) -> String {
    std::env::temp_dir().join(format!("sc-preset-buses-{name}-{}.json", std::process::id())).to_string_lossy().into_owned()
}

/// A session where "Vox" sends A to `bus` (made after other ids were taken), saved as a preset.
fn save_vox_sending_to(bus: &str, format: ChannelFormat, name: &str) -> String {
    let mut e = Engine::default();
    // Tracks and buses share one id counter: these give the saved bus an id that differs from the
    // same-named bus in `session_with`, so the tests see whether recall matches by name.
    e.execute("track.new", &json!({"count": 3})).unwrap();
    e.execute("track.new", &json!({"name": "Vox"})).unwrap();
    e.session_mut().add_bus(bus, format);
    e.execute("mix.send", &json!({"track": "Vox", "slot": 0, "bus": bus, "level_db": -6.0})).unwrap();
    let path = preset_path(name);
    e.execute("track.save_preset", &json!({"path": path, "track": "Vox"})).unwrap();
    path
}

/// A fresh session with one track, "Lead", and the given buses.
fn session_with(buses: &[&str]) -> Engine {
    let mut e = Engine::default();
    for b in buses {
        e.session_mut().add_bus(b, ChannelFormat::Stereo);
    }
    e.execute("track.new", &json!({"name": "Lead"})).unwrap();
    e
}

fn send_a(e: &Engine) -> Option<(Route, f32)> {
    e.session().track_by_name("Lead").unwrap().mixer.sends[0].as_ref().map(|s| (s.target.clone(), s.level_db))
}

#[test]
fn a_send_is_recalled_onto_the_bus_of_the_same_name() {
    let path = save_vox_sending_to("Verb", ChannelFormat::Stereo, "same-name");
    let mut e = session_with(&["Drums", "Verb"]);
    let verb = e.session().bus_by_name("Verb").unwrap().id;
    let r = e.execute("track.load_preset", &json!({"path": path, "tracks": ["Lead"]})).unwrap();
    assert_eq!(send_a(&e), Some((Route::Bus(verb), -6.0)));
    assert_eq!(e.session().busses.len(), 2, "no bus is made");
    assert_eq!((&r["created_buses"], &r["skipped_sends"]), (&json!([]), &json!([])));
    let _ = std::fs::remove_file(&path);
}

#[test]
fn a_missing_bus_is_made_with_the_presets_name_and_format_in_the_same_undo_step() {
    let path = save_vox_sending_to("Vocal Verb", ChannelFormat::Mono, "missing");
    let mut e = session_with(&["Verb"]);
    let r = e.execute("track.load_preset", &json!({"path": path, "tracks": ["Lead"]})).unwrap();
    let bus = e.session().bus_by_name("Vocal Verb").cloned().expect("the bus is made");
    assert_eq!(bus.format, ChannelFormat::Mono);
    assert_eq!(send_a(&e), Some((Route::Bus(bus.id), -6.0)));
    assert_eq!(r["created_buses"], json!(["Vocal Verb"]));
    e.execute("edit.undo", &json!({})).unwrap();
    assert!(e.session().bus_by_name("Vocal Verb").is_none() && send_a(&e).is_none(), "one Undo takes back the bus and the send");
    let _ = std::fs::remove_file(&path);
}

#[test]
fn a_preset_saved_without_bus_names_leaves_its_bus_sends_out() {
    let path = save_vox_sending_to("Verb", ChannelFormat::Stereo, "old");
    let mut v: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    v.as_object_mut().unwrap().remove("send_buses");
    std::fs::write(&path, v.to_string()).unwrap();
    let mut e = session_with(&["Verb"]);
    let r = e.execute("track.load_preset", &json!({"path": path, "tracks": ["Lead"]})).unwrap();
    assert_eq!(send_a(&e), None, "its bus id means nothing in another session");
    assert_eq!(r["skipped_sends"], json!(["a"]));
    let _ = std::fs::remove_file(&path);
}

/// With one send whose bus is there and one whose bus isn't, each slot gets its own bus, and two
/// sends to the same missing bus make it once.
#[test]
fn found_and_missing_buses_in_one_preset_are_matched_slot_by_slot() {
    let mut src = Engine::default();
    src.execute("track.new", &json!({"count": 3})).unwrap();
    src.execute("track.new", &json!({"name": "Vox"})).unwrap();
    for b in ["Room", "Verb"] {
        src.session_mut().add_bus(b, ChannelFormat::Stereo);
    }
    for (slot, bus) in [(0, "Room"), (1, "Room"), (2, "Verb")] {
        src.execute("mix.send", &json!({"track": "Vox", "slot": slot, "bus": bus})).unwrap();
    }
    let path = preset_path("mixed");
    src.execute("track.save_preset", &json!({"path": path, "track": "Vox"})).unwrap();
    let mut e = session_with(&["Verb"]);
    let r = e.execute("track.load_preset", &json!({"path": path, "tracks": ["Lead"]})).unwrap();
    let (room, verb) = (e.session().bus_by_name("Room").unwrap().id, e.session().bus_by_name("Verb").unwrap().id);
    let targets: Vec<Option<Route>> =
        e.session().track_by_name("Lead").unwrap().mixer.sends[..3].iter().map(|s| s.as_ref().map(|s| s.target.clone())).collect();
    assert_eq!(targets, [Some(Route::Bus(room)), Some(Route::Bus(room)), Some(Route::Bus(verb))]);
    assert_eq!(r["created_buses"], json!(["Room"]));
    assert_eq!(e.session().busses.len(), 2);
    let _ = std::fs::remove_file(&path);
}
