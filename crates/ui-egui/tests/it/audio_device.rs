//! `ui.audio_device`: the audio device choice is kept only once the devices reopen with it.
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
