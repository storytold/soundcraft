use egui::accesskit::{Role, Toggled};
use egui::vec2;
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use serde_json::json;
use soundcraft_ui_egui::{Services, SoundApp};

/// The demo with the Keys track selected, so its MIDI clip is in the MIDI Editor.
fn harness() -> Harness<'static, SoundApp> {
    let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
    app.ui.show_midi_editor = true;
    app.ui.show_tracks_list = true;
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

fn lead_shown(h: &Harness<'_, SoundApp>) -> bool {
    let s = h.state().engine.session();
    let lead = s.track_by_name("Lead").unwrap().id;
    soundcraft_engine::cmd::midi_reference_tracks(s).contains(&lead)
}

#[test]
fn the_track_list_toggle_shows_a_track_in_the_midi_editor() {
    let mut h = harness();
    let toggle = h.get_by_label("Show Notes in MIDI Editor: Lead");
    assert_eq!(toggle.accesskit_node().role(), Role::CheckBox);
    assert_eq!(toggle.accesskit_node().toggled(), Some(Toggled::False));
    toggle.click();
    for _ in 0..2 {
        h.step();
    }
    assert!(lead_shown(&h), "the toggle did not reach the engine");
    assert_eq!(h.get_by_label("Show Notes in MIDI Editor: Lead").accesskit_node().toggled(), Some(Toggled::True));
    // The editor says which tracks it shows behind the edited clip.
    h.get_by_label("MIDI Editor: Keys, also showing: Lead");
    // Clicking the toggle selects nothing: the edited clip stays Keys'.
    let s = h.state().engine.session();
    assert!(!s.edit.selected_tracks.contains(&s.track_by_name("Lead").unwrap().id));
}

#[test]
fn audio_tracks_have_no_toggle() {
    let h = harness();
    assert!(h.query_by_label("Show Notes in MIDI Editor: Bass").is_none());
}
