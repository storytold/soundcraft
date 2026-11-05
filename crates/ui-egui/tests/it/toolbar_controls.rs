//! The reference toolbar keeps editing, quantizing and optional tempo controls functional.

use egui::{Event, Key, Modifiers, vec2};
use egui_kittest::{Harness, kittest::Queryable};
use serde_json::json;
use soundcraft_ui_egui::{Services, SoundApp};

fn rig() -> Harness<'static, SoundApp> {
    let mut app = SoundApp::new(soundcraft_engine::Engine::default(), None, Services::default());
    app.run("track.new", json!({"kind": "midi", "name": "Notes"})).unwrap();
    let mut h = Harness::builder().with_size(vec2(1600.0, 900.0)).build_ui_state(
        |ui, app: &mut SoundApp| {
            app.logic(ui.ctx());
            app.ui(ui);
        },
        app,
    );
    h.run_steps(5);
    h
}

fn click(h: &mut Harness<'_, SoundApp>, label: &str) {
    h.get_by_label(label).click();
    h.run_steps(3);
}

fn type_value(h: &mut Harness<'_, SoundApp>, label: &str, value: &str) {
    click(h, label);
    h.event(Event::Text(value.into()));
    h.event(Event::Key { key: Key::Enter, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE });
    h.run_steps(3);
}

#[test]
fn toolbar_quantize_applies_the_displayed_grid_and_remains_undoable() {
    let mut h = rig();
    let app = h.state_mut();
    let clip = app.run("midi.new_clip", json!({"track": "Notes", "start": 0, "end": 96_000})).unwrap()["clip"].clone();
    app.run("midi.note_add", json!({"clip": clip, "pitch": 60, "start_ticks": 300, "length_ticks": 240})).unwrap();
    app.run("edit.select", json!({"clips": [clip]})).unwrap();
    h.run_steps(3);
    click(&mut h, "Quantize grid");
    click(&mut h, "1/4 note");
    assert_eq!(h.state().engine.session().edit.value("midi.input_quantize.grid_ticks", 0.0), 960.0);
    click(&mut h, "Apply quantize");
    let notes = h.state_mut().run("midi.notes", json!({"clip": clip})).unwrap();
    assert_eq!(notes["notes"][0]["start"], 0);
    assert_eq!(notes["notes"][0]["length"], 240);
    h.state_mut().run("edit.undo", json!({})).unwrap();
    let notes = h.state_mut().run("midi.notes", json!({"clip": clip})).unwrap();
    assert_eq!(notes["notes"][0]["start"], 300);
}

#[test]
fn quantize_fields_and_input_toggle_change_command_state_without_editing_notes() {
    let mut h = rig();
    let before = h.state().engine.undo_history();
    type_value(&mut h, "Quantize strength (%)", "75");
    type_value(&mut h, "Quantize swing (%)", "33");
    click(&mut h, "Input Quantize");
    let edit = &h.state().engine.session().edit;
    assert_eq!(edit.value("midi.input_quantize.strength", 0.0), 75.0);
    assert_eq!(edit.value("midi.input_quantize.swing", 0.0), 33.0);
    assert!(edit.flag("midi.input_quantize"));
    assert_eq!(h.state().engine.undo_history(), before);
    click(&mut h, "Input Quantize");
    assert!(!h.state().engine.session().edit.flag("midi.input_quantize"));
}

#[test]
fn toolbar_options_reveal_tempo_and_edit_toggles_dispatch_their_own_commands() {
    let mut h = rig();
    assert!(h.query_by_label("Tempo (BPM)").is_none());
    click(&mut h, "Toolbar options");
    click(&mut h, "MIDI Controls (Tempo, Meter, Count Off)");
    assert!(h.query_by_label("Tempo (BPM)").is_some());
    let before = h.state().engine.session().edit.clone();
    for label in ["Insertion Follows Playback", "Markers Follow Edit", "Layered Editing"] {
        click(&mut h, label);
    }
    let after = &h.state().engine.session().edit;
    assert_eq!(after.insertion_follows_playback, !before.insertion_follows_playback);
    assert_eq!(after.markers_follow_edit, !before.markers_follow_edit);
    assert_eq!(after.layered_editing, !before.layered_editing);
    assert_eq!(after.link_timeline_edit, before.link_timeline_edit);
    assert_eq!(after.link_track_edit, before.link_track_edit);
}
