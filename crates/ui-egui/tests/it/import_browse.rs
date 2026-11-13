//! Import Audio's Browse button lets the user select several files at once (#1089).
use egui::vec2;
use egui_kittest::{Harness, kittest::Queryable};
use soundcraft_ui_egui::{Services, SoundApp, dialogs::Dialog};
use std::path::PathBuf;

fn browse(picked: Vec<PathBuf>) -> Harness<'static, SoundApp> {
    let pick_open_many: Box<dyn Fn(&str) -> Vec<PathBuf>> = Box::new(move |purpose| {
        assert_eq!(purpose, "file.import_audio");
        picked.clone()
    });
    let mut app = SoundApp::new(soundcraft_engine::Engine::default(), None, Services { pick_open_many: Some(pick_open_many), ..Default::default() });
    assert!(app.dialogs.open_for_command(&app.engine, "file.import_audio"));
    let mut h = Harness::builder().with_size(vec2(1600.0, 1000.0)).build_ui_state(
        |ui, app: &mut SoundApp| {
            app.logic(&ui.ctx().clone());
            app.ui(ui);
        },
        app,
    );
    h.run_steps(3);
    h.get_by_label("Browse…").click();
    h.run_steps(1);
    h
}

#[test]
fn browse_with_several_files_imports_them_and_closes_the_dialog() {
    let h = browse(vec!["/nope/a.wav".into(), "/nope/b.wav".into()]);
    assert!(h.state().dialogs.open.is_none());
    assert!(h.state().imports.busy() || !h.state().ui.status.is_empty(), "the files were queued");
}

#[test]
fn browse_with_one_file_fills_the_path_field() {
    let h = browse(vec!["/nope/a.wav".into()]);
    match &h.state().dialogs.open {
        Some(Dialog::PathPrompt { path, .. }) => assert_eq!(path, "/nope/a.wav"),
        other => panic!("{other:?}"),
    }
}
