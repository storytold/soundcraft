use egui::{Event, Key, Modifiers, vec2};
use egui_kittest::{Harness, kittest::Queryable};
use serde_json::json;
use soundcraft_audio_io::{AudioBuffer, BitDepth, EncodeOptions, FileFormat};
use soundcraft_engine::{Engine, io};
use soundcraft_model::SourceId;
use soundcraft_ui_egui::{Services, SoundApp, dialogs::Dialog};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!("soundcraft-relink-ui-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    fn audio(&self, name: &str, level: f32) -> PathBuf {
        let path = self.0.join(name);
        let bytes = soundcraft_audio_io::encode(
            &AudioBuffer { sample_rate: 48_000, channels: vec![vec![level; 64]] },
            &EncodeOptions { format: FileFormat::Wav, bit_depth: BitDepth::Float32, dither: false, bwf: None },
        )
        .unwrap();
        std::fs::write(&path, bytes).unwrap();
        path
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn app(path: &std::path::Path, legacy: bool) -> (SoundApp, SourceId) {
    let mut engine = Engine::default();
    let bytes = std::fs::read(path).unwrap();
    let result = io::import_audio_bytes(&mut engine, "Lost take.wav", &bytes, None, None, 0).unwrap();
    let id = SourceId(result["source"].as_u64().unwrap());
    let mut session = engine.session().clone();
    session.pool.remove(id);
    if legacy {
        session.sources[0].audio_identity = None;
    }
    engine.replace_session(session);
    let selected = path.to_string_lossy().into_owned();
    let services = Services {
        pick_open: Some(Box::new(move |purpose, _| {
            assert_eq!(purpose, "media.relink");
            Some(selected.clone())
        })),
        ..Services::default()
    };
    (SoundApp::new(engine, None, services), id)
}

fn harness(app: SoundApp) -> Harness<'static, SoundApp> {
    Harness::builder().with_size(vec2(1600.0, 1000.0)).build_ui_state(
        |ui, app: &mut SoundApp| {
            let ctx = ui.ctx().clone();
            app.logic(&ctx);
            app.ui(ui);
        },
        app,
    )
}

fn steps(h: &mut Harness<'_, SoundApp>) {
    for _ in 0..4 {
        h.step();
    }
}

fn enter(h: &mut Harness<'_, SoundApp>) {
    h.input_mut().events.push(Event::Key { key: Key::Enter, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE });
    steps(h);
    h.input_mut().events.push(Event::Key { key: Key::Enter, physical_key: None, pressed: false, repeat: false, modifiers: Modifiers::NONE });
    h.step();
}

#[test]
fn audio_health_opens_relink_and_browse_restores_media_without_stale_report() {
    let f = Fixture::new();
    let path = f.audio("original.wav", 0.25);
    let (mut app, id) = app(&path, false);
    app.ui.show_audio_health = true;
    let mut h = harness(app);
    steps(&mut h);
    h.get_by_label("Relink Audio…").click();
    steps(&mut h);
    assert_eq!(h.state().dialogs.open_name(), Some("relink_audio"));
    h.get_by_label("Browse…").click();
    steps(&mut h);
    h.get_by_label("Relink Audio").click();
    steps(&mut h);
    assert!(h.state().dialogs.open.is_none());
    assert!(h.state().engine.session().pool.contains(id));
    assert_eq!(h.state().audio_health_report.as_ref().unwrap().as_ref().unwrap()["issue_count"], 0);
    h.state_mut().run("edit.undo", json!({})).unwrap();
    steps(&mut h);
    assert!(!h.state().engine.session().pool.contains(id));
    assert_eq!(h.state().audio_health_report.as_ref().unwrap().as_ref().unwrap()["issue_count"], 1);
    let _ = h.get_by_label("Relink Audio…");
    h.state_mut().run("edit.redo", json!({})).unwrap();
    steps(&mut h);
    assert!(h.state().engine.session().pool.contains(id));
    assert_eq!(h.state().audio_health_report.as_ref().unwrap().as_ref().unwrap()["issue_count"], 0);
}

#[test]
fn legacy_dialog_requires_confirmation_even_when_enter_is_pressed() {
    let f = Fixture::new();
    let path = f.audio("original.wav", 0.25);
    let (mut app, id) = app(&path, true);
    assert!(app.dialogs.open_relink(&app.engine, id));
    if let Some(Dialog::RelinkAudio { path: selected, .. }) = &mut app.dialogs.open {
        *selected = path.to_string_lossy().into_owned();
    }
    let mut h = harness(app);
    steps(&mut h);
    enter(&mut h);
    assert!(!h.state().engine.session().pool.contains(id));
    assert_eq!(h.state().dialogs.open_name(), Some("relink_audio"));
    assert!(!h.state().engine.can_undo());
    h.get_by_label("I have checked that this is the original audio").click();
    steps(&mut h);
    enter(&mut h);
    assert!(h.state().engine.session().pool.contains(id));
    assert!(h.state().dialogs.open.is_none());
    assert_eq!(h.state().ui.status, "Audio relinked");
    assert!(h.state().engine.session().sources[0].audio_identity.is_some());
}

#[test]
fn wrong_audio_keeps_the_dialog_open_for_a_corrected_selection() {
    let f = Fixture::new();
    let path = f.audio("original.wav", 0.25);
    let wrong = f.audio("wrong.wav", -0.25);
    let (mut app, id) = app(&path, false);
    app.dialogs.open_relink(&app.engine, id);
    if let Some(Dialog::RelinkAudio { path: selected, .. }) = &mut app.dialogs.open {
        *selected = wrong.to_string_lossy().into_owned();
    }
    let mut h = harness(app);
    steps(&mut h);
    enter(&mut h);
    let Some(Dialog::RelinkAudio { error, .. }) = &h.state().dialogs.open else { panic!("failed relink must retain its dialog") };
    assert!(error.contains("identity differs"));
    assert!(!h.state().engine.session().pool.contains(id));
    h.get_by_label("Browse…").click();
    steps(&mut h);
    h.get_by_label("Relink Audio").click();
    steps(&mut h);
    assert!(h.state().engine.session().pool.contains(id));
    assert!(h.state().dialogs.open.is_none());
    assert!(h.state_mut().run("media.relink", json!({"source": id.0, "path": path})).is_err());
    assert!(h.state().dialogs.open.is_none(), "programmatic calls must not open dialogs");
}

#[test]
fn document_replacement_discards_old_relink_consent_but_failed_commands_and_saves_preserve_it() {
    let f = Fixture::new();
    let path = f.audio("original.wav", 0.25);
    let saved = f.0.join("missing.scraft");
    let (original, _) = app(&path, true);
    std::fs::write(&saved, original.engine.session().to_json().unwrap()).unwrap();

    let prepared = || {
        let (mut app, id) = app(&path, true);
        app.engine.path = Some(saved.to_string_lossy().into_owned());
        assert!(app.dialogs.open_relink(&app.engine, id));
        if let Some(Dialog::RelinkAudio { allow_unverified, path: selected, .. }) = &mut app.dialogs.open {
            *allow_unverified = true;
            *selected = path.to_string_lossy().into_owned();
        }
        app.audio_health_report = Some(Ok(json!({"issue_count": 99})));
        app
    };

    for (command, params) in
        [("session.new", json!({})), ("session.close", json!({})), ("session.open", json!({"path": saved})), ("session.revert", json!({}))]
    {
        let mut app = prepared();
        app.run(command, params).unwrap();
        assert!(app.dialogs.open.is_none(), "{command} must discard the old relink dialog");
        assert!(app.audio_health_report.is_none(), "{command} must invalidate the old health report");
    }

    for (command, params) in [
        ("session.new", json!({"sample_rate": 0})),
        ("session.open", json!({"path": f.0.join("absent.scraft")})),
        ("session.save_as", json!({"path": f.0.join("as.scraft")})),
        ("session.save_copy", json!({"path": f.0.join("copy.scraft")})),
    ] {
        let mut app = prepared();
        let result = app.run(command, params);
        assert_eq!(result.is_ok(), command.starts_with("session.save"));
        let Some(Dialog::RelinkAudio { allow_unverified, path: selected, .. }) = &app.dialogs.open else {
            panic!("{command} must retain the original relink dialog");
        };
        assert!(*allow_unverified);
        assert_eq!(selected, &path.to_string_lossy());
        assert_eq!(app.audio_health_report.as_ref().unwrap().as_ref().unwrap()["issue_count"], 99);
    }

    let mut app = prepared();
    app.dialogs.open = Some(Dialog::NewSession { name: "Untitled".into(), sample_rate: 48_000, demo: false });
    app.run("session.close", json!({})).unwrap();
    assert!(matches!(app.dialogs.open, Some(Dialog::NewSession { .. })), "only a relink dialog is discarded");
}
