//! Options › MIDI Thru is on by default and the Options menu shows its check mark, which follows
//! the option (Pro Tools Reference Guide p.811).
use egui::vec2;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use serde_json::json;
use soundcraft_ui_egui::{Services, SoundApp};

fn harness() -> Harness<'static, SoundApp> {
    let app = SoundApp::new(soundcraft_engine::Engine::default(), None, Services::default());
    let mut h =
        Harness::builder().with_context_setup(soundcraft_ui_egui::framework::install_headless).with_size(vec2(1600.0, 1400.0)).build_ui_state(
            |ui, app: &mut SoundApp| {
                let ctx = ui.ctx().clone();
                app.logic(&ctx);
                app.ui(ui);
            },
            app,
        );
    h.run_steps(4);
    h
}

/// Open the Options menu and say whether its MIDI Thru item is checked. The item's label ends in
/// a space where a shortcut would go.
fn checked(h: &mut Harness<'_, SoundApp>) -> bool {
    h.get_by_label("Options").click();
    h.run_steps(4);
    let on = h.query_by_label("✔ MIDI Thru ").is_some();
    assert!(on || h.query_by_label("MIDI Thru ").is_some(), "the Options menu lists MIDI Thru");
    h.key_press(egui::Key::Escape);
    h.run_steps(4);
    on
}

#[test]
fn the_options_menu_shows_whether_midi_thru_is_on() {
    let mut h = harness();
    assert!(soundcraft_mix::midi_thru_enabled(h.state().engine.session()), "on by default");
    assert!(checked(&mut h));
    h.state_mut().run("options.midi_thru", json!({})).unwrap();
    assert!(!soundcraft_mix::midi_thru_enabled(h.state().engine.session()));
    assert!(!checked(&mut h));
    h.state_mut().run("options.midi_thru", json!({})).unwrap();
    assert!(checked(&mut h));
}
