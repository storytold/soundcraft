//! #492: undo is for edits. Listening, recording and view settings aren't undo steps, and undoing
//! an edit leaves them as they are.

use serde_json::{Value, json};
use soundcraft_engine::Engine;
use soundcraft_engine::cmd::find_command;
use soundcraft_model::{AutomationMode, Track, TrackHeight};

fn run(e: &mut Engine, id: &str, p: Value) {
    e.execute(id, &p).unwrap();
}

fn vox(e: &Engine) -> &Track {
    &e.session().tracks[0]
}

fn engine() -> Engine {
    let mut e = Engine::default();
    run(&mut e, "track.new", json!({"name": "Vox", "format": "mono"}));
    e
}

/// Renames Vox to Lead (an edit), changes `settings`, then undoes and redoes the rename. `check`
/// holds the settings throughout.
fn undo_and_redo_the_edit(e: &mut Engine, settings: &[(&str, Value)], check: impl Fn(&Engine)) {
    run(e, "track.rename", json!({"track": "Vox", "name": "Lead"}));
    for (id, p) in settings {
        run(e, id, p.clone());
    }
    check(e);
    run(e, "edit.undo", json!({}));
    assert_eq!(vox(e).name, "Vox", "Undo took back the edit");
    check(e);
    run(e, "edit.redo", json!({}));
    assert_eq!(vox(e).name, "Lead");
    check(e);
}

#[test]
fn listening_recording_and_view_settings_are_not_undo_steps() {
    for id in [
        "mix.mute",
        "mix.solo",
        "mix.solo_safe",
        "mix.clear_solos",
        "mix.clear_mutes",
        "mix.record_arm",
        "mix.input_monitor",
        "mix.automation_mode",
        "mix.insert_bypass",
        "track.set_record_to_input_only",
        "track.height",
        "track.height_up",
        "track.height_down",
        "track.view",
        "track.hide",
        "track.group_toggle",
        "track.bypass_inserts",
        "track.bypass_inserts_ae",
        "track.bypass_inserts_fj",
        "track.bypass_eq",
        "track.bypass_dynamics",
        "track.bypass_reverb",
        "track.bypass_delay",
        "track.bypass_modulation",
    ] {
        assert!(!find_command(id).unwrap().undoable, "{id}");
    }
    for id in ["track.pin", "track.color"] {
        assert!(find_command(id).unwrap().undoable, "{id} stays an undo step, as in Pro Tools");
    }
    // A width change resets the panners, so it's an edit Undo must be able to take back.
    assert!(find_command("track.change_width").unwrap().undoable);
}

#[test]
fn undoing_an_edit_keeps_solo_mute_record_and_the_view() {
    let mut e = engine();
    // The edit renames Vox to Lead.
    let lead = json!({"tracks": ["Lead"]});
    let settings = [
        ("mix.mute", lead.clone()),
        ("mix.solo", lead.clone()),
        ("mix.solo_safe", lead.clone()),
        ("mix.record_arm", lead.clone()),
        ("mix.input_monitor", lead.clone()),
        ("mix.automation_mode", json!({"tracks": ["Lead"], "mode": "latch"})),
        ("track.height", json!({"tracks": ["Lead"], "height": "large"})),
        ("track.view", json!({"tracks": ["Lead"], "view": "volume"})),
        ("track.hide", lead),
    ];
    undo_and_redo_the_edit(&mut e, &settings, |e| {
        let (t, m) = (vox(e), &vox(e).mixer);
        assert!(m.mute && m.solo && m.solo_safe && m.record_arm && m.input_monitor);
        assert_eq!(m.automation_mode, AutomationMode::Latch);
        assert_eq!((t.height, t.view.as_str(), t.hidden), (TrackHeight::Large, "volume", true));
    });
    assert_eq!(e.undo_history(), vec!["New...", "Rename Track"], "only the edits are in the history");
}

#[test]
fn undoing_an_edit_keeps_an_insert_bypassed() {
    let plugin = soundcraft_dsp::plugins().iter().find(|p| !p.is_instrument).unwrap().id;
    let mut e = engine();
    run(&mut e, "mix.insert", json!({"track": "Vox", "slot": 0, "plugin": plugin}));
    let bypassed = |e: &Engine| vox(e).mixer.inserts[0].as_ref().map(|i| i.bypass);
    undo_and_redo_the_edit(&mut e, &[("mix.insert_bypass", json!({"track": "Lead", "slot": 0}))], |e| {
        assert_eq!(bypassed(e), Some(true));
    });
    assert!(e.undo() && e.undo(), "undo the rename, then the insert");
    assert_eq!(bypassed(&e), None, "undoing the insert empties the slot");
}

#[test]
fn undoing_an_edit_keeps_a_group_inactive() {
    let mut e = engine();
    run(&mut e, "track.group", json!({"tracks": ["Vox"], "name": "Vocals"}));
    undo_and_redo_the_edit(&mut e, &[("track.group_toggle", json!({"group": "Vocals"}))], |e| {
        assert!(!e.session().groups[0].active);
    });
}

#[test]
fn track_color_is_still_undone() {
    let mut e = engine();
    let before = vox(&e).color;
    run(&mut e, "track.color", json!({"tracks": ["Vox"], "color": [1, 2, 3]}));
    run(&mut e, "edit.undo", json!({}));
    assert_eq!(vox(&e).color, before);
}

#[test]
fn undoing_an_edit_takes_back_what_the_edit_itself_changed() {
    let dir = std::env::temp_dir().join(format!("soundcraft-undo-edits-only-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("muted.json").to_string_lossy().into_owned();
    let mut e = engine();
    run(&mut e, "mix.mute", json!({"tracks": ["Vox"]}));
    run(&mut e, "track.save_preset", json!({"path": path, "track": "Vox"}));
    run(&mut e, "mix.mute", json!({"tracks": ["Vox"]}));
    run(&mut e, "track.load_preset", json!({"path": path, "tracks": ["Vox"]}));
    assert!(vox(&e).mixer.mute, "the preset mutes the track");
    run(&mut e, "edit.undo", json!({}));
    assert!(!vox(&e).mixer.mute, "undoing the preset takes its mute back");
    run(&mut e, "edit.redo", json!({}));
    assert!(vox(&e).mixer.mute);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_mute_after_the_edits_survives_undoing_them() {
    let dir = std::env::temp_dir().join(format!("soundcraft-undo-later-mute-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let preset = |name: &str| dir.join(name).to_string_lossy().into_owned();
    let mut e = engine();
    run(&mut e, "mix.mute", json!({"tracks": ["Vox"], "value": true}));
    run(&mut e, "track.save_preset", json!({"path": preset("muted.json"), "track": "Vox"}));
    run(&mut e, "mix.mute", json!({"tracks": ["Vox"], "value": false}));
    run(&mut e, "track.save_preset", json!({"path": preset("open.json"), "track": "Vox"}));
    run(&mut e, "track.load_preset", json!({"path": preset("muted.json"), "tracks": ["Vox"]}));
    run(&mut e, "track.load_preset", json!({"path": preset("open.json"), "tracks": ["Vox"]}));
    // The mute button now sets the value the first preset had: it's still the user's.
    run(&mut e, "mix.mute", json!({"tracks": ["Vox"], "value": true}));
    run(&mut e, "edit.undo", json!({}));
    assert!(vox(&e).mixer.mute);
    run(&mut e, "edit.undo", json!({}));
    assert!(vox(&e).mixer.mute, "undoing the first preset keeps the later mute");
    // Between Undo and Redo too.
    run(&mut e, "mix.mute", json!({"tracks": ["Vox"], "value": false}));
    run(&mut e, "edit.redo", json!({}));
    assert!(!vox(&e).mixer.mute, "redoing the first preset keeps the later unmute");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn undoing_an_edit_keeps_a_folder_closed() {
    let mut e = engine();
    run(&mut e, "track.new", json!({"kind": "folder", "name": "Drums"}));
    undo_and_redo_the_edit(&mut e, &[("track.folder_toggle", json!({"track": "Drums", "open": false}))], |e| {
        assert!(!e.session().track_by_name("Drums").unwrap().folder_open);
    });
}

#[test]
fn undoing_an_edit_keeps_zoom_toggle_and_its_view() {
    let mut e = engine();
    let height = vox(&e).height;
    run(&mut e, "view.zoom_set", json!({"samples_per_px": 500.0}));
    run(&mut e, "edit.select", json!({"tracks": ["Vox"], "start": 48_000, "end": 96_000}));
    run(&mut e, "track.rename", json!({"track": "Vox", "name": "Lead"}));
    run(&mut e, "view.zoom_toggle", json!({}));
    let zoomed = (e.session().edit.zoom.samples_per_px, e.session().edit.zoom.scroll);
    assert_ne!(zoomed.0, 500.0);
    assert_ne!(vox(&e).height, height);
    run(&mut e, "edit.undo", json!({}));
    assert_eq!((e.session().edit.zoom.samples_per_px, e.session().edit.zoom.scroll), zoomed, "the view stays on the selection");
    run(&mut e, "view.zoom_toggle", json!({}));
    assert_eq!(e.session().edit.zoom.samples_per_px, 500.0, "Zoom Toggle still goes back out");
    assert_eq!(vox(&e).height, height);
}

#[test]
fn undo_and_redo_commands_take_back_each_edits_own_changes() {
    let dir = std::env::temp_dir().join(format!("soundcraft-undo-own-changes-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let preset = |name: &str| dir.join(name).to_string_lossy().into_owned();
    let mut e = engine();
    run(&mut e, "mix.mute", json!({"tracks": ["Vox"], "value": true}));
    run(&mut e, "track.save_preset", json!({"path": preset("muted.json"), "track": "Vox"}));
    run(&mut e, "mix.mute", json!({"tracks": ["Vox"], "value": false}));
    run(&mut e, "track.save_preset", json!({"path": preset("open.json"), "track": "Vox"}));
    run(&mut e, "track.load_preset", json!({"path": preset("muted.json"), "tracks": ["Vox"]}));
    run(&mut e, "track.load_preset", json!({"path": preset("open.json"), "tracks": ["Vox"]}));
    run(&mut e, "edit.undo", json!({}));
    assert!(vox(&e).mixer.mute, "back to the first preset's mute");
    run(&mut e, "edit.undo", json!({}));
    assert!(!vox(&e).mixer.mute, "undoing the first preset takes its mute back");
    run(&mut e, "edit.redo", json!({}));
    assert!(vox(&e).mixer.mute, "redoing it brings the mute back");
    let _ = std::fs::remove_dir_all(&dir);
}
