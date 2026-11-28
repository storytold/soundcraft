//! What a MIDI or instrument track's MIDI output selector shows when its port leaves the system's
//! port list and comes back, and when the port list can't be read at all. The list is set by the
//! test (no audio engine runs, so nothing rescans it).
use egui::accesskit::Role;
use egui::vec2;
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use serde_json::json;
use soundcraft_model::{MidiOutput, TrackId};
use soundcraft_ui_egui::{MainWindow, Services, SoundApp};

/// A MIDI track "Synth" playing to UM-ONE on channel 2, with UM-ONE and IAC Bus 1 listed.
fn harness(window: MainWindow) -> Harness<'static, SoundApp> {
    let mut engine = soundcraft_engine::Engine::default();
    engine.execute("track.new", &json!({"name": "Synth", "kind": "midi"})).unwrap();
    engine.execute("track.midi_output", &json!({"tracks": ["Synth"], "output": "MIDI:2:UM-ONE"})).unwrap();
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

fn selector_value(h: &Harness<'_, SoundApp>) -> String {
    let node = h.get_by_role_and_label(Role::ComboBox, "MIDI output for Synth");
    node.accesskit_node().value().unwrap_or_default().to_string()
}

fn output(h: &Harness<'_, SoundApp>) -> Option<String> {
    let s = h.state().engine.session();
    let id: TrackId = s.track_by_name("Synth").unwrap().id;
    MidiOutput::of(&s.edit, id).map(|o| o.route())
}

/// Unplugged: the route is kept and the selector says so; plugged back in: it reads as before,
/// with nothing for the user to redo.
#[test]
fn a_port_that_leaves_the_list_and_comes_back_keeps_the_route_and_clears_the_warning() {
    for window in [MainWindow::Mix, MainWindow::Edit] {
        let mut h = harness(window);
        assert_eq!(selector_value(&h), "UM-ONE ch 2", "{window:?}");
        h.state_mut().midi_output_ports = Some(Ok(vec!["IAC Bus 1".into()]));
        h.run_steps(3);
        assert_eq!(selector_value(&h), "UM-ONE ch 2 (unavailable)", "{window:?}");
        assert_eq!(output(&h).as_deref(), Some("MIDI:2:UM-ONE"), "{window:?}: an unplugged port is not dropped from the track");
        h.state_mut().midi_output_ports = Some(Ok(vec!["IAC Bus 1".into(), "UM-ONE".into()]));
        h.run_steps(3);
        assert_eq!(selector_value(&h), "UM-ONE ch 2", "{window:?}: the warning goes when the port is back");
    }
}

/// When the port list can't be read, the menu says why and still offers "No MIDI output".
#[test]
fn a_port_list_failure_is_named_in_the_menu_and_no_midi_output_stays_choosable() {
    let mut h = harness(MainWindow::Mix);
    h.state_mut().midi_output_ports = Some(Err("MIDI output enumeration failed".into()));
    h.run_steps(3);
    assert_eq!(selector_value(&h), "UM-ONE ch 2 (unavailable)");
    h.get_by_label("MIDI output for Synth").click();
    h.run_steps(3);
    h.get_by_label("MIDI output enumeration failed");
    h.get_by_label("UM-ONE (unavailable)");
    h.get_by_label("No MIDI output").click();
    h.run_steps(3);
    assert_eq!(output(&h), None);
    assert_eq!(selector_value(&h), "No MIDI output");
}

/// Deleting a routed track and undoing the delete brings its MIDI output back with it.
#[test]
fn undoing_a_track_delete_restores_its_midi_output() {
    let mut h = harness(MainWindow::Edit);
    h.state_mut().run("track.delete", json!({"tracks": ["Synth"]})).unwrap();
    h.run_steps(2);
    assert!(h.query_by_label("MIDI output for Synth").is_none());
    h.state_mut().run("edit.undo", json!({})).unwrap();
    h.run_steps(3);
    assert_eq!(output(&h).as_deref(), Some("MIDI:2:UM-ONE"));
    assert_eq!(selector_value(&h), "UM-ONE ch 2");
}
