//! `ui.audio_device`: the audio device choice is kept only once the devices reopen with it.
use egui::vec2;
use egui_kittest::{Harness, kittest::Queryable};
use serde_json::json;
use soundcraft_ui_egui::{Services, SoundApp};

#[test]
fn the_audio_device_choice_is_kept_only_when_it_is_applied() {
    let mut app = SoundApp::new(soundcraft_engine::Engine::default(), None, Services::default());
    let before = app.ui.audio.clone();

    let r = app.run("ui.audio_device", json!({})).unwrap();
    assert_eq!(r["config"], json!(before), "no change: just the choice and the device");

    assert!(app.run("ui.audio_device", json!({"sample_rate": 12})).is_err());
    assert_eq!(app.ui.audio, before, "an out-of-range rate changes nothing");

    app.engine.transport.recording = true;
    let err = app.run("ui.audio_device", json!({"buffer_size": 256, "output": "Elsewhere"})).unwrap_err();
    assert!(err.contains("stop recording"), "{err}");
    assert_eq!(app.ui.audio, before, "refused while recording: the saved choice stays the open device's");
    app.engine.transport.recording = false;

    // Opens the chosen device, or the default, or a silent clock without one.
    let r = app.run("ui.audio_device", json!({"buffer_size": 256})).unwrap();
    assert_eq!(app.ui.audio.buffer_size, Some(256));
    assert_eq!(r["config"]["buffer_size"], 256);
    assert!(r["device"].is_object(), "{r}");
}

fn new_app() -> SoundApp {
    SoundApp::new(soundcraft_engine::Engine::default(), None, Services::default())
}

#[test]
fn the_reply_says_when_the_device_does_not_run_at_the_chosen_buffer() {
    // Nothing open: there is no device buffer to explain, so the note is null, never a guess (#1757).
    let mut app = SoundApp::new(soundcraft_engine::Engine::default(), None, Services::default());
    for command in ["ui.audio_device", "setup.playback_engine"] {
        let r = app.run(command, json!({})).unwrap();
        assert!(r.get("buffer_note").is_some_and(serde_json::Value::is_null), "{command}: {r}");
    }
}

#[test]
fn playback_engine_refuses_an_invalid_or_untimely_device_choice_without_side_effects() {
    let mut app = new_app();
    let before = app.ui.audio.clone();
    let bit_depth = app.engine.session().bit_depth;
    assert!(!app.ui.show_playback_engine);

    let r = app.run("setup.hardware", json!({})).unwrap();
    assert_eq!(r["applied"], false, "without a player there is no open device");
    assert_eq!(r["bit_depth"], json!(bit_depth));

    assert!(app.engine.session().edit.delay_compensation, "delay compensation starts on");
    let err = app.run("setup.playback_engine", json!({"buffer_size": 8, "delay_compensation": false})).unwrap_err();
    assert!(err.contains("16..=8192"), "{err}");
    assert_eq!(app.ui.audio, before, "an invalid buffer changes nothing");
    assert!(app.engine.session().edit.delay_compensation, "an invalid buffer does not apply delay compensation");

    app.engine.transport.recording = true;
    let err = app.run("setup.playback_engine", json!({"buffer_size": 256, "delay_compensation": false})).unwrap_err();
    assert!(err.contains("stop recording"), "{err}");
    assert_eq!(app.ui.audio, before);
    assert!(app.engine.session().edit.delay_compensation, "a refused device change does not apply delay compensation");
    app.engine.transport.recording = false;

    let err = app.run("setup.playback_engine", json!({"device": 42, "delay_compensation": false})).unwrap_err();
    assert!(err.contains("`device` must be a string or null"), "{err}");
    assert_eq!(app.ui.audio, before, "an invalid device changes nothing");
    assert!(app.engine.session().edit.delay_compensation, "an invalid device does not apply delay compensation");
}

#[test]
fn playback_engine_applies_the_device_choice_and_reports_the_open_device() {
    let mut app = new_app();
    let bit_depth = app.engine.session().bit_depth;
    let r = app.run("setup.playback_engine", json!({"buffer_size": 256, "delay_compensation": false})).unwrap();
    assert_eq!(app.ui.audio.buffer_size, Some(256));
    assert!(!app.engine.session().edit.delay_compensation);
    assert!(!app.ui.show_playback_engine, "a programmatic call does not open the window");
    assert_eq!(r["delay_compensation"], false);
    let player = app.player.as_ref().unwrap();
    assert_eq!(r["applied"], !player.silent);
    assert_eq!(r["device"], json!(player.device_name));
    assert_eq!(r["buffer_size"], json!(player.device_buffer));
    assert_eq!(r["sample_rate"], player.device_rate);
    assert_eq!(r["bit_depth"], json!(bit_depth));
}

#[test]
fn playback_engine_applies_the_same_device_choice_as_the_audio_command() {
    let mut app = new_app();
    app.run("setup.playback_engine", json!({"delay_compensation": false})).unwrap();

    // 300 is in range and is not rounded onto the window's power-of-two list.
    let mut via_window_command = new_app();
    via_window_command.run("ui.audio_device", json!({"buffer_size": 300, "output": "Elsewhere"})).unwrap();
    app.run("setup.hardware", json!({"buffer_size": 300, "device": "Elsewhere"})).unwrap();
    assert_eq!(app.ui.audio.buffer_size, Some(300));
    assert_eq!(app.ui.audio.buffer_size, via_window_command.ui.audio.buffer_size);
    assert_eq!(app.ui.audio.output, via_window_command.ui.audio.output);
    assert!(!app.ui.show_playback_engine);

    app.run("setup.playback_engine", json!({"buffer_size": null, "device": null})).unwrap();
    assert_eq!(app.ui.audio.buffer_size, None);
    assert_eq!(app.ui.audio.output, None);
    assert!(!app.engine.session().edit.delay_compensation, "clearing the device leaves delay compensation");

    let cfg = app.ui.audio.clone();
    soundcraft_ui_egui::menus::invoke_menu(&mut app, "setup.playback_engine", "Setup > Playback Engine...");
    assert!(app.ui.show_playback_engine, "the menu still opens the Playback Engine window");
    assert_eq!(app.ui.audio, cfg, "opening the window does not change the device");
}

/// The Playback Engine window, rendered, with the app's audio choice set to `buffer` samples.
fn playback_engine_harness(buffer: u32) -> Harness<'static, SoundApp> {
    let mut app = SoundApp::new(soundcraft_engine::Engine::default(), None, Services::default());
    app.ui.audio.buffer_size = Some(buffer);
    app.ui.show_playback_engine = true;
    let mut h =
        Harness::builder().with_context_setup(soundcraft_ui_egui::framework::install_headless).with_size(vec2(1600.0, 1000.0)).build_ui_state(
            |ui, app: &mut SoundApp| {
                app.logic(&ui.ctx().clone());
                app.ui(ui);
            },
            app,
        );
    h.run_steps(3);
    h
}

/// Whether the Buffer size selector shows `text`.
fn shows(h: &Harness<'_, SoundApp>, text: &str) -> bool {
    h.query_all_by_label("Buffer size").any(|n| n.value().is_some_and(|v| v == text))
}

/// A command (or another window) changes the audio choice while the Playback Engine is closed or
/// open: the window then shows the current choice, and Apply has nothing to apply, so it never
/// puts an earlier choice back.
#[test]
fn the_playback_engine_shows_the_current_choice_after_a_command_changes_it() {
    let mut h = playback_engine_harness(256);
    assert!(shows(&h, "256 samples"), "the window opens on the current choice");

    // Closed from the menu (not its close button), then the choice changes by command.
    h.state_mut().ui.show_playback_engine = false;
    h.run_steps(2);
    h.state_mut().ui.audio.buffer_size = Some(512);
    h.state_mut().ui.show_playback_engine = true;
    h.run_steps(3);
    assert!(shows(&h, "512 samples"), "reopened on the choice the command made");
    assert!(!shows(&h, "256 samples"), "not on the earlier one");

    // Changed while the window is open.
    h.state_mut().ui.audio.buffer_size = Some(1024);
    h.run_steps(3);
    assert!(shows(&h, "1024 samples"), "an open window follows a change made elsewhere");
    assert!(h.query_by_label("Revert").is_none(), "nothing to revert: no edit differs from the current choice");
}

/// Make an unapplied edit in the window: its buffer size set to `buffer`, as if chosen in its
/// Buffer size selector (the window keeps its edits with the choice they started from).
fn edit_buffer(h: &mut Harness<'_, SoundApp>, buffer: u32) {
    let from = h.state().ui.audio.clone();
    let draft = soundcraft_playback::AudioConfig { buffer_size: Some(buffer), ..from.clone() };
    h.ctx.data_mut(|d| d.insert_temp(egui::Id::unique("audio_draft"), (from, draft)));
    h.run_steps(2);
}

/// Pro Tools' Playback Engine is a modal dialog, so nothing else changes the device while it has
/// unapplied edits (Reference Guide p. 109). This window is not modal: when a command changes the
/// choice under unapplied edits, the window takes the new choice and says the edits were replaced.
#[test]
fn the_playback_engine_says_when_a_command_replaces_its_unapplied_edits() {
    const NOTE: &str = "The audio settings were changed outside this window; your unapplied changes were replaced.";
    let mut h = playback_engine_harness(256);
    edit_buffer(&mut h, 128);
    h.run_steps(2);
    assert!(shows(&h, "128 samples"), "the edit is in the window");
    assert!(h.query_by_label("Revert").is_some(), "an unapplied edit");

    // A command changes the buffer to 1024 while the edit is unapplied.
    h.state_mut().ui.audio.buffer_size = Some(1024);
    h.run_steps(3);
    assert!(shows(&h, "1024 samples"), "the window takes the current choice");
    assert!(h.query_by_label(NOTE).is_some(), "and says the unapplied edit was replaced");
    assert!(h.state().ui.status.contains(NOTE), "the status line says it too: {}", h.state().ui.status);

    // The next edit takes the note away.
    edit_buffer(&mut h, 512);
    h.run_steps(2);
    assert!(h.query_by_label(NOTE).is_none(), "a new edit: the note goes");
    assert!(!h.state().ui.status.contains(NOTE), "and the status line too: {}", h.state().ui.status);

    // Closing also clears the status line.
    let mut h = playback_engine_harness(256);
    edit_buffer(&mut h, 128);
    h.state_mut().ui.audio.buffer_size = Some(1024);
    h.run_steps(3);
    assert!(h.state().ui.status.contains(NOTE), "the status line says it: {}", h.state().ui.status);
    h.state_mut().ui.show_playback_engine = false;
    h.run_steps(2);
    assert!(!h.state().ui.status.contains(NOTE), "closing clears the status line: {}", h.state().ui.status);

    // A change made elsewhere with no unapplied edit replaces nothing, so nothing is said.
    let mut h = playback_engine_harness(256);
    h.state_mut().ui.audio.buffer_size = Some(1024);
    h.run_steps(3);
    assert!(h.query_by_label(NOTE).is_none(), "no edit was replaced");
}
