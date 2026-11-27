//! Unsaved work outlives a crash or an exit without Quit and is offered at the next launch.
use egui::accesskit::Role;
use egui::vec2;
use egui_kittest::{
    Harness,
    kittest::{NodeT, Queryable, by},
};
use serde_json::json;
use soundcraft_engine::autosave::leftovers;
use soundcraft_ui_egui::{Services, SoundApp};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, PoisonError};

/// One test at a time: a child process another test starts (fontconfig's `fc-match`) holds copies
/// of this process's open files until it execs, so a released autosave lock can still look taken.
fn one_at_a_time() -> MutexGuard<'static, ()> {
    static ONE: Mutex<()> = Mutex::new(());
    ONE.lock().unwrap_or_else(PoisonError::into_inner)
}

fn folder(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("soundcraft-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Autosaves are written in the background.
fn eventually(done: impl Fn() -> bool) -> bool {
    for _ in 0..300 {
        if done() {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    done()
}

fn autosaved(root: &Path) -> String {
    let folders = std::fs::read_dir(root).into_iter().flatten().flatten();
    folders.map(|d| std::fs::read_to_string(d.path().join("session.scraft")).unwrap_or_default()).collect()
}

fn launch(root: &Path) -> SoundApp {
    let mut app = SoundApp::new(soundcraft_engine::Engine::default(), None, Services::default());
    app.autosave_dir = Some(root.to_path_buf());
    app.leftovers = leftovers(root);
    app
}

#[test]
fn an_exit_without_quit_offers_unsaved_changes_at_the_next_launch() {
    let _one = one_at_a_time();
    let dir = folder("recovery-quit");
    let root = dir.join("Autosave");
    let path = dir.join("Song.scraft");
    let mut first = launch(&root);
    first.run("session.save_as", json!({"path": path})).unwrap();
    // Leftovers are offered only when newer than their session file; on filesystems with coarse
    // timestamps (tmpfs) the save and the autosave right after it can share one mtime (#579).
    let then = std::time::SystemTime::now() - std::time::Duration::from_secs(1);
    std::fs::File::options().write(true).open(&path).unwrap().set_modified(then).unwrap();
    first.run("track.new", json!({"name": "Vocal"})).unwrap();
    first.close_autosave();
    drop(first);

    let mut h =
        Harness::builder().with_context_setup(soundcraft_ui_egui::framework::install_headless).with_size(vec2(1600.0, 1000.0)).build_ui_state(
            |ui, app: &mut SoundApp| {
                app.logic(&ui.ctx().clone());
                app.ui(ui);
            },
            launch(&root),
        );
    h.run_steps(3);
    h.get_by_label("Recover").click();
    h.run_steps(3);
    let e = &h.state().engine;
    assert!(e.session().track_by_name("Vocal").is_some() && e.is_dirty());
    assert_eq!(e.path, Some(path.to_string_lossy().into_owned()));
    assert!(leftovers(&root).is_empty());
    assert_eq!(std::fs::read_dir(&root).unwrap().count(), 1);
    h.state_mut().run("session.save", json!({})).unwrap();
    assert!(eventually(|| std::fs::read_dir(&root).unwrap().count() == 0));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn every_change_is_autosaved_within_a_second() {
    let _one = one_at_a_time();
    let dir = folder("recovery-changes");
    let root = dir.join("Autosave");
    let mut h =
        Harness::builder().with_context_setup(soundcraft_ui_egui::framework::install_headless).with_size(vec2(1600.0, 1000.0)).build_ui_state(
            |ui, app: &mut SoundApp| {
                app.logic(&ui.ctx().clone());
                app.ui(ui);
            },
            launch(&root),
        );
    for name in ["Vocal", "Bass"] {
        h.state_mut().run("track.new", json!({"name": name})).unwrap();
        h.run_steps(1);
        assert!(eventually(|| autosaved(&root).contains(name)), "{name}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn recovering_keeps_the_unsaved_work_it_replaces() {
    let _one = one_at_a_time();
    let dir = folder("recovery-replace");
    let root = dir.join("Autosave");
    let mut first = launch(&root);
    first.run("track.new", json!({"name": "Vocal"})).unwrap();
    first.close_autosave();
    let left = leftovers(&root);
    assert_eq!((left.len(), left[0].name.as_str(), left[0].origin.as_deref()), (1, "Untitled", None));
    let mut next = launch(&root);
    next.run("track.new", json!({"name": "Drums"})).unwrap();
    next.recover(&left[0]);
    assert!(next.engine.session().track_by_name("Vocal").is_some() && next.engine.path.is_none());
    let kept = leftovers(&root);
    assert_eq!(kept.len(), 1);
    let mut third = launch(&root);
    third.recover(&kept[0]);
    assert!(third.engine.session().track_by_name("Drums").is_some());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn closing_the_session_or_quitting_drops_its_autosave() {
    let _one = one_at_a_time();
    let dir = folder("recovery-close");
    let root = dir.join("Autosave");
    for end in ["session.close", "app.quit"] {
        let mut first = launch(&root);
        first.run("track.new", json!({"name": "Vocal"})).unwrap();
        first.close_autosave();
        let mut next = launch(&root);
        let left = next.leftovers[0].clone();
        next.recover(&left);
        assert_eq!(std::fs::read_dir(&root).unwrap().count(), 1, "{end}");
        next.run(end, json!({})).unwrap();
        next.close_autosave();
        assert_eq!(std::fs::read_dir(&root).unwrap().count(), 0, "{end}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn quitting_while_recording_keeps_the_take_for_the_next_launch() {
    let _one = one_at_a_time();
    let dir = folder("recovery-recording");
    let root = dir.join("Autosave");
    let mut app = launch(&root);
    app.run("track.new", json!({"name": "Vocal"})).unwrap();
    app.engine.transport.recording = true;
    assert!(app.run("app.quit", json!({})).is_err(), "Quit waits for the take");
    app.quit_requested = true;
    app.close_autosave();
    assert!(!app.engine.transport.recording);
    assert_eq!(leftovers(&root).len(), 1);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_panic_in_a_frame_autosaves_and_the_app_carries_on() {
    let _one = one_at_a_time();
    let dir = folder("recovery-panic");
    let root = dir.join("Autosave");
    let mut app = launch(&root);
    app.run("track.new", json!({"name": "Vocal"})).unwrap();
    app.guarded(|_| std::panic::panic_any("a bug in a frame"));
    assert!(app.ui.status.contains("autosaved"), "{}", app.ui.status);
    app.guarded(|app| {
        app.run("track.new", json!({"name": "Bass"})).unwrap();
    });
    assert!(app.engine.session().track_by_name("Bass").is_some());
    drop(app);
    let left = leftovers(&root);
    let mut next = launch(&root);
    next.recover(&left[0]);
    assert!(next.engine.session().track_by_name("Vocal").is_some());
    let _ = std::fs::remove_dir_all(&dir);
}

/// A launch that finds an unsaved change from an earlier run, as the first test leaves it.
fn launch_with_a_leftover(name: &str) -> (PathBuf, PathBuf, Harness<'static, SoundApp>) {
    let dir = folder(name);
    let root = dir.join("Autosave");
    let path = dir.join("Song.scraft");
    let mut first = launch(&root);
    first.run("session.save_as", json!({"path": path})).unwrap();
    let then = std::time::SystemTime::now() - std::time::Duration::from_secs(1);
    std::fs::File::options().write(true).open(&path).unwrap().set_modified(then).unwrap();
    first.run("track.new", json!({"name": "Vocal"})).unwrap();
    first.close_autosave();
    drop(first);
    let mut h =
        Harness::builder().with_context_setup(soundcraft_ui_egui::framework::install_headless).with_size(vec2(1600.0, 1000.0)).build_ui_state(
            |ui, app: &mut SoundApp| {
                app.logic(&ui.ctx().clone());
                app.ui(ui);
            },
            launch(&root),
        );
    h.run_steps(4);
    (dir, root, h)
}

#[test]
fn the_recovery_question_is_a_modal_alert_that_takes_the_keyboard_and_escape_keeps_the_work() {
    let _one = one_at_a_time();
    let (dir, root, mut h) = launch_with_a_leftover("recovery-keyboard");
    let alert = h.get_by_role_and_label(Role::AlertDialog, "Recover Unsaved Work");
    assert!(alert.accesskit_node().is_modal(), "the question is modal");
    let area = alert.rect();
    assert!(h.get_by_label("Recover").is_focused(), "the question starts on its default button");
    for _ in 0..4 {
        h.key_press(egui::Key::Tab);
        h.run_steps(2);
        let focused = h.query_all(by().predicate(|n| n.is_focused())).next().expect("something has the focus");
        assert!(area.contains(focused.rect().center()), "Tab stays inside the question, but reached {:?}", focused.accesskit_node().label());
    }
    h.key_press(egui::Key::Escape);
    h.run_steps(3);
    assert!(h.query_by_role_and_label(Role::AlertDialog, "Recover Unsaved Work").is_none(), "Escape closes the question");
    assert!(!leftovers(&root).is_empty(), "closing the question keeps the autosave for the next run");
    let _ = std::fs::remove_dir_all(&dir);
}
