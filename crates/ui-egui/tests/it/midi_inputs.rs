//! Setup › Playback Engine lists the MIDI input devices with a switch each: the switches are
//! labelled with the device's name, work from the keyboard, are kept in the app preferences, and
//! the list follows devices that are plugged in or removed. The monitor shows what arrives.
use egui::accesskit::Role;
use egui::{Event, Key, Modifiers, vec2};
use egui_kittest::{
    Harness,
    kittest::{NodeT, Queryable},
};
use soundcraft_playback::midi::MidiInputs;
use soundcraft_ui_egui::{Services, SoundApp, UiState};

fn names(list: &[&str]) -> Vec<String> {
    list.iter().map(|n| (*n).to_string()).collect()
}

/// The app with the Playback Engine window open and two MIDI devices, none of them real.
fn harness(disabled: &[&str]) -> Harness<'static, SoundApp> {
    let mut app = SoundApp::new(soundcraft_engine::Engine::default(), None, Services::default());
    app.midi_inputs = Some(MidiInputs::with_port_names(names(disabled), names(&["Keys", "Drum Pad"])));
    app.ui.midi_inputs_off = names(disabled);
    app.ui.show_playback_engine = true;
    let mut h = Harness::builder().with_size(vec2(1600.0, 1000.0)).build_ui_state(
        |ui, app: &mut SoundApp| {
            app.logic(&ui.ctx().clone());
            app.ui(ui);
        },
        app,
    );
    h.run_steps(3);
    h
}

fn press(h: &mut Harness<'_, SoundApp>, key: Key) {
    h.event(Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE });
    h.run_steps(1);
    h.event(Event::Key { key, physical_key: None, pressed: false, repeat: false, modifiers: Modifiers::NONE });
    h.run_steps(2);
}

fn port(h: &Harness<'_, SoundApp>, name: &str) -> Option<(bool, bool)> {
    h.state().midi_inputs.as_ref().and_then(|m| m.ports().iter().find(|p| p.name == name).map(|p| (p.enabled, p.open)))
}

#[test]
fn every_midi_device_has_a_labelled_switch_that_shows_its_state() {
    let h = harness(&["Drum Pad"]);
    h.get_by_label("MIDI inputs");
    // A checkbox per device, named after it, so a screen reader says which device it switches.
    let keys = h.get_by_role_and_label(Role::CheckBox, "Keys");
    let pad = h.get_by_role_and_label(Role::CheckBox, "Drum Pad");
    assert_eq!(keys.accesskit_node().toggled(), Some(egui::accesskit::Toggled::True));
    assert_eq!(pad.accesskit_node().toggled(), Some(egui::accesskit::Toggled::False));
    // The state is also there as text, not only as a tick.
    h.get_by_label("connected");
    h.get_by_label("off");
    assert_eq!(h.query_all_by_label("Test").count(), 2, "each device has its monitor");
}

#[test]
fn a_switch_works_from_the_keyboard_and_is_kept_in_the_preferences() {
    let mut h = harness(&[]);
    assert_eq!(port(&h, "Keys"), Some((true, true)));
    h.get_by_role_and_label(Role::CheckBox, "Keys").focus();
    h.run_steps(2);
    press(&mut h, Key::Space);
    assert_eq!(port(&h, "Keys"), Some((false, false)), "Space on the focused switch turns the device off");
    assert_eq!(port(&h, "Drum Pad"), Some((true, true)), "the other device is untouched");
    assert_eq!(h.state().ui.midi_inputs_off, ["Keys"]);
    // The session's own list of devices follows the switch.
    assert!(!h.state().engine.session().edit.flags.iter().any(|f| f.starts_with("midi.input.") && f.contains("Keys")));
    // The preferences keep it: written out, read back, and used when the devices are opened again.
    let saved = serde_json::to_string(&h.state().ui).unwrap();
    let restored: UiState = serde_json::from_str(&saved).unwrap();
    assert_eq!(restored.midi_inputs_off, ["Keys"]);
    let reopened = MidiInputs::with_port_names(restored.midi_inputs_off.clone(), names(&["Keys", "Drum Pad"]));
    assert_eq!(reopened.ports().iter().map(|p| (p.name.as_str(), p.enabled)).collect::<Vec<_>>(), [("Keys", false), ("Drum Pad", true)]);
    // Preferences written before there were MIDI switches still load: every device is on.
    let mut old: serde_json::Value = serde_json::from_str(&saved).unwrap();
    old.as_object_mut().unwrap().remove("midi_inputs_off");
    assert!(serde_json::from_value::<UiState>(old).unwrap().midi_inputs_off.is_empty());
    // And on again, from the keyboard.
    h.get_by_role_and_label(Role::CheckBox, "Keys").focus();
    h.run_steps(2);
    press(&mut h, Key::Space);
    assert_eq!(port(&h, "Keys"), Some((true, true)));
    assert!(h.state().ui.midi_inputs_off.is_empty());
}

#[test]
fn the_list_follows_devices_that_are_plugged_in_and_removed() {
    let mut h = harness(&[]);
    assert!(h.query_by_role_and_label(Role::CheckBox, "Launchkey").is_none());
    let rescan = |h: &mut Harness<'_, SoundApp>, list: &[&str]| {
        let m = h.state_mut().midi_inputs.as_mut().unwrap();
        m.set_port_names(names(list));
        let changed = m.rescan();
        h.run_steps(2);
        changed
    };
    assert!(rescan(&mut h, &["Keys", "Drum Pad", "Launchkey"]), "a new device is a change");
    h.get_by_role_and_label(Role::CheckBox, "Launchkey");
    assert!(!rescan(&mut h, &["Keys", "Drum Pad", "Launchkey"]), "nothing changed");
    assert!(rescan(&mut h, &["Drum Pad", "Launchkey"]));
    assert!(h.query_by_role_and_label(Role::CheckBox, "Keys").is_none(), "an unplugged device leaves the list");
    assert!(rescan(&mut h, &[]));
    h.get_by_label("No MIDI input device found. Plug one in: it shows up here by itself.");
}

#[test]
fn the_monitor_shows_what_arrives_from_a_device() {
    let mut h = harness(&[]);
    h.query_all_by_label("Test").next().unwrap().click();
    h.run_steps(2);
    h.get_by_label("Last received: nothing yet");
    let feed = std::sync::Arc::clone(h.state().midi_inputs.as_ref().unwrap().feed());
    feed.receive(feed.ports().assign("Keys"), &[0x92, 60, 100]);
    h.run_steps(2);
    h.get_by_label_contains("note C3 on, velocity 100, channel 3");
}
