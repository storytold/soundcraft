//! New Tracks opens from the Tracks list's + button, and from a double-click below the last
//! track (Edit) or right of the last strip (Mix).

use egui::accesskit::Role;
use egui::{Pos2, pos2, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::{Queryable, by};
use soundcraft_ui_egui::{MainWindow, Services, SoundApp};

fn harness(tracks: usize) -> Harness<'static, SoundApp> {
    let mut engine = soundcraft_engine::Engine::default();
    if tracks > 0 {
        engine.execute("track.new", &serde_json::json!({"count": tracks})).unwrap();
    }
    let app = SoundApp::new(engine, None, Services::default());
    let mut h = Harness::builder().with_size(vec2(1400.0, 900.0)).with_step_dt(1.0 / 60.0).build_ui_state(
        |ui, app: &mut SoundApp| {
            app.logic(&ui.ctx().clone());
            app.ui(ui);
        },
        app,
    );
    h.run_steps(4);
    h
}

fn click(h: &mut Harness<SoundApp>, at: Pos2) {
    h.hover_at(at);
    h.drag_at(at);
    h.drop_at(at);
    h.step();
}

#[test]
fn the_tracks_list_plus_button_opens_new_tracks() {
    let mut h = harness(0);
    h.get_by_label_contains("New Tracks").click();
    h.run_steps(2);
    assert_eq!(h.state().dialogs.open_name(), Some("new_tracks"));
}

#[test]
fn double_click_below_the_last_track_opens_new_tracks() {
    let mut h = harness(2);
    let rows = h.state().edit_layout.rows.clone();
    let below = rows[1].1[3] + 60.0;
    for x in [h.state().edit_layout.timeline[0] + 100.0, rows[0].1[0] + 60.0] {
        click(&mut h, pos2(x, below));
        assert_eq!(h.state().dialogs.open_name(), None, "one click only clears the selection");
        assert!(h.state().engine.session().edit.selected_tracks.is_empty());
        click(&mut h, pos2(x, below));
        assert_eq!(h.state().dialogs.open_name(), Some("new_tracks"));
        h.state_mut().dialogs.open = None;
        h.run_steps(30);
    }
}

/// Double-clicking to the right of the last Mix strip opens the New Tracks dialog.
#[test]
fn double_click_right_of_the_last_strip_opens_new_tracks() {
    let mut h = harness(1);
    h.state_mut().ui.window = MainWindow::Mix;
    h.run_steps(4);
    let strip = h.get_by_label_contains("Record enable").rect();
    let at = pos2(strip.max.x + 150.0, strip.center().y);
    click(&mut h, at);
    click(&mut h, at);
    assert_eq!(h.state().dialogs.open_name(), Some("new_tracks"));
}

#[test]
fn the_new_tracks_name_field_is_wide_enough_to_read_a_name() {
    let mut h = harness(0);
    h.get_by_label_contains("New Tracks").click();
    h.run_steps(4);
    assert_eq!(h.state().dialogs.open_name(), Some("new_tracks"));
    // The field's text run carries the same value as the field itself.
    let width = h.get(by().role(Role::TextInput).value("Audio")).rect().width();
    assert!(width >= 200.0, "the Name field is only {width}px wide");
}
