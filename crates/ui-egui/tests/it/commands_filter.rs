use serde_json::{Value, json};
use soundcraft_ui_egui::{ControlRequest, Services, SoundApp, control, menus};

fn commands(params: Value) -> Vec<Value> {
    let (tx, rx) = std::sync::mpsc::channel();
    let mut app = SoundApp::new(soundcraft_engine::Engine::default(), None, Services::default()).with_control(rx);
    let (req, reply) = ControlRequest::new("engine.commands", params);
    tx.send(req).unwrap();
    control::drain(&mut app, &egui::Context::default());
    let response = reply.try_recv().unwrap();
    assert_eq!(response["ok"], true, "{response}");
    response["result"].as_array().unwrap().clone()
}

fn ids(rows: &[Value]) -> Vec<&str> {
    rows.iter().map(|r| r["id"].as_str().unwrap()).collect()
}

#[test]
fn filter_applies_to_ui_commands() {
    assert_eq!(ids(&commands(json!({"filter": "window.search"}))), ["window.search"]);
    assert_eq!(ids(&commands(json!({"filter": "WINDOW.Search"}))), ["window.search"]);
    // "Arrange Panel" matches only by label, and only for ui.dock.
    assert_eq!(ids(&commands(json!({"filter": "arrange PANEL"}))), ["ui.dock"]);
    assert!(commands(json!({"filter": "zzzznomatch"})).is_empty());
}

#[test]
fn absent_or_empty_filter_lists_every_ui_command() {
    for params in [json!({}), json!({"filter": ""})] {
        let rows = commands(params);
        let ids = ids(&rows);
        for (id, ..) in menus::UI_COMMANDS {
            assert!(ids.contains(id), "{id} missing");
        }
        assert!(ids.contains(&"track.new"));
    }
}
