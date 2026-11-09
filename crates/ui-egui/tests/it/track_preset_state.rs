//! Track presets capture the hosted plugin's current state before writing the model.

#[path = "../../../clap-host/tests/common/mod.rs"]
mod common;

use serde_json::{Value, json};
use soundcraft_engine::Engine;
use soundcraft_model::{ChannelFormat, Insert, TrackKind, b64};
use soundcraft_playback::Player;
use soundcraft_ui_egui::{Services, SoundApp};

#[test]
fn track_preset_captures_live_plugin_state_before_serializing() {
    common::fixture_dir();
    let mut engine = Engine::default();
    let track = engine.session_mut().add_track(TrackKind::Audio, ChannelFormat::Mono, Some("Vox"));
    engine.session_mut().track_mut(track).unwrap().mixer.inserts[0] = Some(Insert::new("clap:org.soundcraft.test.gain"));
    let player = Player::new(engine.session_arc());
    let states = player.capture_states();
    assert_eq!(states.len(), 1, "the synthetic CLAP plugin must load even without an audio device");
    assert_eq!((states[0].0, states[0].1), (track, 0));
    let live = states[0].2.clone();

    // The model still holds the earlier capture while the live plugin has its current settings.
    let stale: Vec<u8> = [0.25_f64, 0.0].into_iter().flat_map(f64::to_le_bytes).collect();
    assert_ne!(live, stale);
    engine.session_mut().track_mut(track).unwrap().mixer.inserts[0].as_mut().unwrap().set_state_bytes(&stale);
    let mut app = SoundApp::new(engine, Some(player), Services::default());
    assert_eq!(app.engine.session().track(track).unwrap().mixer.inserts[0].as_ref().unwrap().state_bytes(), Some(stale));

    let path = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("track-preset-state-{}.json", std::process::id()));
    app.run("track.save_preset", json!({"track": track.0, "path": path.to_string_lossy()})).unwrap();
    let preset: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    std::fs::remove_file(path).unwrap();
    assert_eq!(preset["mixer"]["inserts"][0]["state"], b64::encode(&live), "the preset must contain the current hosted state");
}
