//! A window's egui id must not follow its translated title: moving the Appearance window and then
//! changing the language in that very window keeps its id, so its remembered place survives.

use egui::accesskit::Role;
use egui::{Event, Modifiers, PointerButton, Pos2, Vec2, pos2, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use serde_json::json;
use soundcraft_ui_egui::{Services, SoundApp};

const WINDOW_PX: Vec2 = vec2(1600.0, 1000.0);

/// One harness frame with the pointer at `px`, pressing or releasing on the frames that say so.
fn frame(h: &mut Harness<'_, SoundApp>, px: Pos2, pressed: Option<bool>) {
    h.input_mut().events.push(Event::PointerMoved(px));
    if let Some(pressed) = pressed {
        h.input_mut().events.push(Event::PointerButton { pos: px, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE });
    }
    h.step();
}

#[test]
fn moving_the_appearance_window_then_changing_its_language_keeps_its_place() {
    let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
    app.extra.show_ui_customization = true;
    let mut h = Harness::builder().with_size(WINDOW_PX).with_pixels_per_point(1.0).build_ui_state(
        |ui, app: &mut SoundApp| {
            let ctx = ui.ctx().clone();
            app.logic(&ctx);
            app.ui(ui);
        },
        app,
    );
    h.run_steps(6);
    let before = h.get_by_role_and_label(Role::Window, "Appearance").rect();

    // Drag the window by its title bar to a new place.
    let grab = pos2(before.center().x, before.top() + 12.0);
    let to = grab + vec2(180.0, 110.0);
    frame(&mut h, grab, Some(true));
    for i in 1..=10 {
        frame(&mut h, grab.lerp(to, i as f32 / 10.0), None);
    }
    frame(&mut h, to, Some(false));
    frame(&mut h, to, None);
    h.run_steps(2);
    let moved = h.get_by_role_and_label(Role::Window, "Appearance").rect();
    let drift = moved.min - (before.min + vec2(180.0, 110.0));
    assert!(drift.x.abs() < 8.0 && drift.y.abs() < 8.0, "the drag did not move the window: {before:?} → {moved:?}");

    // Changing the language in that very window must not move the window to a new egui id: it
    // stays where the user put it, now titled in French.
    let _ = h.state_mut().run("ui.language", json!({"lang": "fr"}));
    h.run_steps(6);
    let after = h.get_by_role_and_label(Role::Window, "Apparence").rect();
    let kept = after.min - moved.min;
    assert!(kept.x.abs() < 2.0 && kept.y.abs() < 2.0, "the window jumped when the language changed: {moved:?} → {after:?}");

    soundcraft_ui_egui::i18n::set(soundcraft_ui_egui::i18n::Language::En);
}
