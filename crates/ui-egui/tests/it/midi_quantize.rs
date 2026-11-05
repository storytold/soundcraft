//! The MIDI Editor's Quantize button snaps to the grid the editor shows.
use egui::vec2;
use egui_kittest::{Harness, kittest::Queryable};
use serde_json::json;
use soundcraft_ui_egui::{Services, SoundApp};

fn quantize_on(grid: &str) -> i64 {
    let mut app = SoundApp::new(soundcraft_engine::Engine::default(), None, Services::default());
    app.run("track.new", json!({"name": "Grid Check", "kind": "midi"})).unwrap();
    let clip = app.run("midi.new_clip", json!({"track": "Grid Check", "start": 0, "end": 96_000})).unwrap()["clip"].clone();
    app.run("midi.note_add", json!({"clip": clip, "pitch": 60, "start_ticks": 300, "length_ticks": 240})).unwrap();
    app.run("edit.select", json!({"clips": [clip]})).unwrap();
    app.run("view.grid", json!({"value": grid})).unwrap();
    app.ui.show_midi_editor = true;
    let mut h = Harness::builder().with_size(vec2(1600.0, 1000.0)).build_ui_state(
        |ui, app: &mut SoundApp| {
            app.logic(&ui.ctx().clone());
            app.ui(ui);
        },
        app,
    );
    h.run_steps(5);
    h.get_by_label("Quantize").click();
    h.run_steps(3);
    let notes = h.state_mut().run("midi.notes", json!({"clip": clip})).unwrap();
    assert_eq!(notes["notes"][0]["length"], 240, "{notes}");
    notes["notes"][0]["start"].as_i64().unwrap()
}

#[test]
fn quantize_follows_a_quarter_note_grid() {
    assert_eq!(quantize_on("1/4"), 0);
}

#[test]
fn quantize_follows_a_sixteenth_note_grid() {
    assert_eq!(quantize_on("1/16"), 240);
}
