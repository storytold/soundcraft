//! Ruler menu state and form dispatch regressions.
use serde_json::json;
use soundcraft_ui_egui::{Services, SoundApp, menus};
#[test]
fn expanded_rulers_reveal_parents_and_share_checked_state() {
    let mut app = SoundApp::new(soundcraft_engine::Engine::default(), None, Services::default());
    app.run("view.ruler_minimal", json!({})).unwrap();
    for (path, ruler) in [("View > Rulers > Tempo > Tempo Editor", "tempo"), ("View > Rulers > Key Signature > Key Signature Staff", "key")] {
        let id = menus::command_for_path(path, &menus::ui_aliases()).unwrap();
        menus::invoke_menu(&mut app, &id, path);
        assert!(app.engine.session().edit.rulers.iter().any(|r| r == ruler));
        assert_eq!(menus::item_state(&app, path, Some(&id)), (true, true));
    }
    app.run("view.ruler_timecode2", json!({"visible":true})).unwrap();
    assert_eq!(menus::item_state(&app, "View > Rulers > Timecode 2", Some("view.ruler_timecode2")), (true, true));
}
#[test]
fn key_meter_and_chord_menus_open_real_forms_without_creating_events() {
    let mut app = SoundApp::new(soundcraft_engine::Engine::default(), None, Services::default());
    for id in ["event.add_key_change", "event.meter", "chord.add"] {
        menus::invoke_menu(&mut app, id, "");
        assert_eq!(app.dialogs.open_name(), Some("conductor_change"));
        app.dialogs.open = None;
        assert!(app.engine.session().key_signatures.is_empty());
        assert!(app.engine.session().chords.is_empty());
    }
}

#[test]
fn every_conductor_header_add_button_opens_its_form_without_seeking() {
    use egui_kittest::kittest::Queryable;
    let mut app = SoundApp::new(soundcraft_engine::Engine::default(), None, Services::default());
    app.run("track.new", json!({})).unwrap();
    app.run("view.ruler_all", json!({})).unwrap();
    let mut h = egui_kittest::Harness::builder().with_size(egui::vec2(1600.0, 1200.0)).build_ui_state(
        |ui, app: &mut SoundApp| {
            app.logic(ui.ctx());
            app.ui(ui);
        },
        app,
    );
    h.run_steps(5);
    for (name, expected) in [
        ("Add tempo change", "tempo_change"),
        ("Add meter change", "conductor_change"),
        ("Add key change", "conductor_change"),
        ("Add chords change", "conductor_change"),
    ] {
        let before = h.state().engine.session().edit.selection;
        let at = h.get_by_label(name).rect().center();
        for pressed in [true, false] {
            h.event(egui::Event::PointerMoved(at));
            h.event(egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE });
            h.run_steps(1);
        }
        h.run_steps(3);
        assert_eq!(h.state().dialogs.open_name(), Some(expected), "{name}");
        assert_eq!(h.state().engine.session().edit.selection, before);
        h.state_mut().dialogs.open = None;
        h.run_steps(3);
    }
}

#[test]
fn dragging_a_chord_previews_then_commits_one_undo_and_keeps_markers() {
    use egui_kittest::kittest::Queryable;
    let mut app = SoundApp::new(soundcraft_engine::Engine::default(), None, Services::default());
    app.run("view.ruler_all", json!({})).unwrap();
    app.run("chord.add", json!({"at":96000,"root":"G","quality":"7"})).unwrap();
    app.run("markers.add", json!({"at":96000,"name":"Keep","ruler":2})).unwrap();
    let mut h = egui_kittest::Harness::builder().with_size(egui::vec2(1600.0, 1200.0)).build_ui_state(
        |ui, app: &mut SoundApp| {
            app.logic(ui.ctx());
            app.ui(ui);
        },
        app,
    );
    h.run_steps(5);
    let y = h.get_by_label("Add chords change").rect().center().y;
    let x = h.state().edit_layout.timeline[0] + 96000.0 / h.state().engine.session().edit.zoom.samples_per_px as f32 + 4.0;
    let start = egui::pos2(x, y);
    let end = start + egui::vec2(40.0, 0.0);
    let before = h.state().engine.session().chords.clone();
    let markers = h.state().engine.session().markers.clone();
    let undo = h.state().engine.undo_history().len();
    h.event(egui::Event::PointerMoved(start));
    h.event(egui::Event::PointerButton { pos: start, button: egui::PointerButton::Primary, pressed: true, modifiers: egui::Modifiers::NONE });
    h.run_steps(1);
    for n in 1..=4 {
        h.event(egui::Event::PointerMoved(start + egui::vec2(n as f32 * 10.0, 0.0)));
        h.run_steps(1);
        assert_eq!(h.state().engine.session().chords, before);
    }
    h.event(egui::Event::PointerButton { pos: end, button: egui::PointerButton::Primary, pressed: false, modifiers: egui::Modifiers::NONE });
    h.run_steps(3);
    assert!(h.state().engine.session().chords[0].at > 96000);
    assert_eq!(h.state().engine.session().markers, markers);
    assert_eq!(h.state().engine.undo_history().len(), undo + 1);
    h.state_mut().engine.undo();
    assert_eq!(h.state().engine.session().chords, before);
}
