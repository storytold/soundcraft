use egui::accesskit::Role;
use egui::{Event, Key, Modifiers, PointerButton, Pos2, RawInput, Rect, vec2};
use egui_kittest::{Harness, kittest::Queryable};
use serde_json::json;
use soundcraft_model::TrackId;
use soundcraft_ui_egui::{Services, SoundApp, menus};

/// Loops the Snare clip three times and selects it.
fn looped_app() -> (SoundApp, TrackId, usize) {
    let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
    let track = app.engine.session().track_by_name("Snare").unwrap().id;
    let clip = app.engine.session().track(track).unwrap().clips()[0].id;
    app.run("clip.loop", json!({"clip": clip.0, "count": 3})).unwrap();
    app.run("edit.select", json!({"clips": [clip.0]})).unwrap();
    let looped = app.engine.session().track(track).unwrap().clips().len();
    (app, track, looped)
}

/// Loops the Snare clip three times, selects it and opens Clip > Unloop... from the menu.
fn unloop_app() -> (SoundApp, TrackId, usize) {
    let (mut app, track, looped) = looped_app();
    menus::invoke_menu(&mut app, "clip.unloop", "Clip > Unloop...");
    assert_eq!(app.dialogs.open_name(), Some("unloop"));
    (app, track, looped)
}

/// `unloop_app` in a test harness.
fn unloop_dialog() -> (Harness<'static, SoundApp>, TrackId, usize) {
    let (app, track, looped) = unloop_app();
    let mut h = Harness::builder().with_size(vec2(1600.0, 1000.0)).build_ui_state(
        |ui, app: &mut SoundApp| {
            let ctx = ui.ctx().clone();
            app.logic(&ctx);
            app.ui(ui);
        },
        app,
    );
    h.run_steps(3);
    (h, track, looped)
}

fn clips(h: &Harness<'_, SoundApp>, track: TrackId) -> Vec<Option<u64>> {
    loop_ids(h.state(), track)
}

/// The loop id of each clip on `track`.
fn loop_ids(app: &SoundApp, track: TrackId) -> Vec<Option<u64>> {
    app.engine.session().track(track).map(|t| t.clips().iter().map(|c| c.loop_id).collect()).unwrap_or_default()
}

#[test]
fn unloop_dialogs_flatten_keeps_each_iteration_as_its_own_clip() {
    let (mut h, track, looped) = unloop_dialog();
    assert_eq!(clips(&h, track).len(), looped, "opening the dialog does not unloop yet");
    h.get_by_role_and_label(Role::Button, "Flatten").click();
    h.run_steps(2);
    assert_eq!(h.state().dialogs.open_name(), None);
    assert_eq!(clips(&h, track), vec![None; looped]);
}

#[test]
fn enter_on_a_focused_flatten_flattens() {
    let (mut h, track, looped) = unloop_dialog();
    h.get_by_role_and_label(Role::Button, "Flatten").focus();
    h.run_steps(2);
    h.event(Event::Key { key: Key::Enter, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE });
    h.run_steps(2);
    assert_eq!(h.state().dialogs.open_name(), None);
    assert_eq!(clips(&h, track), vec![None; looped]);
}

#[test]
fn enter_with_no_button_focused_removes() {
    let (mut h, track, looped) = unloop_dialog();
    h.event(Event::Key { key: Key::Enter, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE });
    h.run_steps(2);
    assert_eq!(h.state().dialogs.open_name(), None);
    assert_eq!(clips(&h, track).len(), looped - 2, "Enter presses Remove, which keeps only the source clip");
}

#[test]
fn enter_on_a_focused_cancel_leaves_the_loop() {
    let (mut h, track, _) = unloop_dialog();
    let before = clips(&h, track);
    h.get_by_role_and_label(Role::Button, "Cancel").focus();
    h.run_steps(2);
    h.event(Event::Key { key: Key::Enter, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE });
    h.run_steps(2);
    assert_eq!(h.state().dialogs.open_name(), None);
    assert_eq!(clips(&h, track), before);
}

#[test]
fn a_flatten_click_with_enter_in_the_same_frame_flattens() {
    let (h, _, _) = unloop_dialog();
    let pos = h.get_by_role_and_label(Role::Button, "Flatten").rect().center();
    // The harness sends a pointer release and a key press in separate frames, so the frames are run by hand.
    let (mut app, track, looped) = unloop_app();
    let ctx = egui::Context::default();
    let button = |pressed| Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE };
    for _ in 0..3 {
        run_frame(&ctx, &mut app, vec![]);
    }
    run_frame(&ctx, &mut app, vec![Event::PointerMoved(pos)]);
    run_frame(&ctx, &mut app, vec![button(true)]);
    run_frame(&ctx, &mut app, vec![button(false), enter_key(false)]);
    assert_eq!(app.dialogs.open_name(), None);
    assert_eq!(loop_ids(&app, track), vec![None; looped]);
}

/// Runs one frame of `app` with `events`.
fn run_frame(ctx: &egui::Context, app: &mut SoundApp, events: Vec<Event>) {
    let raw = RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1600.0, 1000.0))), events, ..Default::default() };
    ctx.run_ui(raw, |ui| {
        app.logic(ui.ctx());
        app.ui(ui);
    })
    .textures_delta
    .clear();
}

fn enter_key(repeat: bool) -> Event {
    Event::Key { key: Key::Enter, physical_key: None, pressed: true, repeat, modifiers: Modifiers::NONE }
}

fn enter_up() -> Event {
    Event::Key { key: Key::Enter, physical_key: None, pressed: false, repeat: false, modifiers: Modifiers::NONE }
}

#[test]
fn an_unloop_dialog_that_replaces_an_open_dialog_ignores_the_enter_of_its_first_frame() {
    let (mut app, track, looped) = looped_app();
    let before = loop_ids(&app, track);
    let ctx = egui::Context::default();
    menus::invoke_menu(&mut app, "setup.session", "Setup > Session");
    run_frame(&ctx, &mut app, vec![]);
    run_frame(&ctx, &mut app, vec![]);
    assert_eq!(app.dialogs.open_name(), Some("session"));
    menus::invoke_menu(&mut app, "clip.unloop", "Clip > Unloop...");
    run_frame(&ctx, &mut app, vec![enter_key(false), enter_up()]);
    assert_eq!(app.dialogs.open_name(), Some("unloop"), "the opening Enter leaves the Unloop dialog open");
    assert_eq!(loop_ids(&app, track), before, "the opening Enter does not unloop");
    run_frame(&ctx, &mut app, vec![enter_key(false)]);
    assert_eq!(app.dialogs.open_name(), None);
    assert_eq!(loop_ids(&app, track).len(), looped - 2, "the next Enter presses Remove");
}

#[test]
fn a_held_enter_does_not_press_remove_until_it_is_pressed_again() {
    let (mut app, track, looped) = looped_app();
    let before = loop_ids(&app, track);
    let ctx = egui::Context::default();
    run_frame(&ctx, &mut app, vec![]);
    menus::invoke_menu(&mut app, "clip.unloop", "Clip > Unloop...");
    run_frame(&ctx, &mut app, vec![enter_key(false)]);
    run_frame(&ctx, &mut app, vec![enter_key(true)]);
    run_frame(&ctx, &mut app, vec![enter_key(true)]);
    assert_eq!(app.dialogs.open_name(), Some("unloop"), "the held Enter's repeats leave the dialog open");
    assert_eq!(loop_ids(&app, track), before, "the held Enter's repeats do not unloop");
    run_frame(&ctx, &mut app, vec![enter_up()]);
    run_frame(&ctx, &mut app, vec![enter_key(false)]);
    assert_eq!(app.dialogs.open_name(), None);
    assert_eq!(loop_ids(&app, track).len(), looped - 2, "a new press of Enter presses Remove");
}

#[test]
fn unloop_dialogs_remove_keeps_only_the_source_clip() {
    let (mut h, track, looped) = unloop_dialog();
    h.get_by_role_and_label(Role::Button, "Remove").click();
    h.run_steps(2);
    assert_eq!(h.state().dialogs.open_name(), None);
    assert_eq!(clips(&h, track).len(), looped - 2);
}
