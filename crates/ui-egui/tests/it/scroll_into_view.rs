//! Scroll Into View, in a track name's right-click menu: the track goes to the top of the Edit
//! window and to the left of the Mix window, both windows at once.

use egui::{Event, Modifiers, PointerButton, Pos2, Shape, pos2, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use serde_json::json;
use soundcraft_ui_egui::{MainWindow, Services, SoundApp};

const SIZE: (f32, f32) = (1400.0, 900.0);

fn harness(tracks: usize) -> Harness<'static, SoundApp> {
    let mut app = SoundApp::new(soundcraft_engine::Engine::default(), None, Services::default());
    for i in 1..=tracks {
        app.run("track.new", json!({"name": format!("T{i:02}")})).unwrap();
    }
    // Uneven heights, so an offset counted as "n rows" can't pass for the real one.
    app.run("track.height", json!({"tracks": ["T02"], "height": "large"})).unwrap();
    app.run("track.height", json!({"tracks": ["T04"], "height": "small"})).unwrap();
    let mut h = Harness::builder().with_size(vec2(SIZE.0, SIZE.1)).with_step_dt(1.0 / 60.0).build_ui_state(
        |ui, app: &mut SoundApp| {
            app.logic(&ui.ctx().clone());
            app.ui(ui);
        },
        app,
    );
    h.run_steps(4);
    h
}

fn right_click(h: &mut Harness<'_, SoundApp>, at: Pos2) {
    h.hover_at(at);
    for pressed in [true, false] {
        h.event(Event::PointerButton { pos: at, button: PointerButton::Secondary, pressed, modifiers: Modifiers::NONE });
    }
    h.run_steps(3);
}

fn scroll_into_view(h: &mut Harness<'_, SoundApp>) {
    h.get_by_label("Scroll Into View").click();
    h.run_steps(4);
}

fn track_id(h: &Harness<'_, SoundApp>, name: &str) -> u64 {
    h.state().engine.session().tracks.iter().find(|t| t.name == name).unwrap().id.0
}

fn edit_row(h: &Harness<'_, SoundApp>, name: &str) -> [f32; 4] {
    let id = track_id(h, name);
    h.state().edit_layout.rows.iter().find(|(t, _)| *t == id).unwrap().1
}

/// Height of the Edit window's track area, as last drawn.
fn view_h(h: &Harness<'_, SoundApp>) -> f32 {
    let [_, y0, _, y1] = h.state().edit_layout.timeline;
    y1 - y0
}

fn max_scroll(h: &Harness<'_, SoundApp>) -> f32 {
    h.state().edit_layout.content_h - view_h(h) + 40.0
}

/// The centre of a Mix strip's name plate: its name is painted centred on it, at the bottom of
/// the strip (the galley's box, not the glyphs, so "T01" and "T08" line up alike).
fn plate(h: &Harness<'_, SoundApp>, name: &str) -> Option<Pos2> {
    h.output().shapes.iter().find_map(|shape| match &shape.shape {
        Shape::Text(text) if text.galley.job.text == name && text.pos.y > SIZE.1 - 40.0 => Some(text.pos + text.galley.size() / 2.0),
        _ => None,
    })
}

#[test]
fn the_track_goes_to_the_top_of_the_edit_window() {
    let mut h = harness(20);
    assert_eq!(h.state().edit_layout.scroll_y, 0.0);
    let row = edit_row(&h, "T06");
    right_click(&mut h, pos2(row[0] + 60.0, row[1] + 10.0));
    scroll_into_view(&mut h);

    let above: f32 = ["T01", "T02", "T03", "T04", "T05"].iter().map(|n| edit_row(&h, n)).map(|r| r[3] - r[1] + 1.0).sum();
    assert_eq!(h.state().edit_layout.scroll_y, above);
    assert_eq!(edit_row(&h, "T06")[1], h.state().edit_layout.timeline[1], "T06 is the first row in view");
}

#[test]
fn near_the_end_it_scrolls_only_as_far_as_the_scroll_bar_goes() {
    let mut h = harness(20);
    let max = max_scroll(&h);
    h.state_mut().edit_layout.scroll_y = max - 60.0;
    h.run_steps(3);
    let row = edit_row(&h, "T19");
    assert!(row[1] - h.state().edit_layout.timeline[1] < max, "the fixture puts T19 past the furthest scroll");
    right_click(&mut h, pos2(row[0] + 60.0, row[1] + 10.0));
    scroll_into_view(&mut h);

    assert_eq!(h.state().edit_layout.scroll_y, max);
}

#[test]
fn from_the_mix_the_clicked_strip_goes_left_and_the_edit_window_follows() {
    let mut h = harness(20);
    let t03 = track_id(&h, "T03");
    h.state_mut().engine.session_mut().edit.selected_tracks = vec![soundcraft_model::TrackId(t03)];
    h.state_mut().ui.window = MainWindow::Mix;
    h.run_steps(4);
    let first = plate(&h, "T01").expect("the first strip's name").x;
    let target = plate(&h, "T08").expect("T08 is on screen");
    right_click(&mut h, target);
    scroll_into_view(&mut h);

    let now = plate(&h, "T08").expect("T08 still on screen").x;
    assert!((now - first).abs() < 1.0, "T08 is the leftmost strip: at {now}, the first was at {first}");
    assert_eq!(h.state().engine.session().edit.selected_tracks, vec![soundcraft_model::TrackId(t03)], "the selection is left alone");

    h.state_mut().ui.window = MainWindow::Edit;
    h.run_steps(4);
    assert_eq!(edit_row(&h, "T08")[1], h.state().edit_layout.timeline[1], "the Edit window scrolled too");
}

#[test]
fn a_track_in_a_closed_folder_scrolls_the_mix_but_not_the_edit_window() {
    let mut h = harness(20);
    h.state_mut().run("track.move_to_new_folder", json!({"tracks": ["T08"], "name": "F"})).unwrap();
    h.state_mut().run("track.folder_toggle", json!({"track": "F", "open": false})).unwrap();
    h.state_mut().ui.window = MainWindow::Mix;
    h.run_steps(4);
    let target = plate(&h, "T08").expect("the Mix shows tracks of a closed folder");
    right_click(&mut h, target);
    scroll_into_view(&mut h);

    h.state_mut().ui.window = MainWindow::Edit;
    h.run_steps(4);
    assert_eq!(h.state().edit_layout.scroll_y, 0.0);
}

#[test]
fn the_command_takes_a_track_by_name_from_the_control_channel() {
    let mut h = harness(20);
    h.state_mut().ui.window = MainWindow::Mix;
    h.run_steps(4);
    let first = plate(&h, "T01").expect("the first strip's name").x;
    h.state_mut().run("view.scroll_into_view", json!({"track": "T08"})).unwrap();
    h.run_steps(4);

    let now = plate(&h, "T08").expect("T08 on screen").x;
    assert!((now - first).abs() < 1.0, "T08 is the leftmost strip: at {now}, the first was at {first}");
    h.state_mut().ui.window = MainWindow::Edit;
    h.run_steps(4);
    assert_eq!(edit_row(&h, "T08")[1], h.state().edit_layout.timeline[1], "the Edit window scrolled too");
    assert!(h.state_mut().run("view.scroll_into_view", json!({"track": "T99"})).is_err());
}

#[test]
fn a_hidden_track_leaves_no_scroll_waiting_in_the_mix() {
    let mut h = harness(20);
    h.state_mut().ui.window = MainWindow::Mix;
    h.state_mut().run("track.hide", json!({"tracks": ["T08"], "hidden": true})).unwrap();
    h.run_steps(4);
    let first = plate(&h, "T01").expect("the first strip's name").x;
    h.state_mut().run("view.scroll_into_view", json!({"track": "T08"})).unwrap();
    h.run_steps(4);

    // Shown again later, T08 must not pull the Mix to itself: the request was for frames long gone.
    h.state_mut().run("track.hide", json!({"tracks": ["T08"], "hidden": false})).unwrap();
    h.run_steps(4);
    let now = plate(&h, "T01").expect("T01 still on screen").x;
    assert!((now - first).abs() < 1.0, "the Mix stayed put: T01 at {now}, was at {first}");
}
