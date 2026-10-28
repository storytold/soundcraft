//! Bounce Mix starts in the session's folder and has a Browse button for the file (#186).
use egui::vec2;
use egui_kittest::{Harness, kittest::Queryable};
use soundcraft_audio_io::FileFormat;
use soundcraft_ui_egui::{Services, SoundApp, dialogs::Dialog};
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

#[test]
fn bounce_starts_in_the_session_folder_and_browse_picks_the_file_and_format() {
    let asked = Rc::new(RefCell::new(Vec::new()));
    let log = Rc::clone(&asked);
    let pick_save: Box<dyn Fn(&str, &Path) -> Option<PathBuf>> = Box::new(move |purpose, default| {
        log.borrow_mut().push((purpose.to_string(), default.to_path_buf()));
        Some("/Mixes/Final Mix.FLAC".into())
    });
    let mut engine = soundcraft_engine::demo::demo_engine();
    engine.path = Some("/Songs/Midnight Groove/Midnight Groove.scraft".into());
    let mut app = SoundApp::new(engine, None, Services { pick_save: Some(pick_save), ..Default::default() });
    assert!(app.dialogs.open_for_command(&app.engine, "file.bounce_mix"));
    let mut h = Harness::builder().with_size(vec2(1600.0, 1000.0)).build_ui_state(
        |ui, app: &mut SoundApp| {
            app.logic(&ui.ctx().clone());
            app.ui(ui);
        },
        app,
    );
    h.run_steps(3);
    h.get_by_label("Browse…").click();
    h.run_steps(3);
    let default = PathBuf::from("/Songs/Midnight Groove/Midnight Groove Bounce.wav");
    assert_eq!(*asked.borrow(), vec![("file.bounce_mix".to_string(), default)]);
    match &h.state().dialogs.open {
        Some(Dialog::Bounce { path, format, .. }) => assert_eq!((path.as_str(), *format), ("/Mixes/Final Mix.FLAC", FileFormat::Flac)),
        other => panic!("{other:?}"),
    }
}

#[test]
fn bounce_of_a_session_with_a_relative_path_starts_in_the_default_folder() {
    let mut engine = soundcraft_engine::demo::demo_engine();
    engine.path = Some("Midnight Groove.scraft".into());
    let name = engine.session().name.clone();
    let mut app = SoundApp::new(engine, None, Services::default());
    assert!(app.dialogs.open_for_command(&app.engine, "file.bounce_mix"));
    let mut fresh = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
    assert!(fresh.dialogs.open_for_command(&fresh.engine, "file.bounce_mix"));
    match (&app.dialogs.open, &fresh.dialogs.open) {
        (Some(Dialog::Bounce { path, .. }), Some(Dialog::Bounce { path: unsaved, .. })) => {
            assert_ne!(path.as_str(), format!("/{name} Bounce.wav"));
            assert_eq!(path, unsaved, "the same folder as a session never saved");
        }
        other => panic!("{other:?}"),
    }
}
