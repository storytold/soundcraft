//! The session view as a Tab stop: it takes the keyboard when it opens and when it is pressed, keeps Tab while Tab has
//! a stop to go to, and leaves every other key to the shortcuts, as with nothing focused.

use std::cell::{Cell, RefCell};

use egui::accesskit::Role;
use egui::{Event, Id, Key, Modifiers, PointerButton, Pos2, RawInput, Rect, ViewportId, ViewportInfo, vec2};
use egui_kittest::{Harness, kittest::Queryable};
use serde_json::json;
use soundcraft_model::{ClipId, Tool};
use soundcraft_ui_egui::framework::session_view_id;
use soundcraft_ui_egui::{Services, SoundApp, edit_window, menus};

type AppHarness = Harness<'static, SoundApp>;

fn harness() -> AppHarness {
    harness_of(soundcraft_engine::demo::demo_engine())
}

fn harness_of(engine: soundcraft_engine::Engine) -> AppHarness {
    let app = SoundApp::new(engine, None, Services::default());
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
fn a_held_tab_that_reaches_the_last_stop_stays_in_the_session_view() {
    let mut h = harness();
    let end = kick_clips(&h).into_iter().map(|clip| clip.2).max().unwrap();
    select(&mut h, json!({"tracks": ["Kick"], "start": end - 1, "exact": true}));
    let tab = |h: &mut AppHarness, pressed: bool, repeat: bool| {
        h.event(Event::Key { key: Key::Tab, physical_key: None, pressed, repeat, modifiers: Modifiers::NONE });
        h.run_steps(1);
    };
    tab(&mut h, true, false);
    assert_eq!((insertion(&h), focused(&h)), (end, edit_view()), "the press reaches the last stop");
    for _ in 0..3 {
        tab(&mut h, true, true);
        assert_eq!(focused(&h), edit_view(), "a repeat");
    }
    tab(&mut h, false, false);
    h.run_steps(2);
    assert_eq!(focused(&h), edit_view());
    press(&mut h, Modifiers::NONE, Key::Tab);
    assert!(focused(&h).is_some() && focused(&h) != edit_view(), "the next press moves on");
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
fn a_closed_search_gives_the_focus_back_to_the_session_view() {
    let mut h = harness();
    for (typed, key) in [("", Key::Escape), ("Zoom In", Key::Enter)] {
        h.state_mut().run("window.search", json!({})).unwrap();
        h.run_steps(3);
        assert!(h.ctx.text_edit_focused(), "the search field");
        h.event(Event::Text(typed.into()));
        press(&mut h, Modifiers::NONE, key);
        h.run_steps(2);
        assert!(!h.state().ui.show_search);
        assert_eq!(focused(&h), edit_view(), "{key:?}");
        select(&mut h, json!({"tracks": ["Kick"], "start": 0}));
        press(&mut h, Modifiers::NONE, Key::Tab);
        assert!(insertion(&h) > 0, "{key:?}: Tab moves the insertion point");
    }
}

#[test]
fn a_session_made_from_a_dialog_takes_the_focus() {
    let mut h = harness_of(soundcraft_engine::Engine::default());
    h.get_by_label("Trim (F6)").focus();
    h.run_steps(2);
    assert_ne!(focused(&h), edit_view());
    menus::invoke_menu(h.state_mut(), "session.new", "File > New Session...");
    h.run_steps(3);
    assert_eq!(h.state().dialogs.open_name(), Some("new_session"));
    h.get_by_role_and_label(Role::Button, "Create").focus();
    h.run_steps(2);
    press(&mut h, Modifiers::NONE, Key::Enter);
    h.run_steps(2);
    assert_eq!((h.state().dialogs.open_name(), focused(&h)), (None, edit_view()));
}

thread_local! {
    /// The events of the Mix window's next pass, and the widget that had its focus in the last one.
    static MIX_EVENTS: RefCell<Vec<Event>> = const { RefCell::new(Vec::new()) };
    static MIX_FOCUS: Cell<Option<Id>> = const { Cell::new(None) };
}

/// The open windows, as eframe lists them in the input of each: the runtime drops what it keeps of a window that the
/// input leaves out.
fn windows() -> egui::ViewportIdMap<ViewportInfo> {
    let mix = ViewportInfo { parent: Some(ViewportId::ROOT), ..Default::default() };
    [(ViewportId::ROOT, ViewportInfo::default()), (ViewportId::from_hash_of("soundcraft.mix"), mix)].into_iter().collect()
}

/// Draws the Mix window as a native one: its pass runs inside the main window's, as eframe's does.
fn render_mix(ctx: &egui::Context, viewport: egui::ImmediateViewport<'_>) {
    let mut show = viewport.viewport_ui_cb;
    let input = RawInput {
        viewport_id: viewport.ids.this,
        viewports: windows(),
        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1100.0, 720.0))),
        events: MIX_EVENTS.take(),
        ..Default::default()
    };
    let mut output = ctx.run_ui(input, |ui| {
        show(ui);
        MIX_FOCUS.set(ui.ctx().memory(|m| m.focused()));
    });
    output.textures_delta.clear();
}

/// Runs a frame of both windows with `events` in the Mix window, then settles.
fn mix_frames(ctx: &egui::Context, app: &mut SoundApp, events: Vec<Event>) -> Option<Id> {
    MIX_EVENTS.set(events);
    for _ in 0..4 {
        let screen_rect = Some(Rect::from_min_size(Pos2::ZERO, vec2(1600.0, 1000.0)));
        ctx.run_ui(RawInput { viewports: windows(), screen_rect, ..Default::default() }, |ui| {
            app.logic(ui.ctx());
            app.ui(ui);
        })
        .textures_delta
        .clear();
    }
    MIX_FOCUS.get()
}

#[test]
fn tab_and_shift_tab_leave_the_session_view_of_a_separate_mix_window() {
    let ctx = egui::Context::default();
    soundcraft_ui_egui::framework::install_headless(&ctx);
    ctx.set_embed_viewports(false);
    egui::Context::set_immediate_viewport_renderer(render_mix);
    let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
    app.run("edit.select", json!({"tracks": ["Kick"], "start": 0})).unwrap();
    app.run("window.mix_separate", json!({"value": true})).unwrap();
    let view = Some(session_view_id(false));
    assert_eq!(mix_frames(&ctx, &mut app, Vec::new()), view, "the opened Mix window's session view has its focus");
    let tab = |modifiers: Modifiers| -> Vec<Event> {
        let key = [true, false].map(|pressed| Event::Key { key: Key::Tab, physical_key: None, pressed, repeat: false, modifiers });
        [Event::ModifiersChanged(modifiers)].into_iter().chain(key).chain([Event::ModifiersChanged(Modifiers::NONE)]).collect()
    };
    let after = mix_frames(&ctx, &mut app, tab(Modifiers::NONE));
    assert!(after.is_some() && after != view, "Tab goes on to the strips: {after:?}, the view is {view:?}");
    assert_eq!(mix_frames(&ctx, &mut app, tab(Modifiers::SHIFT)), view, "Shift+Tab walks back");
    let before = mix_frames(&ctx, &mut app, tab(Modifiers::SHIFT));
    assert!(before.is_some() && before != view, "and past the view: {before:?}");
    assert_eq!(app.engine.session().edit.selection.start, 0);
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
