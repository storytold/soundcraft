use super::*;
use soundcraft_model::{ChannelFormat, Route, TrackId, TrackKind};

type Wire = Vec<(String, Vec<u8>)>;

fn session() -> Session {
    Session::new("t", soundcraft_time::SampleRate::HZ_48000)
}

/// A record-enabled track of `kind` that plays to `port` on `channel` (0..=15).
fn track(s: &mut Session, kind: TrackKind, port: &str, channel: u8) -> TrackId {
    let t = s.add_track(kind, ChannelFormat::Stereo, None);
    MidiOutput::store(&mut s.edit, t, Some(MidiOutput { device: port, channel }));
    s.track_mut(t).unwrap().mixer.record_arm = true;
    t
}

/// Input ports 0 "Keys" and 1 "Pads".
fn names() -> MidiPorts {
    let n = MidiPorts::default();
    assert_eq!(n.assign("Keys"), 0);
    assert_eq!(n.assign("Pads"), 1);
    n
}

fn ev(port: u8, status: u8, data1: u8, data2: u8) -> LiveMidi {
    LiveMidi { offset: 0, port, status, data1, data2, echo: false }
}

fn play(thru: &mut Thru, s: &Session, names: &MidiPorts, events: &[LiveMidi]) -> Wire {
    let mut wire = Wire::new();
    for e in events {
        thru.route(s, names, 0, e, &mut |d, m| wire.push((d.to_owned(), m.to_vec())));
    }
    wire
}

fn follow(thru: &mut Thru, s: &Session, names: &MidiPorts) -> Wire {
    let mut wire = Wire::new();
    thru.follow(s, names, 0, &mut |d, m| wire.push((d.to_owned(), m.to_vec())));
    wire
}

fn w(dest: &str, m: &[u8]) -> (String, Vec<u8>) {
    (dest.to_owned(), m.to_vec())
}

#[test]
fn every_message_goes_out_on_the_track_output_channel() {
    let (mut s, names) = (session(), names());
    track(&mut s, TrackKind::Midi, "PC1X", 4);
    let mut thru = Thru::default();
    let wire = play(
        &mut thru,
        &s,
        &names,
        &[
            ev(0, 0x90, 60, 100),
            ev(0, 0xB1, 1, 50),
            ev(0, 0xE2, 0, 70),
            ev(0, 0xC3, 7, 0),
            ev(0, 0xD0, 30, 0),
            ev(0, 0xA0, 60, 20),
            ev(0, 0x80, 60, 40),
        ],
    );
    assert_eq!(
        wire,
        [
            w("PC1X", &[0x94, 60, 100]),
            w("PC1X", &[0xB4, 1, 50]),
            w("PC1X", &[0xE4, 0, 70]),
            w("PC1X", &[0xC4, 7]),
            w("PC1X", &[0xD4, 30]),
            w("PC1X", &[0xA4, 60, 20]),
            w("PC1X", &[0x84, 60, 40]),
        ]
    );
    assert!(!thru.holds());
}

#[test]
fn only_tracks_that_play_live_input_with_an_output_and_an_accepting_input_thru() {
    let (mut s, names) = (session(), names());
    let midi = track(&mut s, TrackKind::Midi, "A", 0);
    let inst = track(&mut s, TrackKind::Instrument, "B", 1);
    let audio = s.add_track(TrackKind::Audio, ChannelFormat::Stereo, None);
    s.track_mut(audio).unwrap().mixer.record_arm = true;
    let unrouted = s.add_track(TrackKind::Midi, ChannelFormat::Stereo, None);
    s.track_mut(unrouted).unwrap().mixer.record_arm = true;
    let mut thru = Thru::default();
    let on = [ev(0, 0x90, 60, 100)];
    let off = [ev(0, 0x80, 60, 0)];
    assert_eq!(play(&mut thru, &s, &names, &on), [w("A", &[0x90, 60, 100]), w("B", &[0x91, 60, 100])]);
    play(&mut thru, &s, &names, &off);
    // Out of record but input-monitored: still thru (Pro Tools p.820).
    s.track_mut(inst).unwrap().mixer.record_arm = false;
    s.track_mut(inst).unwrap().mixer.input_monitor = true;
    assert_eq!(play(&mut thru, &s, &names, &on).len(), 2);
    play(&mut thru, &s, &names, &off);
    s.track_mut(inst).unwrap().mixer.input_monitor = false;
    // The MIDI track listens to the pads on channel 2 only.
    s.track_mut(midi).unwrap().mixer.input = Route::Hardware("MIDI:2:Pads".into());
    assert!(play(&mut thru, &s, &names, &on).is_empty());
    assert!(play(&mut thru, &s, &names, &[ev(1, 0x90, 60, 100)]).is_empty(), "channel 1 of the pads");
    assert_eq!(play(&mut thru, &s, &names, &[ev(1, 0x91, 60, 100)]), [w("A", &[0x90, 60, 100])]);
    // Inactive: nothing.
    s.track_mut(midi).unwrap().inactive = true;
    play(&mut thru, &s, &names, &[ev(1, 0x81, 60, 0)]);
    assert!(play(&mut thru, &s, &names, &[ev(1, 0x91, 61, 100)]).is_empty());
}

#[test]
fn two_tracks_on_one_destination_send_each_message_once() {
    let (mut s, names) = (session(), names());
    track(&mut s, TrackKind::Midi, "PC1X", 0);
    track(&mut s, TrackKind::Instrument, "PC1X", 0);
    let mut thru = Thru::default();
    assert_eq!(play(&mut thru, &s, &names, &[ev(0, 0x90, 60, 100), ev(0, 0x80, 60, 0)]), [w("PC1X", &[0x90, 60, 100]), w("PC1X", &[0x80, 60, 0])]);
}

#[test]
fn a_key_two_sources_hold_ends_when_the_last_lets_go_and_a_stray_release_sends_nothing() {
    let (mut s, names) = (session(), names());
    track(&mut s, TrackKind::Midi, "PC1X", 0);
    let mut thru = Thru::default();
    // A release of a key pressed before thru (or already released) is not sent.
    assert!(play(&mut thru, &s, &names, &[ev(0, 0x80, 50, 0)]).is_empty());
    let wire = play(&mut thru, &s, &names, &[ev(0, 0x90, 60, 100), ev(1, 0x93, 60, 90), ev(0, 0x90, 60, 0)]);
    assert_eq!(wire, [w("PC1X", &[0x90, 60, 100]), w("PC1X", &[0x90, 60, 90])], "the first release does not end the key the pads hold");
    assert_eq!(play(&mut thru, &s, &names, &[ev(1, 0x83, 60, 10)]), [w("PC1X", &[0x80, 60, 10])]);
    assert!(!thru.holds());
}

#[test]
fn the_pedal_two_sources_hold_goes_up_when_the_last_lets_go() {
    let (mut s, names) = (session(), names());
    track(&mut s, TrackKind::Midi, "PC1X", 0);
    let mut thru = Thru::default();
    let wire = play(&mut thru, &s, &names, &[ev(0, 0xB0, 64, 127), ev(1, 0xB3, 64, 100), ev(0, 0xB0, 64, 0)]);
    assert_eq!(wire, [w("PC1X", &[0xB0, 64, 127]), w("PC1X", &[0xB0, 64, 100])], "the pads still hold the pedal");
    assert_eq!(play(&mut thru, &s, &names, &[ev(1, 0xB3, 64, 0)]), [w("PC1X", &[0xB0, 64, 0])]);
    assert!(!thru.holds());
}

#[test]
fn a_track_that_stops_playing_live_input_lets_go_of_its_notes_and_pedal() {
    let changes: [(&str, fn(&mut Session, TrackId)); 7] = [
        ("taken out of record", |s, t| s.track_mut(t).unwrap().mixer.record_arm = false),
        ("made inactive", |s, t| s.track_mut(t).unwrap().inactive = true),
        ("muted", |s, t| s.track_mut(t).unwrap().mixer.mute = true),
        ("given another channel", |s, t| MidiOutput::store(&mut s.edit, t, Some(MidiOutput { device: "PC1X", channel: 5 }))),
        ("given no output", |s, t| MidiOutput::store(&mut s.edit, t, None)),
        ("given another input", |s, t| s.track_mut(t).unwrap().mixer.input = Route::Hardware("MIDI:*:Pads".into())),
        ("MIDI Thru switched off", |s, _| s.edit.set_flag(soundcraft_mix::NO_MIDI_THRU, true)),
    ];
    for (what, change) in changes {
        let (mut s, names) = (session(), names());
        let t = track(&mut s, TrackKind::Midi, "PC1X", 0);
        let mut thru = Thru::default();
        play(&mut thru, &s, &names, &[ev(0, 0xB0, 64, 127), ev(0, 0x90, 60, 100), ev(0, 0x90, 64, 100)]);
        assert!(follow(&mut thru, &s, &names).is_empty(), "{what}: nothing changed yet");
        change(&mut s, t);
        let mut wire = follow(&mut thru, &s, &names);
        wire.sort();
        assert_eq!(wire, [w("PC1X", &[0x80, 60, 0]), w("PC1X", &[0x80, 64, 0]), w("PC1X", &[0xB0, 64, 0])], "{what}");
        assert!(!thru.holds(), "{what}");
        assert!(
            play(&mut thru, &s, &names, &[ev(0, 0x80, 60, 0), ev(0, 0xB0, 64, 0)]).iter().all(|(_, m)| m[0] & 0xF0 != 0x80),
            "{what}: the key's release sends nothing more"
        );
    }
}

#[test]
fn a_second_track_on_the_destination_keeps_the_notes_another_leaves() {
    let (mut s, names) = (session(), names());
    let a = track(&mut s, TrackKind::Midi, "PC1X", 0);
    track(&mut s, TrackKind::Midi, "PC1X", 0);
    let mut thru = Thru::default();
    play(&mut thru, &s, &names, &[ev(0, 0x90, 60, 100)]);
    s.track_mut(a).unwrap().mixer.record_arm = false;
    assert!(follow(&mut thru, &s, &names).is_empty());
    assert!(thru.holds());
}

#[test]
fn an_instrument_track_mute_is_for_its_audio_and_does_not_stop_thru() {
    let (mut s, names) = (session(), names());
    let t = track(&mut s, TrackKind::Instrument, "PC1X", 0);
    s.track_mut(t).unwrap().mixer.mute = true;
    let mut thru = Thru::default();
    assert_eq!(play(&mut thru, &s, &names, &[ev(0, 0x90, 60, 100)]).len(), 1);
}

#[test]
fn a_lost_port_stop_and_a_reopened_port_end_what_thru_holds() {
    let (mut s, names) = (session(), names());
    track(&mut s, TrackKind::Midi, "PC1X", 2);
    track(&mut s, TrackKind::Midi, "Other", 3);
    let mut thru = Thru::default();
    play(&mut thru, &s, &names, &[ev(0, 0x90, 60, 100), ev(1, 0x90, 62, 100), ev(1, 0xB0, 64, 100)]);
    let mut wire = Wire::new();
    thru.release_ports(midi_port_bit(1), &mut |d, m| wire.push((d.to_owned(), m.to_vec())));
    wire.sort();
    assert_eq!(wire, [w("Other", &[0x83, 62, 0]), w("Other", &[0xB3, 64, 0]), w("PC1X", &[0x82, 62, 0]), w("PC1X", &[0xB2, 64, 0])]);
    wire.clear();
    thru.release_all(&mut |d, m| wire.push((d.to_owned(), m.to_vec())));
    wire.sort();
    assert_eq!(wire, [w("Other", &[0x83, 60, 0]), w("PC1X", &[0x82, 60, 0])]);
    play(&mut thru, &s, &names, &[ev(0, 0x90, 60, 100), ev(0, 0xB0, 64, 100)]);
    assert_eq!(thru.reopened("PC1X"), 1 << 2);
    assert_eq!(thru.reopened("PC1X"), 0);
    assert!(thru.holds(), "the other port still holds its note");
}

#[test]
fn the_midi_keyboard_window_thrus_to_tracks_that_take_all_devices() {
    // Pro Tools Reference Guide p.828: MIDI Input set to All, the track record-enabled, the keys
    // of the MIDI Keyboard window play the device on its MIDI output.
    let (mut s, names) = (session(), names());
    let window = names.assign(crate::midi::KEYBOARD_PORT);
    track(&mut s, TrackKind::Midi, "PC1X", 2);
    let keys_only = track(&mut s, TrackKind::Instrument, "Module", 0);
    s.track_mut(keys_only).unwrap().mixer.input = Route::Hardware("MIDI:*:Keys".into());
    let mut thru = Thru::default();
    let wire = play(&mut thru, &s, &names, &[ev(window, 0x90, 60, 100), ev(window, 0x80, 60, 0)]);
    assert_eq!(wire, [w("PC1X", &[0x92, 60, 100]), w("PC1X", &[0x82, 60, 0])]);
    // Released on Stop like a hardware key; letting go afterwards sends nothing more.
    play(&mut thru, &s, &names, &[ev(window, 0x90, 64, 100)]);
    let mut stop = Wire::new();
    thru.release_all(&mut |d, m| stop.push((d.to_owned(), m.to_vec())));
    assert_eq!(stop, [w("PC1X", &[0x82, 64, 0])]);
    assert!(play(&mut thru, &s, &names, &[ev(window, 0x80, 64, 0)]).is_empty());
}

#[test]
fn a_reopened_port_quiets_every_channel_thru_played_on_since_it_was_opened_even_released() {
    // Windows: a device added or removed breaks every open output handle, so the releases Thru
    // sends then fail. Thru has let go of the note all the same: the reopen must quiet its
    // channel. Released notes, pedals let up and notes held all count, until the next reopen.
    let (mut s, names) = (session(), names());
    track(&mut s, TrackKind::Midi, "SYN", 0);
    track(&mut s, TrackKind::Midi, "Other", 5);
    let mut thru = Thru::default();
    play(&mut thru, &s, &names, &[ev(0, 0x90, 60, 100), ev(0, 0xB0, 64, 127)]);
    let mut lost = Wire::new();
    thru.release_ports(midi_port_bit(0), &mut |d, m| lost.push((d.to_owned(), m.to_vec())));
    assert!(!thru.holds() && !lost.is_empty(), "released (the sends may have failed)");
    assert_eq!(thru.reopened("SYN"), 1 << 0);
    assert_eq!(thru.reopened("Other"), 1 << 5);
    assert_eq!(thru.reopened("SYN"), 0, "only since the port was last opened");
    // A controller alone leaves nothing sounding.
    play(&mut thru, &s, &names, &[ev(0, 0xB0, 1, 20)]);
    assert_eq!(thru.reopened("SYN"), 0);
}
