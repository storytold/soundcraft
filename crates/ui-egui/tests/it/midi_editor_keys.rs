use egui::{Event, Key, Modifiers, vec2};
use egui_kittest::Harness;
use serde_json::json;
use soundcraft_ui_egui::dialogs::Dialog;
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
    let mut h = Harness::builder().with_size(vec2(1600.0, 1000.0)).build_ui_state(
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

/// Cmd+A over the piano roll selects every note; Cmd+Shift+A is another chord (it can be the Help
/// shortcut) and must leave the selection alone.
#[test]
fn only_plain_cmd_a_selects_every_note() {
    let mut h = harness(false);
    h.ctx.set_os(egui::os::OperatingSystem::Nix);
    let ctrl = Modifiers { ctrl: true, command: true, ..Modifiers::NONE };
    let chord = |h: &mut Harness<'_, SoundApp>, modifiers: Modifiers| {
        h.key_press_modifiers(modifiers, Key::A);
        h.step();
    };
    // The roll starts under the editor's header, where its Quantize button sits.
    use egui_kittest::kittest::Queryable;
    let header = h.get_by_label("Quantize").rect();
    let at = egui::pos2(header.min.x - 300.0, header.max.y + 80.0);
    h.state_mut().midi.selected = vec![0];
    h.hover_at(at);
    h.step();
    // The pointer is over the roll: Up there transposes the selected note.
    let before = h.state().engine.session().clone();
    h.input_mut().events.push(Event::Key { key: Key::ArrowUp, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE });
    h.step();
    assert!(*h.state().engine.session() != before, "the pointer at {at:?} should be over the piano roll");
    for extra in [Modifiers::SHIFT, Modifiers::ALT] {
        h.state_mut().midi.selected = vec![0];
        h.hover_at(at);
        h.step();
        chord(&mut h, ctrl | extra);
        assert_eq!(h.state().midi.selected, vec![0], "Cmd+A with {extra:?} selected every note");
    }
}
