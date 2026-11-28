use super::*;
use crate::sync_clock::Presentation;
use soundcraft_midi::{CtrlEvent, CtrlKind, Note, Sequence};
use soundcraft_model::{AutoParam, AutomationPoint, ChannelFormat, Clip, TrackKind};

/// At 120 bpm and 48 kHz one tick is 25 samples.
const TICK: i64 = 25;

fn note(pitch: u8, velocity: u8, start: i64, length: i64) -> Note {
    Note { pitch, velocity, release_velocity: 64, channel: 0, start, length }
}
fn cc(tick: i64, kind: CtrlKind, value: i32) -> CtrlEvent {
    CtrlEvent { tick, channel: 0, kind, value }
}

/// A track of `kind` whose one clip holds `notes` and `ctrls`, played to `port` on `channel`.
fn add_track(s: &mut Session, kind: TrackKind, notes: Vec<Note>, ctrls: Vec<CtrlEvent>, port: &str, channel: u8) -> TrackId {
    let t = s.add_track(kind, ChannelFormat::Stereo, None);
    let id = s.new_clip_id();
    let seq = Sequence { notes, ctrls, ..Default::default() };
    s.track_mut(t).unwrap().playlist_mut().unwrap().clips.push(Clip::midi(id, "m", 0, 48_000 * 20, seq));
    MidiOutput::store(&mut s.edit, t, Some(MidiOutput { device: port, channel }));
    t
}

fn session() -> Session {
    Session::new("t", soundcraft_time::SampleRate::HZ_48000)
}

/// One long presented block of pass `pass` that starts at `sample` at `at`.
fn rolling(at: Instant, pass: u64, sample: i64) -> Presentation {
    Presentation {
        generation: 0,
        sample_rate: soundcraft_time::SampleRate::HZ_48000,
        system_delay_samples: 0,
        pass,
        origin: sample,
        playing: true,
        sample: sample as f64,
        heard_at: at,
        samples_per_second: 48_000.,
        limit: 1e12,
        looped: None,
        end: None,
        lead: Some(Duration::ZERO),
    }
}
fn stopped(at: Instant, pass: u64, sample: i64) -> Presentation {
    Presentation { playing: false, ..rolling(at, pass, sample) }
}

type Wire = Vec<(Instant, Vec<u8>)>;

/// Update `enc` every 250 µs from `from` to `to` against `b`, as the worker does when woken.
fn run(enc: &mut TrackMidi, s: &Session, b: Presentation, from: Duration, to: Duration, wire: &mut Wire) {
    let mut t = from;
    while t <= to {
        enc.update(b, b.heard_at + t, s, &mut |at, m| wire.push((at, m.to_vec())));
        t += Duration::from_micros(250);
    }
}
fn bytes(wire: &Wire) -> Vec<Vec<u8>> {
    wire.iter().map(|(_, m)| m.clone()).collect()
}
fn note_ons(wire: &[(Instant, Vec<u8>)]) -> Vec<(Instant, u8)> {
    wire.iter().filter(|(_, m)| m[0] & 0xf0 == 0x90).map(|(at, m)| (*at, m[1])).collect()
}

#[test]
fn notes_leave_at_their_presentation_deadlines_and_the_track_offset_moves_them() {
    let base = Instant::now();
    let mut deadlines = Vec::new();
    for offset in [0.0, 480.0, -480.0] {
        let mut s = session();
        let t = add_track(&mut s, TrackKind::Midi, vec![note(60, 100, 96, 48), note(64, 40, 480, 48)], vec![], "DIN", 0);
        s.edit.values.insert(format!("midi.offset.{}", t.0), offset);
        let b = rolling(base, 1, 0);
        let mut enc = TrackMidi::new("DIN");
        let mut wire = Wire::new();
        run(&mut enc, &s, b, Duration::ZERO, Duration::from_millis(700), &mut wire);
        let ons = note_ons(&wire);
        assert_eq!(ons.iter().map(|(_, p)| *p).collect::<Vec<_>>(), [60, 64]);
        for (at, pitch) in &ons {
            let tick = if *pitch == 60 { 96 } else { 480 };
            assert_eq!(*at, b.instant_for((tick * TICK) as f64 + offset), "pitch {pitch}, offset {offset}");
        }
        deadlines.push(ons[0].0);
    }
    assert!(deadlines[1] > deadlines[0] && deadlines[2] < deadlines[0], "a later offset plays later, an earlier one earlier");
}

#[test]
fn every_event_leaves_on_the_selected_channel_and_two_tracks_interleave() {
    let base = Instant::now();
    let mut s = session();
    let mut recorded_on_five = note(60, 100, 0, 96);
    recorded_on_five.channel = 4;
    add_track(&mut s, TrackKind::Midi, vec![recorded_on_five, note(62, 90, 192, 96)], vec![cc(48, CtrlKind::Cc(7), 90)], "DIN", 9);
    add_track(&mut s, TrackKind::Instrument, vec![note(48, 80, 96, 96)], vec![cc(144, CtrlKind::Program, 3)], "DIN", 1);
    add_track(&mut s, TrackKind::Midi, vec![note(30, 80, 0, 96)], vec![], "Other port", 0);
    let mut enc = TrackMidi::new("DIN");
    let mut wire = Wire::new();
    run(&mut enc, &s, rolling(base, 1, 0), Duration::ZERO, Duration::from_millis(400), &mut wire);
    let channels: Vec<u8> = wire.iter().map(|(_, m)| m[0] & 0x0f).collect();
    assert!(channels.iter().all(|c| *c == 9 || *c == 1), "{wire:?}");
    assert!(!note_ons(&wire).iter().any(|(_, p)| *p == 30), "another port's track is not sent here");
    assert!(wire.windows(2).all(|w| w[0].0 <= w[1].0), "the two tracks' events interleave in time order");
    let statuses: Vec<u8> = wire.iter().map(|(_, m)| m[0]).collect();
    assert!(statuses.contains(&0x99) && statuses.contains(&0x91));
    let program = wire.iter().find(|(_, m)| m[0] & 0xf0 == 0xc0).unwrap();
    assert_eq!(program.1, [0xc1, 3], "a program change is two bytes on the track's channel");
}

#[test]
fn a_start_chases_the_last_values_bank_before_program_and_never_resets_controllers() {
    let base = Instant::now();
    let mut s = session();
    let ctrls = vec![
        cc(0, CtrlKind::Cc(7), 127),
        cc(480, CtrlKind::Cc(7), 20),
        cc(0, CtrlKind::Program, 5),
        cc(10, CtrlKind::Cc(0), 1),
        cc(10, CtrlKind::Cc(32), 2),
        cc(5000, CtrlKind::Cc(7), 99),
    ];
    add_track(&mut s, TrackKind::Midi, vec![], ctrls, "DIN", 0);
    let mut enc = TrackMidi::new("DIN");
    let mut wire = Wire::new();
    run(&mut enc, &s, rolling(base, 1, 960 * TICK), Duration::ZERO, Duration::from_millis(5), &mut wire);
    let sent = bytes(&wire);
    assert!(!sent.iter().any(|m| m[0] & 0xf0 == 0xb0 && m[1] == 121), "no Reset All Controllers: {sent:?}");
    assert!(sent.contains(&vec![0xb0, 7, 20]) && !sent.contains(&vec![0xb0, 7, 127]), "only the last value: {sent:?}");
    assert!(!sent.contains(&vec![0xb0, 7, 99]), "nothing from after the play position");
    let at = |m: &[u8]| sent.iter().position(|x| x == m).unwrap();
    assert!(at(&[0xb0, 0, 1]) < at(&[0xc0, 5]) && at(&[0xb0, 32, 2]) < at(&[0xc0, 5]), "bank select before the program");
    assert!(!sent.iter().any(|m| m[0] & 0xf0 == 0xb0 && m[1] == 10), "a controller with no event sends nothing");
}

#[test]
fn stop_sends_each_note_off_then_pedal_up_then_all_notes_off() {
    let base = Instant::now();
    let mut s = session();
    add_track(&mut s, TrackKind::Midi, vec![note(60, 100, 0, 9600), note(64, 100, 0, 9600)], vec![cc(1, CtrlKind::Cc(64), 127)], "DIN", 2);
    let mut enc = TrackMidi::new("DIN");
    let mut wire = Wire::new();
    run(&mut enc, &s, rolling(base, 1, 0), Duration::ZERO, Duration::from_millis(50), &mut wire);
    wire.clear();
    run(&mut enc, &s, stopped(base + Duration::from_millis(60), 1, 2880), Duration::ZERO, Duration::from_millis(5), &mut wire);
    let sent = bytes(&wire);
    assert_eq!(sent.len(), 4, "{sent:?}");
    let offs: Vec<_> = sent[..2].iter().map(|m| (m[0], m[1])).collect();
    assert!(offs.contains(&(0x82, 60)) && offs.contains(&(0x82, 64)), "{sent:?}");
    assert_eq!(sent[2..], [vec![0xb2, 64, 0], vec![0xb2, 123, 0]]);
}

#[test]
fn a_locate_while_playing_releases_notes_without_all_notes_off_and_chases_the_new_position() {
    let base = Instant::now();
    let mut s = session();
    let ctrls = vec![cc(0, CtrlKind::Cc(64), 127), cc(1920, CtrlKind::Cc(7), 50)];
    add_track(&mut s, TrackKind::Midi, vec![note(60, 100, 0, 9600)], ctrls, "DIN", 0);
    let mut enc = TrackMidi::new("DIN");
    let mut wire = Wire::new();
    run(&mut enc, &s, rolling(base, 1, 0), Duration::ZERO, Duration::from_millis(30), &mut wire);
    wire.clear();
    run(&mut enc, &s, rolling(base + Duration::from_millis(40), 2, 2400 * TICK), Duration::ZERO, Duration::ZERO, &mut wire);
    let sent = bytes(&wire);
    assert_eq!(sent[..2], [vec![0x80, 60, 0], vec![0xb0, 64, 0]], "{sent:?}");
    assert!(!sent.iter().any(|m| m[1] == 123 && m[0] & 0xf0 == 0xb0), "no All Notes Off while playing on");
    assert!(sent.contains(&vec![0xb0, 7, 50]) && sent.contains(&vec![0xb0, 64, 127]), "chased at the new position: {sent:?}");
}

#[test]
fn a_loop_wrap_releases_notes_that_cross_the_loop_end_and_chases_the_loop_start() {
    let base = Instant::now();
    let mut s = session();
    let ctrls = vec![cc(0, CtrlKind::Cc(1), 10), cc(200, CtrlKind::Cc(1), 90)];
    add_track(&mut s, TrackKind::Midi, vec![note(60, 100, 0, 96), note(72, 100, 300, 9600)], ctrls, "DIN", 0);
    let mut enc = TrackMidi::new("DIN");
    let mut wire = Wire::new();
    // The loop runs from tick 100 to tick 400: the note at 300 crosses its end.
    let first = rolling(base, 1, 100 * TICK);
    run(&mut enc, &s, first, Duration::ZERO, Duration::from_millis(155), &mut wire);
    assert!(note_ons(&wire).iter().any(|(_, p)| *p == 72));
    wire.clear();
    let wrap = rolling(base + Duration::from_millis(156), 2, 100 * TICK);
    run(&mut enc, &s, wrap, Duration::ZERO, Duration::ZERO, &mut wire);
    let sent = bytes(&wire);
    assert_eq!(sent[0], [0x80, 72, 0], "the crossing note ends at the wrap: {sent:?}");
    assert!(sent.contains(&vec![0xb0, 1, 10]), "the loop start's value is chased again: {sent:?}");
}

#[test]
fn an_edit_that_deletes_a_sounding_note_still_ends_it_at_its_original_time() {
    let base = Instant::now();
    let mut s = session();
    let t = add_track(&mut s, TrackKind::Midi, vec![note(60, 100, 0, 480)], vec![], "DIN", 0);
    let b = rolling(base, 1, 0);
    let mut enc = TrackMidi::new("DIN");
    let mut wire = Wire::new();
    run(&mut enc, &s, b, Duration::ZERO, Duration::from_millis(20), &mut wire);
    for clip in &mut s.track_mut(t).unwrap().playlist_mut().unwrap().clips {
        if let soundcraft_model::ClipContent::Midi { sequence } = &mut clip.content {
            sequence.notes.clear();
        }
    }
    run(&mut enc, &s, b, Duration::from_millis(20), Duration::from_millis(300), &mut wire);
    let off = wire.iter().find(|(_, m)| m[0] == 0x80).expect("the deleted note was released");
    assert_eq!(off.0, b.instant_for((480 * TICK) as f64));
}

#[test]
fn mute_and_solo_silence_a_midi_track_and_release_its_notes_but_not_an_instrument_track() {
    let base = Instant::now();
    type Change = fn(&mut Session, TrackId, TrackId);
    let cases: [(&str, Change, bool); 5] = [
        ("mute", |s, t, _| s.track_mut(t).unwrap().mixer.mute = true, true),
        ("another track soloed", |s, _, o| s.track_mut(o).unwrap().mixer.solo = true, true),
        (
            "another track soloed, this one solo safe",
            |s, t, o| {
                s.track_mut(o).unwrap().mixer.solo = true;
                s.track_mut(t).unwrap().mixer.solo_safe = true;
            },
            false,
        ),
        (
            "mute automation",
            |s, t, _| {
                let track = s.track_mut(t).unwrap();
                track.mixer.automation_mode = soundcraft_model::AutomationMode::Read;
                let lane = track.lane_mut(&AutoParam::Mute);
                lane.points = vec![AutomationPoint { at: 0, value: 0.0 }, AutomationPoint { at: 600 * TICK, value: 1.0 }];
            },
            true,
        ),
        ("nothing", |_, _, _| {}, false),
    ];
    for (case, change, silenced) in cases {
        for kind in [TrackKind::Midi, TrackKind::Instrument] {
            let mut s = session();
            let notes = (0..20u8).map(|i| note(60 + i, 100, i64::from(i) * 96, 960)).collect();
            let t = add_track(&mut s, kind, notes, vec![], "DIN", 0);
            let other = s.add_track(TrackKind::Audio, ChannelFormat::Mono, None);
            let b = rolling(base, 1, 0);
            let mut enc = TrackMidi::new("DIN");
            let mut wire = Wire::new();
            run(&mut enc, &s, b, Duration::ZERO, Duration::from_millis(310), &mut wire);
            change(&mut s, t, other);
            let before = wire.len();
            run(&mut enc, &s, b, Duration::from_millis(310), Duration::from_millis(2000), &mut wire);
            let ons = note_ons(&wire[before..]).len();
            let muted = silenced && kind == TrackKind::Midi;
            assert_eq!(ons == 0, muted, "{case} on {kind:?}: {ons} note-ons");
            if muted {
                // Every note it held ended when it went silent, before its own end (from 500 ms).
                let held = note_ons(&wire[..before]).len();
                let offs: Vec<_> = wire.iter().filter(|(_, m)| m[0] & 0xf0 == 0x80).collect();
                assert_eq!(offs.len(), held, "{case}");
                assert!(offs.iter().all(|(at, _)| *at < base + Duration::from_millis(320)), "{case}: {offs:?}");
            }
        }
    }
}

#[test]
fn a_dense_clip_loses_no_note_off() {
    let base = Instant::now();
    let mut s = session();
    // 400 notes start together, far more than the port's deadline queue holds; the notes that
    // share a key end at different times.
    let notes = (0..400u32).map(|i| note((i % 128) as u8, 100, 0, 96 + i64::from(i / 128))).collect();
    add_track(&mut s, TrackKind::Midi, notes, vec![], "DIN", 0);
    let mut enc = TrackMidi::new("DIN");
    let mut wire = Wire::new();
    run(&mut enc, &s, rolling(base, 1, 0), Duration::ZERO, Duration::from_millis(100), &mut wire);
    assert_eq!(note_ons(&wire).len(), 400);
    // Overlapping notes on one key reach the device as one Note-Off, when the last one ends.
    let offs: Vec<_> = wire.iter().filter(|(_, m)| m[0] == 0x80).collect();
    assert_eq!(offs.len(), 128, "one Note-Off per key");
    let last_end = offs.iter().map(|(at, _)| *at).max().unwrap();
    assert_eq!(last_end, rolling(base, 1, 0).instant_for(((96 + 3) * TICK) as f64));
}

#[test]
fn a_reopened_port_quiets_the_channels_in_use_and_does_not_replay_old_notes() {
    let base = Instant::now();
    let mut s = session();
    add_track(&mut s, TrackKind::Midi, vec![note(60, 100, 0, 9600), note(67, 100, 1920, 96)], vec![], "DIN", 3);
    let b = rolling(base, 1, 0);
    let mut enc = TrackMidi::new("DIN");
    let mut wire = Wire::new();
    run(&mut enc, &s, b, Duration::ZERO, Duration::from_millis(100), &mut wire);
    // The port was unplugged and reopened: the worker resets the encoder.
    enc.reset();
    wire.clear();
    run(&mut enc, &s, b, Duration::from_millis(500), Duration::from_millis(1100), &mut wire);
    assert_eq!(wire[..2].iter().map(|(_, m)| m.clone()).collect::<Vec<_>>(), [vec![0xb3, 64, 0], vec![0xb3, 123, 0]], "{wire:?}");
    let ons = note_ons(&wire);
    assert_eq!(ons.iter().map(|(_, p)| *p).collect::<Vec<_>>(), [67], "the note held before the unplug is not struck again");
}

#[test]
fn a_reopen_after_a_stop_whose_releases_failed_still_quiets_the_channel() {
    // Windows: the Stop's Note-Off and All Notes Off went to a broken handle. The encoder had
    // moved on (nothing held, nothing in use), but the reopen still quiets that channel.
    let base = Instant::now();
    let mut s = session();
    add_track(&mut s, TrackKind::Midi, vec![note(60, 100, 0, 9600)], vec![], "DIN", 5);
    let mut enc = TrackMidi::new("DIN");
    let mut wire = Wire::new();
    run(&mut enc, &s, rolling(base, 1, 0), Duration::ZERO, Duration::from_millis(100), &mut wire);
    enc.update(stopped(base, 1, 4800), base + Duration::from_millis(100), &s, &mut |_, _| {});
    enc.reset();
    wire.clear();
    enc.update(stopped(base, 1, 4800), base + Duration::from_millis(1200), &s, &mut |at, m| wire.push((at, m.to_vec())));
    assert_eq!(wire.iter().map(|(_, m)| m.clone()).collect::<Vec<_>>(), [vec![0xb5, 64, 0], vec![0xb5, 123, 0]]);
    // Only once: with the quieting delivered, the next reopen has nothing to make good.
    enc.delivered();
    enc.reset();
    wire.clear();
    enc.update(stopped(base, 1, 4800), base + Duration::from_millis(2400), &s, &mut |at, m| wire.push((at, m.to_vec())));
    assert!(wire.is_empty(), "{wire:?}");
}

#[test]
fn all_midi_notes_off_releases_each_held_note_and_playback_continues() {
    let base = Instant::now();
    let mut s = session();
    add_track(&mut s, TrackKind::Midi, vec![note(60, 100, 0, 9600), note(67, 100, 960, 96)], vec![], "DIN", 0);
    let b = rolling(base, 1, 0);
    let mut enc = TrackMidi::new("DIN");
    let mut wire = Wire::new();
    run(&mut enc, &s, b, Duration::ZERO, Duration::from_millis(100), &mut wire);
    let mut panic = Vec::new();
    enc.panic(&mut |m| panic.push(m.to_vec()));
    panic_messages(&mut |m| panic.push(m.to_vec()));
    assert_eq!(panic[0], [0x80, 60, 0]);
    let at = |m: &[u8]| panic.iter().position(|x| x == m).unwrap();
    assert!(at(&[0xb5, 64, 0]) < at(&[0xb5, 123, 0]) && at(&[0xb5, 123, 0]) < at(&[0xb5, 120, 0]));
    wire.clear();
    run(&mut enc, &s, b, Duration::from_millis(100), Duration::from_millis(600), &mut wire);
    assert_eq!(note_ons(&wire).len(), 1, "the next note still plays");
    assert!(!wire.iter().any(|(_, m)| m == &[0x80, 60, 0]), "the released note is not released twice");
}

#[test]
fn at_one_position_bank_select_precedes_the_program_and_both_precede_the_note() {
    let base = Instant::now();
    let mut s = session();
    // Stored program first, as a file may hold them.
    let ctrls = vec![cc(96, CtrlKind::Program, 9), cc(96, CtrlKind::Cc(0), 1), cc(96, CtrlKind::Cc(32), 3)];
    add_track(&mut s, TrackKind::Midi, vec![note(60, 100, 96, 48)], ctrls, "DIN", 0);
    let mut enc = TrackMidi::new("DIN");
    let mut wire = Wire::new();
    run(&mut enc, &s, rolling(base, 1, 0), Duration::ZERO, Duration::from_millis(100), &mut wire);
    let sent = bytes(&wire);
    let at = |m: &[u8]| sent.iter().position(|x| x == m).unwrap();
    assert!(at(&[0xb0, 0, 1]) < at(&[0xc0, 9]) && at(&[0xb0, 32, 3]) < at(&[0xc0, 9]), "{sent:?}");
    assert!(at(&[0xc0, 9]) < at(&[0x90, 60, 100]), "{sent:?}");
}

/// Play `s` for 50 ms, apply `change`, play on: what was sent after the change.
fn after_change(s: &mut Session, change: impl FnOnce(&mut Session)) -> Vec<Vec<u8>> {
    let base = Instant::now();
    let b = rolling(base, 1, 0);
    let mut enc = TrackMidi::new("DIN");
    let mut wire = Wire::new();
    run(&mut enc, s, b, Duration::ZERO, Duration::from_millis(50), &mut wire);
    change(s);
    wire.clear();
    run(&mut enc, s, b, Duration::from_millis(50), Duration::from_millis(60), &mut wire);
    bytes(&wire)
}

#[test]
fn a_track_that_leaves_a_channel_while_playing_lifts_the_pedal_it_left_down_there() {
    let ctrls = || vec![cc(0, CtrlKind::Cc(64), 127), cc(0, CtrlKind::Cc(7), 90), cc(0, CtrlKind::Program, 5)];
    type Change = fn(&mut Session, TrackId);
    let cases: [(&str, Change); 6] = [
        ("another channel", |s, t| MidiOutput::store(&mut s.edit, t, Some(MidiOutput { device: "DIN", channel: 1 }))),
        ("another port", |s, t| MidiOutput::store(&mut s.edit, t, Some(MidiOutput { device: "Other", channel: 0 }))),
        ("no output", |s, t| MidiOutput::store(&mut s.edit, t, None)),
        ("mute", |s, t| s.track_mut(t).unwrap().mixer.mute = true),
        ("inactive", |s, t| s.track_mut(t).unwrap().inactive = true),
        ("deleted", |s, t| s.tracks.retain(|track| track.id != t)),
    ];
    for (case, change) in cases {
        let mut s = session();
        let t = add_track(&mut s, TrackKind::Midi, vec![note(60, 100, 0, 9600)], ctrls(), "DIN", 0);
        let sent = after_change(&mut s, |s| change(s, t));
        assert_eq!(sent[..2], [vec![0x80, 60, 0], vec![0xb0, 64, 0]], "{case}: the note, then the pedal: {sent:?}");
        if case == "another channel" {
            // The new channel gets the controllers and program at the play position.
            for m in [vec![0xb1, 64, 127], vec![0xb1, 7, 90], vec![0xc1, 5]] {
                assert!(sent.contains(&m), "{case}: {m:?} chased: {sent:?}");
            }
        } else {
            assert_eq!(sent.len(), 2, "{case}: {sent:?}");
        }
    }
}

#[test]
fn the_pedal_stays_down_while_another_track_still_plays_on_that_channel() {
    let mut s = session();
    let pedal = vec![cc(0, CtrlKind::Cc(64), 127)];
    let t = add_track(&mut s, TrackKind::Midi, vec![note(60, 100, 0, 9600)], pedal, "DIN", 0);
    add_track(&mut s, TrackKind::Instrument, vec![note(48, 100, 0, 9600)], vec![], "DIN", 0);
    let sent = after_change(&mut s, |s| MidiOutput::store(&mut s.edit, t, Some(MidiOutput { device: "DIN", channel: 1 })));
    assert!(sent.contains(&vec![0x80, 60, 0]), "{sent:?}");
    assert!(!sent.contains(&vec![0xb0, 64, 0]), "the other track's notes keep their pedal: {sent:?}");
}

#[test]
fn a_track_routed_here_while_playing_is_chased_like_a_locate() {
    for (case, unmute) in [("routed", false), ("unmuted", true)] {
        let mut s = session();
        add_track(&mut s, TrackKind::Midi, vec![note(60, 100, 0, 9600)], vec![], "DIN", 0);
        let ctrls = vec![cc(0, CtrlKind::Cc(7), 30), cc(48, CtrlKind::Cc(7), 70), cc(0, CtrlKind::Program, 12), cc(9000, CtrlKind::Cc(7), 1)];
        let late = add_track(&mut s, TrackKind::Midi, vec![], ctrls, if unmute { "DIN" } else { "Other" }, 2);
        if unmute {
            s.track_mut(late).unwrap().mixer.mute = true;
        }
        let sent = after_change(&mut s, |s| {
            if unmute {
                s.track_mut(late).unwrap().mixer.mute = false;
            } else {
                MidiOutput::store(&mut s.edit, late, Some(MidiOutput { device: "DIN", channel: 2 }));
            }
        });
        // The values at the play position (50 ms is past tick 48), not the later one.
        assert_eq!(sent, [vec![0xb2, 7, 70], vec![0xc2, 12]], "{case}");
    }
}

#[test]
fn a_timing_loss_before_a_stop_does_not_restart_the_next_pass() {
    // The timing was lost while playing, then the transport stopped before the next update. The
    // next pass starts once: its notes are not cut by a second start one update later.
    let base = Instant::now();
    let mut s = session();
    add_track(&mut s, TrackKind::Midi, vec![note(60, 100, 0, 9600)], vec![], "DIN", 0);
    let mut enc = TrackMidi::new("DIN");
    let mut wire = Wire::new();
    run(&mut enc, &s, rolling(base, 1, 0), Duration::ZERO, Duration::from_millis(100), &mut wire);
    enc.timing_lost();
    enc.update(stopped(base, 1, 4800), base + Duration::from_millis(100), &s, &mut |_, _| {});
    wire.clear();
    let again = base + Duration::from_millis(200);
    run(&mut enc, &s, rolling(again, 2, 0), Duration::ZERO, Duration::from_millis(100), &mut wire);
    assert_eq!(note_ons(&wire).iter().map(|(_, p)| *p).collect::<Vec<_>>(), [60]);
    assert!(!wire.iter().any(|(_, m)| m[0] & 0xf0 == 0x80), "{wire:?}");
}

#[test]
fn a_reconnect_quieting_that_did_not_go_out_is_sent_again_on_the_next_reopen() {
    let base = Instant::now();
    let mut s = session();
    add_track(&mut s, TrackKind::Midi, vec![note(60, 100, 0, 9600)], vec![], "DIN", 2);
    let mut enc = TrackMidi::new("DIN");
    let mut wire = Wire::new();
    run(&mut enc, &s, rolling(base, 1, 0), Duration::ZERO, Duration::from_millis(100), &mut wire);
    enc.update(stopped(base, 1, 4800), base + Duration::from_millis(100), &s, &mut |_, _| {});
    let quieting = || vec![vec![0xb2, 64, 0], vec![0xb2, 123, 0]];
    let reopen = |enc: &mut TrackMidi, at: u64| {
        enc.reset();
        let mut wire = Wire::new();
        enc.update(stopped(base, 1, 4800), base + Duration::from_millis(at), &s, &mut |at, m| wire.push((at, m.to_vec())));
        bytes(&wire)
    };
    // The port failed again while the quieting went out (the worker reports no delivery).
    assert_eq!(reopen(&mut enc, 1200), quieting());
    assert_eq!(reopen(&mut enc, 2400), quieting(), "the next reopen quiets the channel again");
    // This time the port took it: the reopen after that has nothing to make good.
    enc.delivered();
    assert!(reopen(&mut enc, 3600).is_empty());
}

#[test]
fn a_key_struck_by_more_than_255_overlapping_notes_ends_with_its_last_holder() {
    let base = Instant::now();
    let mut s = session();
    let notes = (0..300).map(|i| note(60, 100, 0, 100 + i)).collect();
    add_track(&mut s, TrackKind::Midi, notes, vec![], "DIN", 0);
    let mut enc = TrackMidi::new("DIN");
    let mut wire = Wire::new();
    run(&mut enc, &s, rolling(base, 1, 0), Duration::ZERO, Duration::from_millis(300), &mut wire);
    let offs: Vec<_> = wire.iter().filter(|(_, m)| m[0] == 0x80 && m[1] == 60).map(|(at, _)| *at).collect();
    assert_eq!(offs.len(), 1, "one Note-Off, when the last of the 300 notes ends");
    let last_end = Duration::from_secs_f64((399 * TICK) as f64 / 48_000.);
    assert!(offs[0] >= base + last_end - Duration::from_millis(1), "{:?}", offs[0] - base);
}
