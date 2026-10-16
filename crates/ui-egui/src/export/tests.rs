#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use super::*;
use crate::Services;
use soundcraft_model::{ChannelFormat, Insert, Session, TrackKind};
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

struct Folder(PathBuf);
impl Folder {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        for _ in 0..128 {
            let path = std::env::temp_dir().join(format!("sc-export-isolation-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
            match std::fs::create_dir(&path) {
                Ok(()) => return Self(path),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => panic!("owned export directory: {error}"),
            }
        }
        panic!("unique export directory unavailable")
    }
    fn path(&self, leaf: &str) -> PathBuf {
        self.0.join(leaf)
    }
}
impl Drop for Folder {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn fixture() -> (SoundApp, TrackId) {
    let mut session = Session::default();
    let track = session.add_track(TrackKind::Audio, ChannelFormat::Stereo, None);
    let tr = session.track_mut(track).unwrap();
    let mut insert = Insert::new("clap:owned.export-state");
    insert.set_state_bytes(b"stored insert");
    tr.mixer.inserts[0] = Some(insert);
    let mut instrument = Insert::new("clap:owned.instrument-state");
    instrument.set_state_bytes(b"stored instrument");
    tr.instrument = Some(instrument);
    let mut engine = Engine::new(session);
    engine.path = Some("original-session.scraft".into());
    engine.execute("mix.volume", &json!({"track":track.0,"db":-3})).unwrap();
    engine.execute("mix.volume", &json!({"track":track.0,"db":-6})).unwrap();
    assert!(engine.undo());
    assert!(engine.can_undo() && engine.can_redo() && engine.is_dirty());
    (SoundApp::new(engine, None, Services::default()), track)
}
struct Before {
    doc: Arc<Session>,
    revision: u64,
    dirty: bool,
    path: Option<String>,
    undo: Vec<String>,
    redo: Option<String>,
    journal: Vec<(String, serde_json::Value)>,
    recovery: u64,
}
impl Before {
    fn new(app: &SoundApp) -> Self {
        Self {
            doc: app.engine.session_arc(),
            revision: app.engine.revision,
            dirty: app.engine.is_dirty(),
            path: app.engine.path.clone(),
            undo: app.engine.undo_history(),
            redo: app.engine.redo_label().map(str::to_string),
            journal: app.engine.journal.clone(),
            recovery: app.autosaved_rev,
        }
    }
    fn check(&self, app: &SoundApp) {
        assert!(Arc::ptr_eq(&self.doc, &app.engine.session_arc()));
        assert_eq!(self.doc.to_json().unwrap(), app.engine.session().to_json().unwrap());
        assert_eq!(self.revision, app.engine.revision);
        assert_eq!(self.dirty, app.engine.is_dirty());
        assert_eq!(self.path, app.engine.path);
        assert_eq!(self.undo, app.engine.undo_history());
        assert_eq!(self.redo.as_deref(), app.engine.redo_label());
        assert!(app.engine.can_undo() && app.engine.can_redo());
        assert_eq!(self.journal, app.engine.journal);
        assert_eq!(self.recovery, app.autosaved_rev);
    }
}
fn capture(track: TrackId) -> Vec<(TrackId, usize, Vec<u8>)> {
    vec![(track, 0, b"live insert".to_vec()), (track, soundcraft_mix::INSTRUMENT_SLOT, b"live instrument".to_vec())]
}
fn check_states(engine: &Engine, track: TrackId) {
    let tr = engine.session().track(track).unwrap();
    assert_eq!(tr.mixer.inserts[0].as_ref().unwrap().state_bytes().unwrap(), b"live insert");
    assert_eq!(tr.instrument.as_ref().unwrap().state_bytes().unwrap(), b"live instrument");
}
#[test]
fn captured_insert_and_instrument_bytes_reach_saved_copy_without_editing_original() {
    let dir = Folder::new();
    let (app, track) = fixture();
    let before = Before::new(&app);
    let mut copy = detached_engine(&app.engine, capture(track)).unwrap();
    check_states(&copy, track);
    let path = dir.path("Copy.scraft");
    copy.execute("session.save_copy", &json!({"path":path})).unwrap();
    let mut reopened = Engine::default();
    soundcraft_engine::io::open_session(&mut reopened, path.to_str().unwrap()).unwrap();
    check_states(&reopened, track);
    before.check(&app);
}
#[test]
fn partial_capture_application_failure_never_writes_original() {
    let (app, track) = fixture();
    let before = Before::new(&app);
    let mut states = capture(track);
    states.push((TrackId(u64::MAX), 0, b"stale".to_vec()));
    assert!(detached_engine(&app.engine, states).is_err());
    before.check(&app);
}
#[test]
fn direct_file_exports_preserve_dirty_history_and_document_identity() {
    let dir = Folder::new();
    let (mut app, _) = fixture();
    let before = Before::new(&app);
    for (id, params) in [
        ("file.bounce_mix", json!({"path":dir.path("mix.wav"),"start":0,"end":32,"dither":false})),
        ("file.bounce_stems", json!({"dir":dir.path("stems"),"start":0,"end":32})),
        ("session.save_copy", json!({"path":dir.path("copy.scraft")})),
    ] {
        app.run(id, params).unwrap();
        before.check(&app);
    }
}
#[test]
fn refused_direct_exports_preserve_original_and_recovery_bookkeeping() {
    let (mut app, _) = fixture();
    let before = Before::new(&app);
    for id in ["file.bounce_mix", "file.bounce_stems", "session.save_copy"] {
        assert!(app.run(id, json!({})).is_err());
        before.check(&app);
    }
}
#[test]
fn modal_bounce_start_cancel_and_refusal_preserve_original() {
    let dir = Folder::new();
    let (mut app, _) = fixture();
    let before = Before::new(&app);
    crate::bounce::start(&mut app, &json!({"path":dir.path("modal.wav"),"start":0,"end":32}));
    assert!(app.bounce.is_some());
    before.check(&app);
    app.bounce = None;
    before.check(&app);
    crate::bounce::start(&mut app, &json!({}));
    assert!(app.bounce.is_none());
    before.check(&app);
}
#[test]
fn detached_exports_keep_the_current_file_gateway_authority() {
    let dir = Folder::new();
    let (mut app, _) = fixture();
    let before = Before::new(&app);
    let allowed = dir.path("allowed");
    std::fs::create_dir(&allowed).unwrap();
    let roots = soundcraft_engine::file_access::AutomationRoots::new(None, Some(&allowed)).unwrap();
    soundcraft_engine::file_access::confine(roots.as_ref(), || {
        for (id, params) in [
            ("session.save_copy", json!({"path":dir.path("outside.scraft")})),
            ("file.bounce_mix", json!({"path":dir.path("outside.wav"),"start":0,"end":32})),
            ("file.bounce_stems", json!({"dir":dir.path("outside-stems"),"start":0,"end":32})),
        ] {
            assert!(app.run(id, params).is_err());
        }
    });
    before.check(&app);
    assert!(!dir.path("outside.scraft").exists());
    assert!(!dir.path("outside.wav").exists());
    assert!(!dir.path("outside-stems").exists());
}
#[test]
fn document_freeze_edits_the_active_track_without_consulting_stale_export_capture() {
    let (mut app, track) = fixture();
    app.captured_states_for_test = Some(vec![(TrackId(u64::MAX), 0, b"stale export state".to_vec())]);
    let original = app.engine.session_arc();
    let revision = app.engine.revision;
    app.run("track.freeze", json!({"tracks":[track.0],"value":true})).unwrap();
    assert!(app.engine.session().track(track).unwrap().frozen);
    assert!(!Arc::ptr_eq(&original, &app.engine.session_arc()));
    assert!(app.engine.revision > revision);
    assert!(app.engine.undo());
    assert!(!app.engine.session().track(track).unwrap().frozen);
    assert_eq!(app.engine.session().track(track).unwrap().mixer.inserts[0].as_ref().unwrap().state_bytes().unwrap(), b"stored insert");
}

#[test]
fn ordinary_save_and_save_as_still_update_the_active_document() {
    let dir = Folder::new();
    let (mut app, track) = fixture();
    app.captured_states_for_test = Some(capture(track));
    let first = dir.path("ordinary.scraft");
    app.run("session.save_as", json!({"path":first})).unwrap();
    assert!(!app.engine.is_dirty());
    assert_eq!(app.engine.path.as_deref(), first.to_str());
    check_states(&app.engine, track);
    app.engine.execute("mix.volume", &json!({"track":track.0,"db":-9})).unwrap();
    assert!(app.engine.is_dirty());
    app.run("session.save", json!({})).unwrap();
    assert!(!app.engine.is_dirty());
}

#[test]
fn direct_save_copy_routes_injected_capture_only_into_the_saved_document() {
    let dir = Folder::new();
    let (mut app, track) = fixture();
    let before = Before::new(&app);
    app.captured_states_for_test = Some(capture(track));
    let path = dir.path("Captured.scraft");
    app.run("session.save_copy", json!({"path":path})).unwrap();
    let mut reopened = Engine::default();
    soundcraft_engine::io::open_session(&mut reopened, path.to_str().unwrap()).unwrap();
    check_states(&reopened, track);
    before.check(&app);
}
#[test]
fn stale_capture_refuses_direct_and_modal_bounces_before_creating_output() {
    let dir = Folder::new();
    let (mut app, track) = fixture();
    let before = Before::new(&app);
    let mut states = capture(track);
    states.push((TrackId(u64::MAX), 0, b"stale".to_vec()));
    app.captured_states_for_test = Some(states);
    let direct = dir.path("direct.wav");
    assert!(app.run("file.bounce_mix", json!({"path":direct,"start":0,"end":32})).is_err());
    before.check(&app);
    assert!(!direct.exists());
    let modal = dir.path("modal.wav");
    crate::bounce::start(&mut app, &json!({"path":modal,"start":0,"end":32}));
    assert!(app.bounce.is_none());
    assert!(!modal.exists());
    assert_eq!(std::fs::read_dir(&dir.0).unwrap().count(), 0);
    before.check(&app);
}
#[test]
fn clean_original_save_copy_keeps_the_original_recovery_folder_open() {
    let dir = Folder::new();
    let (mut app, track) = fixture();
    app.engine.mark_clean();
    app.captured_states_for_test = Some(capture(track));
    app.autosaved_rev = app.engine.revision;
    let root = dir.path("Recovery");
    app.autosaver = Some(soundcraft_engine::autosave::Autosaver::new(root.clone()));
    let writer = app.autosaver.as_ref().unwrap();
    writer.write(app.engine.session_arc(), None, None);
    writer.flush().unwrap();
    let folders: Vec<_> = std::fs::read_dir(&root).unwrap().map(|entry| entry.unwrap().path()).collect();
    assert_eq!(folders.len(), 1);
    let before = Before::new(&app);
    app.run("session.save_copy", json!({"path":dir.path("CleanCopy.scraft")})).unwrap();
    before.check(&app);
    let writer = app.autosaver.as_ref().unwrap();
    writer.flush().unwrap();
    assert!(folders[0].is_dir());
    assert!(soundcraft_engine::autosave::leftovers(&root).is_empty(), "original recovery is still locked by its owner");
    writer.close(true);
    writer.flush().unwrap();
    assert_eq!(soundcraft_engine::autosave::leftovers(&root).len(), 1);
}
