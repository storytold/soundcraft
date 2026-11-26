//! The MIDI loop and merge tasks as a user does them in the UI (usability pass on G-02):
//! choose the MIDI Merge Release Mode from the MIDI Merge button's right-click menu (Reference
//! Guide p.814), switch MIDI Merge and Loop Record from the Transport window's MIDI controls
//! (p.813), pick an earlier Loop Record take from the track's playlist selector (p.801, p.826),
//! and take a loop pass back with one undo, named MIDI Recording (p.820). Loop Record shows on
//! the Record buttons (p.771), the MIDI Merge button explains itself and keeps its menu while
//! dimmed, and the Preferences say new playlists are always made when loop recording (p.801).

use egui::accesskit::Toggled;
use egui::{Event, Modifiers, PointerButton, pos2, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use serde_json::json;
use soundcraft_engine::io::{MergeRelease, MidiPass, MidiRecordStyle, MidiTakeEvent, add_midi_recording_with, midi_record_inputs};
use soundcraft_time::Range;
use soundcraft_ui_egui::{Services, SoundApp, menus};

fn harness(app: SoundApp) -> Harness<'static, SoundApp> {
    let mut h = Harness::builder().with_context_setup(soundcraft_ui_egui::framework::install_headless).with_size(vec2(1600.0, 932.0)).build_ui_state(
        |ui, app: &mut SoundApp| {
            app.logic(&ui.ctx().clone());
            app.ui(ui);
        },
        app,
    );
    h.run_steps(4);
    h
}

/// A Transport window showing its MIDI controls (View > Transport > MIDI Controls, p.813).
fn transport() -> Harness<'static, SoundApp> {
    let mut app = SoundApp::new(soundcraft_engine::Engine::default(), None, Services::default());
    app.ui.show_transport = true;
    app.run("view.transport_midi_controls", json!({"value": true})).unwrap();
    harness(app)
}

/// The mode the MIDI Merge button's right-click menu shows as chosen.
fn menu_mode(h: &mut Harness<'static, SoundApp>) -> Vec<(&'static str, Option<Toggled>)> {
    h.get_by_label("MIDI Merge").click_secondary();
    h.run_steps(3);
    assert!(h.query_by_label("MIDI Merge Release Mode").is_some(), "the menu is titled with the Pro Tools name");
    let modes = ["Latch", "Latch Until Next Event"].map(|m| (m, h.get_by_label(m).accesskit_node().toggled()));
    h.key_press(egui::Key::Escape);
    h.run_steps(3);
    modes.to_vec()
}

#[test]
fn the_release_mode_is_chosen_from_the_midi_merge_button_menu_and_shown_there() {
    let mut h = transport();
    assert_eq!(h.state().ui.midi_merge_release, MergeRelease::LatchUntilNextEvent, "the default");
    assert_eq!(menu_mode(&mut h), [("Latch", Some(Toggled::False)), ("Latch Until Next Event", Some(Toggled::True))]);
    let dirty = h.state().engine.is_dirty();
    h.get_by_label("MIDI Merge").click_secondary();
    h.run_steps(3);
    h.get_by_label("Latch").click();
    h.run_steps(3);
    assert_eq!(h.state().ui.midi_merge_release, MergeRelease::Latch);
    assert!(h.query_by_label("MIDI Merge Release Mode").is_none(), "picking a mode closes the menu");
    assert!(!h.state().engine.session().edit.midi_merge, "a right-click does not switch MIDI Merge itself");
    assert_eq!(h.state().engine.is_dirty(), dirty, "the release mode is a preference, not a session edit");
    assert_eq!(menu_mode(&mut h), [("Latch", Some(Toggled::True)), ("Latch Until Next Event", Some(Toggled::False))]);
}

#[test]
fn loop_rec_in_the_transport_dims_midi_merge_and_keeps_its_state() {
    let mut h = transport();
    h.get_by_label("MIDI Merge").click();
    h.run_steps(2);
    h.get_by_label("Loop Rec").click();
    h.run_steps(2);
    let e = h.state().engine.session().edit.clone();
    assert!(e.loop_record && e.midi_merge, "Loop Rec switches Loop Record; MIDI Merge stays on underneath");
    assert!(h.get_by_label("MIDI Merge").accesskit_node().is_disabled(), "dimmed in Loop Record (p.813)");
    h.get_by_label("Loop Rec").click();
    h.run_steps(2);
    assert!(!h.get_by_label("MIDI Merge").accesskit_node().is_disabled());
    assert!(h.state().engine.session().edit.midi_merge, "and is back on when Loop Record is off");
}

/// Hover `label` until its tooltip has shown.
fn hover(h: &mut Harness<'static, SoundApp>, label: &str) {
    h.get_by_label(label).hover();
    h.run_steps(40);
}

#[test]
fn the_midi_merge_button_says_what_merge_and_replace_do_and_where_the_release_mode_is() {
    let mut h = transport();
    hover(&mut h, "MIDI Merge");
    assert!(h.query_by_label_contains("On (Merge): recorded MIDI is merged").is_some(), "p.813");
    assert!(h.query_by_label_contains("Off (Replace): recorded MIDI replaces").is_some());
    assert!(h.query_by_label_contains("Right-click to choose the MIDI Merge Release Mode.").is_some(), "p.814");
}

#[test]
fn the_dimmed_midi_merge_button_keeps_its_release_mode_menu() {
    let mut h = transport();
    h.state_mut().run("options.loop_record", json!({"value": true})).unwrap();
    h.run_steps(2);
    assert!(h.get_by_label("MIDI Merge").accesskit_node().is_disabled());
    hover(&mut h, "MIDI Merge");
    assert!(h.query_by_label_contains("No effect while Loop Record is on").is_some());
    assert!(h.query_by_label_contains("Right-click to choose the MIDI Merge Release Mode.").is_some());
    assert_eq!(menu_mode(&mut h), [("Latch", Some(Toggled::False)), ("Latch Until Next Event", Some(Toggled::True))]);
    h.get_by_label("MIDI Merge").click_secondary();
    h.run_steps(3);
    h.get_by_label("Latch").click();
    h.run_steps(3);
    assert_eq!(h.state().ui.midi_merge_release, MergeRelease::Latch, "chosen while dimmed");
    let e = h.state().engine.session().edit.clone();
    assert!(e.loop_record && !e.midi_merge, "a right-click switches neither mode");
}

#[test]
fn loop_record_shows_on_both_record_buttons() {
    let mut h = transport();
    let mode = |h: &Harness<'static, SoundApp>, label: &str| {
        h.get_by_label(label).accesskit_node().description().is_some_and(|d| d.lines().any(|l| l == "Loop Record mode"))
    };
    // Normal mode shows nothing (p.771).
    assert_eq!((mode(&h, "Record"), mode(&h, "transport.record")), (false, false));
    h.state_mut().run("options.loop_record", json!({"value": true})).unwrap();
    h.run_steps(2);
    // The toolbar's and the Transport window's Record buttons draw the loop symbol and name the mode.
    assert_eq!((mode(&h, "Record"), mode(&h, "transport.record")), (true, true));
    for button in ["Record", "transport.record"] {
        hover(&mut h, button);
        assert_eq!(h.query_all_by_label("Loop Record mode").count(), 1, "{button}'s tooltip names the mode (p.826 step 2)");
        h.event(Event::PointerGone);
        h.run_steps(3);
    }
    h.state_mut().run("options.loop_record", json!({"value": false})).unwrap();
    h.run_steps(2);
    assert!(!mode(&h, "Record"));
}

const BAR: i64 = 96_000; // 120 bpm 4/4 at 48 kHz

fn ev(at: i64, status: u8, data1: u8, data2: u8) -> MidiTakeEvent {
    MidiTakeEvent { at, device: 0, status, data1, data2 }
}

/// An armed instrument track "keys" with a three-round Loop Record pass over bars 2–3, as the
/// UI writes it when the pass stops: a different pitch in each round, nothing in the fourth.
fn after_loop_record() -> SoundApp {
    let mut app = SoundApp::new(soundcraft_engine::Engine::default(), None, Services::default());
    app.run("track.new", json!({"name": "keys", "kind": "instrument"})).unwrap();
    app.run("track.input", json!({"tracks": ["keys"], "input": "none"})).unwrap();
    app.run("mix.record_arm", json!({"tracks": ["keys"], "value": true})).unwrap();
    app.run("options.loop_record", json!({"value": true})).unwrap();
    let range = Range::new(BAR, 3 * BAR);
    let wrap = MidiTakeEvent { at: range.end, device: u8::MAX, status: MidiTakeEvent::WRAP, data1: 0, data2: 0 };
    let mut events = Vec::new();
    for pitch in [60, 64, 67] {
        events.extend([ev(BAR + 10_000, 0x90, pitch, 100), ev(BAR + 40_000, 0x80, pitch, 0), wrap]);
    }
    let style = MidiRecordStyle { loop_record: true, merge: vec![false], release: MergeRelease::default(), same_pass: false, punched_in: None };
    let inputs = midi_record_inputs(&app.engine);
    let pass = MidiPass::Loop { stopped: Some(BAR + BAR / 2), marked: true };
    let take = add_midi_recording_with(&mut app.engine, range, pass, &events, &[], &inputs, &style);
    assert_eq!(take.clips.len(), 3);
    app
}

fn playlists(h: &Harness<'_, SoundApp>) -> (Vec<String>, usize) {
    let t = &h.state().engine.session().tracks[0];
    (t.playlists.iter().map(|p| p.name.clone()).collect(), t.active_playlist)
}

fn click_at(h: &mut Harness<'_, SoundApp>, at: egui::Pos2) {
    h.event(Event::PointerMoved(at));
    h.run_steps(1);
    h.event(Event::PointerButton { pos: at, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE });
    h.event(Event::PointerButton { pos: at, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
    h.run_steps(3);
}

#[test]
fn an_earlier_loop_record_take_is_picked_from_the_playlist_selector_and_the_pass_undoes_in_one_step() {
    let mut h = harness(after_loop_record());
    assert_eq!(playlists(&h), (vec!["keys".into(), "keys.02".into(), "keys.03".into()], 2), "the last take is the track's");
    assert_eq!(menus::live_label(h.state(), Some("edit.undo")).as_deref(), Some("Undo MIDI Recording"));
    // The playlist selector is the small arrow beside the track name.
    let row = h.state().edit_layout.rows[0].1;
    click_at(&mut h, pos2(row[0] + 119.5, row[1] + 8.0));
    // "keys" (the main playlist) is there too, next to the track name of the same text.
    for name in ["New...", "Duplicate...", "Delete Unused...", "keys.02", "keys.03"] {
        assert!(h.query_by_label(name).is_some(), "{name} is in the playlist menu");
    }
    h.get_by_label("keys.02").click();
    h.run_steps(3);
    assert_eq!(playlists(&h).1, 1, "take 2 is the track's playlist again");
    assert_eq!(menus::live_label(h.state(), Some("edit.undo")).as_deref(), Some("Undo Select Playlist"));
    // Undo the selection, then the pass: every take and playlist of it goes in one step.
    h.state_mut().run("edit.undo", json!({})).unwrap();
    assert_eq!(menus::live_label(h.state(), Some("edit.undo")).as_deref(), Some("Undo MIDI Recording"));
    h.state_mut().run("edit.undo", json!({})).unwrap();
    h.run_steps(2);
    assert_eq!(playlists(&h), (vec!["keys".into()], 0));
    assert!(h.state().engine.session().tracks[0].clips().is_empty());
    assert_eq!(menus::live_label(h.state(), Some("edit.redo")).as_deref(), Some("Redo MIDI Recording"));
}
