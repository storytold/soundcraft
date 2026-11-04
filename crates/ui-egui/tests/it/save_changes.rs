use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use serde_json::json;
use soundcraft_ui_egui::dialogs::Dialog;
use soundcraft_ui_egui::{Services, SoundApp, dialogs, menus};

fn harness() -> Harness<'static, SoundApp> {
    let app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
    Harness::builder().with_size(egui::vec2(1600.0, 1000.0)).build_ui_state(
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

fn click(h: &mut Harness<'_, SoundApp>, label: &str) {
    h.get_by_label(label).click();
    steps(h);
}

fn close_session(h: &mut Harness<'_, SoundApp>) {
    menus::invoke_menu(h.state_mut(), "session.close", "File > Close Session");
    steps(h);
}

fn mute_kick(h: &mut Harness<'_, SoundApp>) {
    h.state_mut().run("mix.mute", json!({"track": "Kick"})).unwrap();
}

#[test]
fn closing_a_modified_session_asks_first() {
    let mut h = harness();
    let tracks = h.state().engine.session().tracks.len();
    h.state_mut().run("edit.select", json!({"tracks": ["Kick"], "start": 0, "end": 48_000})).unwrap();
    close_session(&mut h);
    assert!(h.state().dialogs.open.is_none(), "a selection alone is not a change");
    assert!(h.state().engine.session().tracks.is_empty());

    let mut h = harness();
    mute_kick(&mut h);
    close_session(&mut h);
    assert_eq!(h.state().dialogs.open_name(), Some("save_changes"));
    click(&mut h, "Cancel");
    assert!(h.state().dialogs.open.is_none());
    assert_eq!(h.state().engine.session().tracks.len(), tracks);

    close_session(&mut h);
    click(&mut h, "Don't Save");
    assert!(h.state().engine.session().tracks.is_empty());

    let mut h = harness();
    mute_kick(&mut h);
    menus::invoke_menu(h.state_mut(), "app.quit", "");
    steps(&mut h);
    assert!(!h.state().quit_requested);
    click(&mut h, "Don't Save");
    assert!(h.state().quit_requested);
}

#[test]
fn save_in_the_question_saves_before_closing() {
    let dir = std::env::temp_dir().join(format!("soundcraft-save-changes-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let path = dir.join("Song.scraft").to_string_lossy().into_owned();

    let mut h = harness();
    mute_kick(&mut h);
    close_session(&mut h);
    click(&mut h, "Save");
    match &mut h.state_mut().dialogs.open {
        Some(Dialog::PathPrompt { cmd, path: p, .. }) if cmd == "session.save_as" => p.clone_from(&path),
        other => panic!("expected Save As, got {other:?}"),
    }
    steps(&mut h);
    click(&mut h, "OK");
    assert!(h.state().engine.session().tracks.is_empty());

    h.state_mut().run("session.open", json!({"path": path})).unwrap();
    assert!(h.state().engine.session().track_by_name("Kick").unwrap().mixer.mute);
    h.state_mut().run("mix.mute", json!({"track": "Kick"})).unwrap();
    close_session(&mut h);
    click(&mut h, "Save");
    assert!(h.state().dialogs.open.is_none());
    assert!(h.state().engine.session().tracks.is_empty());
    h.state_mut().run("session.open", json!({"path": path})).unwrap();
    assert!(!h.state().engine.session().track_by_name("Kick").unwrap().mixer.mute);
    let _ = std::fs::remove_dir_all(&dir);
}

/// A session the browser read: an empty one, so opening it is easy to see.
fn session_bytes() -> Vec<u8> {
    soundcraft_engine::Engine::default().session().to_json().unwrap().into_bytes()
}

fn open_bytes(h: &mut Harness<'_, SoundApp>) {
    dialogs::open_session_bytes(h.state_mut(), "Other.scraft".into(), session_bytes());
    steps(h);
}

#[test]
fn a_session_read_in_the_browser_asks_before_replacing_a_modified_one() {
    let mut h = harness();
    open_bytes(&mut h);
    assert!(h.state().dialogs.open.is_none(), "an unmodified session is replaced without asking");
    assert!(h.state().engine.session().tracks.is_empty());

    let mut h = harness();
    let tracks = h.state().engine.session().tracks.len();
    mute_kick(&mut h);
    open_bytes(&mut h);
    assert_eq!(h.state().dialogs.open_name(), Some("save_changes"));
    click(&mut h, "Cancel");
    assert!(h.state().dialogs.open.is_none());
    assert_eq!(h.state().engine.session().tracks.len(), tracks);
    h.state_mut().run("edit.undo", json!({})).unwrap();
    assert!(!h.state().engine.session().track_by_name("Kick").unwrap().mixer.mute, "Cancel keeps the undo history");

    mute_kick(&mut h);
    open_bytes(&mut h);
    click(&mut h, "Don't Save");
    assert!(h.state().engine.session().tracks.is_empty());
}

#[test]
fn save_in_the_question_saves_before_opening_a_session_read_in_the_browser() {
    let dir = std::env::temp_dir().join(format!("soundcraft-save-changes-bytes-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let path = dir.join("Song.scraft").to_string_lossy().into_owned();

    let mut h = harness();
    mute_kick(&mut h);
    open_bytes(&mut h);
    click(&mut h, "Save");
    match &mut h.state_mut().dialogs.open {
        Some(Dialog::PathPrompt { cmd, path: p, .. }) if cmd == "session.save_as" => p.clone_from(&path),
        other => panic!("expected Save As, got {other:?}"),
    }
    steps(&mut h);
    click(&mut h, "OK");
    assert!(h.state().engine.session().tracks.is_empty());
    h.state_mut().run("session.open", json!({"path": path})).unwrap();
    assert!(h.state().engine.session().track_by_name("Kick").unwrap().mixer.mute);
    let _ = std::fs::remove_dir_all(&dir);
}
