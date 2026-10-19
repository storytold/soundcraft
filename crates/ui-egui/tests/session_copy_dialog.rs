//! Exercise the real Copy dialog and UI dispatcher without a player or hosted plugins.
use egui::{Event, Key, Modifiers, vec2};
use egui_kittest::{Harness, kittest::Queryable};
use serde_json::json;
use soundcraft_engine::{Engine, io, session_copy};
use soundcraft_ui_egui::{Services, SoundApp, dialogs::Dialog};
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

static NEXT: AtomicU64 = AtomicU64::new(0);
struct Fixture(std::path::PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("soundcraft-copy-ui-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        let _ = std::fs::remove_dir_all(&path);
        Self(path)
    }
    fn path(&self) -> String {
        self.0.join("Copy.scraft").to_string_lossy().into()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn harness(e: Engine, path: &str) -> Harness<'static, SoundApp> {
    let preview = session_copy::preview(&e, Some(path));
    let document = e.session_arc();
    let revision = e.revision;
    let mut app = SoundApp::new(e, None, Services::default());
    app.dialogs.open = Some(Dialog::SaveCopy { path: path.into(), preview, document, revision });
    let mut h = Harness::builder().with_size(vec2(1400.0, 1000.0)).with_max_steps(1_000_000).wgpu().build_ui_state(
        |ui, app: &mut SoundApp| {
            let ctx = ui.ctx().clone();
            app.logic(&ctx);
            app.ui(ui);
        },
        app,
    );
    h.input_mut().max_texture_side = Some(8192);
    h.run_steps(6);
    h
}

fn shot(h: &mut Harness<'_, SoundApp>, name: &str) {
    if let Ok(dir) = std::env::var("SOUNDCRAFT_COPY_ARTIFACT_DIR") {
        h.render().unwrap().save(std::path::Path::new(&dir).join(name)).unwrap();
    }
}

#[test]
fn valid_copy_dialog_and_ui_run_leave_open_document_and_history_untouched() {
    let dir = Fixture::new();
    let mut e = soundcraft_engine::demo::demo_engine();
    e.execute("mix.volume", &json!({"track": "Kick", "db": -6})).unwrap();
    e.execute("track.new", &json!({"name": "Redo me"})).unwrap();
    assert!(e.undo());
    let mut h = harness(e, &dir.path());
    let engine = &h.state().engine;
    let (arc, revision, dirty, path, history, redo, journal) = (
        engine.session_arc(),
        engine.revision,
        engine.is_dirty(),
        engine.path.clone(),
        engine.undo_history(),
        engine.redo_label().map(str::to_string),
        engine.journal.clone(),
    );
    shot(&mut h, "save-copy-ready.png");
    h.get_by_label("Save Copy").click();
    h.run_steps(3);
    let engine = &h.state().engine;
    assert!(Arc::ptr_eq(&arc, &engine.session_arc()));
    assert_eq!(
        (
            engine.revision,
            engine.is_dirty(),
            engine.path.clone(),
            engine.undo_history(),
            engine.redo_label().map(str::to_string),
            engine.journal.clone()
        ),
        (revision, dirty, path, history, redo, journal)
    );
    assert!(h.state().dialogs.open.is_none());
    let mut opened = Engine::default();
    assert!(io::open_session(&mut opened, &dir.path()).unwrap().is_empty());
}

#[test]
fn missing_audio_blocks_click_and_enter_and_preview_never_changes_the_document() {
    let dir = Fixture::new();
    let mut e = soundcraft_engine::demo::demo_engine();
    let source = e.session().sources[0].id;
    e.session_mut().pool.remove(source);
    let mut h = harness(e, &dir.path());
    shot(&mut h, "save-copy-blocked.png");
    h.get_by_label("Save Copy").click();
    h.key_press(Key::Enter);
    h.run_steps(3);
    assert!(!dir.0.exists());
    assert_eq!(h.state().dialogs.open_name(), Some("save_copy"));
    let before = h.state().engine.session_arc();
    let revision = h.state().engine.revision;
    let v = h.state_mut().run("session.save_copy_preview", json!({"path": dir.path()})).unwrap();
    assert_eq!(v["missing_audio"], 1);
    assert!(Arc::ptr_eq(&before, &h.state().engine.session_arc()));
    assert_eq!(revision, h.state().engine.revision);
}

#[test]
fn stale_preview_retains_typed_path_and_requires_explicit_refresh() {
    let dir = Fixture::new();
    let mut h = harness(Engine::default(), &dir.path());
    h.state_mut().run("track.new", json!({"name": "Changed"})).unwrap();
    h.run_steps(3);
    h.key_press(Key::Enter);
    h.run_steps(3);
    assert!(!dir.0.exists());
    match &h.state().dialogs.open {
        Some(Dialog::SaveCopy { path, .. }) => assert_eq!(path, &dir.path()),
        _ => panic!("copy dialog should preserve the typed destination"),
    }
    h.get_by_label("Refresh Preview").click();
    h.run_steps(3);
    h.get_by_label("Save Copy").click();
    h.run_steps(3);
    assert!(std::path::Path::new(&dir.path()).exists());
}

#[test]
fn replacing_a_copy_needs_a_button_and_escape_cancels_even_with_enter() {
    let dir = Fixture::new();
    let mut first = Engine::default();
    io::save_session(&mut first, &dir.path()).unwrap();
    let original = std::fs::read(dir.path()).unwrap();
    let mut h = harness(soundcraft_engine::demo::demo_engine(), &dir.path());
    h.key_press(Key::Enter);
    h.run_steps(3);
    assert_eq!(std::fs::read(dir.path()).unwrap(), original);
    h.get_by_label("Replace Copy").click();
    h.run_steps(3);
    assert_ne!(std::fs::read(dir.path()).unwrap(), original);
    let absent = Fixture::new();
    let mut h = harness(Engine::default(), &absent.path());
    for key in [Key::Enter, Key::Escape] {
        h.input_mut().events.push(Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE });
    }
    h.step();
    assert!(!absent.0.exists());
    assert!(h.state().dialogs.open.is_none());
}

#[test]
fn successful_document_changes_close_only_the_copy_dialog() {
    let dir = Fixture::new();
    let saved = dir.0.join("Original.scraft");
    let mut e = Engine::default();
    io::save_session(&mut e, &saved.to_string_lossy()).unwrap();
    for (command, params) in
        [("session.new", json!({})), ("session.open", json!({"path": saved})), ("session.close", json!({})), ("session.revert", json!({}))]
    {
        let mut e = Engine::default();
        io::open_session(&mut e, &saved.to_string_lossy()).unwrap();
        let mut h = harness(e, &dir.path());
        h.state_mut().run(command, params).unwrap();
        assert!(h.state().dialogs.open.is_none(), "{command}");
    }
    let mut h = harness(Engine::default(), &dir.path());
    assert!(h.state_mut().run("session.open", json!({"path": dir.0.join("absent.scraft")})).is_err());
    assert_eq!(h.state().dialogs.open_name(), Some("save_copy"));
    h.state_mut().dialogs.open =
        Some(Dialog::Number { cmd: "edit.repeat".into(), title: "Repeat".into(), key: "count".into(), value: 2.0, suffix: "times".into() });
    h.state_mut().run("session.new", json!({})).unwrap();
    assert_eq!(h.state().dialogs.open_name(), Some("number"));
}

#[test]
fn ui_copy_rejects_stale_expected_revision_before_creating_the_detached_snapshot() {
    let dir = Fixture::new();
    let mut h = harness(Engine::default(), &dir.path());
    let previous = h.state().engine.revision;
    h.state_mut().run("track.new", json!({})).unwrap();
    let before = h.state().engine.session_arc();
    assert!(h.state_mut().run("session.save_copy", json!({"path": dir.path(), "expected_revision": previous})).is_err());
    assert!(Arc::ptr_eq(&before, &h.state().engine.session_arc()));
    assert!(!dir.0.exists());
}
