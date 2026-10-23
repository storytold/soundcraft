//! MIDI Editor lane header menus: the Velocity header adds controller lanes, a controller lane's
//! header changes what it shows or removes it.

use egui::vec2;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use serde_json::json;
use soundcraft_midi::CtrlKind;
use soundcraft_ui_egui::{Services, SoundApp};

/// A MIDI track "Keys" with a clip open in the MIDI Editor, showing `lanes`.
fn harness(lanes: &[&str]) -> Harness<'static, SoundApp> {
    let mut app = SoundApp::new(soundcraft_engine::Engine::default(), None, Services::default());
    app.run("track.new", json!({"name": "Keys", "kind": "midi"})).unwrap();
    let clip = app.run("midi.new_clip", json!({"track": "Keys", "start": 0, "end": 96_000})).unwrap()["clip"].clone();
    app.run("midi.ctrl_add", json!({"clip": clip, "kind": "cc64", "tick": 0, "value": 127})).unwrap();
    app.run("midi.ctrl_add", json!({"clip": clip, "kind": "pitchbend", "tick": 0, "value": 100})).unwrap();
    app.run("midi.lanes", json!({"track": "Keys", "lanes": lanes})).unwrap();
    app.run("edit.select", json!({"clips": [clip]})).unwrap();
    app.ui.show_midi_editor = true;
    let mut h = Harness::builder().with_size(vec2(1400.0, 1000.0)).build_ui_state(
        |ui, app: &mut SoundApp| {
            app.logic(&ui.ctx().clone());
            app.ui(ui);
        },
        app,
    );
    h.run_steps(4);
    h
}

fn lanes(h: &Harness<SoundApp>) -> Vec<CtrlKind> {
    h.state().engine.session().tracks[0].midi_lanes.clone()
}

fn open(h: &mut Harness<SoundApp>, header: &str) {
    h.get_by_label(header).click();
    h.run_steps(3);
}

fn hover(h: &mut Harness<SoundApp>, item: &str) {
    h.get_by_label(item).hover();
    h.run_steps(3);
}

fn pick(h: &mut Harness<SoundApp>, item: &str) {
    h.get_by_label(item).click();
    h.run_steps(3);
}

#[test]
fn the_velocity_header_adds_a_lane() {
    let mut h = harness(&[]);
    open(&mut h, "Velocity lane");
    hover(&mut h, "Add Controller Lane ⏵");
    pick(&mut h, "Mod Wheel (CC 1)");
    assert_eq!(lanes(&h), vec![CtrlKind::Cc(1)]);
    assert!(h.query_by_label("Mod Wheel (CC 1) lane").is_some(), "the new lane has a header");
}

#[test]
fn the_velocity_header_shows_the_lanes_the_clip_uses() {
    let mut h = harness(&["cc1"]);
    open(&mut h, "Velocity lane");
    pick(&mut h, "Show Lanes with Data");
    assert_eq!(lanes(&h), vec![CtrlKind::Cc(1), CtrlKind::Cc(64), CtrlKind::PitchBend]);
}

#[test]
fn a_lane_header_changes_its_controller() {
    let mut h = harness(&["cc1", "cc7"]);
    open(&mut h, "Mod Wheel (CC 1) lane");
    // The clip's sustain events are marked.
    pick(&mut h, "Sustain (CC 64)  •");
    assert_eq!(lanes(&h), vec![CtrlKind::Cc(64), CtrlKind::Cc(7)]);
    // Picking a controller another lane shows moves it here.
    open(&mut h, "Volume (CC 7) lane");
    pick(&mut h, "Sustain (CC 64)  •");
    assert_eq!(lanes(&h), vec![CtrlKind::Cc(64)]);
}

#[test]
fn a_lane_header_reaches_every_cc_number() {
    let mut h = harness(&["cc1"]);
    open(&mut h, "Mod Wheel (CC 1) lane");
    hover(&mut h, "Other Controllers ⏵");
    hover(&mut h, "CC 0–31 ⏵");
    pick(&mut h, "CC 3");
    assert_eq!(lanes(&h), vec![CtrlKind::Cc(3)]);
}

#[test]
fn a_lane_header_removes_its_lane() {
    let mut h = harness(&["cc1", "pitchbend"]);
    open(&mut h, "Pitch Bend lane");
    pick(&mut h, "Remove Lane");
    assert_eq!(lanes(&h), vec![CtrlKind::Cc(1)]);
}
