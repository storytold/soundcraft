//! Delete and Backspace clear the edit selection, as Pro Tools' Delete key does (Edit › Clear is Cmd+B).

use egui::os::OperatingSystem;
use egui::{Event, Key, Modifiers, RawInput, vec2};
use egui_kittest::Harness;
use serde_json::json;
use soundcraft_ui_egui::{Services, SoundApp, shortcuts};

fn press(ctx: &egui::Context, app: &mut SoundApp, key: Key, modifiers: Modifiers) {
    let presses = [true, false].map(|pressed| Event::Key { key, physical_key: None, pressed, repeat: false, modifiers });
    let events = [Event::ModifiersChanged(modifiers)].into_iter().chain(presses).collect();
    ctx.run_ui(RawInput { events, ..Default::default() }, |ui| shortcuts::handle(app, ui.ctx())).textures_delta.clear();
}

fn rig(os: OperatingSystem) -> (egui::Context, SoundApp) {
    let ctx = egui::Context::default();
    ctx.set_os(os);
    (ctx, SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default()))
}

fn clips(app: &SoundApp, track: &str) -> usize {
    app.engine.session().track_by_name(track).and_then(|t| t.playlist()).map_or(0, |p| p.clips.len())
}

#[test]
fn delete_and_backspace_clear_the_edit_selection() {
    for os in [OperatingSystem::Mac, OperatingSystem::Windows, OperatingSystem::Nix] {
        for key in [Key::Delete, Key::Backspace] {
            let (ctx, mut app) = rig(os);
            let before = clips(&app, "Kick");
            assert!(before > 0);
            let end: i64 = 48_000 * 600;
            app.run("edit.select", json!({"tracks": ["Kick"], "start": 0, "end": end})).unwrap();
            press(&ctx, &mut app, key, Modifiers::NONE);
            assert_eq!(clips(&app, "Kick"), 0, "{os:?} {key:?}");
            // One Undo brings it back, like the menu item.
            app.run("edit.undo", json!({})).unwrap();
            assert_eq!(clips(&app, "Kick"), before, "{os:?} {key:?}");
        }
    }
}

#[test]
fn delete_with_nothing_selected_or_in_the_mix_window_changes_nothing() {
    let (ctx, mut app) = rig(OperatingSystem::Nix);
    let before = app.engine.session().clone();
    press(&ctx, &mut app, Key::Delete, Modifiers::NONE);
    assert!(*app.engine.session() == before, "no selection");
    let end: i64 = 48_000 * 600;
    app.run("edit.select", json!({"tracks": ["Kick"], "start": 0, "end": end})).unwrap();
    app.run("window.toggle_mix_edit", json!({})).unwrap();
    let selected = app.engine.session().clone();
    press(&ctx, &mut app, Key::Delete, Modifiers::NONE);
    assert!(*app.engine.session() == selected, "the Mix window leaves the edit selection alone");
    // Modified Delete is not Clear.
    app.run("window.toggle_mix_edit", json!({})).unwrap();
    press(&ctx, &mut app, Key::Delete, Modifiers::SHIFT);
    assert!(*app.engine.session() == selected, "Shift+Delete");
}

#[test]
fn the_midi_editor_keeps_the_key_for_its_selected_notes() {
    let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
    app.ui.show_midi_editor = true;
    let end: i64 = 48_000 * 600;
    app.run("edit.select", json!({"tracks": ["Keys"], "start": 0, "end": end})).unwrap();
    let clips_before = clips(&app, "Keys");
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
    h.input_mut().events.push(Event::Key { key: Key::Delete, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE });
    h.step();
    assert!(h.state().midi.selected.is_empty(), "the notes were deleted");
    assert_eq!(clips(h.state(), "Keys"), clips_before, "and the clip was not cleared with them");
}
