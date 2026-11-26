//! The Transport window's MIDI Merge button is dimmed while Loop Record is on, where it has no
//! effect (Pro Tools Reference Guide p.813).

use egui::vec2;
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use serde_json::json;
use soundcraft_ui_egui::{Services, SoundApp};

#[test]
fn midi_merge_is_dimmed_while_loop_record_is_on() {
    let mut app = SoundApp::new(soundcraft_engine::Engine::default(), None, Services::default());
    app.ui.show_transport = true;
    app.run("view.transport_midi_controls", json!({"value": true})).unwrap();
    let mut h =
        Harness::builder().with_context_setup(soundcraft_ui_egui::framework::install_headless).with_size(vec2(1600.0, 1000.0)).build_ui_state(
            |ui, app: &mut SoundApp| {
                app.logic(&ui.ctx().clone());
                app.ui(ui);
            },
            app,
        );
    h.run_steps(4);
    assert!(!h.get_by_label("MIDI Merge").accesskit_node().is_disabled());
    h.get_by_label("MIDI Merge").click();
    h.run_steps(2);
    assert!(h.state().engine.session().edit.midi_merge, "the button runs options.midi_merge");
    h.state_mut().run("options.loop_record", json!({"value": true})).unwrap();
    h.run_steps(2);
    assert!(h.get_by_label("MIDI Merge").accesskit_node().is_disabled());
}
