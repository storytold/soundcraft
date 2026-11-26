//! Dialogs are modal dialogs for the keyboard and screen readers: they take the focus when they open, keep Tab inside,
//! and give the focus back when they close.

use egui::accesskit::Role;
use egui::{Key, vec2};
use egui_kittest::{
    Harness,
    kittest::{NodeT, Queryable, by},
};
use serde_json::json;
use soundcraft_ui_egui::dialogs::Dialog;
use soundcraft_ui_egui::{Services, SoundApp};

fn harness() -> Harness<'static, SoundApp> {
    let mut app = SoundApp::new(soundcraft_engine::Engine::default(), None, Services::default());
    app.run("track.new", json!({"name": "Vocal"})).unwrap();
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

fn focused_label(h: &Harness<'_, SoundApp>) -> Option<String> {
    h.query_all(by().predicate(|n| n.is_focused())).next().and_then(|n| n.accesskit_node().label())
}

fn ask(h: &mut Harness<'_, SoundApp>, then: &str) {
    h.state_mut().dialogs.open = Some(Dialog::SaveChanges { then: (then.into(), json!({})) });
    h.run_steps(4);
}

#[test]
fn a_question_takes_the_focus_keeps_tab_inside_and_gives_the_focus_back() {
    let mut h = harness();
    h.get_by_label("SHUFFLE").focus();
    h.run_steps(2);
    assert_eq!(focused_label(&h).as_deref(), Some("SHUFFLE"));

    ask(&mut h, "session.close");
    let dialog = h.get_by_role_and_label(Role::AlertDialog, "Unsaved Changes");
    assert!(dialog.accesskit_node().is_modal());
    let area = dialog.rect();
    assert_eq!(focused_label(&h).as_deref(), Some("Save"), "the default button has the focus");
    for _ in 0..5 {
        h.key_press(Key::Tab);
        h.run_steps(2);
        let focused = h.query_all(by().predicate(|n| n.is_focused())).next().expect("something has the focus");
        assert!(area.contains(focused.rect().center()), "Tab stays in the dialog, but reached {:?}", focused.accesskit_node().label());
    }

    h.key_press(Key::Escape);
    h.run_steps(3);
    assert!(h.query_by_role_and_label(Role::AlertDialog, "Unsaved Changes").is_none());
    assert_eq!(focused_label(&h).as_deref(), Some("SHUFFLE"), "the focus goes back where it was");
}

#[test]
fn a_question_that_throws_work_away_starts_on_cancel() {
    let mut h = harness();
    ask(&mut h, "session.revert");
    h.get_by_role_and_label(Role::AlertDialog, "Revert to Saved");
    assert_eq!(focused_label(&h).as_deref(), Some("Cancel"));
}
