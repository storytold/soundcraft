//! `ui.audio_load`: the audio callback's load, for agents.
use serde_json::json;
use soundcraft_ui_egui::{Services, SoundApp};

#[test]
fn the_audio_load_is_unmeasured_without_an_audio_engine() {
    let mut app = SoundApp::new(soundcraft_engine::Engine::default(), None, Services::default());
    for p in [json!({}), json!({"reset": true})] {
        let r = app.run("ui.audio_load", p).unwrap();
        assert_eq!(r["measured"], false, "{r}");
        for key in ["average", "peak", "worst", "late", "callbacks", "buffer_ms"] {
            assert!(r[key].is_number(), "{key}: {r}");
        }
    }
    assert!(soundcraft_ui_egui::menus::UI_COMMANDS.iter().any(|c| c.0 == "ui.audio_load"), "listed for agents");
}

/// `reset: true` returns the current values then clears them. A `Player` in these tests uses the
/// silent clock (no device callbacks), so the meter never leaves the unmeasured default; the
/// pre-reset-then-clear contract for measured values is tested on `LoadMeter` in
/// `soundcraft-playback`.
#[test]
fn reset_without_an_audio_engine_returns_the_unmeasured_default_then_stays_cleared() {
    let mut app = SoundApp::new(soundcraft_engine::Engine::default(), None, Services::default());
    let first = app.run("ui.audio_load", json!({"reset": true})).unwrap();
    assert_eq!(first["measured"], false, "{first}");
    let second = app.run("ui.audio_load", json!({})).unwrap();
    assert_eq!(second["measured"], false, "{second}");
    for key in ["average", "peak", "worst", "late", "callbacks", "buffer_ms"] {
        assert_eq!(second[key].as_f64(), Some(0.0), "{key}: {second}");
    }
}
