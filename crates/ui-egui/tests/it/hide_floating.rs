use serde_json::json;
use soundcraft_ui_egui::{Services, SoundApp};

#[test]
fn hide_floating_closes_automation() {
    let mut app = SoundApp::new(soundcraft_engine::Engine::default(), None, Services::default());
    let main_window = app.ui.window;
    app.run("window.automation", json!({"value": true})).unwrap();
    app.run("window.transport", json!({"value": true})).unwrap();
    assert!(app.ui.show_automation && app.ui.show_transport);

    app.run("window.hide_floating", json!({})).unwrap();
    assert!(!app.ui.show_transport);
    assert!(!app.ui.show_automation, "Automation must be hidden");
    assert_eq!(app.ui.window, main_window);

    app.run("window.automation", json!({"value": true})).unwrap();
    assert!(app.ui.show_automation);
}
