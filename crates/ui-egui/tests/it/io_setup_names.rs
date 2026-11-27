use egui::vec2;
use egui_kittest::{Harness, kittest::Queryable};
use serde_json::json;
use soundcraft_ui_egui::{Services, SoundApp, menus};

/// Setup › I/O shows a bus's whole name in its name field, as wide as an output path's.
#[test]
fn a_bus_name_field_is_wide_enough_for_its_name() {
    let mut app = SoundApp::new(soundcraft_engine::Engine::default(), None, Services::default());
    app.run("mix.new_bus", json!({"name": "Verb Bus"})).unwrap();
    menus::invoke_menu(&mut app, "window.io_setup", "Setup > I/O...");
    let mut h =
        Harness::builder().with_context_setup(soundcraft_ui_egui::framework::install_headless).with_size(vec2(1600.0, 1000.0)).build_ui_state(
            |ui, app: &mut SoundApp| {
                let ctx = ui.ctx().clone();
                app.logic(&ctx);
                app.ui(ui);
            },
            app,
        );
    for _ in 0..3 {
        h.step();
    }
    h.get_by_label("Outputs").click();
    for _ in 0..3 {
        h.step();
    }
    let output = h.get_by_label("Output path name").rect();
    h.get_by_label("Busses").click();
    for _ in 0..3 {
        h.step();
    }
    let bus = h.get_by_label("Bus name: Verb Bus").rect();
    assert!(bus.width() >= output.width() - 0.5, "bus name field {:.0} pt, output path name field {:.0} pt", bus.width(), output.width());
}

/// Typing a new name into an input path's name field renames the path when the field loses focus,
/// also while the window is in the background.
#[test]
fn typing_in_an_input_path_name_renames_it() {
    let mut app = SoundApp::new(soundcraft_engine::Engine::default(), None, Services::default());
    app.run("setup.io", json!({"action": "create_input", "name": "Input 1", "format": "stereo", "first_channel": 6})).unwrap();
    menus::invoke_menu(&mut app, "window.io_setup", "Setup > I/O...");
    let mut h =
        Harness::builder().with_context_setup(soundcraft_ui_egui::framework::install_headless).with_size(vec2(1600.0, 1000.0)).build_ui_state(
            |ui, app: &mut SoundApp| {
                let ctx = ui.ctx().clone();
                app.logic(&ctx);
                app.ui(ui);
            },
            app,
        );
    for _ in 0..3 {
        h.step();
    }
    h.get_by_label("Input path name").click();
    h.step();
    // As when the keys come from the control connection while the window is in the background.
    h.input_mut().focused = false;
    h.input_mut().events.push(egui::Event::Key {
        key: egui::Key::A,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::COMMAND,
    });
    h.step();
    h.input_mut().events.push(egui::Event::Text("PC1X".into()));
    h.step();
    h.input_mut().events.push(egui::Event::Key {
        key: egui::Key::Enter,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    });
    for _ in 0..3 {
        h.step();
    }
    assert_eq!(h.state().engine.session().inputs[0].name, "PC1X");
}

/// Setup › I/O's Default restores Pro Tools' default output paths (Reference Guide, printed p. 58
/// and 82–83): the unused path is deleted and the main path is "Out 1-2" on channels 1-2 again.
#[test]
fn default_restores_the_default_output_paths() {
    let mut app = SoundApp::new(soundcraft_engine::Engine::default(), None, Services::default());
    app.run("setup.io", json!({"action": "rename_output", "name": "Out 1-2", "new_name": "Monitors"})).unwrap();
    app.run("setup.io", json!({"action": "set_output", "name": "Monitors", "first_channel": 2})).unwrap();
    app.run("setup.io", json!({"action": "create_output", "name": "Spare", "first_channel": 4})).unwrap();
    menus::invoke_menu(&mut app, "window.io_setup", "Setup > I/O...");
    let mut h =
        Harness::builder().with_context_setup(soundcraft_ui_egui::framework::install_headless).with_size(vec2(1600.0, 1000.0)).build_ui_state(
            |ui, app: &mut SoundApp| {
                let ctx = ui.ctx().clone();
                app.logic(&ctx);
                app.ui(ui);
            },
            app,
        );
    for _ in 0..3 {
        h.step();
    }
    h.get_by_label("Outputs").click();
    for _ in 0..3 {
        h.step();
    }
    h.get_by_label("Default").click();
    for _ in 0..3 {
        h.step();
    }
    let outputs = &h.state().engine.session().outputs;
    assert_eq!(outputs.len(), 1, "{outputs:?}");
    assert_eq!((outputs[0].name.as_str(), outputs[0].first_channel), ("Out 1-2", 0));
}
