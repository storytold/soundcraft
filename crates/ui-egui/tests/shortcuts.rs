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
fn return_goes_to_the_session_start_and_the_keypad_enter_adds_a_marker() {
    let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
    let ctx = egui::Context::default();
    let markers = app.engine.session().markers.len();
    app.run("transport.locate", json!({"at": 96_000})).unwrap();
    press(&ctx, &mut app, Key::Enter, Modifiers::NONE);
    assert_eq!(app.engine.session().edit.playhead, 0);
    assert_eq!(app.engine.session().markers.len(), markers);
    app.run("transport.locate", json!({"at": 96_000})).unwrap();
    app.keypad_enter.store(true, std::sync::atomic::Ordering::Relaxed);
    press(&ctx, &mut app, Key::Enter, Modifiers::NONE);
    assert_eq!(app.engine.session().edit.playhead, 96_000);
    assert_eq!(app.engine.session().markers.len(), markers + 1);
}
