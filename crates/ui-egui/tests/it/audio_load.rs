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
