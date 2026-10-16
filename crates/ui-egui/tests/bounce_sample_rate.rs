use egui::vec2;
use egui_kittest::{Harness, kittest::Queryable};
use serde_json::json;
use soundcraft_engine::Engine;
use soundcraft_time::{Range, SampleRate};
use soundcraft_ui_egui::{Services, SoundApp, dialogs::Dialog, menus};

#[test]
fn bounce_dialog_defaults_to_session_rate_and_dispatches_the_selected_delivery_rate() {
    let mut e = Engine::default();
    e.session_mut().edit.selection = Range::new(100, 580);
    let mut app = SoundApp::new(e, None, Services::default());
    menus::invoke_menu(&mut app, "file.bounce_mix", "File > Bounce Mix...");
    let path = std::env::temp_dir().join(format!("sc-ui-bounce-rate-{}.wav", std::process::id()));
    match app.dialogs.open.as_mut().unwrap() {
        Dialog::Bounce { path: value, sample_rate, .. } => {
            assert_eq!(*sample_rate, 48000);
            *value = path.to_string_lossy().into_owned();
        }
        _ => panic!("Bounce dialog required"),
    }
    let shot = std::env::var_os("SOUNDCRAFT_BOUNCE_RATE_SHOT");
    let mut builder = Harness::builder().with_size(vec2(1200.0, 800.0));
    if shot.is_some() {
        builder = builder.wgpu();
    }
    let mut h = builder.build_ui_state(
        |ui, app: &mut SoundApp| {
            app.logic(&ui.ctx().clone());
            app.ui(ui);
        },
        app,
    );
    h.run_steps(3);
    h.get_by(|node| node.role() == egui::accesskit::Role::ComboBox && node.value().as_deref() == Some("48000 Hz")).click();
    h.run_steps(3);
    h.get_by_label("96000 Hz").click();
    h.run_steps(3);
    assert!(matches!(h.state().dialogs.open, Some(Dialog::Bounce { sample_rate: 96000, .. })));
    if let Some(path) = shot {
        h.render().unwrap().save(path).unwrap();
    }
    h.get_by_label("Bounce").click();
    h.run_steps(3);
    assert!(h.state().dialogs.open.is_none());
    let (_, audio) = soundcraft_audio_io::decode(&std::fs::read(&path).unwrap(), Some("wav")).unwrap();
    assert_eq!(audio.sample_rate, 96000);
    assert_eq!(audio.frames(), 960);
    assert_eq!(h.state().engine.session().sample_rate.hz(), 48000);
    std::fs::remove_file(&path).unwrap();
}

#[test]
fn menu_uses_a_nondefault_session_rate_and_programmatic_bounce_opens_no_dialog() {
    let mut e = Engine::default();
    e.session_mut().sample_rate = SampleRate::new(96000).unwrap();
    let mut app = SoundApp::new(e, None, Services::default());
    assert!(app.dialogs.open_for_command(&app.engine, "file.bounce_mix"));
    assert!(matches!(app.dialogs.open, Some(Dialog::Bounce { sample_rate: 96000, .. })));
    app.dialogs.open = None;
    let path = std::env::temp_dir().join(format!("sc-ui-direct-rate-{}.wav", std::process::id()));
    let result = app.run("file.bounce_mix", json!({"path": path, "start": 100, "end": 1060, "sample_rate": 44100})).unwrap();
    assert_eq!(result["sample_rate"], 44100);
    assert_eq!(result["frames"], 441);
    assert!(app.dialogs.open.is_none());
    std::fs::remove_file(&path).unwrap();
}
