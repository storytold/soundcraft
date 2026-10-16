//! Track/menu/shortcut entry points share immutable source, range and revision previews.
use egui::{Event, Modifiers, PointerButton, pos2};
use egui_kittest::{
    Harness,
    kittest::{NodeT, Queryable},
};
use serde_json::{Value, json};
use soundcraft_ui_egui::{Services, SoundApp, dialogs::Dialog, menus};
fn harness() -> Harness<'static, SoundApp> {
    Harness::builder().with_size(egui::vec2(1600.0, 1000.0)).build_ui_state(
        |ui, app: &mut SoundApp| {
            let ctx = ui.ctx().clone();
            app.logic(&ctx);
            app.ui(ui);
        },
        SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default()),
    )
}
fn steps(h: &mut Harness<'_, SoundApp>) {
    for _ in 0..6 {
        let mut raw = std::mem::take(h.input_mut());
        h.state_mut().raw_input_hook(&mut raw);
        *h.input_mut() = raw;
        h.step();
    }
}
fn params(h: &Harness<'_, SoundApp>) -> Value {
    match &h.state().dialogs.open {
        Some(Dialog::TrackBounce(d)) => d.params(),
        other => panic!("expected Track Bounce, got {other:?}"),
    }
}
#[test]
fn menu_and_shortcut_snapshot_sources_timeline_and_revision_and_disable_stale_exports() {
    let mut h = harness();
    h.state_mut().run("edit.select", json!({"tracks":["Kick"],"start":100,"end":200})).unwrap();
    menus::invoke_menu(h.state_mut(), "track.bounce", "Track > Bounce...");
    steps(&mut h);
    let p = params(&h);
    assert_eq!(p["start"], 100);
    assert_eq!(p["end"], 200);
    let kick = h.state().engine.session().track_by_name("Kick").unwrap().id;
    assert_eq!(p["tracks"], json!([kick]));
    assert_eq!(p["expected_revision"], h.state().engine.revision);
    h.get_by_label("Cancel").click();
    steps(&mut h);
    assert!(h.state().dialogs.open.is_none());
    assert!(menus::invoke_shortcut_dialog(h.state_mut(), "track.bounce"));
    steps(&mut h);
    assert_eq!(params(&h), p);
    h.state_mut().run("mix.mute", json!({"track":"Kick"})).unwrap();
    steps(&mut h);
    assert!(h.state_mut().engine.execute("track.bounce_preview", &p).unwrap_err().to_string().contains("stale"));
    assert!(h.get_by_label("Bounce").accesskit_node().is_disabled());
    h.state_mut().engine.replace_session(soundcraft_model::Session::default());
    steps(&mut h);
    assert!(h.state().dialogs.open.is_none(), "document replacement must close the old export dialog");
}
#[test]
fn header_context_bounce_uses_clicked_track_even_when_another_is_selected() {
    let mut h = harness();
    h.state_mut().run("edit.select", json!({"tracks":["Snare"]})).unwrap();
    steps(&mut h);
    let kick = h.state().engine.session().track_by_name("Kick").unwrap().id;
    let (_, r) = h.state().edit_layout.rows.iter().find(|(id, _)| *id == kick.0).unwrap();
    let pos = pos2(r[0] + 60.0, r[1] + 10.0);
    h.state_mut().synthetic.extend([
        Event::PointerMoved(pos),
        Event::PointerButton { pos, button: PointerButton::Secondary, pressed: true, modifiers: Modifiers::NONE },
        Event::PointerButton { pos, button: PointerButton::Secondary, pressed: false, modifiers: Modifiers::NONE },
    ]);
    steps(&mut h);
    h.get_by_label("Bounce...").click();
    steps(&mut h);
    assert_eq!(params(&h)["tracks"], json!([kick]));
    assert_eq!(h.state().engine.session().edit.selected_tracks, vec![h.state().engine.session().track_by_name("Snare").unwrap().id]);
}

#[test]
fn actual_platform_shortcut_opens_the_track_bounce_dialog() {
    for os in [egui::os::OperatingSystem::Mac, egui::os::OperatingSystem::Nix, egui::os::OperatingSystem::Windows] {
        let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
        app.engine.execute("edit.select", &json!({"tracks":["Kick"]})).unwrap();
        let ctx = egui::Context::default();
        ctx.set_os(os);
        let modifiers = Modifiers {
            command: true,
            mac_cmd: os == egui::os::OperatingSystem::Mac,
            ctrl: os != egui::os::OperatingSystem::Mac,
            alt: true,
            shift: true,
        };
        let events = [true, false].map(|pressed| Event::Key { key: egui::Key::B, physical_key: None, pressed, repeat: false, modifiers }).to_vec();
        ctx.run_ui(egui::RawInput { events, ..Default::default() }, |ui| soundcraft_ui_egui::shortcuts::handle(&mut app, ui.ctx()))
            .textures_delta
            .clear();
        assert_eq!(app.dialogs.open_name(), Some("track_bounce"), "{os:?}");
    }
}

#[cfg(unix)]
#[test]
fn programmatic_app_export_keeps_document_and_history_identity() {
    let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
    let dir = std::env::temp_dir().join(format!("sc-ui-track-bounce-{}", std::process::id()));
    std::fs::create_dir(&dir).unwrap();
    let path = dir.join("kick.wav");
    let before = app.engine.session_arc();
    let revision = app.engine.revision;
    let journal = app.engine.journal.clone();
    let undo = app.engine.undo_history();
    app.run("track.bounce", json!({"tracks":["Kick"],"path":path,"start":0,"end":4000,"render_pan":false})).unwrap();
    assert!(std::sync::Arc::ptr_eq(&before, &app.engine.session_arc()));
    assert_eq!(app.engine.revision, revision);
    assert_eq!(app.engine.journal, journal);
    assert_eq!(app.engine.undo_history(), undo);
    let decoded = soundcraft_audio_io::decode(&std::fs::read(&path).unwrap(), None).unwrap().1;
    assert_eq!(decoded.frames(), 4000);
    assert!(app.dialogs.open.is_none());
    std::fs::remove_dir_all(dir).unwrap();
}

#[cfg(unix)]
#[test]
fn escape_or_cancel_with_enter_never_delivers_a_ready_export() {
    let dir = std::env::temp_dir().join(format!("sc-ui-track-bounce-cancel-{}", std::process::id()));
    std::fs::create_dir(&dir).unwrap();
    for cancel_button in [false, true] {
        let mut h = harness();
        h.state_mut().run("edit.select", json!({"tracks":["Kick"],"start":0,"end":128})).unwrap();
        menus::invoke_menu(h.state_mut(), "track.bounce", "Track > Bounce...");
        steps(&mut h);
        let path = dir.join(if cancel_button { "cancel.wav" } else { "escape.wav" });
        h.get_all_by_role(egui::accesskit::Role::TextInput)
            .into_iter()
            .find(|n| n.value().is_some_and(|v| v.ends_with("Bounce.wav")))
            .unwrap()
            .click();
        steps(&mut h);
        let command = Modifiers { command: true, mac_cmd: true, ..Modifiers::NONE };
        h.state_mut().synthetic.extend([
            Event::Key { key: egui::Key::A, physical_key: None, pressed: true, repeat: false, modifiers: command },
            Event::Key { key: egui::Key::A, physical_key: None, pressed: false, repeat: false, modifiers: command },
            Event::Text(path.to_string_lossy().into_owned()),
        ]);
        steps(&mut h);
        assert_eq!(params(&h)["path"], json!(path));
        let p = params(&h);
        assert_eq!(h.state_mut().engine.execute("track.bounce_preview", &p).unwrap()["ready"], true);
        if cancel_button {
            let pos = h.get_by_label("Cancel").rect().center();
            h.state_mut().synthetic.extend([
                Event::PointerMoved(pos),
                Event::PointerButton { pos, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE },
                Event::PointerButton { pos, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE },
            ]);
        } else {
            h.state_mut().synthetic.push(Event::Key {
                key: egui::Key::Escape,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: Modifiers::NONE,
            });
        }
        h.state_mut().synthetic.push(Event::Key {
            key: egui::Key::Enter,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Modifiers::NONE,
        });
        steps(&mut h);
        assert!(h.state().dialogs.open.is_none());
        assert!(!path.exists(), "Cancel/Escape must suppress same-frame Enter export; cancel_button={cancel_button}");
    }
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
    std::fs::remove_dir(dir).unwrap();
}
