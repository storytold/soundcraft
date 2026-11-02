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

fn ruler_harness() -> egui_kittest::Harness<'static, SoundApp> {
    let mut app = SoundApp::new(soundcraft_engine::Engine::default(), None, Services::default());
    app.run("view.ruler_all", json!({})).unwrap();
    let mut h = egui_kittest::Harness::builder().with_size(egui::vec2(1600.0, 1200.0)).with_step_dt(1.0 / 60.0).build_ui_state(
        |ui, app: &mut SoundApp| {
            app.logic(ui.ctx());
            app.ui(ui);
        },
        app,
    );
    h.run_steps(5);
    h
}

fn click(h: &mut egui_kittest::Harness<'_, SoundApp>, at: egui::Pos2, button: egui::PointerButton) {
    for pressed in [true, false] {
        h.event(egui::Event::PointerMoved(at));
        h.event(egui::Event::PointerButton { pos: at, button, pressed, modifiers: egui::Modifiers::NONE });
        h.run_steps(1);
    }
    h.run_steps(3);
}

#[test]
fn changing_an_add_forms_position_preserves_the_source_event() {
    use egui_kittest::kittest::Queryable;
    use soundcraft_ui_egui::dialogs::Dialog;
    for kind in ["tempo", "meter", "key"] {
        let mut h = ruler_harness();
        h.state_mut().run("event.tempo", json!({"at":96000,"bpm":140})).unwrap();
        h.state_mut().run("event.meter", json!({"at":96000,"numerator":3,"denominator":4})).unwrap();
        h.state_mut().run("event.key_edit", json!({"at":96000,"key":"D major"})).unwrap();
        h.state_mut().run("edit.select", json!({"start":96000,"end":96000})).unwrap();
        h.run_steps(3);
        let source = h.state().engine.session().clone();
        let at = h.get_by_label(&format!("Add {kind} change")).rect().center();
        click(&mut h, at, egui::PointerButton::Primary);
        match h.state_mut().dialogs.open.as_mut().unwrap() {
            Dialog::TempoChange { at, original, bpm } => {
                assert_eq!(*original, None);
                *at = 384000;
                *bpm = 150.0;
            }
            Dialog::Conductor(form) => {
                assert_eq!(form.original, None);
                form.at = 384000;
                form.numerator = 7;
                form.key = "E major".into();
            }
            _ => panic!("unexpected form"),
        }
        h.run_steps(2);
        let ok = h.get_by_label("OK").rect().center();
        click(&mut h, ok, egui::PointerButton::Primary);
        assert!(h.state().dialogs.open.is_none());
        let result = h.state().engine.session();
        match kind {
            "tempo" => {
                assert!(result.tempo.tempos().iter().any(|t| t.tick == source.tempo.tempos()[1].tick && t.bpm == 140.0));
                assert_eq!(result.tempo.tempos().len(), source.tempo.tempos().len() + 1);
            }
            "meter" => {
                assert!(result.tempo.meters().iter().any(|m| m.tick == source.tempo.meters()[1].tick && m.numerator == 3));
                assert_eq!(result.tempo.meters().len(), source.tempo.meters().len() + 1);
            }
            _ => assert_eq!(result.key_signatures, vec![(96000, "D major".into()), (384000, "E major".into())]),
        }
    }
}

#[test]
fn conductor_popup_keeps_its_right_click_target_when_pointer_leaves_ruler() {
    use egui_kittest::kittest::Queryable;
    let mut h = ruler_harness();
    h.state_mut().run("chord.add", json!({"at":96000,"root":"D"})).unwrap();
    h.state_mut().run("chord.add", json!({"at":384000,"root":"G"})).unwrap();
    h.run_steps(3);
    let y = h.get_by_label("Add chords change").rect().center().y;
    let x = h.state().edit_layout.timeline[0] + 96000.0 / h.state().engine.session().edit.zoom.samples_per_px as f32 + 4.0;
    click(&mut h, egui::pos2(x, y), egui::PointerButton::Secondary);
    let delete = h.get_by_label("Delete").rect().center();
    assert!(delete.y > y + 10.0);
    h.event(egui::Event::PointerMoved(delete));
    h.run_steps(5);
    let delete = h.get_by_label("Delete").rect().center();
    let before = h.state().engine.session().chords.clone();
    let undo = h.state().engine.undo_history().len();
    click(&mut h, delete, egui::PointerButton::Primary);
    assert_eq!(h.state().engine.session().chords, vec![before[1].clone()]);
    assert_eq!(h.state().engine.undo_history().len(), undo + 1);
    assert!(h.state_mut().engine.undo());
    assert_eq!(h.state().engine.session().chords, before);
}

#[test]
fn conductor_popup_cannot_delete_from_a_replaced_document() {
    use egui_kittest::kittest::Queryable;
    let mut h = ruler_harness();
    h.state_mut().run("chord.add", json!({"at":96000,"root":"D"})).unwrap();
    h.run_steps(3);
    let y = h.get_by_label("Add chords change").rect().center().y;
    let x = h.state().edit_layout.timeline[0] + 96000.0 / h.state().engine.session().edit.zoom.samples_per_px as f32 + 4.0;
    click(&mut h, egui::pos2(x, y), egui::PointerButton::Secondary);
    let delete = h.get_by_label("Delete").rect().center();
    h.state_mut().engine = soundcraft_engine::Engine::default();
    h.state_mut().run("view.ruler_all", json!({})).unwrap();
    h.state_mut().run("chord.add", json!({"at":96000,"root":"G"})).unwrap();
    let before = h.state().engine.session().chords.clone();
    click(&mut h, delete, egui::PointerButton::Primary);
    assert_eq!(h.state().engine.session().chords, before);
}

#[test]
fn double_click_distinguishes_existing_events_from_empty_ruler_space() {
    use egui_kittest::kittest::Queryable;
    use soundcraft_ui_egui::dialogs::Dialog;
    for (kind, command, params) in [
        ("tempo", "event.tempo", json!({"at":96000,"bpm":140})),
        ("meter", "event.meter", json!({"at":96000,"numerator":3,"denominator":4})),
        ("key", "event.key_edit", json!({"at":96000,"key":"D major"})),
    ] {
        for existing in [false, true] {
            let mut h = ruler_harness();
            h.state_mut().run(command, params.clone()).unwrap();
            h.run_steps(3);
            let sample = if existing { 96000.0 } else { 384000.0 };
            let y = h.get_by_label(&format!("Add {kind} change")).rect().center().y;
            let x = h.state().edit_layout.timeline[0] + sample / h.state().engine.session().edit.zoom.samples_per_px as f32 + 4.0;
            click(&mut h, egui::pos2(x, y), egui::PointerButton::Primary);
            click(&mut h, egui::pos2(x, y), egui::PointerButton::Primary);
            assert!(h.state().dialogs.open.is_some(), "no form: {kind}, existing={existing}, position={x},{y}");
            let original = match h.state().dialogs.open.as_ref().unwrap() {
                Dialog::TempoChange { original, .. } => *original,
                Dialog::Conductor(form) => form.original,
                _ => panic!("unexpected form"),
            };
            assert_eq!(original, existing.then_some(96000), "{kind}, existing={existing}");
        }
    }
}
