use egui_kittest::{Harness, kittest::Queryable};
use serde_json::json;
use soundcraft_ui_egui::{Services, SoundApp, menus};

#[test]
fn preferences_menu_opens_a_window_without_changing_the_session() {
    let mut app = SoundApp::new(soundcraft_engine::Engine::default(), None, Services::default());
    let before = serde_json::to_value(app.engine.session()).unwrap();
    // The existing engine query stays headless; only the menu opens the window.
    assert!(!soundcraft_engine::find_command("setup.preferences").unwrap().journal);
    assert!(app.run("setup.preferences", json!({})).unwrap().get("edit").is_some());
    let aliases = menus::ui_aliases();
    let command = menus::command_for_path("Setup > Preferences...", &aliases).unwrap();
    menus::invoke_menu(&mut app, &command, "Setup > Preferences...");
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_ui_state(
        |ui, app: &mut SoundApp| {
            let ctx = ui.ctx().clone();
            app.logic(&ctx);
            app.ui(ui);
        },
        app,
    );
    h.run_steps(5);
    h.get_by_label("Preferences");
    h.get_by_label("Appearance");
    h.get_by_label("Plugin folders");
    assert_eq!(serde_json::to_value(h.state().engine.session()).unwrap(), before);
    if let Ok(path) = std::env::var("SOUNDCRAFT_PREFERENCES_SHOT") {
        h.render().unwrap().save(path).unwrap();
    }
}

fn folder() -> String {
    std::env::temp_dir().join("soundcraft-preferences-test-plugins").to_string_lossy().into_owned()
}

#[test]
fn preferences_shortcut_works_on_mac_windows_and_linux() {
    use egui::{Key, Modifiers, os::OperatingSystem};
    let mac = Modifiers { mac_cmd: true, command: true, alt: true, ..Modifiers::NONE };
    let pc = Modifiers { ctrl: true, command: true, alt: true, ..Modifiers::NONE };
    for (os, modifiers) in [(OperatingSystem::Mac, mac), (OperatingSystem::Windows, pc), (OperatingSystem::Nix, pc)] {
        let mut app = SoundApp::new(soundcraft_engine::Engine::default(), None, Services::default());
        app.run("setup.preferences", json!({})).unwrap();
        assert!(!app.preferences.open, "the engine query must not open a dialog");
        let ctx = egui::Context::default();
        ctx.set_os(os);
        let events =
            [true, false].map(|pressed| egui::Event::Key { key: Key::Comma, physical_key: None, pressed, repeat: false, modifiers }).to_vec();
        ctx.run_ui(egui::RawInput { events, ..Default::default() }, |ui| soundcraft_ui_egui::shortcuts::handle(&mut app, ui.ctx()))
            .textures_delta
            .clear();
        assert!(app.preferences.open, "{os:?}");
        app.run("window.hide_floating", json!({})).unwrap();
        assert!(!app.preferences.open);

        let mut selected = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
        let clip = selected.engine.session().track_by_name("Kick").unwrap().clips()[0].clone();
        selected.engine.session_mut().edit.selected_clips = vec![clip.id];
        selected.engine.session_mut().edit.selection.start = clip.start + 10;
        selected.engine.session_mut().edit.selection.end = clip.start + 10;
        let events =
            [true, false].map(|pressed| egui::Event::Key { key: Key::Comma, physical_key: None, pressed, repeat: false, modifiers }).to_vec();
        ctx.run_ui(egui::RawInput { events, ..Default::default() }, |ui| soundcraft_ui_egui::shortcuts::handle(&mut selected, ui.ctx()))
            .textures_delta
            .clear();
        assert!(selected.preferences.open, "Preferences must open even with a selected clip");
        assert_eq!(selected.engine.session().find_clip(clip.id).unwrap().1.sync_point, 0);
        selected.run("window.preferences", json!({"value":false})).unwrap();
        let modifiers = Modifiers { alt: false, ..modifiers };
        let events =
            [true, false].map(|pressed| egui::Event::Key { key: Key::Comma, physical_key: None, pressed, repeat: false, modifiers }).to_vec();
        ctx.run_ui(egui::RawInput { events, ..Default::default() }, |ui| soundcraft_ui_egui::shortcuts::handle(&mut selected, ui.ctx()))
            .textures_delta
            .clear();
        assert!(!selected.preferences.open, "the clip shortcut must not open Preferences");
        assert_eq!(selected.engine.session().find_clip(clip.id).unwrap().1.sync_point, 10);
    }
}

#[test]
fn automation_cannot_change_plugin_folders() {
    let mut app = SoundApp::new(soundcraft_engine::Engine::default(), None, Services::default());
    let path = folder();
    app.ui.plugin_folders = vec![path.clone()];
    assert_eq!(app.ui.plugin_folders.as_slice(), std::slice::from_ref(&path));
    let saved = serde_json::to_value(&app.ui).unwrap();
    let restored: soundcraft_ui_egui::UiState = serde_json::from_value(saved).unwrap();
    assert_eq!(restored.plugin_folders.as_slice(), std::slice::from_ref(&path));
    let legacy: soundcraft_ui_egui::UiState = serde_json::from_value(json!({"ui_scale":1.25})).unwrap();
    assert!(legacy.plugin_folders.is_empty());
    let other = std::env::temp_dir().join("untrusted-plugins").to_string_lossy().into_owned();
    for folders in [json!([other]), json!([]), json!(null), json!(["relative"])] {
        assert!(app.run("ui.plugin_folders", json!({"folders":folders})).is_err());
        let before = serde_json::to_value(&app.ui).unwrap();
        assert!(app.run("ui.set", json!({"plugin_folders":folders, "ui_scale":1.75})).is_err());
        assert_eq!(serde_json::to_value(&app.ui).unwrap(), before, "rejected patches must be atomic");
        assert_eq!(app.ui.plugin_folders.as_slice(), std::slice::from_ref(&path), "failed updates must retain saved folders");
    }
    app.run("ui.set", json!({"ui_scale":1.25})).unwrap();
    assert_eq!(app.ui.ui_scale, 1.25);
    assert!(!menus::UI_COMMANDS.iter().any(|c| c.0 == "ui.plugin_folders"));
    assert_eq!(app.ui.plugin_folders, [path]);
    assert!(!app.preferences.open, "setting folders programmatically must stay headless");
}

#[test]
fn plugin_folder_picker_requires_apply_and_closing_discards_draft() {
    let path = folder();
    let chosen = path.clone();
    let services = Services { pick_folder: Some(Box::new(move |_| Some(chosen.clone()))), ..Default::default() };
    let mut app = SoundApp::new(soundcraft_engine::Engine::default(), None, services);
    app.run("window.preferences", json!({})).unwrap();
    let mut h = Harness::builder()
        .with_size(egui::vec2(1400.0, 900.0))
        .build_ui_state(|ui, app: &mut SoundApp| soundcraft_ui_egui::preferences::show(app, ui.ctx()), app);
    h.run_steps(3);
    h.get_by_label("Add folder…").click();
    h.run_steps(2);
    assert!(h.state().ui.plugin_folders.is_empty());
    h.state_mut().run("window.preferences", json!({"value":false})).unwrap();
    h.run_steps(2);
    h.state_mut().run("window.preferences", json!({"value":true})).unwrap();
    h.run_steps(3);
    h.get_by_label("Apply").click();
    h.run_steps(2);
    assert!(h.state().ui.plugin_folders.is_empty(), "closing must discard unapplied edits");
    h.get_by_label("Add folder…").click();
    h.run_steps(2);
    h.get_by_label("Revert").click();
    h.run_steps(2);
    h.get_by_label("Apply").click();
    h.run_steps(2);
    assert!(h.state().ui.plugin_folders.is_empty());
    h.get_by_label("Add folder…").click();
    h.run_steps(2);
    h.get_by_label("Apply").click();
    h.run_steps(2);
    assert_eq!(h.state().ui.plugin_folders, [path]);
    h.get_by_label("Appearance").click();
    h.run_steps(2);
    assert!(h.state().extra.show_ui_customization);
}

#[test]
fn control_and_mcp_dispatch_cannot_persist_plugin_folders() {
    use soundcraft_ui_egui::{ControlRequest, control};
    let (tx, rx) = std::sync::mpsc::channel();
    let mut app = SoundApp::new(soundcraft_engine::Engine::default(), None, Services::default()).with_control(rx);
    let path = folder();
    let mut before = serde_json::to_value(&app.ui).unwrap();
    before["status"] = serde_json::Value::Null;
    for (method, params) in [
        ("ui.set", json!({"plugin_folders":[path],"ui_scale":1.5})),
        ("engine.execute", json!({"command":"ui.set","params":{"plugin_folders":[path]}})),
        ("command", json!({"command":"ui.plugin_folders","params":{"folders":[path]}})),
    ] {
        let (req, reply) = ControlRequest::new(method, params);
        tx.send(req).unwrap();
        control::drain(&mut app, &egui::Context::default());
        assert_eq!(reply.try_recv().unwrap()["ok"], false, "{method}");
        // Unknown commands may report an error in the status bar, but cannot change settings.
        let mut after = serde_json::to_value(&app.ui).unwrap();
        after["status"] = serde_json::Value::Null;
        assert_eq!(after, before);
    }
}
