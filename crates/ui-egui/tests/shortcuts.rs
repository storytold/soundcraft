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

#[test]
fn optional_window_and_zoom_keys_are_disabled_until_enabled() {
    let cmd = Modifiers { mac_cmd: true, command: true, ..Modifiers::NONE };
    let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
    let ctx = egui::Context::default();
    ctx.set_os(OperatingSystem::Mac);

    press(&ctx, &mut app, Key::Num2, cmd);
    assert_eq!(app.ui.window, MainWindow::Edit);

    let mut native = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
    let native_ctx = egui::Context::default();
    native_ctx.set_os(OperatingSystem::Mac);
    press(&native_ctx, &mut native, Key::Num1, cmd);
    assert!(native.ui.show_transport);

    let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
    let ctx = egui::Context::default();
    ctx.set_os(OperatingSystem::Mac);
    app.ui.shortcut_options.direct_window_keys = true;
    press(&ctx, &mut app, Key::Num2, cmd);
    assert_eq!(app.ui.window, MainWindow::Mix);
    press(&ctx, &mut app, Key::Num1, cmd);
    assert_eq!(app.ui.window, MainWindow::Edit);
    assert!(!app.ui.show_transport);

    let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
    let ctx = egui::Context::default();
    ctx.set_os(OperatingSystem::Mac);
    let before: Vec<_> = app.engine.session().tracks.iter().map(|track| track.height).collect();
    press(&ctx, &mut app, Key::ArrowDown, cmd);
    assert_eq!(app.engine.session().tracks.iter().map(|track| track.height).collect::<Vec<_>>(), before);

    let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
    let ctx = egui::Context::default();
    ctx.set_os(OperatingSystem::Mac);
    app.ui.shortcut_options.arrow_zoom_keys = true;
    press(&ctx, &mut app, Key::ArrowDown, cmd);
    let selected = app.engine.session().edit.selected_tracks.clone();
    let after: Vec<_> = app.engine.session().tracks.iter().map(|track| track.height).collect();
    assert!(app.engine.session().tracks.iter().zip(before.iter().zip(&after)).all(|(track, (old, new))| selected.contains(&track.id) || old == new));
    assert!(app.engine.session().tracks.iter().zip(before.iter().zip(&after)).any(|(track, (old, new))| selected.contains(&track.id) && old != new));

    // Enabling the Logic-style option must not intercept the built-in PC key map.
    let pc_ctrl = Modifiers { ctrl: true, command: true, ..Modifiers::NONE };
    let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
    let ctx = egui::Context::default();
    ctx.set_os(OperatingSystem::Windows);
    app.ui.shortcut_options.arrow_zoom_keys = true;
    let before: Vec<_> = app.engine.session().tracks.iter().map(|track| track.height).collect();
    let selected = app.engine.session().edit.selected_tracks.clone();
    press(&ctx, &mut app, Key::ArrowDown, pc_ctrl);
    let after: Vec<_> = app.engine.session().tracks.iter().map(|track| track.height).collect();
    assert!(app.engine.session().tracks.iter().zip(before.iter().zip(&after)).all(|(track, (old, new))| selected.contains(&track.id) || old == new));
}
