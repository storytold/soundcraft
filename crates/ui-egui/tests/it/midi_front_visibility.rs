use serde_json::json;
use soundcraft_ui_egui::{Services, SoundApp, menus};

#[test]
fn bring_to_front_keeps_an_open_midi_editor_open() {
    let mut app = SoundApp::new(soundcraft_engine::Engine::default(), None, Services::default());
    app.run("window.midi_editor", json!({"value": true})).unwrap();
    let path = "Window > MIDI Editors > Bring to Front";
    let id = menus::command_for_path(path, &menus::ui_aliases()).unwrap();
    assert_eq!(id, "window.midi_front");
    for _ in 0..2 {
        menus::invoke_menu(&mut app, &id, path);
        assert!(app.ui.show_midi_editor, "Bring to Front hid the editor");
    }

    app.run("window.midi_editor", json!({"value": false})).unwrap();
    app.run("window.midi_front", json!({})).unwrap();
    assert!(app.ui.show_midi_editor, "Bring to Front did not show a hidden editor");
}

#[test]
fn the_midi_editor_command_still_toggles() {
    let mut app = SoundApp::new(soundcraft_engine::Engine::default(), None, Services::default());
    app.run("window.midi_editor", json!({})).unwrap();
    assert!(app.ui.show_midi_editor);
    app.run("window.midi_editor", json!({})).unwrap();
    assert!(!app.ui.show_midi_editor);
}
