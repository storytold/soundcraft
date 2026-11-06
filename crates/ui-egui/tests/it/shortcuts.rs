use egui::os::OperatingSystem;
use egui::{Key, Modifiers};
use serde_json::json;
use soundcraft_ui_egui::{MainWindow, Services, SoundApp, shortcuts};

fn press(ctx: &egui::Context, app: &mut SoundApp, key: Key, modifiers: Modifiers) {
    let events = [true, false].map(|pressed| egui::Event::Key { key, physical_key: None, pressed, repeat: false, modifiers }).to_vec();
    ctx.run_ui(egui::RawInput { events, ..Default::default() }, |ui| shortcuts::handle(app, ui.ctx())).textures_delta.clear();
}

#[test]
fn ctrl_shift_arrows_nudge_the_selected_clip_gain() {
    let pc_ctrl = Modifiers { ctrl: true, command: true, ..Modifiers::NONE };
    for (os, ctrl) in [(OperatingSystem::Mac, Modifiers::CTRL), (OperatingSystem::Windows, pc_ctrl), (OperatingSystem::Nix, pc_ctrl)] {
        let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
        let kick = app.engine.session().track_by_name("Kick").unwrap().clips()[0].id;
        app.engine.execute("edit.select", &json!({"clips": [kick.0]})).unwrap();
        let ctx = egui::Context::default();
        ctx.set_os(os);
        let ctrl_shift = Modifiers { shift: true, ..ctrl };
        press(&ctx, &mut app, Key::ArrowUp, ctrl_shift);
        press(&ctx, &mut app, Key::ArrowUp, ctrl_shift);
        press(&ctx, &mut app, Key::ArrowDown, ctrl_shift);
        assert_eq!(app.engine.session().find_clip(kick).unwrap().1.gain_db, 1.0, "{os:?}");
    }
}

fn type_in_app(ctx: &egui::Context, app: &mut SoundApp, keys: &[(Key, Modifiers)]) {
    let events = keys
        .iter()
        .flat_map(|&(key, modifiers)| [true, false].map(|pressed| egui::Event::Key { key, physical_key: None, pressed, repeat: false, modifiers }))
        .collect();
    ctx.run_ui(egui::RawInput { events, ..Default::default() }, |ui| {
        app.logic(ui.ctx());
        app.ui(ui);
    })
    .textures_delta
    .clear();
}

#[test]
fn tab_leaves_the_keys_to_the_shortcuts() {
    let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
    let ctx = egui::Context::default();
    for _ in 0..3 {
        type_in_app(&ctx, &mut app, &[]);
    }
    for tab in [Modifiers::NONE, Modifiers::SHIFT] {
        type_in_app(&ctx, &mut app, &[(Key::Tab, tab)]);
        assert_eq!(ctx.memory(|m| m.focused()), None, "{tab:?}");
        let zoom = app.engine.session().edit.zoom.samples_per_px;
        type_in_app(&ctx, &mut app, &[(Key::T, Modifiers::NONE)]);
        assert_eq!(app.engine.session().edit.zoom.samples_per_px, zoom / 2.0, "{tab:?}");
        type_in_app(&ctx, &mut app, &[(Key::R, Modifiers::NONE)]);
        assert_eq!(app.engine.session().edit.zoom.samples_per_px, zoom, "{tab:?}");
    }
}

#[test]
fn bare_plus_and_minus_nudge_the_selected_clip_in_edit() {
    // A laptop keyboard has no dedicated "+": typing it needs Shift, which is already claimed by
    // the Grid-value shortcut (#125 above). So nudge must fire on the bare, unshifted key the "+"
    // symbol sits on, same as "-" already does; `Key::Plus` stays bound too for a keyboard (e.g. a
    // numpad) that does send it on its own.
    let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
    let kick = app.engine.session().track_by_name("Kick").unwrap().clips()[0].id;
    app.engine.execute("edit.select", &json!({"clips": [kick.0]})).unwrap();
    let ctx = egui::Context::default();
    let start = |app: &SoundApp| app.engine.session().find_clip(kick).unwrap().1.start;
    let original = start(&app);

    press(&ctx, &mut app, Key::Minus, Modifiers::NONE);
    let after_minus = start(&app);
    assert!(after_minus < original, "bare Minus must nudge left");

    press(&ctx, &mut app, Key::Equals, Modifiers::NONE);
    assert_eq!(start(&app), original, "bare Equals must nudge back right, same step as Minus");

    press(&ctx, &mut app, Key::Plus, Modifiers::NONE);
    assert!(start(&app) > original, "Key::Plus (e.g. a numpad +) must still nudge right");

    // Shift+Equals is the pre-existing, separate Grid-value shortcut: it must not also nudge.
    let before_shift = start(&app);
    let grid = app.engine.session().edit.grid;
    press(&ctx, &mut app, Key::Equals, Modifiers::SHIFT);
    assert_eq!(start(&app), before_shift, "Shift+Equals steps the Grid value, not the clip");
    assert_ne!(app.engine.session().edit.grid, grid, "Shift+Equals must still step the Grid value");

    // Alt+Equals is the registered `window.event_list` shortcut: it must not also nudge.
    let before_alt = start(&app);
    assert!(!app.ui.show_event_list);
    press(&ctx, &mut app, Key::Equals, Modifiers::ALT);
    assert_eq!(start(&app), before_alt, "Alt+Equals opens the MIDI Event List, not a nudge");
    assert!(app.ui.show_event_list, "Alt+Equals must still open the MIDI Event List");
}

#[test]
fn cmd_equals_toggles_mix_and_edit_without_zooming() {
    let mac_cmd = Modifiers { mac_cmd: true, command: true, ..Modifiers::NONE };
    let pc_ctrl = Modifiers { ctrl: true, command: true, ..Modifiers::NONE };
    for (os, cmd) in [(OperatingSystem::Mac, mac_cmd), (OperatingSystem::Windows, pc_ctrl), (OperatingSystem::Nix, pc_ctrl)] {
        let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
        let ctx = egui::Context::default();
        ctx.set_os(os);
        ctx.set_zoom_factor(1.25);
        press(&ctx, &mut app, Key::Equals, cmd);
        assert_eq!(app.ui.window, MainWindow::Mix, "{os:?}");
        press(&ctx, &mut app, Key::Equals, cmd);
        press(&ctx, &mut app, Key::Num0, cmd);
        press(&ctx, &mut app, Key::Minus, cmd);
        ctx.run_ui(egui::RawInput::default(), |_| {}).textures_delta.clear();
        assert_eq!(app.ui.window, MainWindow::Edit, "{os:?}");
        assert_eq!(ctx.zoom_factor(), 1.25, "{os:?}");
    }
}

/// One full app frame on a 1600×1000 screen.
fn app_frame(ctx: &egui::Context, app: &mut SoundApp, events: Vec<egui::Event>) {
    let screen_rect = Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1600.0, 1000.0)));
    ctx.run_ui(egui::RawInput { screen_rect, events, ..Default::default() }, |ui| {
        app.logic(ui.ctx());
        app.ui(ui);
    })
    .textures_delta
    .clear();
}

fn arrow(key: Key) -> Vec<egui::Event> {
    [true, false].map(|pressed| egui::Event::Key { key, physical_key: None, pressed, repeat: false, modifiers: Modifiers::NONE }).to_vec()
}

/// Where `at` is drawn in the Edit window's timeline, and the timeline's middle (#1376).
fn x_and_middle(app: &SoundApp, at: i64) -> (f32, f32) {
    let tl = soundcraft_ui_egui::edit_window::rect(app.edit_layout.timeline);
    (soundcraft_ui_egui::edit_window::x_of(app.engine.session(), tl, at), tl.center().x)
}

#[test]
fn left_and_right_arrows_centre_the_edit_window_on_the_selected_clip() {
    let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
    let ctx = egui::Context::default();
    let (clip_id, clip) = app.engine.session().tracks.iter().flat_map(|t| t.clips()).find(|c| c.start > 100_000).map(|c| (c.id, c.range())).unwrap();
    app.run("view.zoom_set", json!({"samples_per_px": 10.0})).unwrap();
    app.run("view.scroll", json!({"to": 0})).unwrap();
    for _ in 0..3 {
        app_frame(&ctx, &mut app, vec![]);
    }
    app.run("edit.select", json!({"clips": [clip_id.0]})).unwrap();
    assert!(x_and_middle(&app, clip.start).0 > app.edit_layout.timeline[2], "the clip starts off-screen");

    app_frame(&ctx, &mut app, arrow(Key::ArrowLeft));
    let (x, middle) = x_and_middle(&app, clip.start);
    assert!((x - middle).abs() < 1.0, "Left: the clip start at {x}, the middle at {middle}");
    app_frame(&ctx, &mut app, arrow(Key::ArrowRight));
    let (x, middle) = x_and_middle(&app, clip.end);
    assert!((x - middle).abs() < 1.0, "Right: the clip end at {x}, the middle at {middle}");

    // An insertion point alone leaves the view where it is.
    app.run("edit.select", json!({"start": clip.start})).unwrap();
    let scroll = app.engine.session().edit.zoom.scroll;
    app_frame(&ctx, &mut app, arrow(Key::ArrowLeft));
    app_frame(&ctx, &mut app, arrow(Key::ArrowRight));
    assert_eq!(app.engine.session().edit.zoom.scroll, scroll);
}

/// Over the piano roll the arrows move the selected note and leave the Edit window's view alone.
#[test]
fn the_piano_roll_keeps_the_plain_arrows() {
    let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
    app.ui.show_midi_editor = true;
    app.run("edit.select", json!({"tracks": ["Keys"], "start": 0, "end": 480_000})).unwrap();
    app.run("view.zoom_set", json!({"samples_per_px": 10.0})).unwrap();
    app.run("view.scroll", json!({"to": 1_000_000})).unwrap();
    let ctx = egui::Context::default();
    for _ in 0..6 {
        app_frame(&ctx, &mut app, vec![]);
    }
    let body = ctx.data(|d| d.get_temp::<egui::Rect>(egui::Id::unique("soundcraft-midi-body"))).unwrap();
    app_frame(&ctx, &mut app, vec![egui::Event::PointerMoved(body.center())]);
    app.midi.selected = vec![0];
    let note_start = |app: &SoundApp| {
        let clip = soundcraft_ui_egui::midi_editor::target_clip(app).unwrap();
        match &app.engine.session().find_clip(clip).unwrap().1.content {
            soundcraft_model::ClipContent::Midi { sequence } => sequence.notes[app.midi.selected[0]].start,
            _ => panic!("not a MIDI clip"),
        }
    };
    let (before, scroll) = (note_start(&app), app.engine.session().edit.zoom.scroll);
    app_frame(&ctx, &mut app, arrow(Key::ArrowRight));
    assert!(note_start(&app) > before, "the note moved right");
    assert_eq!(app.engine.session().edit.zoom.scroll, scroll, "the Edit window kept its view");
}
