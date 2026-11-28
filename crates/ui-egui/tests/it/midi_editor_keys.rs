use craft_behavior::dock::{Layout, Node, SplitSize};
use craft_behavior::flow::Axis;
use egui::{Event, Key, Modifiers, vec2};
use egui_kittest::Harness;
use serde_json::json;
use soundcraft_ui_egui::dialogs::Dialog;
use soundcraft_ui_egui::docking::{Panel, Workspace};
use soundcraft_ui_egui::{Services, SoundApp};

/// The Keys track's MIDI clip in the editor with its first two notes selected, the Rename Track dialog open or not.
fn harness(rename: bool) -> Harness<'static, SoundApp> {
    let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
    app.ui.show_midi_editor = true;
    app.run("edit.select", json!({"tracks": ["Keys"]})).unwrap();
    let keys = app.engine.session().track_by_name("Keys").unwrap().id;
    if rename {
        app.dialogs.open_rename_track(keys, "Keys");
    }
    let mut h =
        Harness::builder().with_context_setup(soundcraft_ui_egui::framework::install_headless).with_size(vec2(1600.0, 1000.0)).build_ui_state(
            |ui, app: &mut SoundApp| {
                let ctx = ui.ctx().clone();
                app.logic(&ctx);
                app.ui(ui);
            },
            app,
        );
    for _ in 0..3 {
        h.step();
    }
    h.state_mut().midi.selected = vec![0, 1];
    h
}

fn press(h: &mut Harness<'_, SoundApp>, key: Key) {
    h.input_mut().events.push(Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE });
    h.step();
}

#[test]
fn deleting_text_in_the_rename_field_keeps_the_selected_notes() {
    for key in [Key::Backspace, Key::Delete] {
        let mut h = harness(true);
        let before = h.state().engine.session().clone();
        press(&mut h, key);
        // The field opens with the old name selected, so the key clears it.
        assert!(matches!(&h.state().dialogs.open, Some(Dialog::RenameTrack { name, .. }) if name.is_empty()), "{key:?}");
        assert!(*h.state().engine.session() == before, "{key:?} edited the session");
    }
}

#[test]
fn deleting_in_the_editor_still_removes_the_selected_notes() {
    for key in [Key::Backspace, Key::Delete] {
        let mut h = harness(false);
        let before = h.state().engine.session().clone();
        press(&mut h, key);
        assert!(*h.state().engine.session() != before, "{key:?} left the notes");
        assert!(h.state().midi.selected.is_empty());
    }
}

/// #1627: the MIDI editor docked behind another tab keeps `show_midi_editor` true, but its body is
/// never drawn, so nothing consumes Delete. It must fall through to `edit.clear` instead.
#[test]
fn delete_runs_clear_when_the_docked_midi_editor_sits_behind_another_tab() {
    // MIDI Editor shares a tab group with Clips, Clips active: the panel is in the layout
    // (Window > MIDI Editor stays checked) but nothing draws it.
    let root =
        Node::split(Axis::Y, SplitSize::FixedSecond(340.0), Node::tabs(vec![Panel::Canvas], 0), Node::tabs(vec![Panel::Midi, Panel::Clips], 1));
    let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
    app.ui.docking = Workspace::new(Layout::new(Some(root), vec![]));
    app.run("edit.select", json!({"tracks": ["Keys"]})).unwrap();
    let mut h =
        Harness::builder().with_context_setup(soundcraft_ui_egui::framework::install_headless).with_size(vec2(1600.0, 1000.0)).build_ui_state(
            |ui, app: &mut SoundApp| {
                let ctx = ui.ctx().clone();
                app.logic(&ctx);
                app.ui(ui);
            },
            app,
        );
    for _ in 0..3 {
        h.step();
    }
    h.state_mut().midi.selected = vec![0, 1];
    let keys_clip = h.state().engine.session().track_by_name("Keys").unwrap().clips()[0].id;
    h.state_mut().engine.execute("edit.select", &json!({"clips": [keys_clip.0]})).unwrap();
    press(&mut h, Key::Delete);
    assert!(h.state().engine.session().find_clip(keys_clip).is_none(), "Delete reached edit.clear");
    assert_eq!(h.state().midi.selected, vec![0, 1], "the hidden editor's notes were left alone");
}
