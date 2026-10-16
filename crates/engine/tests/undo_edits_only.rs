//! #492: undo is for edits. Listening, recording and view settings aren't undo steps, and undoing
//! an edit leaves them as they are.

use serde_json::{Value, json};
use soundcraft_engine::Engine;
use soundcraft_engine::cmd::find_command;
use soundcraft_model::{AutomationMode, ChannelFormat, Track, TrackHeight};

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
    assert!(e.undo());
    assert_eq!(vox(e).name, "Vox", "Undo took back the edit");
    check(e);
    assert!(e.redo());
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
        "track.change_width",
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
fn undoing_an_edit_keeps_the_width_and_its_panners() {
    let mut e = engine();
    let mono_pan = vox(&e).mixer.pan.clone();
    undo_and_redo_the_edit(&mut e, &[("track.change_width", json!({"tracks": ["Lead"], "format": "stereo"}))], |e| {
        assert_eq!(vox(e).format, ChannelFormat::Stereo);
        assert_ne!(vox(e).mixer.pan, mono_pan, "the panners stay the stereo track's");
        assert_eq!(vox(e).mixer.pan.len(), 2);
    });
}

#[test]
fn track_color_is_still_undone() {
    let mut e = engine();
    let before = vox(&e).color;
    run(&mut e, "track.color", json!({"tracks": ["Vox"], "color": [1, 2, 3]}));
    assert!(e.undo());
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
    assert!(e.undo());
    assert!(!vox(&e).mixer.mute, "undoing the preset takes its mute back");
    assert!(e.redo());
    assert!(vox(&e).mixer.mute);
    let _ = std::fs::remove_dir_all(&dir);
}
