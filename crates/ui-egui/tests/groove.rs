use egui_kittest::{Harness, kittest::Queryable};
use serde_json::json;
use soundcraft_ui_egui::{Services, SoundApp, menus};

fn harness() -> Harness<'static, SoundApp> {
    let app = SoundApp::new(soundcraft_engine::Engine::default(), None, Services::default());
    let mut h = Harness::builder().with_size(egui::vec2(1300.0, 850.0)).build_ui_state(
        |ui, app: &mut SoundApp| {
            app.logic(&ui.ctx().clone());
            app.ui(ui);
        },
        app,
    );
    h.run_steps(5);
    h
}

fn click(h: &mut Harness<'_, SoundApp>, label: &str) {
    h.get_by_label(label).click();
    h.run_steps(5);
}

#[test]
fn create_edit_reopen_and_detach_use_the_visible_controls() {
    let mut h = harness();
    menus::invoke_menu(h.state_mut(), "groove.new", "Track > New Groove...");
    h.run_steps(5);
    let kick = h.get_by_label("Kick step 1").rect();
    let snare = h.get_by_label("Snare step 1").rect();
    let ride = h.get_by_label("Ride step 1").rect();
    assert!(kick.max.y < snare.min.y && snare.max.y < ride.min.y);
    assert!((kick.min.x - ride.min.x).abs() < 1.0, "step columns must line up across voices");
    click(&mut h, "Create groove track");
    assert_eq!(h.state().engine.session().tracks.len(), 1);
    let clip = h.state().engine.session().tracks[0].clips()[0].clone();
    assert!(clip.groove.is_some());
    assert_eq!(h.state().engine.undo_history().len(), 1);
    click(&mut h, "Kick step 2");
    assert!(h.state().groove.dirty);
    assert_eq!(h.state().engine.session().tracks[0].clips()[0], clip);
    click(&mut h, "Apply changes");
    let edited = h.state().engine.session().tracks[0].clips()[0].clone();
    assert_ne!(clip.content, edited.content);
    assert_eq!(h.state().engine.undo_history().len(), 2);
    h.state_mut().run("window.groove", json!({"value":false})).unwrap();
    h.state_mut().run("groove.open", json!({"clip":clip.id})).unwrap();
    h.run_steps(5);
    assert_eq!(h.state().engine.session().tracks[0].clips()[0], edited);
    click(&mut h, "Make ordinary MIDI");
    let detached = &h.state().engine.session().tracks[0].clips()[0];
    assert!(detached.groove.is_none());
    assert_eq!(detached.content, edited.content);
    assert!(h.state().ui.show_midi_editor);
}

#[test]
fn undo_refreshes_the_editor_and_new_session_drops_stale_targets() {
    let mut h = harness();
    let v = h.state_mut().run("groove.create", json!({})).unwrap();
    h.state_mut().run("groove.open", json!({"clip":v["clip"]})).unwrap();
    h.run_steps(5);
    click(&mut h, "Kick step 2");
    click(&mut h, "Apply changes");
    assert!(h.state().groove.config.lanes[0].cells.is_some());
    h.state_mut().run("edit.undo", json!({})).unwrap();
    h.run_steps(5);
    assert!(h.state().groove.config.lanes[0].cells.is_none());
    h.state_mut().run("session.new", json!({})).unwrap();
    h.run_steps(5);
    assert!(h.state().groove.target.is_none());
}

#[test]
fn new_track_dialog_links_to_builder_without_creating_a_track() {
    let mut h = harness();
    menus::invoke_menu(h.state_mut(), "track.new", "Track > New...");
    h.run_steps(5);
    click(&mut h, "Build a groove pattern...");
    assert!(h.state().ui.show_groove);
    assert!(h.state().dialogs.open_name().is_none());
    assert!(h.state().engine.session().tracks.is_empty());
}

#[test]
fn builder_menu_entries_exist() {
    let tree = menus::tree();
    for path in ["Track > New Groove...", "Window > Groove Builder"] {
        assert!(tree.iter().flat_map(|root| &root.children).any(|node| node.path == path));
    }
}
