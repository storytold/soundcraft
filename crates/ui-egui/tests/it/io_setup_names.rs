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
    let bus = h.get_by_label("Bus name: Verb Bus").rect();
    let output = h.get_by_label("Output path name").rect();
    assert!(bus.width() >= output.width() - 0.5, "bus name field {:.0} pt, output path name field {:.0} pt", bus.width(), output.width());
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
    h.get_by_label("Default").click();
    for _ in 0..3 {
        h.step();
    }
    let outputs = &h.state().engine.session().outputs;
    assert_eq!(outputs.len(), 1, "{outputs:?}");
    assert_eq!((outputs[0].name.as_str(), outputs[0].first_channel), ("Out 1-2", 0));
}

/// Setup › I/O's New Bus skips a name an output path has: a route names a bus or an output path,
/// so the two never share a name.
#[test]
fn new_bus_skips_an_output_path_name() {
    let mut app = SoundApp::new(soundcraft_engine::Engine::default(), None, Services::default());
    app.run("setup.io", json!({"action": "create_output", "name": "Bus 1", "first_channel": 2})).unwrap();
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
    h.get_by_label("New Bus").click();
    for _ in 0..3 {
        h.step();
    }
    let busses: Vec<_> = h.state().engine.session().busses.iter().map(|bus| bus.name.clone()).collect();
    assert_eq!(busses, ["Bus 2"]);
}

/// Setup › I/O says why it refused an edit in its own window, in plain words, as Pro Tools tells
/// you in the I/O Setup what to correct (Reference Guide, printed p. 55, 77 and 87). The field
/// goes back to the path's name.
#[test]
fn a_refused_edit_is_explained_in_the_io_setup_window() {
    let mut app = SoundApp::new(soundcraft_engine::Engine::default(), None, Services::default());
    app.run("mix.new_bus", json!({"name": "Verb"})).unwrap();
    app.run("setup.io", json!({"action": "create_output", "name": "Cue", "first_channel": 2})).unwrap();
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
    h.get_all_by_label("Output path name").nth(1).expect("the Cue row").click();
    h.step();
    let key = |key, modifiers| egui::Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers };
    h.input_mut().events.push(key(egui::Key::A, egui::Modifiers::COMMAND));
    h.step();
    h.input_mut().events.push(egui::Event::Text("Verb".into()));
    h.step();
    h.input_mut().events.push(key(egui::Key::Enter, egui::Modifiers::NONE));
    for _ in 0..3 {
        h.step();
    }
    assert_eq!(h.state().engine.session().outputs[1].name, "Cue");
    assert!(h.query_by_label_contains("a bus named `Verb` exists").is_some(), "the reason shows in I/O Setup");
    assert!(h.query_by_label_contains("bad parameters").is_none());
    assert!(!h.state().ui.status.contains("bad parameters"), "{}", h.state().ui.status);
}
