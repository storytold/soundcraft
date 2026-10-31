//! A send's level opens its Send window, as in Pro Tools: a fader with Alt-click for 0 dB, moved to
//! another send by clicking that send's level, closed by clicking the same one again.

use egui::{Event, Modifiers, PointerButton, Pos2, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use serde_json::json;
use soundcraft_ui_egui::{MainWindow, Services, SoundApp};

fn harness() -> (Harness<'static, SoundApp>, soundcraft_model::TrackId) {
    let mut engine = soundcraft_engine::Engine::default();
    engine.execute("track.new", &json!({"name": "Vox"})).unwrap();
    let vox = engine.session().tracks[0].id;
    for (slot, bus) in [(0, "Verb"), (1, "Delay")] {
        engine.execute("mix.new_bus", &json!({"name": bus})).unwrap();
        engine.execute("mix.send", &json!({"track": vox.0, "slot": slot, "bus": bus, "level_db": -10.0})).unwrap();
    }
    let mut app = SoundApp::new(engine, None, Services::default());
    app.ui.window = MainWindow::Mix;
    let mut h = Harness::builder().with_size(vec2(1400.0, 900.0)).with_step_dt(1.0 / 60.0).build_ui_state(
        |ui, app: &mut SoundApp| {
            app.logic(&ui.ctx().clone());
            app.ui(ui);
        },
        app,
    );
    h.run_steps(4);
    (h, vox)
}

fn level(h: &Harness<SoundApp>, slot: usize) -> f32 {
    h.state().engine.session().tracks[0].mixer.sends[slot].as_ref().unwrap().level_db
}

fn press(h: &mut Harness<SoundApp>, at: Pos2, pressed: bool, modifiers: Modifiers) {
    h.event(Event::ModifiersChanged(modifiers));
    h.event(Event::PointerButton { pos: at, button: PointerButton::Primary, pressed, modifiers });
    h.step();
}

/// A send's level opens its window; Alt-click resets the fader to 0 dB, dragging reaches −∞, and clicking the
/// same send again closes the window.
#[test]
fn a_sends_level_opens_its_send_window() {
    let (mut h, vox) = harness();
    h.get_by_label("Send A level for Vox").click();
    h.run_steps(3);
    assert_eq!(h.state().ui.send_window, Some((vox, 0)));
    assert!(h.query_by_label_contains("Vox · Send A").is_some());

    let fader = h.get_by_label("Send level").rect();
    h.hover_at(fader.center());
    press(&mut h, fader.center(), true, Modifiers::ALT);
    press(&mut h, fader.center(), false, Modifiers::ALT);
    h.event(Event::ModifiersChanged(Modifiers::NONE));
    h.run_steps(2);
    assert_eq!(level(&h, 0), 0.0, "Alt-click on the fader gives 0 dB");

    press(&mut h, fader.center(), true, Modifiers::NONE);
    for dy in [10.0, 60.0, 150.0, 300.0] {
        h.hover_at(fader.center() + vec2(0.0, dy));
        h.step();
    }
    press(&mut h, fader.center() + vec2(0.0, 300.0), false, Modifiers::NONE);
    assert_eq!(level(&h, 0), -144.0, "the fader goes down to −∞");
    assert_eq!(level(&h, 1), -10.0);

    h.get_by_label("Send B level for Vox").click();
    h.run_steps(3);
    assert_eq!(h.state().ui.send_window, Some((vox, 1)), "another send's level moves the window there");
    assert!(h.query_by_label_contains("Vox · Send B").is_some());
    h.get_by_label("Send B level for Vox").click();
    h.run_steps(3);
    assert_eq!(h.state().ui.send_window, None, "clicking the same send again closes it");
    assert!(h.query_by_label_contains("Vox · Send").is_none());
}

/// Escape first closes a send menu or cancels level typing, then a further Escape closes the send window.
#[test]
fn escape_closes_the_send_window_after_menus_and_typing() {
    let (mut h, vox) = harness();
    h.get_by_label("Send A level for Vox").click();
    h.run_steps(3);

    // A right-click menu on top takes the first Escape.
    let at = h.get_by_label("Send A level for Vox").rect().center();
    h.hover_at(at);
    for pressed in [true, false] {
        h.event(Event::PointerButton { pos: at, button: PointerButton::Secondary, pressed, modifiers: Modifiers::NONE });
    }
    h.run_steps(3);
    // The window's own PRE toggle is a button labelled "Pre-fader"; the menu's is a checkbox.
    assert!(h.query_by_role_and_label(egui::accesskit::Role::CheckBox, "Pre-fader").is_some(), "the send's options menu is open");
    h.key_press(egui::Key::Escape);
    h.run_steps(3);
    assert!(h.query_by_role_and_label(egui::accesskit::Role::CheckBox, "Pre-fader").is_none());
    assert_eq!(h.state().ui.send_window, Some((vox, 0)), "that Escape only closed the menu");

    // So does typing a level in the window.
    let fader = h.get_by_label("Send level").rect();
    let field = egui::pos2(fader.center().x, fader.max.y + 10.0);
    h.hover_at(field);
    press(&mut h, field, true, Modifiers::NONE);
    press(&mut h, field, false, Modifiers::NONE);
    h.run_steps(3);
    assert!(h.state().ui.send_window.is_some());
    h.key_press(egui::Key::Escape);
    h.run_steps(3);
    assert_eq!(h.state().ui.send_window, Some((vox, 0)), "that Escape only cancelled the typing");

    h.key_press(egui::Key::Escape);
    h.run_steps(3);
    assert_eq!(h.state().ui.send_window, None);
    assert_eq!(level(&h, 0), -10.0);
}
