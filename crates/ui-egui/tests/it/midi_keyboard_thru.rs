//! The MIDI Keyboard window plays live, like a hardware keyboard: a key held down sends a Note-On
//! into the MIDI input (port "MIDI Keyboard window", channel 1) and its release the Note-Off, so
//! record-enabled and input-monitored tracks whose input is All devices play and thru it, and the
//! record-enabled ones record it (Pro Tools Reference Guide p.828). With no MIDI or instrument track record-enabled a click still
//! step-enters the note into the selected clip.
use egui::{Event, Modifiers, PointerButton, Pos2, pos2, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use serde_json::json;
use soundcraft_playback::midi::{KEYBOARD_PORT, MidiInputs};
use soundcraft_ui_egui::{Services, SoundApp};

fn harness() -> Harness<'static, SoundApp> {
    let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
    app.midi_inputs = Some(MidiInputs::with_port_names(Vec::new(), vec!["Keys".to_string()]));
    app.ui.show_midi_keyboard = true;
    let mut h = Harness::builder()
        .with_context_setup(soundcraft_ui_egui::framework::install_headless)
        .with_size(vec2(1600.0, 1000.0))
        .with_pixels_per_point(1.0)
        .build_ui_state(
            |ui, app: &mut SoundApp| {
                let ctx = ui.ctx().clone();
                app.logic(&ctx);
                app.ui(ui);
            },
            app,
        );
    h.run_steps(4);
    h
}

/// The lowest white keys (C3, D3) in their lower half, where no black key covers them: the
/// keyboard sits right above the window's hint, at least 400 points wide (28 white keys).
fn keys(h: &Harness<'_, SoundApp>) -> (Pos2, Pos2) {
    let hint = h.get_by_label_contains("Click keys to play").rect();
    let y = hint.top() - 12.0;
    (pos2(hint.left() + 5.0, y), pos2(hint.left() + 24.0, y))
}

fn pointer(h: &mut Harness<'_, SoundApp>, pos: Pos2, pressed: Option<bool>) {
    h.input_mut().events.push(Event::PointerMoved(pos));
    if let Some(pressed) = pressed {
        h.input_mut().events.push(Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE });
    }
    h.run_steps(2);
}

/// How many messages the MIDI input took so far, and the last one (port name, status, key).
fn last(h: &Harness<'_, SoundApp>) -> (u32, Option<(String, u8, u8)>) {
    let feed = h.state().midi_inputs.as_ref().unwrap().feed();
    let (count, e) = feed.activity();
    (count, e.map(|e| (feed.ports().name(e.port).unwrap_or_default().to_string(), e.status, e.data1)))
}

fn on(key: u8) -> Option<(String, u8, u8)> {
    Some((KEYBOARD_PORT.to_string(), 0x90, key))
}

fn off(key: u8) -> Option<(String, u8, u8)> {
    Some((KEYBOARD_PORT.to_string(), 0x80, key))
}

fn notes_on_keys(h: &mut Harness<'_, SoundApp>) -> usize {
    let s = h.state().engine.session();
    let ids: Vec<_> = s.tracks.iter().filter(|t| t.name == "Keys").flat_map(|t| t.clips().iter().map(|c| c.id.0)).collect();
    let engine = &mut h.state_mut().engine;
    ids.iter().map(|id| engine.execute("midi.notes", &json!({"clip": id})).ok().and_then(|v| v["notes"].as_array().map(Vec::len)).unwrap_or(0)).sum()
}

#[test]
fn a_held_key_plays_until_it_is_let_go_or_left_for_another() {
    let mut h = harness();
    h.state_mut().run("mix.record_arm", json!({"tracks": ["Keys"], "value": true})).unwrap();
    let (c3, d3) = keys(&h);
    pointer(&mut h, c3, None);
    assert_eq!(last(&h), (0, None), "hovering plays nothing");
    pointer(&mut h, c3, Some(true));
    assert_eq!(last(&h), (1, on(48)));
    pointer(&mut h, c3, None);
    assert_eq!(last(&h).0, 1, "held: nothing more");
    // Sliding onto the next key ends the first before the second starts.
    pointer(&mut h, d3, None);
    assert_eq!(last(&h), (3, on(50)));
    pointer(&mut h, d3, Some(false));
    assert_eq!(last(&h), (4, off(50)));
    pointer(&mut h, d3, None);
    assert_eq!(last(&h).0, 4);
}

#[test]
fn closing_the_window_lets_go_of_the_held_key() {
    let mut h = harness();
    let (c3, _) = keys(&h);
    pointer(&mut h, c3, Some(true));
    assert_eq!(last(&h), (1, on(48)));
    h.state_mut().ui.show_midi_keyboard = false;
    h.run_steps(2);
    assert_eq!(last(&h), (2, off(48)));
}

#[test]
fn a_click_within_one_frame_still_plays_the_note_and_ends_it() {
    let mut h = harness();
    h.state_mut().run("mix.record_arm", json!({"tracks": ["Keys"], "value": true})).unwrap();
    let (c3, _) = keys(&h);
    h.input_mut().events.push(Event::PointerMoved(c3));
    for pressed in [true, false] {
        h.input_mut().events.push(Event::PointerButton { pos: c3, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE });
    }
    h.run_steps(2);
    assert_eq!(last(&h), (2, off(48)));
}

#[test]
fn a_click_step_enters_only_while_no_track_records_what_is_played() {
    let mut h = harness();
    h.state_mut().run("edit.select", json!({"tracks": ["Keys"]})).unwrap();
    h.run_steps(2);
    let before = notes_on_keys(&mut h);
    let (c3, _) = keys(&h);
    pointer(&mut h, c3, Some(true));
    pointer(&mut h, c3, Some(false));
    assert_eq!(notes_on_keys(&mut h), before + 1, "step input into the selected clip");
    assert_eq!(last(&h), (2, off(48)), "and the note is heard");
    // Record-enabled: the key is played (and recorded when recording), not step-entered.
    h.state_mut().run("mix.record_arm", json!({"tracks": ["Keys"], "value": true})).unwrap();
    pointer(&mut h, c3, Some(true));
    pointer(&mut h, c3, Some(false));
    assert_eq!(notes_on_keys(&mut h), before + 1);
    assert_eq!(last(&h), (4, off(48)));
}
