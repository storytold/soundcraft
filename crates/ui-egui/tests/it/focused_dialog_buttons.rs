//! Enter on a focused dialog button, checkbox, radio button or combo box acts on that widget and does not press the
//! dialog's default button.

use std::path::PathBuf;

use egui::accesskit::Role;
use egui::epaint::ClippedShape;
use egui::{Event, Key, Modifiers, PointerButton, Pos2, RawInput, Rect, Shape, pos2, vec2};
use egui_kittest::{Harness, kittest::Queryable};
use serde_json::json;
use soundcraft_audio_io::FileFormat;
use soundcraft_model::GroupId;
use soundcraft_ui_egui::dialogs::{Dialog, ask_to_save};
use soundcraft_ui_egui::{Services, SoundApp, menus};

fn harness(app: SoundApp) -> Harness<'static, SoundApp> {
    let mut h = Harness::builder().with_size(vec2(1600.0, 1000.0)).build_ui_state(
        |ui, app: &mut SoundApp| {
            let ctx = ui.ctx().clone();
            app.logic(&ctx);
            app.ui(ui);
        },
        app,
    );
    h.run_steps(3);
    h
}

/// Gives the button the keyboard focus, then presses Enter.
fn enter_on(h: &mut Harness<'_, SoundApp>, button: &str) {
    enter_on_widget(h, Role::Button, button);
}

/// Gives the widget the keyboard focus, then presses Enter.
fn enter_on_widget(h: &mut Harness<'_, SoundApp>, role: Role, label: &str) {
    h.get_by_role_and_label(role, label).focus();
    h.run_steps(2);
    h.event(Event::Key { key: Key::Enter, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE });
    h.run_steps(1);
    h.event(Event::Key { key: Key::Enter, physical_key: None, pressed: false, repeat: false, modifiers: Modifiers::NONE });
    h.run_steps(2);
}

#[test]
fn enter_on_a_focused_cancel_closes_the_dialog_without_its_action() {
    let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
    let groups = app.engine.session().groups.len();
    menus::invoke_menu(&mut app, "track.group", "Track > Group...");
    let mut h = harness(app);
    assert_eq!(h.state().dialogs.open_name(), Some("group"));
    enter_on(&mut h, "Cancel");
    assert_eq!(h.state().dialogs.open_name(), None);
    assert_eq!(h.state().engine.session().groups.len(), groups);
}

#[test]
fn enter_on_a_focused_dont_save_closes_without_saving() {
    let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
    app.run("mix.mute", json!({"track": "Kick"})).unwrap();
    menus::invoke_menu(&mut app, "session.close", "File > Close Session");
    let mut h = harness(app);
    assert_eq!(h.state().dialogs.open_name(), Some("save_changes"));
    enter_on(&mut h, "Don't Save");
    assert_eq!(h.state().dialogs.open_name(), None, "no Save As prompt");
    assert!(h.state().engine.session().tracks.is_empty());
}

#[test]
fn enter_on_a_focused_browse_browses_without_pressing_ok() {
    let picked = "picked/Song.scraft";
    let services = Services { pick_open: Some(Box::new(move |_, _| Some(PathBuf::from(picked)))), ..Services::default() };
    let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, services);
    menus::invoke_menu(&mut app, "session.open", "File > Open Session...");
    let mut h = harness(app);
    enter_on(&mut h, "Browse…");
    assert!(matches!(&h.state().dialogs.open, Some(Dialog::PathPrompt { path, .. }) if path == picked));
    assert!(h.state().engine.session().track_by_name("Kick").is_some(), "the demo session is still open");
}

#[test]
fn enter_on_a_focused_browse_in_bounce_mix_does_not_bounce() {
    let picked = "picked/Mix.flac";
    let services = Services { pick_save: Some(Box::new(move |_, _| Some(PathBuf::from(picked)))), ..Services::default() };
    let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, services);
    menus::invoke_menu(&mut app, "file.bounce_mix", "File > Bounce Mix...");
    let mut h = harness(app);
    enter_on(&mut h, "Browse…");
    assert!(h.state().bounce.is_none(), "Enter on Browse started a bounce");
    assert!(matches!(&h.state().dialogs.open, Some(Dialog::Bounce { path, format, .. }) if path == picked && *format == FileFormat::Flac));
}

#[test]
fn enter_on_a_focused_default_button_runs_its_action_once() {
    let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
    let tracks = app.engine.session().tracks.len();
    assert!(app.dialogs.open_for_command(&app.engine, "track.new"));
    let mut h = harness(app);
    enter_on(&mut h, "Create");
    assert_eq!(h.state().dialogs.open_name(), None);
    assert_eq!(h.state().engine.session().tracks.len(), tracks + 1);
}

#[test]
fn enter_on_a_focused_checkbox_toggles_it_and_keeps_the_dialog_open() {
    let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
    menus::invoke_menu(&mut app, "file.score_setup", "File > Score Setup...");
    let mut h = harness(app);
    let Some(Dialog::ScoreSetup { show_track_names: before, .. }) = h.state().dialogs.open.clone() else { panic!("Score Setup did not open") };
    enter_on_widget(&mut h, Role::CheckBox, "Show track names");
    assert!(matches!(&h.state().dialogs.open, Some(Dialog::ScoreSetup { show_track_names, .. }) if *show_track_names != before));
}

#[test]
fn enter_on_a_focused_radio_button_picks_it_and_keeps_the_dialog_open() {
    let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
    menus::invoke_menu(&mut app, "edit.fades_create", "Edit > Fades > Create...");
    let mut h = harness(app);
    assert_eq!(h.state().dialogs.open_name(), Some("fades"));
    enter_on_widget(&mut h, Role::RadioButton, "Linear");
    assert!(matches!(&h.state().dialogs.open, Some(Dialog::Fades { shape }) if shape == "linear"));
}

#[test]
fn enter_with_nothing_focused_presses_the_default_button() {
    let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
    menus::invoke_menu(&mut app, "edit.fades_create", "Edit > Fades > Create...");
    let mut h = harness(app);
    assert_eq!(h.state().dialogs.open_name(), Some("fades"));
    h.event(Event::Key { key: Key::Enter, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE });
    h.run_steps(2);
    assert_eq!(h.state().dialogs.open_name(), None);
}

#[test]
fn a_dialog_opened_while_a_window_button_has_the_focus_takes_enter() {
    let app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
    let mut h = harness(app);
    h.get_all_by_role(Role::Button).next().expect("the window has a button").focus();
    h.run_steps(2);
    assert!(h.ctx.memory(|m| m.focused().is_some()));
    menus::invoke_menu(h.state_mut(), "edit.fades_create", "Edit > Fades > Create...");
    h.run_steps(2);
    assert_eq!(h.state().dialogs.open_name(), Some("fades"));
    h.event(Event::Key { key: Key::Enter, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE });
    h.run_steps(2);
    assert_eq!(h.state().dialogs.open_name(), None, "Enter pressed Create");
}

#[test]
fn the_enter_that_opens_a_dialog_does_not_press_its_default_button() {
    let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
    let ctx = egui::Context::default();
    run_frame(&ctx, &mut app, vec![]);
    menus::invoke_menu(&mut app, "edit.fades_create", "Edit > Fades > Create...");
    run_frame(&ctx, &mut app, vec![enter_key(false), enter_up()]);
    assert_eq!(app.dialogs.open_name(), Some("fades"), "the opening Enter leaves the dialog open");
    run_frame(&ctx, &mut app, vec![enter_key(false)]);
    assert_eq!(app.dialogs.open_name(), None, "the next Enter presses Create");
}

/// Runs one frame of `app` with `events` and returns what it painted.
fn run_frame(ctx: &egui::Context, app: &mut SoundApp, events: Vec<Event>) -> Vec<ClippedShape> {
    let raw = RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1600.0, 1000.0))), events, ..Default::default() };
    let mut out = ctx.run_ui(raw, |ui| {
        app.logic(ui.ctx());
        app.ui(ui);
    });
    out.textures_delta.clear();
    out.shapes
}

fn enter_key(repeat: bool) -> Event {
    Event::Key { key: Key::Enter, physical_key: None, pressed: true, repeat, modifiers: Modifiers::NONE }
}

fn enter_up() -> Event {
    Event::Key { key: Key::Enter, physical_key: None, pressed: false, repeat: false, modifiers: Modifiers::NONE }
}

#[test]
fn the_enter_that_replaces_an_open_dialog_does_not_press_the_new_default_button() {
    let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
    let ctx = egui::Context::default();
    menus::invoke_menu(&mut app, "setup.session", "Setup > Session");
    run_frame(&ctx, &mut app, vec![]);
    run_frame(&ctx, &mut app, vec![]);
    assert_eq!(app.dialogs.open_name(), Some("session"));
    menus::invoke_menu(&mut app, "edit.fades_create", "Edit > Fades > Create...");
    run_frame(&ctx, &mut app, vec![enter_key(false), enter_up()]);
    assert_eq!(app.dialogs.open_name(), Some("fades"), "the opening Enter leaves the new dialog open");
    run_frame(&ctx, &mut app, vec![enter_key(false)]);
    assert_eq!(app.dialogs.open_name(), None, "the next Enter presses Create");
}

#[test]
fn a_rename_dialog_that_replaces_an_open_dialog_ignores_the_enter_of_its_first_frame() {
    let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
    let ctx = egui::Context::default();
    menus::invoke_menu(&mut app, "setup.session", "Setup > Session");
    run_frame(&ctx, &mut app, vec![]);
    run_frame(&ctx, &mut app, vec![]);
    assert_eq!(app.dialogs.open_name(), Some("session"));
    let id = app.engine.session().tracks[0].id;
    app.dialogs.open_rename_track(id, "Renamed");
    run_frame(&ctx, &mut app, vec![enter_key(false), enter_up()]);
    assert_eq!(app.dialogs.open_name(), Some("rename_track"), "the first frame's Enter leaves the new dialog open");
}

/// The first Kick clip of `app`.
fn kick_clip(app: &SoundApp) -> soundcraft_model::ClipId {
    app.engine.session().track_by_name("Kick").unwrap().clips()[0].id
}

#[test]
fn clear_auto_rename_modify_group_and_save_changes_replacing_a_dialog_ignore_the_enter_of_their_first_frame() {
    type Open = fn(&mut SoundApp);
    let cases: [(&str, Open); 4] = [
        ("clear_clips", |app| {
            let clip = kick_clip(app);
            assert!(app.dialogs.open_clear(&app.engine, vec![clip], vec![]));
        }),
        ("auto_rename", |app| {
            let clip = kick_clip(app);
            assert!(app.dialogs.open_auto_rename(&app.engine, vec![clip]));
        }),
        ("modify_group", |app| {
            let track = app.engine.session().tracks[0].id.0;
            let group = app.run("track.group", json!({"tracks": [track], "name": "Drums"})).unwrap()["group"].as_u64().unwrap();
            assert!(app.dialogs.open_modify_group(&app.engine, GroupId(group)));
        }),
        ("save_changes", |app| {
            app.run("mix.mute", json!({"track": "Kick"})).unwrap();
            assert!(ask_to_save(app, "session.close", &json!({})));
        }),
    ];
    for (name, open) in cases {
        let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
        let ctx = egui::Context::default();
        menus::invoke_menu(&mut app, "setup.session", "Setup > Session");
        run_frame(&ctx, &mut app, vec![]);
        run_frame(&ctx, &mut app, vec![]);
        open(&mut app);
        run_frame(&ctx, &mut app, vec![enter_key(false), enter_up()]);
        assert_eq!(app.dialogs.open_name(), Some(name), "the first frame's Enter leaves the new dialog open");
    }
}

#[test]
fn a_tempo_change_dialog_that_replaces_an_open_dialog_ignores_the_enter_of_its_first_frame() {
    let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
    let ctx = egui::Context::default();
    menus::invoke_menu(&mut app, "setup.session", "Setup > Session");
    run_frame(&ctx, &mut app, vec![]);
    let shapes = run_frame(&ctx, &mut app, vec![]);
    assert_eq!(app.dialogs.open_name(), Some("session"));
    let [left, ..] = app.edit_layout.timeline;
    let y = shapes
        .iter()
        .find_map(|c| match &c.shape {
            Shape::Text(t) if t.galley.text() == "Tempo" && t.pos.x < left => Some(t.galley.rect.translate(t.pos.to_vec2()).center().y),
            _ => None,
        })
        .expect("the Tempo ruler's label");
    let at = pos2(left + 50.0, y);
    let button = |pressed| Event::PointerButton { pos: at, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE };
    for pressed in [true, false, true] {
        run_frame(&ctx, &mut app, vec![Event::PointerMoved(at), button(pressed)]);
    }
    run_frame(&ctx, &mut app, vec![button(false), enter_key(false), enter_up()]);
    assert_eq!(app.dialogs.open_name(), Some("tempo_change"), "the first frame's Enter leaves the new dialog open");
}

#[test]
fn a_held_enter_does_not_press_the_default_button_until_it_is_pressed_again() {
    let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
    let ctx = egui::Context::default();
    run_frame(&ctx, &mut app, vec![]);
    menus::invoke_menu(&mut app, "edit.fades_create", "Edit > Fades > Create...");
    run_frame(&ctx, &mut app, vec![enter_key(false)]);
    run_frame(&ctx, &mut app, vec![enter_key(true)]);
    run_frame(&ctx, &mut app, vec![enter_key(true)]);
    assert_eq!(app.dialogs.open_name(), Some("fades"), "the held Enter's repeats leave the dialog open");
    run_frame(&ctx, &mut app, vec![enter_up()]);
    run_frame(&ctx, &mut app, vec![enter_key(false)]);
    assert_eq!(app.dialogs.open_name(), None, "a new press of Enter presses Create");
}

#[test]
fn enter_on_a_focused_combo_box_opens_it_and_keeps_the_dialog_open() {
    let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
    menus::invoke_menu(&mut app, "setup.session", "Setup > Session");
    let mut h = harness(app);
    assert_eq!(h.state().dialogs.open_name(), Some("session"));
    assert!(h.query_by_label("47.952").is_none());
    enter_on_widget(&mut h, Role::ComboBox, "Timecode Rate");
    assert_eq!(h.state().dialogs.open_name(), Some("session"));
    assert!(h.query_by_label("47.952").is_some(), "the Timecode Rate list is open");
}
