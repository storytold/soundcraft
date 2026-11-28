use egui::accesskit::Role;
use egui::{Key, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use serde_json::json;
use soundcraft_engine::cmd::{ScaleChoice, midi_editor_scale};
use soundcraft_midi::{KeyScale, Scale};
use soundcraft_ui_egui::{Services, SoundApp};

/// The demo in D minor with the Keys track selected, so its MIDI clip is in the MIDI Editor.
fn harness() -> Harness<'static, SoundApp> {
    let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
    app.ui.show_midi_editor = true;
    app.run("event.add_key_change", json!({"key": "D minor", "at": 0})).unwrap();
    app.run("edit.select", json!({"tracks": ["Keys"]})).unwrap();
    let mut h =
        Harness::builder().with_context_setup(soundcraft_ui_egui::framework::install_headless).with_size(vec2(1600.0, 1000.0)).build_ui_state(
            |ui, app: &mut SoundApp| {
                let ctx = ui.ctx().clone();
                app.logic(&ctx);
                app.ui(ui);
            },
            app,
        );
    for _ in 0..3 {
        h.step();
    }
    h
}

fn value(h: &Harness<'_, SoundApp>, name: &str) -> String {
    h.get_by_role_and_label(Role::ComboBox, name).value().unwrap_or_default()
}

fn choice(h: &Harness<'_, SoundApp>) -> ScaleChoice {
    midi_editor_scale(h.state().engine.session())
}

#[test]
fn the_picker_follows_the_session_key_until_a_scale_is_chosen() {
    let mut h = harness();
    let following = value(&h, "Scale");
    assert!(following.contains("Session key") && following.contains("D Natural Minor"), "{following}");
    assert_eq!(value(&h, "Scale root"), "D");

    // Choosing a scale while following keeps the followed root.
    h.get_by_role_and_label(Role::ComboBox, "Scale").click();
    h.run();
    h.get_by_label("Dorian").click();
    h.run();
    assert_eq!(choice(&h), ScaleChoice::Fixed(KeyScale::new(2, Scale::Dorian)));
    assert_ne!(value(&h, "Scale"), following, "the picker no longer says it follows the session key");

    // An automation call shows in the picker too.
    h.state_mut().run("midi.scale", json!({"scale": "none"})).unwrap();
    h.run();
    assert_eq!(value(&h, "Scale"), "None");
    assert_eq!(value(&h, "Scale root"), "–");
}

fn focused(h: &Harness<'_, SoundApp>) -> Vec<String> {
    h.query_all(egui_kittest::kittest::by()).filter(|n| n.accesskit_node().is_focused()).filter_map(|n| n.accesskit_node().label()).collect()
}

fn press(h: &mut Harness<'_, SoundApp>, key: Key) {
    h.key_press(key);
    h.run();
}

#[test]
fn the_scale_picker_works_from_the_keyboard() {
    let mut h = harness();
    h.get_by_role_and_label(Role::ComboBox, "Scale root").focus();
    h.run();
    // Enter opens the roots; arrows move focus down C, C#, D … to G; Enter chooses it.
    press(&mut h, Key::Enter);
    for _ in 0..8 {
        press(&mut h, Key::ArrowDown);
    }
    assert_eq!(focused(&h), ["G"]);
    press(&mut h, Key::Enter);
    assert_eq!(choice(&h), ScaleChoice::Fixed(KeyScale::new(7, Scale::NaturalMinor)), "a root keeps the followed scale");
    assert_eq!(h.query_all_by_label("G").count(), 0, "choosing closes the menu");
    assert_eq!(focused(&h), ["Scale root"], "focus returns to the picker");
    // Escape closes the menu without a choice and also returns focus.
    press(&mut h, Key::Enter);
    press(&mut h, Key::ArrowDown);
    press(&mut h, Key::Escape);
    assert_eq!(focused(&h), ["Scale root"]);
    assert_eq!(choice(&h), ScaleChoice::Fixed(KeyScale::new(7, Scale::NaturalMinor)));
}
