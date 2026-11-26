//! The menu bar's keyboard, as on Windows and Linux: a lone Alt reaches the first menu title, Left and Right move along
//! the titles, Escape gives the focus back, and Tab never stops on a title.

use egui::accesskit::Action;
use egui::{Event, Key, Modifiers, vec2};
use egui_kittest::{
    Harness,
    kittest::{NodeT, Queryable, by},
};
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
    // Windows and Linux: a Mac keeps its titles in Tab and takes no lone Option.
    h.ctx.set_os(egui::os::OperatingSystem::Nix);
    h.run_steps(3);
    h
}

fn focused_label(h: &Harness<'_, SoundApp>) -> Option<String> {
    h.query_all(by().predicate(|n| n.is_focused())).next().and_then(|n| n.accesskit_node().label())
}

fn lone_alt(h: &mut Harness<'_, SoundApp>) {
    h.event(Event::ModifiersChanged(Modifiers::ALT));
    h.run_steps(1);
    h.event(Event::ModifiersChanged(Modifiers::NONE));
    h.run_steps(3);
}

fn press(h: &mut Harness<'_, SoundApp>, key: Key) {
    h.key_press(key);
    h.run_steps(3);
}

#[test]
fn a_lone_alt_reaches_the_first_menu_title_the_arrows_move_along_and_escape_goes_back() {
    let mut h = harness();
    h.get_by_label("SHUFFLE").focus();
    h.run_steps(2);
    lone_alt(&mut h);
    assert_eq!(focused_label(&h).as_deref(), Some("SoundCraft"), "Alt focuses the first menu title");
    press(&mut h, Key::ArrowRight);
    assert_eq!(focused_label(&h).as_deref(), Some("File"));
    press(&mut h, Key::ArrowLeft);
    press(&mut h, Key::ArrowLeft);
    assert_eq!(focused_label(&h).as_deref(), Some("Window"), "Left from the first title goes round to the last");
    press(&mut h, Key::Escape);
    assert_eq!(focused_label(&h).as_deref(), Some("SHUFFLE"), "Escape gives the focus back where it was");
    lone_alt(&mut h);
    lone_alt(&mut h);
    assert_eq!(focused_label(&h).as_deref(), Some("SHUFFLE"), "a second Alt leaves the bar again");
}

#[test]
fn tab_never_stops_on_a_menu_title() {
    let mut h = harness();
    // The titles by place, whatever their role (a button, or a menu item with storytold/egui#63).
    let titles: Vec<String> = h
        .query_all(by().predicate(|n| n.data().supports_action(Action::Click)))
        .filter(|n| n.rect().min.y < 24.0)
        .filter_map(|n| n.accesskit_node().label())
        .collect();
    assert!(titles.contains(&"File".to_owned()), "the bar's titles are found: {titles:?}");
    for _ in 0..12 {
        h.key_press(Key::Tab);
        h.run_steps(2);
        let at = focused_label(&h).unwrap_or_default();
        assert!(!titles.contains(&at), "Tab stopped on the menu title {at:?}");
    }
}

#[test]
fn a_menu_opened_from_the_bar_closes_to_its_title_and_a_choice_gives_the_keyboard_back() {
    let mut h = harness();
    h.get_by_label("SHUFFLE").focus();
    h.run_steps(2);
    lone_alt(&mut h);
    press(&mut h, Key::ArrowRight);
    press(&mut h, Key::Enter);
    assert!(egui::Popup::is_any_open(&h.ctx), "Enter on a title opens its menu");
    press(&mut h, Key::ArrowDown);
    assert_ne!(focused_label(&h).as_deref(), Some("File"), "Down moves into the open menu");
    press(&mut h, Key::Escape);
    assert!(!egui::Popup::is_any_open(&h.ctx));
    assert_eq!(focused_label(&h).as_deref(), Some("File"), "Escape closes the menu back to its title");
    press(&mut h, Key::Escape);
    assert_eq!(focused_label(&h).as_deref(), Some("SHUFFLE"), "a second Escape gives the focus back");

    let looped = h.state().engine.session().edit.loop_playback;
    lone_alt(&mut h);
    for _ in 0..3 {
        press(&mut h, Key::ArrowLeft);
    }
    assert_eq!(focused_label(&h).as_deref(), Some("Options"));
    press(&mut h, Key::Enter);
    // The menu entry, drawn after the toolbar button of the same name.
    h.query_all_by_label_contains("Loop Playback").last().expect("the Options menu entry").click();
    h.run_steps(4);
    assert_ne!(h.state().engine.session().edit.loop_playback, looped, "the choice ran");
    assert_eq!(focused_label(&h).as_deref(), Some("SHUFFLE"), "a choice gives the keyboard back where it was");
}

#[test]
fn on_a_mac_the_titles_stay_tab_stops_and_option_takes_nothing() {
    let mut h = harness();
    h.ctx.set_os(egui::os::OperatingSystem::Mac);
    h.run_steps(2);
    h.get_by_label("SHUFFLE").focus();
    h.run_steps(2);
    lone_alt(&mut h);
    assert_eq!(focused_label(&h).as_deref(), Some("SHUFFLE"), "a lone Option leaves the focus");
    let reached_a_title = (0..40).any(|_| {
        h.key_press_modifiers(Modifiers::SHIFT, Key::Tab);
        h.run_steps(2);
        focused_label(&h).as_deref() == Some("File")
    });
    assert!(reached_a_title, "Shift+Tab reaches the menu titles on a Mac");
}
