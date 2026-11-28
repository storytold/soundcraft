//! The Edit window toolbar and the Transport window publish every control to AccessKit with a role, a
//! name and its state, so screen readers hear "Play, toggle button, pressed" instead of an unnamed object.

use egui::accesskit::{Action, HasPopup, Role, Toggled};
use egui::vec2;
use egui_kittest::{
    Harness,
    kittest::{NodeT, Queryable, by},
};
use serde_json::json;
use soundcraft_ui_egui::{Services, SoundApp};

fn harness() -> Harness<'static, SoundApp> {
    let app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
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

fn toggled(h: &Harness<'_, SoundApp>, name: &str) -> Option<Toggled> {
    h.get_by_role_and_label(Role::Button, name).accesskit_node().toggled()
}

#[test]
fn every_clickable_toolbar_control_has_a_role_and_a_name() {
    let h = harness();
    // The toolbar band runs from the edit-mode buttons down to the transport's second row.
    let top = h.get_by_label("SHUFFLE").rect().min.y;
    let bottom = h.get_by_label("Return to Zero (Home)").rect().max.y;
    let mut seen = 0;
    for node in h.query_all(by().predicate(|n| n.data().supports_action(Action::Click))) {
        let center = node.rect().center().y;
        if center < top || center > bottom {
            continue;
        }
        seen += 1;
        let ak = node.accesskit_node();
        let label = ak.label().unwrap_or_default();
        assert_ne!(ak.role(), Role::Unknown, "a toolbar control at {:?} has no role ({label:?})", node.rect());
        assert!(!label.trim().is_empty(), "a toolbar control at {:?} with role {:?} has no name", node.rect(), ak.role());
    }
    assert!(seen > 20, "the band should hold the toolbar's controls, found {seen}");
}

#[test]
fn transport_buttons_report_play_and_record_as_toggles_and_the_rest_as_push_buttons() {
    let mut h = harness();
    assert_eq!(toggled(&h, "Stop"), None);
    assert_eq!(toggled(&h, "Play"), Some(Toggled::False));
    assert_eq!(toggled(&h, "Record"), Some(Toggled::False));
    for momentary in ["Return to Zero (Home)", "Rewind", "Fast Forward", "Go to End (End)", "Zoom in (Cmd+])", "Zoom out (Cmd+[)"] {
        assert_eq!(toggled(&h, momentary), None, "{momentary} runs an action and has no pressed state");
    }
    h.state_mut().run("transport.play", json!({})).unwrap();
    h.run_steps(2);
    assert_eq!(toggled(&h, "Play"), Some(Toggled::True));
}

#[test]
fn zoom_presets_are_told_apart_by_number() {
    let h = harness();
    for i in 1..=5 {
        let name = format!("Zoom Preset {i}");
        let node = h.get_by_role_and_label(Role::Button, &name);
        assert_eq!(node.accesskit_node().toggled(), None, "recalling preset {i} sets no state");
    }
}

#[test]
fn readouts_that_open_a_menu_say_so_and_option_switches_report_their_state() {
    let mut h = harness();
    for name in ["Main Counter", "Sub Counter", "Grid Value", "Nudge Value"] {
        assert_eq!(h.get_by_role_and_label(Role::Button, name).accesskit_node().has_popup(), Some(HasPopup::Menu), "{name}");
    }
    let before = toggled(&h, "Count Off");
    assert!(before.is_some());
    h.get_by_role_and_label(Role::Button, "Count Off").click();
    h.run_steps(2);
    assert_ne!(toggled(&h, "Count Off"), before, "Count Off's reported state follows the option");
}

#[test]
fn the_transport_window_names_its_buttons_as_the_toolbar_does() {
    let mut h = harness();
    let named = |h: &Harness<'_, SoundApp>, name: &str| h.query_all_by_role_and_label(Role::Button, name).count();
    let before = named(&h, "Rewind");
    h.state_mut().run("window.transport", json!({"value": true})).unwrap();
    h.run_steps(3);
    assert!(named(&h, "Rewind") > before, "the Transport window adds its own Rewind button");
    let ids = h.query_all(by().predicate(|n| n.label().is_some_and(|l| l.starts_with("transport.")))).count();
    assert_eq!(ids, 0, "no control is named by its command id");
}
