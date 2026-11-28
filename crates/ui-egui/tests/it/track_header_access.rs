//! Edit window track headers publish their controls to AccessKit: each a role, a name that says which
//! track it belongs to and its state, and no nameless Tab stops for areas only the pointer needs.

use egui::accesskit::{Action, HasPopup, Role, Toggled};
use egui::{Rect, vec2};
use egui_kittest::{
    Harness, Node,
    kittest::{NodeT, Queryable, by},
};
use serde_json::json;
use soundcraft_ui_egui::{Services, SoundApp};

fn harness() -> Harness<'static, SoundApp> {
    let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
    app.engine.session_mut().edit.selected_tracks.clear();
    let mut h =
        Harness::builder().with_context_setup(soundcraft_ui_egui::framework::install_headless).with_size(vec2(1600.0, 1000.0)).build_ui_state(
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

fn first_track(h: &Harness<'_, SoundApp>) -> (u64, String) {
    let t = &h.state().engine.session().tracks[0];
    (t.id.0, t.name.clone())
}

/// The first track's header: from its options button to its automation-mode selector, as tall as the track.
fn header_rect(h: &Harness<'_, SoundApp>, name: &str) -> Rect {
    let options = h.get_by_label(&format!("Track options for {name}")).rect();
    let mode = h.get_by_label(&format!("Automation mode for {name}")).rect();
    let height = h.state().engine.session().tracks[0].height.points();
    Rect::from_min_max(options.min - vec2(4.0, 4.0), egui::pos2(mode.max.x + 4.0, options.min.y + height))
}

fn name_plate<'a>(h: &'a Harness<'_, SoundApp>, name: &str, header: Rect) -> Node<'a> {
    let label = format!("{name} track");
    h.query_all_by_role(Role::Button)
        .find(|n| n.accesskit_node().label().as_deref() == Some(label.as_str()) && header.contains(n.rect().center()))
        .expect("the header's name plate")
}

#[test]
fn every_header_control_has_a_role_and_a_name_and_pointer_areas_are_no_tab_stops() {
    let h = harness();
    let (_, name) = first_track(&h);
    let header = header_rect(&h, &name);
    let mut controls = 0;
    for node in h.query_all(by().predicate(|n| n.data().supports_action(Action::Click) || n.data().supports_action(Action::Focus))) {
        if !header.contains(node.rect().center()) {
            continue;
        }
        let ak = node.accesskit_node();
        if ak.role() == Role::GenericContainer {
            assert!(!ak.data().supports_action(Action::Focus), "a pointer-only area at {:?} is a Tab stop", node.rect());
            continue;
        }
        controls += 1;
        let label = ak.label().unwrap_or_default();
        assert_ne!(ak.role(), Role::Unknown, "a header control at {:?} has no role ({label:?})", node.rect());
        assert!(!label.trim().is_empty(), "a header control at {:?} with role {:?} has no name", node.rect(), ak.role());
    }
    assert!(controls >= 8, "the header's own controls are reachable, found {controls}");
}

#[test]
fn header_menus_say_they_open_a_menu_and_name_their_track() {
    let h = harness();
    let (_, name) = first_track(&h);
    for label in ["Track options", "Playlists"] {
        let full = format!("{label} for {name}");
        let node = h.get_by_role_and_label(Role::Button, &full);
        assert_eq!(node.accesskit_node().has_popup(), Some(HasPopup::Menu), "{label}");
    }
}

#[test]
fn the_name_plate_reports_the_tracks_selection() {
    let mut h = harness();
    let (id, name) = first_track(&h);
    let header = header_rect(&h, &name);
    assert_eq!(name_plate(&h, &name, header).accesskit_node().toggled(), Some(Toggled::False));
    name_plate(&h, &name, header).click();
    h.run_steps(2);
    assert!(h.state().engine.session().edit.selected_tracks.iter().any(|t| t.0 == id));
    assert_eq!(name_plate(&h, &name, header).accesskit_node().toggled(), Some(Toggled::True));
}

#[test]
fn freeze_keeps_its_name_and_reports_its_state() {
    let mut h = harness();
    let (id, name) = first_track(&h);
    let label = format!("Freeze for {name}");
    assert_eq!(h.get_by_role_and_label(Role::Button, &label).accesskit_node().toggled(), Some(Toggled::False));
    h.state_mut().run("track.freeze", json!({"tracks": [id]})).unwrap();
    h.run_steps(2);
    assert_eq!(h.get_by_role_and_label(Role::Button, &label).accesskit_node().toggled(), Some(Toggled::True));
}
