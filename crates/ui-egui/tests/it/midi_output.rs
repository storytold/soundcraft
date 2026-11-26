//! MIDI and instrument tracks choose a MIDI output (a port and a channel) in the Mix window's I/O
//! section and the Edit window's I/O column (an instrument track also in the Instruments column).
//! The selectors are labelled combo boxes, list the
//! ports (one that is unplugged says so), mark channels other tracks use with text, run
//! `track.midi_output`, and work from the keyboard.
use egui::accesskit::Role;
use egui::{Key, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use serde_json::json;
use soundcraft_model::{MidiOutput, TrackId};
use soundcraft_ui_egui::{MainWindow, Services, SoundApp};

/// A session with a MIDI track "Synth", an instrument track "Bass" and an audio track "Vox";
/// the listed ports are none of them real.
fn harness(window: MainWindow) -> Harness<'static, SoundApp> {
    let mut engine = soundcraft_engine::Engine::default();
    engine.execute("track.new", &json!({"name": "Synth", "kind": "midi"})).unwrap();
    engine.execute("track.new", &json!({"name": "Bass", "kind": "instrument"})).unwrap();
    engine.execute("track.new", &json!({"name": "Vox"})).unwrap();
    engine.session_mut().edit.set_flag("edit_view.io", true);
    let mut app = SoundApp::new(engine, None, Services::default());
    app.midi_output_ports = Some(Ok(vec!["UM-ONE".into(), "IAC Bus 1".into()]));
    app.ui.window = window;
    let mut h =
        Harness::builder().with_context_setup(soundcraft_ui_egui::framework::install_headless).with_size(vec2(1600.0, 1100.0)).build_ui_state(
            |ui, app: &mut SoundApp| {
                app.logic(&ui.ctx().clone());
                app.ui(ui);
            },
            app,
        );
    h.run_steps(4);
    h
}

fn output(h: &Harness<'_, SoundApp>, name: &str) -> Option<String> {
    let s = h.state().engine.session();
    let id: TrackId = s.track_by_name(name).unwrap().id;
    MidiOutput::of(&s.edit, id).map(|o| o.route())
}

fn selector_value(h: &Harness<'_, SoundApp>, name: &str) -> String {
    let label = format!("MIDI output for {name}");
    let node = h.get_by_role_and_label(Role::ComboBox, &label);
    node.accesskit_node().value().unwrap_or_default().to_string()
}

#[test]
fn choosing_a_port_and_a_channel_in_the_mix_window_routes_the_track() {
    let mut h = harness(MainWindow::Mix);
    assert_eq!(selector_value(&h, "Synth"), "No MIDI output");
    assert!(h.query_by_label("MIDI output for Vox").is_none(), "an audio track has no MIDI output");
    h.get_by_label("MIDI output for Synth").click();
    h.run_steps(3);
    h.get_by_label("UM-ONE").click();
    h.run_steps(3);
    assert_eq!(output(&h, "Synth").as_deref(), Some("MIDI:1:UM-ONE"));
    assert_eq!(selector_value(&h, "Synth"), "UM-ONE ch 1");
    h.get_by_label("10").click();
    h.run_steps(3);
    assert_eq!(output(&h, "Synth").as_deref(), Some("MIDI:10:UM-ONE"));
    // Another track on the same port sees channel 10 marked in words, not only by its look.
    h.key_press(Key::Escape);
    h.run_steps(2);
    h.get_by_label("MIDI output for Bass").click();
    h.run_steps(3);
    h.get_by_label("UM-ONE").click();
    h.run_steps(3);
    h.get_by_label("10 (in use)");
    h.get_by_label("No MIDI output").click();
    h.run_steps(3);
    assert_eq!(output(&h, "Bass"), None);
}

#[test]
fn an_unplugged_port_stays_chosen_and_says_it_is_unavailable() {
    let mut h = harness(MainWindow::Edit);
    h.state_mut().run("track.midi_output", json!({"tracks": [1], "output": "MIDI:3:Gone"})).unwrap();
    h.run_steps(3);
    // The Edit window's I/O column shows it, with the same name as in the Mix window.
    assert_eq!(selector_value(&h, "Synth"), "Gone ch 3 (unavailable)");
    h.get_by_label("MIDI output for Synth").click();
    h.run_steps(3);
    h.get_by_label("Gone (unavailable)");
    h.get_by_label("UM-ONE");
}

/// A narrow selector can't show all of "Gone ch 3 (unavailable)", so its tooltip does, in both
/// windows; the selector also takes the warning colour, which a test can't read.
#[test]
fn an_unplugged_port_is_named_in_full_on_hover() {
    for window in [MainWindow::Mix, MainWindow::Edit] {
        let mut h = harness(window);
        h.state_mut().run("track.midi_output", json!({"tracks": [1], "output": "MIDI:3:Gone"})).unwrap();
        h.run_steps(3);
        h.get_by_label("MIDI output for Synth").hover();
        h.run_steps(30);
        assert!(h.query_by_label("MIDI output: Gone ch 3 (unavailable)").is_some(), "{window:?}: no tooltip naming the missing port");
    }
}

#[test]
fn the_mix_window_selector_works_from_the_keyboard() {
    let mut h = harness(MainWindow::Mix);
    h.get_by_label("MIDI output for Synth").focus();
    h.run_steps(2);
    h.key_press(Key::Space);
    h.run_steps(3);
    h.get_by_label("No MIDI output");
    // Tab moves through the menu to the first port; Enter chooses it.
    for _ in 0..8 {
        if h.get_by_label("UM-ONE").accesskit_node().is_focused() {
            break;
        }
        h.key_press(Key::Tab);
        h.run_steps(2);
    }
    assert!(h.get_by_label("UM-ONE").accesskit_node().is_focused(), "the keyboard reaches the ports");
    h.key_press(Key::Enter);
    h.run_steps(3);
    assert_eq!(output(&h, "Synth").as_deref(), Some("MIDI:1:UM-ONE"));
}

/// Pro Tools puts an instrument track's MIDI output selector in the Instruments view too
/// (Reference Guide p320, p368): with only that view on, the instrument track can be routed there.
#[test]
fn the_instruments_view_holds_an_instrument_tracks_midi_output() {
    let mut h = harness(MainWindow::Edit);
    {
        let edit = &mut h.state_mut().engine.session_mut().edit;
        edit.set_flag("edit_view.io", false);
        edit.set_flag("edit_view.instruments", true);
    }
    h.run_steps(4);
    assert!(h.query_by_label("MIDI output for Synth").is_none(), "a MIDI track has no instrument, so no Instruments view selector");
    assert_eq!(selector_value(&h, "Bass"), "No MIDI output");
    h.get_by_label("MIDI output for Bass").click();
    h.run_steps(3);
    h.get_by_label("IAC Bus 1").click();
    h.run_steps(3);
    assert_eq!(output(&h, "Bass").as_deref(), Some("MIDI:1:IAC Bus 1"));
    assert_eq!(selector_value(&h, "Bass"), "IAC Bus 1 ch 1");
}
