use super::*;
use crate::cmd::command_specs;
use crate::{Engine, EngineError};
use serde_json::json;

/// An engine with a mono track `Vox` and a bus `Rev`, as in the report in #1440.
fn vox_and_rev() -> Engine {
    let mut e = Engine::default();
    e.execute("track.new", &json!({"count": 1, "name": "Vox", "format": "mono"})).unwrap();
    e.execute("mix.new_bus", &json!({"name": "Rev"})).unwrap();
    e
}

/// The message of a `BadParams` error for `id`; panics on any other result.
fn bad_params(r: crate::Result<Value>, id: &str) -> String {
    match r {
        Err(EngineError::BadParams(cmd, msg)) if cmd == id => msg,
        other => panic!("{id}: expected bad parameters, got {other:?}"),
    }
}

#[test]
fn doc_keys_reads_the_leading_braces() {
    assert_eq!(doc_keys("{}"), Some(vec![]));
    assert_eq!(doc_keys("{track?, slot?: 0..9|a..j, bus: name}"), Some(vec!["track", "slot", "bus"]));
    // Alternatives with `|` and `/`, and `?` on either side of the spaces.
    assert_eq!(doc_keys("{number | id | name}"), Some(vec!["number", "id", "name"]));
    assert_eq!(doc_keys("{tracks?, at?|start/end}"), Some(vec!["tracks", "at", "start", "end"]));
    assert_eq!(doc_keys("{set?|add?|scale?|min?,max?, clips?}"), Some(vec!["set", "add", "scale", "min", "max", "clips"]));
    // Values, and the brackets inside them, name no keys.
    assert_eq!(doc_keys("{mode: off|read|touch/latch, color: [r,g,b] | null}"), Some(vec!["mode", "color"]));
    assert_eq!(doc_keys("{track?, notes?: [{pitch, start?: position}], x?: 0..1 (left: 0, right: 1)}"), Some(vec!["track", "notes", "x"]));
    assert_eq!(doc_keys("{track?: video track (default: the first), at: position}"), Some(vec!["track", "at"]));
    // Text after the braces is description or a result, except a second shape after ` or `.
    assert_eq!(doc_keys("{rescan?: bool} → [{id, name}]"), Some(vec!["rescan"]));
    assert_eq!(doc_keys("{value?: bool} (toggles when omitted) — {not, keys}"), Some(vec!["value"]));
    assert_eq!(doc_keys("{shape?, tracks?} or {clip: id, shape?}"), Some(vec!["shape", "tracks", "clip"]));
    // A repeated name is listed once.
    assert_eq!(doc_keys("{name, name?, rename?}"), Some(vec!["name", "rename"]));
    // No leading group, or one that never closes.
    assert_eq!(doc_keys("track, slot"), None);
    assert_eq!(doc_keys("{track, slot: [0..9}"), None);
    assert_eq!(doc_keys("{track, slot: 0..9)}"), None);
}

#[test]
fn helpers_accept_their_paired_keys() {
    let keys = ["tracks", "start", "end"];
    for k in ["tracks", "track", "start", "end", "length"] {
        assert!(accepted("edit.cut", &keys, k), "{k}");
    }
    assert!(!accepted("edit.cut", &keys, "clip"));
    assert!(accepted("clip.rename", &["clip", "name"], "clips") && accepted("edit.mute_clips", &["clips"], "clip"));
    // `length` comes with `range_param`, which reads `start` and `end` both.
    assert!(!accepted("x.y", &["start", "at"], "length"));
    // A command that reads the documented key alone does not take the pair.
    assert!(!accepted("edit.select", &["tracks", "start", "end", "clips"], "length"));
    assert!(!accepted("edit.select", &["tracks", "start", "end", "clips"], "clip"));
    assert!(accepted("edit.select", &["tracks", "start", "end", "clips"], "track"));
    assert!(!accepted("track.rename", &["track", "name"], "tracks"));
}

#[test]
fn unpaired_entries_name_commands_that_would_take_the_pair() {
    for (key, ids) in UNPAIRED {
        for id in ids {
            let c = crate::cmd::find_command(id).unwrap_or_else(|| panic!("{id} is not a command"));
            let keys = doc_keys(c.params).unwrap();
            assert!(!keys.contains(&key), "{id} documents `{key}`");
            assert!(paired(&keys, key), "{id}: `{key}` is not paired with a key its doc names, so the entry does nothing");
            assert!(!accepted(id, &keys, key), "{id} takes `{key}`");
        }
    }
}

#[test]
fn every_command_doc_lists_its_keys() {
    for c in command_specs() {
        let keys = doc_keys(c.params).unwrap_or_else(|| panic!("{}: params doc does not start with a balanced {{…}}: {}", c.id, c.params));
        let mut params = serde_json::Map::new();
        for k in &keys {
            params.insert((*k).to_string(), Value::Null);
        }
        assert_eq!(check(c, &Value::Object(params)), Ok(()), "{} refuses a key its doc names", c.id);
    }
}

#[test]
fn every_command_refuses_an_unknown_key_before_it_runs() {
    let mut e = crate::demo::demo_engine();
    let (rev, undo, journal) = (e.revision, e.can_undo(), e.journal.len());
    for c in command_specs() {
        let msg = bad_params(e.execute(c.id, &json!({"no_such_key": 1})), c.id);
        assert!(msg.starts_with("unknown parameter `no_such_key`"), "{}: {msg}", c.id);
        assert_eq!((e.revision, e.can_undo()), (rev, undo), "{} changed the session", c.id);
    }
    assert_eq!(e.journal.len(), journal, "refused calls are not journaled");
}

#[test]
fn mix_send_refuses_db_and_names_level_db() {
    let mut e = vox_and_rev();
    let rev = e.revision;
    let msg = bad_params(e.execute("mix.send", &json!({"track": "Vox", "bus": "Rev", "db": -6})), "mix.send");
    assert_eq!(msg, "unknown parameter `db` (did you mean `level_db`?); it takes track, slot, bus, level_db, pre_fader");
    assert!(e.session().tracks[0].mixer.sends.iter().all(Option::is_none), "no send was made");
    assert_eq!(e.revision, rev);
    // The documented key, and `db` where it is documented (`mix.send_level`), still work.
    e.execute("mix.send", &json!({"track": "Vox", "bus": "Rev", "level_db": -6})).unwrap();
    assert_eq!(e.session().tracks[0].mixer.sends[0].as_ref().map(|s| s.level_db), Some(-6.0));
    e.execute("mix.send_level", &json!({"track": "Vox", "slot": 0, "db": -12})).unwrap();
    assert_eq!(e.session().tracks[0].mixer.sends[0].as_ref().map(|s| s.level_db), Some(-12.0));
}

#[test]
fn other_commands_refuse_unknown_keys_too() {
    let mut e = vox_and_rev();
    let msg = bad_params(e.execute("mix.volume", &json!({"track": "Vox", "level_db": -3})), "mix.volume");
    assert_eq!(msg, "unknown parameter `level_db` (did you mean `db`?); it takes tracks, db, delta_db");
    assert_eq!(e.session().tracks[0].mixer.volume_db, 0.0);
    let msg = bad_params(e.execute("track.new", &json!({"count": 1, "nmae": "Bass"})), "track.new");
    assert!(msg.starts_with("unknown parameter `nmae` (did you mean `name`?); it takes count, format, kind, name,"), "{msg}");
    assert_eq!(e.session().tracks.len(), 1, "no track was added");
    let msg = bad_params(e.execute("mix.clear_solos", &json!({"tracks": ["Vox"]})), "mix.clear_solos");
    assert_eq!(msg, "unknown parameter `tracks`; it takes no parameters");
    let msg = bad_params(e.execute("markers.add", &json!({"name": "Verse", "at": 0, "colour": [1, 2, 3], "rular": 2})), "markers.add");
    assert!(msg.starts_with("unknown parameters `colour`, `rular`; it takes name, at, start, end, kind, ruler"), "{msg}");
    assert!(e.session().markers.is_empty());
    // A view command is refused the same way.
    let zoom = e.session().edit.zoom;
    bad_params(e.execute("view.waveform_zoom", &json!({"zoom": 4})), "view.waveform_zoom");
    assert_eq!(e.session().edit.zoom, zoom);
}

#[test]
fn documented_and_paired_keys_still_work() {
    let mut e = vox_and_rev();
    // `track` for a `{tracks?}` doc, and `delta_db`, the second alternative.
    e.execute("mix.volume", &json!({"track": "Vox", "delta_db": -3})).unwrap();
    assert_eq!(e.session().tracks[0].mixer.volume_db, -3.0);
    // `length` beside `start` for a `{tracks?, start?, end?}` doc read through `range_param`…
    e.execute("edit.copy", &json!({"tracks": ["Vox"], "start": 100, "length": 50})).unwrap();
    assert_eq!(e.clipboard.length, 50);
    // …but not where the command reads `start` and `end` itself.
    let msg = bad_params(e.execute("edit.select", &json!({"tracks": ["Vox"], "start": 100, "length": 50})), "edit.select");
    assert!(msg.starts_with("unknown parameter `length`; it takes tracks, start, end, clips, sources, exact"), "{msg}");
    // Keys the code always read and the docs now name.
    e.execute("markers.add", &json!({"name": "Verse", "at": 480})).unwrap();
    e.execute("markers.edit", &json!({"name": "Verse", "rename": "Chorus"})).unwrap();
    e.execute("markers.recall", &json!({"name": "Chorus"})).unwrap();
    assert_eq!(e.session().markers[0].name, "Chorus");
    e.execute("edit.clear_all_automation", &json!({"tracks": ["Vox"], "start": 0, "end": 10})).unwrap();
    // Params that are not an object are left to the command, as before.
    e.execute("mix.clear_solos", &Value::Null).unwrap();
}

#[test]
fn many_or_long_unknown_keys_keep_the_error_short() {
    let mut e = Engine::default();
    let mut params = serde_json::Map::new();
    for i in 0..20 {
        params.insert(format!("k{i:02}"), json!(1));
    }
    let msg = bad_params(e.execute("mix.clear_solos", &Value::Object(params)), "mix.clear_solos");
    assert_eq!(msg, "unknown parameters `k00`, `k01`, `k02`, `k03`, `k04`, `k05`, `k06`, `k07` and 12 more; it takes no parameters");
    let long = "level_db".repeat(1000);
    let msg = bad_params(e.execute("mix.send", &json!({ long.clone(): 1 })), "mix.send");
    assert_eq!(msg, format!("unknown parameter `{}…`; it takes track, slot, bus, level_db, pre_fader", &long[..64]));
}

#[test]
fn suggestions_prefer_containment_then_small_edits() {
    let keys = ["track", "slot", "bus", "level_db", "pre_fader"];
    assert_eq!(suggestion(&keys, "db"), Some("level_db"));
    assert_eq!(suggestion(&keys, "prefader"), Some("pre_fader"));
    assert_eq!(suggestion(&keys, "buss"), Some("bus"));
    assert_eq!(suggestion(&keys, "volume"), None);
    assert_eq!(suggestion(&keys, "x"), None, "one letter suggests nothing");
    assert_eq!(edit_distance("kitten", "sitting"), 3);
    assert_eq!(edit_distance("", "abc"), 3);
}
