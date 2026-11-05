use serde_json::json;
use soundcraft_ui_egui::{Services, SoundApp, menus};

#[test]
fn undo_and_redo_name_the_step_they_would_take() {
    let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
    let label = |app: &SoundApp, id| menus::live_label(app, Some(id));
    assert_eq!(label(&app, "edit.undo").as_deref(), Some("Can't Undo"));
    app.run("mix.mute", json!({"track": "Kick"})).unwrap();
    assert_eq!(label(&app, "edit.undo").as_deref(), Some("Undo Mute"));
    assert_eq!(label(&app, "edit.redo").as_deref(), Some("Can't Redo"));
    app.run("edit.undo", json!({})).unwrap();
    assert_eq!(label(&app, "edit.redo").as_deref(), Some("Redo Mute"));
    assert_eq!(label(&app, "edit.copy"), None);
}
