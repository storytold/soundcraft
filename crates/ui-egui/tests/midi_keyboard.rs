use egui::{Event, Key, Modifiers, vec2};
use egui_kittest::Harness;
use serde_json::json;
use soundcraft_model::{ClipContent, TrackId};
use soundcraft_ui_egui::{Services, SoundApp};

/// The demo with the Keys track selected and Window › MIDI Keyboard open.
fn harness() -> (Harness<'static, SoundApp>, TrackId) {
    let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
    app.run("edit.select", json!({"tracks": ["Keys"]})).unwrap();
    let keys = app.engine.session().track_by_name("Keys").unwrap().id;
    app.ui.show_midi_keyboard = true;
    let mut h = Harness::builder().with_size(vec2(1600.0, 1000.0)).build_ui_state(
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
    (h, keys)
}

fn key(h: &mut Harness<'_, SoundApp>, key: Key, pressed: bool) {
    key_with(h, key, pressed, Modifiers::NONE);
}

fn key_with(h: &mut Harness<'_, SoundApp>, key: Key, pressed: bool, modifiers: Modifiers) {
    h.input_mut().events.push(Event::Key { key, physical_key: Some(key), pressed, repeat: false, modifiers });
    h.step();
}

fn sounding(h: &Harness<'_, SoundApp>) -> Vec<(TrackId, u8)> {
    h.state().keyboard.sounding.keys().copied().collect()
}

fn notes(h: &Harness<'_, SoundApp>, track: TrackId) -> Vec<(u8, i64)> {
    let s = h.state().engine.session();
    let t = s.tracks.iter().find(|t| t.id == track).unwrap();
    let mut v: Vec<(u8, i64)> = t
        .clips()
        .iter()
        .filter_map(|c| match &c.content {
            ClipContent::Midi { sequence, .. } => Some(sequence.notes.iter().map(|n| (n.pitch, n.start)).collect::<Vec<_>>()),
            _ => None,
        })
        .flatten()
        .collect();
    v.sort();
    v
}

#[test]
fn the_computer_rows_play_two_octaves_while_the_keyboard_is_open() {
    let (mut h, keys) = harness();
    assert!(h.state().keyboard.active, "opening the keyboard gives it the keys");
    key(&mut h, Key::Z, true);
    key(&mut h, Key::Num2, true);
    assert_eq!(sounding(&h), vec![(keys, 60), (keys, 73)]);
    key(&mut h, Key::Z, false);
    assert_eq!(sounding(&h), vec![(keys, 73)]);
    key(&mut h, Key::Num2, false);
    assert!(sounding(&h).is_empty());
}

#[test]
fn octave_velocity_and_transpose_move_the_notes() {
    let (mut h, keys) = harness();
    key(&mut h, Key::Quote, true);
    key(&mut h, Key::Quote, false);
    key(&mut h, Key::CloseBracket, true);
    assert_eq!(h.state().ui.midi_keyboard.octave, 4);
    assert_eq!(h.state().ui.midi_keyboard.velocity, 105);
    h.state_mut().ui.midi_keyboard.transpose = 2;
    key(&mut h, Key::Q, true);
    assert_eq!(sounding(&h), vec![(keys, 86)]);
    // Changing the octave while the key is down still lets go of the note it started.
    key(&mut h, Key::Semicolon, true);
    key(&mut h, Key::Q, false);
    assert!(sounding(&h).is_empty());
}

#[test]
fn a_key_let_go_with_a_modifier_or_while_a_field_has_the_keys_still_stops_its_note() {
    let (mut h, keys) = harness();
    key(&mut h, Key::Z, true);
    key_with(&mut h, Key::Z, false, Modifiers::CTRL);
    assert!(sounding(&h).is_empty(), "Ctrl pressed while Z was down");
    key(&mut h, Key::X, true);
    assert_eq!(sounding(&h), vec![(keys, 62)]);
    // A dialog (like a text field taking the keyboard) stops the keys from playing, not the
    // release of one already down.
    let app = h.state_mut();
    assert!(app.dialogs.open_for_command(&app.engine, "file.bounce_mix"));
    h.step();
    key(&mut h, Key::X, false);
    assert!(sounding(&h).is_empty(), "X let go while a dialog was open");
}

#[test]
fn closing_the_keyboard_lets_go_of_held_notes_and_gives_the_keys_back() {
    let (mut h, _) = harness();
    key(&mut h, Key::X, true);
    assert_eq!(sounding(&h).len(), 1);
    h.state_mut().ui.show_midi_keyboard = false;
    h.step();
    assert!(sounding(&h).is_empty());
    assert!(!h.state().keyboard.active);
    key(&mut h, Key::X, true);
    assert!(sounding(&h).is_empty(), "a closed keyboard plays nothing");
}

#[test]
fn step_mode_writes_notes_and_chords_at_the_insertion_point() {
    let (mut h, keys) = harness();
    h.state_mut().ui.midi_keyboard.step = true;
    let before = notes(&h, keys);
    let at = h.state().engine.session().edit.selection.start;
    // A chord: C and E held together, then G alone.
    key(&mut h, Key::Z, true);
    key(&mut h, Key::C, true);
    key(&mut h, Key::Z, false);
    key(&mut h, Key::C, false);
    key(&mut h, Key::B, true);
    key(&mut h, Key::B, false);
    let after = notes(&h, keys);
    // What was added, counting duplicates of notes already there.
    let mut added = after.clone();
    for n in &before {
        if let Some(i) = added.iter().position(|m| m == n) {
            added.remove(i);
        }
    }
    assert_eq!(added.len(), 3, "{added:?}");
    let start = |p: u8| added.iter().find(|n| n.0 == p).unwrap().1;
    assert_eq!(start(60), start(64), "the chord's notes start together");
    assert_eq!(start(67) - start(60), soundcraft_time::TICKS_PER_QUARTER / 2, "G follows one eighth later");
    assert!(h.state().engine.session().edit.selection.start > at, "the insertion point moved on");
}

#[test]
fn step_mode_only_plays_while_the_transport_runs() {
    let (mut h, keys) = harness();
    h.state_mut().ui.midi_keyboard.step = true;
    let before = notes(&h, keys);
    // No audio device here: mark the transport as running.
    h.state_mut().engine.transport.playing = true;
    key(&mut h, Key::Z, true);
    assert_eq!(sounding(&h), vec![(keys, 60)], "the key still plays");
    key(&mut h, Key::Z, false);
    assert_eq!(notes(&h, keys), before, "nothing is written while playing");
}
