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

/// The app with a connected MIDI device, an instrument track and an audio track, none armed yet.
fn recording_harness() -> (Harness<'static, SoundApp>, u64, u64) {
    let mut h = harness(&[]);
    let new =
        |h: &mut Harness<'_, SoundApp>, params: serde_json::Value| h.state_mut().run("track.new", params).unwrap()["tracks"][0].as_u64().unwrap();
    let synth = new(&mut h, serde_json::json!({"name": "Synth", "kind": "instrument"}));
    let vox = new(&mut h, serde_json::json!({"name": "Vox", "format": "mono"}));
    h.state_mut().ui.show_playback_engine = false;
    h.state_mut().ui.show_transport = true;
    h.run_steps(3);
    (h, synth, vox)
}

fn arm(h: &mut Harness<'_, SoundApp>, track: u64, on: bool) {
    h.state_mut().run("mix.record_arm", serde_json::json!({"tracks": [track], "value": on})).unwrap();
    h.run_steps(2);
}

#[test]
fn a_pass_with_only_midi_tracks_starts_without_an_audio_input() {
    let (mut h, synth, _) = recording_harness();
    arm(&mut h, synth, true);
    assert!(h.state().recorder.is_none(), "arming an instrument track does not open the audio input");
    h.get_by_label("transport.record").click();
    h.run_steps(3);
    assert!(h.state().engine.transport.recording, "{}", h.state().ui.status);
    assert!(h.state().recorder.is_none());
}

#[test]
fn a_mixed_pass_whose_audio_cannot_be_recorded_is_refused_with_the_reason() {
    let (mut h, synth, vox) = recording_harness();
    arm(&mut h, synth, true);
    arm(&mut h, vox, true);
    // This app has no audio engine, so the audio track's take cannot be made. MIDI is ready,
    // but recording only the instrument track would lose the audio without a word.
    h.get_by_label("transport.record").click();
    h.run_steps(3);
    assert!(!h.state().engine.transport.recording);
    let status = h.state().ui.status.clone();
    assert!(status.starts_with("Cannot record: ") && status.contains("no audio input device"), "{status}");
    // Without the audio track in record, the same pass starts.
    arm(&mut h, vox, false);
    h.get_by_label("transport.record").click();
    h.run_steps(3);
    assert!(h.state().engine.transport.recording, "{}", h.state().ui.status);
}

#[test]
fn an_inactive_record_enabled_audio_track_does_not_stop_a_midi_pass() {
    let (mut h, synth, vox) = recording_harness();
    arm(&mut h, synth, true);
    arm(&mut h, vox, true);
    // The audio track keeps its record button when it is made inactive, but records nothing:
    // the pass is a MIDI one and needs no audio input.
    h.state_mut().run("track.make_inactive", serde_json::json!({"tracks": [vox], "inactive": true})).unwrap();
    h.run_steps(2);
    let s = h.state().engine.session();
    assert!(s.tracks.iter().any(|t| t.id.0 == vox && t.inactive && t.mixer.record_arm), "inactive and still record-enabled");
    h.get_by_label("transport.record").click();
    h.run_steps(3);
    assert!(h.state().engine.transport.recording, "{}", h.state().ui.status);
    assert!(h.state().recorder.is_none());
}

#[test]
fn a_midi_pass_does_not_start_while_the_previous_one_is_unconfirmed() {
    let (mut h, synth, vox) = recording_harness();
    arm(&mut h, synth, true);
    // The audio engine never confirmed the end of the last pass (it stopped calling back). It
    // would write the notes of a new pass down with the old pass's state.
    let feed = std::sync::Arc::clone(h.state().midi_inputs.as_ref().unwrap().feed());
    let pass = feed.end_pass();
    h.get_by_label("transport.record").click();
    h.run_steps(3);
    assert!(!h.state().engine.transport.recording);
    assert!(h.state().ui.status.contains("has not finished the previous record pass"), "{}", h.state().ui.status);
    assert!(feed.pass_owed() && !feed.captured(pass), "still waiting");
    // A pass without MIDI tracks does not wait for it (here it is refused for its own reason).
    arm(&mut h, synth, false);
    arm(&mut h, vox, true);
    h.get_by_label("transport.record").click();
    h.run_steps(3);
    // It got past the MIDI guard to the audio setup, which refused it (this app has no audio
    // engine), and the earlier pass is still owed.
    let status = h.state().ui.status.clone();
    assert!(!h.state().engine.transport.recording);
    assert!(status.contains("no audio input device") && !status.contains("previous record pass"), "{status}");
    assert!(feed.pass_owed() && !feed.captured(pass), "the attempt did not clear the pending pass");
    // Once nothing is owed, the MIDI pass starts.
    arm(&mut h, vox, false);
    arm(&mut h, synth, true);
    feed.forgive_passes();
    h.get_by_label("transport.record").click();
    h.run_steps(3);
    assert!(h.state().engine.transport.recording, "{}", h.state().ui.status);
}

#[test]
fn a_midi_pass_without_a_connected_device_says_so() {
    let (mut h, synth, _) = recording_harness();
    arm(&mut h, synth, true);
    h.state_mut().midi_inputs.as_mut().unwrap().set_port_names(Vec::new());
    h.state_mut().midi_inputs.as_mut().unwrap().rescan();
    h.get_by_label("transport.record").click();
    h.run_steps(3);
    assert!(!h.state().engine.transport.recording);
    assert!(h.state().ui.status.contains("no MIDI input device is connected"), "{}", h.state().ui.status);
}
