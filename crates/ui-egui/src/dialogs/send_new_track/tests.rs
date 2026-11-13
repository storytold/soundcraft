use super::*;
use crate::{Services, SoundApp, dialogs::Dialog};
use egui::{Modifiers, accesskit::Role, vec2};
use egui_kittest::{Harness, kittest::Queryable};
use soundcraft_model::Route;

/// The demo session with the New Track dialog open for Kick's send A, drawn in the whole app.
/// With `existing_send`, Kick already sends to a bus "Old" at -9 dB from that slot.
fn harness(existing_send: bool) -> (Harness<'static, SoundApp>, TrackId) {
    let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
    let kick = app.engine.session().track_by_name("Kick").map(|t| t.id).unwrap();
    if existing_send {
        app.run("mix.new_bus", json!({"name": "Old"})).unwrap();
        app.run("mix.send", json!({"track": kick.0, "slot": 0, "bus": "Old", "level_db": -9.0})).unwrap();
    }
    app.dialogs.set(Dialog::SendToNewTrack(SendToNewTrack::new(kick, 0, !existing_send)));
    let mut h = Harness::builder().with_size(vec2(1600.0, 1000.0)).build_ui_state(
        |ui, app: &mut SoundApp| {
            app.logic(ui.ctx());
            app.ui(ui);
        },
        app,
    );
    h.ctx.set_os(egui::os::OperatingSystem::Mac);
    h.run_steps(4);
    (h, kick)
}

/// Where Kick's send A goes, and the track names in order.
fn routing(app: &SoundApp, kick: TrackId) -> (Option<Route>, Vec<String>) {
    let s = app.engine.session();
    let send = s.track(kick).and_then(|t| t.mixer.sends.first().cloned().flatten()).map(|send| send.target);
    (send, s.tracks.iter().map(|t| t.name.clone()).collect())
}

/// Choosing a kind, typing a name and unticking "next to" makes exactly that track, fed by a bus
/// of its name, and opens the new send's window.
#[test]
fn create_makes_the_track_the_dialog_shows_and_opens_the_send_window() {
    let (mut h, kick) = harness(false);
    assert_eq!(h.state().dialogs.open_name(), Some("send_new_track"));
    h.get_by_label("Track kind").click();
    h.run();
    h.get_by_label("Audio Track").click();
    h.run();
    assert_eq!(h.get_by_role_and_label(Role::TextInput, "Name:").value().as_deref(), Some("Audio"), "the default name follows the kind");
    h.get_by_role_and_label(Role::TextInput, "Name:").click();
    h.run();
    h.key_press_modifiers(Modifiers::COMMAND, egui::Key::A);
    h.get_by_role_and_label(Role::TextInput, "Name:").type_text("Print");
    h.run();
    h.get_by_label("Create next to current track").click();
    h.run();
    h.get_by_role_and_label(Role::Button, "Create").click();
    h.run_steps(2);

    let app = h.state();
    assert_eq!(app.dialogs.open_name(), None, "the dialog closed");
    let s = app.engine.session();
    let print = s.track_by_name("Print").cloned().unwrap();
    let bus = s.bus_by_name("Print").cloned().unwrap();
    assert_eq!((print.kind, print.format, bus.format), (TrackKind::Audio, ChannelFormat::Stereo, ChannelFormat::Stereo));
    assert_eq!(print.mixer.input, Route::Bus(bus.id));
    let (send, names) = routing(app, kick);
    assert_eq!(send, Some(Route::Bus(bus.id)), "Kick's send A feeds the new bus");
    let master = names.iter().position(|n| s.track_by_name(n).is_some_and(|t| t.kind == TrackKind::Master)).unwrap_or(names.len());
    assert_eq!(names.iter().position(|n| n == "Print"), Some(master - 1), "not next to Kick: where Track › New puts it");
    assert_eq!(app.ui.send_window, Some((kick, 0)), "the new send's window is open");
}

/// Re-targeting an existing send keeps its level and leaves the Send window alone; by default the
/// new Aux Input sits right below the track with the send.
#[test]
fn an_existing_send_moves_to_a_new_aux_next_to_its_track() {
    let (mut h, kick) = harness(true);
    h.get_by_role_and_label(Role::Button, "Create").click();
    h.run_steps(2);
    let app = h.state();
    let s = app.engine.session();
    let (send, names) = routing(app, kick);
    let kick_at = names.iter().position(|n| n == "Kick").unwrap();
    let aux = s.tracks.get(kick_at + 1).cloned().unwrap();
    assert_eq!((aux.kind, aux.format), (TrackKind::Aux, ChannelFormat::Stereo));
    let bus = s.bus_by_name(&aux.name).map(|b| b.id).unwrap();
    assert_eq!(aux.mixer.input, Route::Bus(bus));
    assert_eq!(send, Some(Route::Bus(bus)));
    let level = s.track(kick).and_then(|t| t.mixer.sends[0].as_ref()).map(|send| send.level_db);
    assert_eq!(level, Some(-9.0), "the send keeps its level");
    assert_eq!(app.ui.send_window, None, "no window opened");
}

/// Cancel and Escape close the dialog and make nothing.
#[test]
fn cancel_and_escape_make_nothing() {
    for escape in [false, true] {
        let (mut h, kick) = harness(false);
        let before = (routing(h.state(), kick), h.state().engine.session().busses.len(), h.state().engine.undo_history().len());
        if escape {
            h.key_press(egui::Key::Escape);
        } else {
            h.get_by_role_and_label(Role::Button, "Cancel").click();
        }
        h.run_steps(2);
        let app = h.state();
        assert_eq!(app.dialogs.open_name(), None, "escape: {escape}");
        assert_eq!((routing(app, kick), app.engine.session().busses.len(), app.engine.undo_history().len()), before, "escape: {escape}");
        assert_eq!(app.ui.send_window, None);
    }
}

/// The dialog offers only kinds with an input a bus can feed, and sends what it shows.
#[test]
fn the_dialog_offers_aux_audio_and_instrument_tracks() {
    let (mut h, kick) = harness(false);
    h.get_by_label("Track kind").click();
    h.run();
    for k in TrackKind::ALL {
        let offered = h.query_by_label(k.label()).is_some();
        assert_eq!(offered, KINDS.contains(&k), "{}", k.label());
    }
    let d = SendToNewTrack { name: "FX".into(), timebase: "ticks".into(), next_to_track: false, ..SendToNewTrack::new(kick, 3, true) };
    assert_eq!(
        d.params(),
        json!({"track": kick.0, "slot": 3, "kind": "aux", "format": "Stereo", "timebase": "ticks", "name": "FX", "next_to_track": false})
    );
}
