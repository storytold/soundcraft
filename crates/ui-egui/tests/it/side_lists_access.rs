//! #651: the Tracks and Groups lists publish their rows to AccessKit, by name, so screen readers
//! can read them and tests can reach them by label instead of by geometry.

use egui::accesskit::{Role, Toggled};
use egui::vec2;
use egui_kittest::{
    Harness,
    kittest::{NodeT, Queryable},
};
use serde_json::json;
use soundcraft_ui_egui::{Services, SoundApp};

fn harness() -> Harness<'static, SoundApp> {
    let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
    let first = app.engine.session().tracks.first().map(|t| t.id.0).unwrap_or_default();
    app.run("track.group", json!({"name": "Rhythm", "tracks": [first]})).unwrap();
    app.engine.session_mut().edit.selected_tracks.clear();
    let mut h = Harness::builder().with_size(vec2(1600.0, 1000.0)).build_ui_state(
        |ui, app: &mut SoundApp| {
            let ctx = ui.ctx().clone();
            app.logic(&ctx);
            app.ui(ui);
        },
        app,
    );
    h.run_steps(3);
    h
}

#[test]
fn tracks_list_rows_are_named_and_report_their_selection() {
    let mut h = harness();
    let tracks: Vec<(u64, String)> = h.state().engine.session().tracks.iter().map(|t| (t.id.0, t.name.clone())).collect();
    assert!(tracks.len() >= 2);
    for (_, name) in &tracks {
        assert_eq!(h.get_by_role_and_label(Role::Button, name).accesskit_node().toggled(), Some(Toggled::False), "{name}");
    }
    let (id, name) = &tracks[1];
    h.get_by_role_and_label(Role::Button, name).click();
    h.run_steps(2);
    assert_eq!(h.state().engine.session().edit.selected_tracks.iter().map(|t| t.0).collect::<Vec<_>>(), vec![*id]);
    assert_eq!(h.get_by_role_and_label(Role::Button, name).accesskit_node().toggled(), Some(Toggled::True));
}

#[test]
fn groups_list_rows_are_named_and_report_whether_the_group_is_active() {
    let mut h = harness();
    h.get_by_label("<ALL>");
    assert_eq!(h.get_by_role_and_label(Role::CheckBox, "Rhythm").accesskit_node().toggled(), Some(Toggled::True));
    h.get_by_role_and_label(Role::CheckBox, "Rhythm").click();
    h.run_steps(2);
    assert!(h.state().engine.session().groups.iter().all(|g| !g.active));
    assert_eq!(h.get_by_role_and_label(Role::CheckBox, "Rhythm").accesskit_node().toggled(), Some(Toggled::False));
}
