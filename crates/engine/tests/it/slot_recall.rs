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

/// A selector that isn't "inserts" or "sends" is an error that leaves the session as it was, never
/// a whole-preset recall (volume, pan, instrument...).
#[test]
fn an_invalid_only_is_an_error_and_changes_nothing() {
    let mut e = Engine::default();
    let (src, dst) = tracks(&mut e);
    e.execute("mix.volume", &json!({"tracks": [src.0], "db": -7.0})).unwrap();
    let path = preset(&mut e, src, "non-string");
    let before = serde_json::to_value(e.session()).unwrap();
    for only in [json!(true), json!(false), json!(1), json!(["inserts"]), json!({"inserts": true}), json!("clips"), json!(""), json!("INSERTS")] {
        let r = e.execute("track.load_preset", &json!({"path": path, "tracks": [dst.0], "only": only}));
        assert!(r.is_err(), "`only: {only}` is refused");
        assert_eq!(serde_json::to_value(e.session()).unwrap(), before, "`only: {only}` changes nothing");
    }
    let _ = std::fs::remove_file(&path);
}

/// `only: null` and `from: null` are the same as leaving them out: the whole preset is recalled.
#[test]
fn null_only_and_from_recall_the_whole_preset() {
    let mut e = Engine::default();
    let (src, dst) = tracks(&mut e);
    e.execute("mix.volume", &json!({"tracks": [src.0], "db": -7.0})).unwrap();
    let path = preset(&mut e, src, "null");
    e.execute("track.load_preset", &json!({"path": path, "tracks": [dst.0], "only": null, "from": null})).unwrap();
    assert_eq!(e.session().track(dst).unwrap().mixer.volume_db, -7.0);
    assert_eq!(inserts(&e, dst)[..2], [s("compressor"), s("gain")]);
    let _ = std::fs::remove_file(&path);
}

/// A preset whose Vox sends A to "Room" and B to "Hall", saved in a session that has both buses.
fn preset_with_sends(name: &str) -> String {
    let mut src = Engine::default();
    src.execute("track.new", &json!({"name": "Vox"})).unwrap();
    for b in ["Room", "Hall"] {
        src.session_mut().add_bus(b, ChannelFormat::Stereo);
    }
    src.execute("mix.send", &json!({"track": "Vox", "slot": 0, "bus": "Room"})).unwrap();
    src.execute("mix.send", &json!({"track": "Vox", "slot": 1, "bus": "Hall"})).unwrap();
    let vox = src.session().track_by_name("Vox").unwrap().id;
    insert(&mut src, vox, 0, "compressor");
    save(&mut src, vox, name)
}

#[test]
fn recall_inserts_makes_no_buses() {
    let path = preset_with_sends("inserts-no-bus");
    let mut e = Engine::default();
    let (dst, _) = tracks(&mut e);
    let r = e.execute("track.load_preset", &json!({"path": path, "tracks": [dst.0], "only": "inserts"})).unwrap();
    assert!(e.session().busses.is_empty(), "only the inserts are recalled");
    assert_eq!(r["created_buses"], json!([]));
    let _ = std::fs::remove_file(&path);
}

/// Recall Sends from C: the preset's missing buses are made, and what is reported names the track's
/// slots; a send that would land past J is dropped without making its bus.
#[test]
fn recall_sends_makes_buses_for_the_sends_that_land() {
    let path = preset_with_sends("sends-bus");
    let mut e = Engine::default();
    let (dst, _) = tracks(&mut e);
    let r = e.execute("track.load_preset", &json!({"path": path, "tracks": [dst.0], "only": "sends", "from": "c"})).unwrap();
    let room = e.session().bus_by_name("Room").map(|b| b.id).expect("Room is made");
    let send_c = e.session().track(dst).unwrap().mixer.sends[2].as_ref().map(|s| s.target.clone());
    assert_eq!(send_c, Some(soundcraft_model::Route::Bus(room)));
    assert_eq!(r["created_buses"], json!(["Room", "Hall"]));
    let mut e = Engine::default();
    let (dst, _) = tracks(&mut e);
    let r = e.execute("track.load_preset", &json!({"path": path, "tracks": [dst.0], "only": "sends", "from": "j"})).unwrap();
    assert_eq!(r["created_buses"], json!(["Room"]), "B would land past J: Hall isn't made");
    assert!(e.session().bus_by_name("Hall").is_none());
    let _ = std::fs::remove_file(&path);
}

#[test]
fn a_send_left_out_by_recall_sends_is_reported_by_the_tracks_slot() {
    let path = preset_with_sends("sends-old");
    let mut v: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    v.as_object_mut().unwrap().remove("send_buses");
    std::fs::write(&path, v.to_string()).unwrap();
    let mut e = Engine::default();
    let (dst, _) = tracks(&mut e);
    e.session_mut().add_bus("Drums", ChannelFormat::Stereo);
    e.execute("mix.send", &json!({"track": dst.0, "slot": 2, "bus": "Drums"})).unwrap();
    let r = e.execute("track.load_preset", &json!({"path": path, "tracks": [dst.0], "only": "sends", "from": "c"})).unwrap();
    assert_eq!(r["skipped_sends"], json!(["c", "d"]), "preset A and B would land on C and D");
    let drums = e.session().bus_by_name("Drums").unwrap().id;
    let send_c = e.session().track(dst).unwrap().mixer.sends[2].as_ref().map(|s| s.target.clone());
    assert_eq!(send_c, Some(soundcraft_model::Route::Bus(drums)), "a send left out leaves the track's send alone, like an empty preset slot");
    let r = e.execute("track.load_preset", &json!({"path": path, "tracks": [dst.0], "only": "sends", "from": "j"})).unwrap();
    assert_eq!(r["skipped_sends"], json!(["j"]), "B would land past J and isn't reported");
    let _ = std::fs::remove_file(&path);
}
