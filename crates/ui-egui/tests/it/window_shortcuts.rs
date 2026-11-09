use egui::os::OperatingSystem;
use egui::{Key, Modifiers};
use soundcraft_ui_egui::{MainWindow, Services, SoundApp, shortcuts};

const MAC_CMD: Modifiers = Modifiers { mac_cmd: true, command: true, ..Modifiers::NONE };

fn press(ctx: &egui::Context, app: &mut SoundApp, key: Key, modifiers: Modifiers) {
    let events = [true, false].map(|pressed| egui::Event::Key { key, physical_key: None, pressed, repeat: false, modifiers }).to_vec();
    ctx.run_ui(egui::RawInput { events, ..Default::default() }, |ui| shortcuts::handle(app, ui.ctx())).textures_delta.clear();
}

#[test]
fn window_shortcuts_open_and_close_windows() {
    let pc_ctrl = Modifiers { ctrl: true, command: true, ..Modifiers::NONE };
    for (os, cmd) in [(OperatingSystem::Mac, MAC_CMD), (OperatingSystem::Windows, pc_ctrl), (OperatingSystem::Nix, pc_ctrl)] {
        let mut app = SoundApp::new(soundcraft_engine::Engine::default(), None, Services::default());
        let ctx = egui::Context::default();
        ctx.set_os(os);
        for key in [Key::Num1, Key::Num3, Key::Num4, Key::Num5, Key::Num9] {
            press(&ctx, &mut app, key, cmd);
        }
        press(&ctx, &mut app, Key::M, Modifiers { alt: true, ..cmd });
        let ui = &app.ui;
        assert!(
            ui.show_transport && ui.show_big_counter && ui.show_automation && ui.show_memory_locations && ui.show_video && ui.narrow_mix,
            "{os:?}"
        );
        press(&ctx, &mut app, Key::W, cmd);
        assert!(!app.ui.show_transport && !app.ui.show_big_counter && !app.ui.show_memory_locations, "{os:?}");
    }
}

#[test]
fn cmd_ctrl_equals_opens_the_score_editor_on_a_mac() {
    let mut app = SoundApp::new(soundcraft_engine::Engine::default(), None, Services::default());
    let ctx = egui::Context::default();
    ctx.set_os(OperatingSystem::Mac);
    press(&ctx, &mut app, Key::Equals, Modifiers { ctrl: true, ..MAC_CMD });
    assert!(app.ui.show_score);
    assert_eq!(app.ui.window, MainWindow::Edit);
    press(&ctx, &mut app, Key::Equals, MAC_CMD);
    assert_eq!(app.ui.window, MainWindow::Mix);
}
