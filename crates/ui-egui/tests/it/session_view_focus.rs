//! The session view as a Tab stop: it takes the keyboard when it opens and when it is pressed, keeps Tab while Tab has
//! a stop to go to, and leaves every other key to the shortcuts, as with nothing focused.

use egui::{Event, Id, Key, Modifiers, PointerButton, Pos2, vec2};
use egui_kittest::{Harness, kittest::Queryable};
use serde_json::json;
use soundcraft_model::{ClipId, Tool};
use soundcraft_ui_egui::framework::session_view_id;
use soundcraft_ui_egui::{Services, SoundApp, edit_window};

type AppHarness = Harness<'static, SoundApp>;

fn harness() -> AppHarness {
    let app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
    let mut h =
        Harness::builder().with_context_setup(soundcraft_ui_egui::framework::install_headless).with_size(vec2(1600.0, 1000.0)).build_ui_state(
            |ui, app: &mut SoundApp| {
                app.logic(&ui.ctx().clone());
                app.ui(ui);
            },
            app,
        );
    h.run_steps(5);
    h
}

fn focused(h: &AppHarness) -> Option<Id> {
    h.ctx.memory(|m| m.focused())
}

fn edit_view() -> Option<Id> {
    Some(session_view_id(true))
}

fn press(h: &mut AppHarness, modifiers: Modifiers, key: Key) {
    h.key_press_modifiers(modifiers, key);
    h.run_steps(3);
}

fn select(h: &mut AppHarness, params: serde_json::Value) {
    h.state_mut().run("edit.select", params).unwrap();
    h.run_steps(2);
}

fn insertion(h: &AppHarness) -> i64 {
    h.state().engine.session().edit.selection.start
}

/// Where each of Kick's clips starts and ends.
fn kick_clips(h: &AppHarness) -> Vec<(ClipId, i64, i64)> {
    h.state().engine.session().track_by_name("Kick").unwrap().clips().iter().map(|c| (c.id, c.start, c.end())).collect()
}

/// A point in the Edit window's timeline.
fn in_timeline(h: &AppHarness) -> Pos2 {
    edit_window::rect(h.state().edit_layout.timeline).left_top() + vec2(300.0, 20.0)
}

fn button(h: &mut AppHarness, pos: Pos2, pressed: bool) {
    h.event(Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE });
    h.run_steps(2);
}

#[test]
fn tab_and_shift_tab_pass_through_a_session_view_with_no_stop_ahead() {
    let mut h = harness();
    select(&mut h, json!({"start": 1_000_000_000}));
    assert_eq!(focused(&h), edit_view(), "the opened session view has the focus");
    press(&mut h, Modifiers::SHIFT, Key::Tab);
    let before = focused(&h);
    assert!(before.is_some() && before != edit_view(), "Shift+Tab leaves the view");
    press(&mut h, Modifiers::NONE, Key::Tab);
    assert_eq!(focused(&h), edit_view(), "Tab from the stop before the view");
    for _ in 0..3 {
        press(&mut h, Modifiers::NONE, Key::Tab);
        let after = focused(&h);
        assert!(after.is_some() && after != edit_view() && after != before, "Tab goes on through the stops after the view");
    }
    for _ in 0..3 {
        press(&mut h, Modifiers::SHIFT, Key::Tab);
    }
    assert_eq!(focused(&h), edit_view(), "Shift+Tab walks back to the view");
    press(&mut h, Modifiers::SHIFT, Key::Tab);
    assert_eq!(focused(&h), before, "and past it");
    assert_eq!(insertion(&h), 1_000_000_000);
}

#[test]
fn the_focused_session_view_keeps_tab_until_the_last_stop() {
    let mut h = harness();
    let (_, start, end) = kick_clips(&h).into_iter().max_by_key(|clip| clip.2).unwrap();
    select(&mut h, json!({"tracks": ["Kick"], "start": start, "exact": true}));
    press(&mut h, Modifiers::SHIFT, Key::Tab);
    assert_eq!((insertion(&h), focused(&h)), (start, edit_view()), "Shift+Tab extends the selection");
    assert!(h.state().engine.session().edit.selection.end > start);
    select(&mut h, json!({"tracks": ["Kick"], "start": start, "exact": true}));
    while insertion(&h) < end {
        let at = insertion(&h);
        press(&mut h, Modifiers::NONE, Key::Tab);
        assert!(insertion(&h) > at, "Tab goes to the next stop");
        assert_eq!(focused(&h), edit_view());
    }
    press(&mut h, Modifiers::NONE, Key::Tab);
    assert_eq!(insertion(&h), end);
    assert!(focused(&h).is_some() && focused(&h) != edit_view(), "past the last stop Tab moves on");
}

#[test]
fn a_press_in_the_session_view_takes_the_keyboard_from_a_toolbar_button() {
    let mut h = harness();
    let tool = h.state().engine.session().edit.tool;
    assert_ne!(tool, Tool::Trim);
    h.get_by_label("Trim (F6)").focus();
    h.run_steps(2);
    assert!(h.get_by_label("Trim (F6)").is_focused());
    let at = in_timeline(&h);
    h.hover_at(at);
    button(&mut h, at, true);
    assert_eq!(focused(&h), edit_view(), "the press");
    button(&mut h, at, false);
    assert_eq!(focused(&h), edit_view(), "the click");
    let markers = h.state().engine.session().markers.len();
    press(&mut h, Modifiers::NONE, Key::Enter);
    assert_eq!(h.state().engine.session().markers.len(), markers + 1, "Enter adds a marker");
    assert_eq!(h.state().engine.session().edit.tool, tool, "and leaves the button alone");
    button(&mut h, at, true);
    button(&mut h, at, false);
    assert_eq!(focused(&h), edit_view(), "a press in the focused view");

    h.get_by_label("Trim (F6)").click();
    h.run_steps(3);
    assert_eq!(h.state().engine.session().edit.tool, Tool::Trim);
    select(&mut h, json!({"tracks": ["Kick"], "start": 0}));
    press(&mut h, Modifiers::NONE, Key::Tab);
    assert!(insertion(&h) > 0, "a click on a toolbar button leaves Tab to the session view");
}

#[test]
fn a_session_view_that_opens_takes_the_focus() {
    let mut h = harness();
    h.state_mut().run("window.toggle_mix_edit", json!({})).unwrap();
    h.run_steps(3);
    assert_eq!(focused(&h), Some(session_view_id(false)), "the Mix window's");
    h.state_mut().run("window.toggle_mix_edit", json!({})).unwrap();
    h.run_steps(3);
    assert_eq!(focused(&h), edit_view());
}

#[test]
fn the_shortcuts_run_with_the_session_view_focused() {
    let mut h = harness();
    let zoom = |h: &AppHarness| h.state().engine.session().edit.zoom.samples_per_px;
    let before = zoom(&h);
    press(&mut h, Modifiers::NONE, Key::T);
    assert!(zoom(&h) < before, "T zooms in");

    let (clip, ..) = kick_clips(&h)[0];
    select(&mut h, json!({"clips": [clip.0]}));
    press(&mut h, Modifiers::NONE, Key::Delete);
    assert!(h.state().engine.session().find_clip(clip).is_none(), "Delete clears the clip");
    press(&mut h, Modifiers::COMMAND, Key::Z);
    assert!(h.state().engine.session().find_clip(clip).is_some(), "Ctrl+Z brings it back");

    press(&mut h, Modifiers::NONE, Key::Home);
    assert_eq!(h.state().engine.session().edit.playhead, 0, "Home returns to zero");
    press(&mut h, Modifiers::NONE, Key::ArrowDown);
    press(&mut h, Modifiers::NONE, Key::Escape);
    assert_eq!(focused(&h), edit_view(), "the arrows and Escape leave the focus in the view");
}
