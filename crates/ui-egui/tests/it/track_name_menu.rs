//! Right-clicking a track's name opens the track menu, in the Edit and in the Mix window.

use egui::{Event, Modifiers, PointerButton, Pos2, pos2, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use soundcraft_ui_egui::{MainWindow, Services, SoundApp};

fn right_click(h: &mut Harness<SoundApp>, at: Pos2) {
    h.hover_at(at);
    for pressed in [true, false] {
        h.event(Event::PointerButton { pos: at, button: PointerButton::Secondary, pressed, modifiers: Modifiers::NONE });
    }
    h.run_steps(3);
}

/// Right-clicking a track name opens the track menu in both the Edit window and the Mix strip.
#[test]
fn right_click_on_a_track_name_opens_the_track_menu() {
    let mut engine = soundcraft_engine::Engine::default();
    engine.execute("track.new", &serde_json::json!({"name": "Voice"})).unwrap();
    let app = SoundApp::new(engine, None, Services::default());
    let mut h = Harness::builder().with_size(vec2(1400.0, 900.0)).with_step_dt(1.0 / 60.0).build_ui_state(
        |ui, app: &mut SoundApp| {
            app.logic(&ui.ctx().clone());
            app.ui(ui);
        },
        app,
    );
    h.run_steps(4);
    let row = h.state().edit_layout.rows[0].1;
    right_click(&mut h, pos2(row[0] + 60.0, row[1] + 10.0));
    assert!(h.query_by_label("Make Inactive").is_some(), "the Edit window's name plate");

    h.key_press(egui::Key::Escape);
    h.state_mut().ui.window = MainWindow::Mix;
    h.run_steps(4);
    assert!(h.query_by_label("Make Inactive").is_none());
    let strip = h.get_by_label_contains("Record enable").rect();
    right_click(&mut h, pos2(strip.center().x, 900.0 - 17.0));
    assert!(h.query_by_label("Make Inactive").is_some(), "the Mix strip's name plate");
}
