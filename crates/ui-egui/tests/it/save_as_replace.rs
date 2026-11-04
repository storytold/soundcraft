//! Save As over another session asks first, and Enter alone never replaces it.
use egui::vec2;
use egui_kittest::{Harness, kittest::Queryable};
use serde_json::json;
use soundcraft_ui_egui::{Services, SoundApp, dialogs::Dialog};

#[test]
fn saving_over_another_session_takes_the_replace_button() {
    let dir = std::env::temp_dir().join(format!("soundcraft-ui-replace-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("Song.scraft");
    soundcraft_engine::Engine::default().execute("session.save_as", &json!({"path": path})).unwrap();
    let other = std::fs::read(&path).unwrap();
    let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
    app.dialogs.open = Some(Dialog::PathPrompt {
        cmd: "session.save_as".into(),
        title: "Save Session As".into(),
        path: path.to_string_lossy().into(),
        key: "path".into(),
    });
    let mut h = Harness::builder().with_size(vec2(1600.0, 1000.0)).build_ui_state(
        |ui, app: &mut SoundApp| {
            app.logic(&ui.ctx().clone());
            app.ui(ui);
        },
        app,
    );
    h.run_steps(3);
    h.key_press(egui::Key::Enter);
    h.run_steps(3);
    assert_eq!(std::fs::read(&path).unwrap(), other);
    h.get_by_label("Replace").click();
    h.run_steps(3);
    assert_ne!(std::fs::read(&path).unwrap(), other);
    assert_eq!(h.state().engine.path, Some(path.to_string_lossy().into_owned()));
    let _ = std::fs::remove_dir_all(&dir);
}
