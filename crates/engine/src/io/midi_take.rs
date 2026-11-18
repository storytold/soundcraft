//! MIDI played during a record pass, turned into MIDI clips.

use super::{Clip, Engine, Range, Samples, Session, TrackId, TrackKind};
use soundcraft_midi::{CtrlEvent, CtrlKind, Note, Sequence};
use soundcraft_model::{ClipId, MidiInput};
use std::collections::HashMap;

/// A MIDI message played during a record pass, at its timeline position; or, with the status
/// [`MidiTakeEvent::LOST`], the point where the input queues dropped events.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MidiTakeEvent {
    pub at: Samples,
    /// Index into the take's device names.
    pub device: u8,
    pub status: u8,
    pub data1: u8,
    pub data2: u8,
}

impl MidiTakeEvent {
    /// The `status` of an entry that marks dropped events: events of `device` (of every device
    /// when it is `u8::MAX`) were lost at `at`. Their note-offs may be among them, so the notes
    /// that device held end there.
    pub const LOST: u8 = 0xFF;
}

/// What a record pass made of the MIDI that was played.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MidiTake {
    /// The clips made, one per track that received something.
    pub clips: Vec<ClipId>,
    /// Places within the pass where input was dropped ([`MidiTakeEvent::LOST`]): the take is
    /// incomplete when this is not zero.
    pub gaps: usize,
}

/// Whether a record pass would record MIDI: a record-enabled MIDI or instrument track exists.
pub fn records_midi(e: &Engine) -> bool {
    e.session().tracks.iter().any(records_midi_on)
}

fn records_midi_on(t: &soundcraft_model::Track) -> bool {
    t.mixer.record_arm && !t.inactive && matches!(t.kind, TrackKind::Instrument | TrackKind::Midi)
}

/// The MIDI kinds Setup › MIDI › Input Filter keeps out of recordings.
fn midi_filtered(s: &Session, status: u8) -> bool {
    let kind = match status & 0xF0 {
        0x80 | 0x90 => "notes",
        0xA0 => "poly_aftertouch",
        0xB0 => "cc",
        0xC0 => "program",
        0xD0 => "mono_aftertouch",
        0xE0 => "pitch_bend",
        _ => return true,
    };
    !s.edit.flags.is_empty() && s.edit.flag(&format!("midi.filter.{kind}"))
}

/// Whether a [`MidiTakeEvent::LOST`] entry for `lost` covers `device`.
fn lost_covers(lost: u8, device: u8) -> bool {
    lost == u8::MAX || lost == device
}

/// What `track` recorded of `events` over `range`, in ticks from the start of the range. A key
/// already down when the range starts is left out; one still down at its end ends there, and one
/// whose device lost input ends where that happened. Keys are told apart by device, channel and
/// pitch: the same pitch held on two devices gives two notes.
fn midi_take_sequence(s: &Session, input: &MidiInput<'_>, range: Range, events: &[MidiTakeEvent], devices: &[String]) -> Sequence {
    let sr = s.sample_rate;
    let base = s.tempo.samples_to_ticks(range.start, sr);
    let tick = |at: Samples| s.tempo.samples_to_ticks(at, sr).saturating_sub(base).max(0);
    let end_tick = tick(range.end).max(1);
    let mut seq = Sequence::default();
    // Keys down: (device, channel, pitch) -> (start tick, velocity).
    let mut down: HashMap<(u8, u8, u8), (i64, u8)> = HashMap::new();
    let close = |seq: &mut Sequence, (_, channel, pitch): (u8, u8, u8), (start, velocity): (i64, u8), end: i64, release: u8| {
        seq.notes.push(Note { pitch, velocity, release_velocity: release, channel, start, length: end.saturating_sub(start).max(1) });
    };
    for e in events.iter().take(MAX_MIDI_TAKE_EVENTS) {
        if e.at < range.start || e.at >= range.end {
            continue;
        }
        let at = tick(e.at);
        if e.status == MidiTakeEvent::LOST {
            let ended: Vec<(u8, u8, u8)> = down.keys().copied().filter(|k| lost_covers(e.device, k.0)).collect();
            for key in ended {
                if let Some(open) = down.remove(&key) {
                    close(&mut seq, key, open, at, 64);
                }
            }
            continue;
        }
        let (channel, d1, d2) = (e.status & 0x0F, e.data1 & 0x7F, e.data2 & 0x7F);
        if midi_filtered(s, e.status) || !input.accepts(devices.get(usize::from(e.device)).map(String::as_str), channel) {
            continue;
        }
        let key = (e.device, channel, d1);
        let ctrl = |kind, value| CtrlEvent { tick: at, channel, kind, value };
        match e.status & 0xF0 {
            0x90 if d2 > 0 => {
                // The same key again without a release: the first note ends here.
                if let Some(open) = down.insert(key, (at, d2)) {
                    close(&mut seq, key, open, at, 64);
                }
            }
            0x80 | 0x90 => {
                if let Some(open) = down.remove(&key) {
                    close(&mut seq, key, open, at, if e.status & 0xF0 == 0x80 { d2 } else { 64 });
                }
            }
            0xA0 => seq.ctrls.push(ctrl(CtrlKind::PolyPressure(d1), i32::from(d2))),
            0xB0 => seq.ctrls.push(ctrl(CtrlKind::Cc(d1), i32::from(d2))),
            0xC0 => seq.ctrls.push(ctrl(CtrlKind::Program, i32::from(d1))),
            0xD0 => seq.ctrls.push(ctrl(CtrlKind::ChannelPressure, i32::from(d1))),
            0xE0 => seq.ctrls.push(ctrl(CtrlKind::PitchBend, (i32::from(d2) << 7 | i32::from(d1)) - 8192)),
            _ => {}
        }
    }
    for (key, open) in down {
        close(&mut seq, key, open, end_tick, 64);
    }
    seq.sort();
    seq
}

/// Most events of one MIDI record pass that are kept.
pub const MAX_MIDI_TAKE_EVENTS: usize = 1 << 20;

/// Turn the MIDI played during a record pass into a clip over `range` on every record-enabled
/// MIDI and instrument track, each taking the events of its MIDI input (device and channel).
/// `devices` names the devices by [`MidiTakeEvent::device`]. A track that received nothing gets
/// no clip. One undo step ("Record"); with `same_pass` the clips join the step the audio takes
/// of this pass already made. The result says where input was dropped during the pass.
pub fn add_midi_recording(e: &mut Engine, range: Range, events: &[MidiTakeEvent], devices: &[String], same_pass: bool) -> MidiTake {
    let before = e.session_arc();
    let mut out = MidiTake::default();
    if range.len() <= 0 {
        return out;
    }
    out.gaps =
        events.iter().take(MAX_MIDI_TAKE_EVENTS).filter(|ev| ev.status == MidiTakeEvent::LOST && ev.at >= range.start && ev.at < range.end).count()
            + usize::from(events.len() > MAX_MIDI_TAKE_EVENTS);
    let takes: Vec<(TrackId, String, Sequence)> = {
        let s = e.session();
        s.tracks
            .iter()
            .filter(|t| records_midi_on(t))
            .map(|t| (t.id, t.name.clone(), midi_take_sequence(s, &MidiInput::of(&t.mixer.input), range, events, devices)))
            .filter(|(_, _, seq)| !seq.notes.is_empty() || !seq.ctrls.is_empty())
            .collect()
    };
    for (tid, name, seq) in takes {
        let s = e.session_mut();
        let cid = s.new_clip_id();
        crate::edit::place_clip(s, tid, Clip::midi(cid, name, range.start, range.len(), seq));
        out.clips.push(cid);
    }
    if !out.clips.is_empty() && !same_pass {
        e.push_undo("Record", before);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use soundcraft_model::ClipContent;

    fn armed(e: &mut Engine, name: &str, input: &str) -> TrackId {
        let id = e.execute("track.new", &json!({"name": name, "kind": "instrument"})).unwrap()["tracks"][0].as_u64().unwrap();
        e.execute("track.input", &json!({"tracks": [id], "input": input})).unwrap();
        e.execute("mix.record_arm", &json!({"tracks": [id], "value": true})).unwrap();
        TrackId(id)
    }

    fn ev(at: Samples, device: u8, status: u8, data1: u8, data2: u8) -> MidiTakeEvent {
        MidiTakeEvent { at, device, status, data1, data2 }
    }

    fn sequence(e: &Engine, t: TrackId, range: Range) -> Sequence {
        let clip = e.session().track(t).unwrap().clips().first().unwrap().clone();
        assert_eq!(clip.range(), range);
        let ClipContent::Midi { sequence } = clip.content else { panic!("not a MIDI clip") };
        sequence
    }

    #[test]
    fn midi_played_during_a_pass_becomes_a_clip_on_each_record_enabled_midi_track() {
        let mut e = Engine::default();
        let all = armed(&mut e, "Keys", "none");
        let pad = armed(&mut e, "Pad", "MIDI:10:Pad");
        let idle = armed(&mut e, "Idle", "MIDI:*:Nothing");
        assert_eq!(e.session().busses.len(), 0, "a MIDI input is not a bus name");
        assert!(records_midi(&e));
        let events = [
            ev(40_000, 0, 0x90, 59, 80),  // before the pass: left out, and so is its release
            ev(48_000, 0, 0x90, 60, 100), // one beat at 120 bpm, 48 kHz: 24 000 samples = 960 ticks
            ev(50_000, 0, 0x80, 59, 0),
            ev(60_000, 1, 0x99, 36, 127),
            ev(72_000, 0, 0x80, 60, 30),
            ev(72_000, 0, 0xE0, 0, 96),
            ev(84_000, 1, 0x99, 36, 0),
            ev(90_000, 0, 0xB0, 64, 127),
            ev(95_000, 0, 0x90, 67, 90), // still down at the end
            ev(96_000, 0, 0x90, 72, 90), // after the pass
        ];
        let devices = ["Keys".to_string(), "Pad".to_string()];
        let range = Range::new(48_000, 96_000);
        let before = e.session().tracks.iter().map(|t| t.clips().len()).sum::<usize>();
        let take = add_midi_recording(&mut e, range, &events, &devices, false);
        assert_eq!((before, take.clips.len(), take.gaps), (0, 2, 0), "the track whose device was silent gets no clip");
        let keys = sequence(&e, all, range);
        let notes: Vec<(u8, u8, i64, i64, u8)> = keys.notes.iter().map(|n| (n.channel, n.pitch, n.start, n.length, n.release_velocity)).collect();
        assert_eq!(notes, [(0, 60, 0, 960, 30), (9, 36, 480, 960, 64), (0, 67, 1880, 40, 64)]);
        assert_eq!(
            keys.ctrls.iter().map(|c| (c.tick, c.kind, c.value)).collect::<Vec<_>>(),
            [(960, CtrlKind::PitchBend, 4096), (1680, CtrlKind::Cc(64), 127)]
        );
        let pads = sequence(&e, pad, range);
        assert_eq!((pads.notes.len(), pads.notes[0].pitch, pads.ctrls.len()), (1, 36, 0), "only its device and channel");
        assert!(e.session().track(idle).unwrap().clips().is_empty());
        // One undo step for the pass; the input filter keeps kinds out.
        e.run("edit.undo").unwrap();
        assert_eq!(e.session().tracks.iter().map(|t| t.clips().len()).sum::<usize>(), 0);
        e.execute("setup.midi_input_filter", &json!({"filter": ["pitch_bend", "cc"]})).unwrap();
        add_midi_recording(&mut e, range, &events, &devices, false);
        assert!(sequence(&e, all, range).ctrls.is_empty());
        assert!(add_midi_recording(&mut e, Range::new(5, 5), &events, &devices, false).clips.is_empty());
    }

    #[test]
    fn the_same_pitch_on_two_devices_is_two_notes_and_lost_input_ends_only_its_device() {
        let mut e = Engine::default();
        let all = armed(&mut e, "Keys", "none");
        let range = Range::new(0, 96_000);
        let events = [
            ev(0, 0, 0x90, 60, 100),
            ev(12_000, 1, 0x90, 60, 90), // the same pitch and channel from another device
            ev(24_000, 0, 0x80, 60, 0),  // ends the first device's note only
            ev(24_000, 0, 0x90, 64, 100),
            ev(36_000, 0, MidiTakeEvent::LOST, 0, 0), // device 0 lost input: its note-off never comes
            ev(48_000, 1, 0x80, 60, 0),
            ev(60_000, 1, 0x90, 67, 100),
            ev(72_000, u8::MAX, MidiTakeEvent::LOST, 0, 0), // every device
            ev(200_000, 0, MidiTakeEvent::LOST, 0, 0),      // after the pass
        ];
        let take = add_midi_recording(&mut e, range, &events, &["A".to_string(), "B".to_string()], false);
        assert_eq!((take.clips.len(), take.gaps), (1, 2), "the take says it is incomplete");
        let notes: Vec<(u8, i64, i64)> = sequence(&e, all, range).notes.iter().map(|n| (n.pitch, n.start, n.length)).collect();
        assert_eq!(notes, [(60, 0, 960), (60, 480, 1440), (64, 960, 480), (67, 2400, 480)]);
    }
    /// `(channel, pitch, start, length, velocity, release velocity)` of every note.
    fn notes_of(seq: &Sequence) -> Vec<(u8, u8, i64, i64, u8, u8)> {
        seq.notes.iter().map(|n| (n.channel, n.pitch, n.start, n.length, n.velocity, n.release_velocity)).collect()
    }

    #[test]
    fn note_edge_cases_make_the_notes_a_player_would_expect() {
        // 120 bpm at 48 kHz: a quarter note is 24 000 samples and 960 ticks.
        let s = Session::default();
        let range = Range::new(0, 96_000);
        let q = 24_000;
        let cases: [(&str, Vec<MidiTakeEvent>, Vec<(u8, u8, i64, i64, u8, u8)>); 7] = [
            (
                "the same key again without a release: the first note ends where the second starts",
                vec![ev(0, 0, 0x90, 60, 100), ev(q, 0, 0x90, 60, 90), ev(2 * q, 0, 0x80, 60, 0)],
                vec![(0, 60, 0, 960, 100, 64), (0, 60, 960, 960, 90, 0)],
            ),
            ("a note-on with velocity 0 is a release", vec![ev(0, 0, 0x90, 60, 100), ev(q, 0, 0x90, 60, 0)], vec![(0, 60, 0, 960, 100, 64)]),
            ("a note-off keeps its release velocity", vec![ev(0, 0, 0x90, 60, 100), ev(q, 0, 0x80, 60, 30)], vec![(0, 60, 0, 960, 100, 30)]),
            (
                "data bytes above 127 are masked",
                vec![ev(0, 0, 0x91, 0x80 | 60, 0x80 | 100), ev(q, 0, 0x81, 0x80 | 60, 0xFF)],
                vec![(1, 60, 0, 960, 100, 127)],
            ),
            ("a key still down at the end of the pass ends there", vec![ev(q, 0, 0x90, 60, 100)], vec![(0, 60, 960, 2880, 100, 64)]),
            ("a release without a key down is nothing", vec![ev(q, 0, 0x80, 60, 0), ev(q, 0, 0x90, 61, 0)], vec![]),
            (
                "a note on and off at the same sample is one tick long, not empty",
                vec![ev(q, 0, 0x90, 60, 100), ev(q, 0, 0x80, 60, 0)],
                vec![(0, 60, 960, 1, 100, 0)],
            ),
        ];
        for (what, events, want) in cases {
            let seq = midi_take_sequence(&s, &MidiInput::default(), range, &events, &[]);
            assert_eq!(notes_of(&seq), want, "{what}");
            assert!(seq.notes.iter().all(|n| n.length > 0 && n.pitch < 128 && n.velocity < 128 && n.channel < 16), "{what}");
        }
        // Controller data is masked too: a pitch bend of two 0xFF bytes is full up, not more.
        let seq = midi_take_sequence(&s, &MidiInput::default(), range, &[ev(0, 0, 0xE0, 0xFF, 0xFF), ev(0, 0, 0xB0, 0x87, 0xFF)], &[]);
        assert_eq!(seq.ctrls.iter().map(|c| (c.kind, c.value)).collect::<Vec<_>>(), [(CtrlKind::PitchBend, 8191), (CtrlKind::Cc(7), 127)]);
    }

    #[test]
    fn notes_keep_their_musical_place_at_any_tempo_and_sample_rate() {
        for (hz, bpm) in [(44_100u32, 120.0), (48_000, 90.0), (96_000, 60.0), (192_000, 174.5)] {
            let mut s = Session::new("t", soundcraft_time::SampleRate::new(hz).unwrap());
            s.tempo.set_tempo(0, bpm).unwrap();
            // The pass starts one quarter note in and lasts four.
            let q = (f64::from(hz) * 60.0 / bpm).round() as i64;
            let range = Range::new(q, 5 * q);
            let events = [
                ev(2 * q, 0, 0x90, 60, 100), // held to the end of the pass
                ev(3 * q, 0, 0x90, 64, 100),
                ev(4 * q, 0, 0x80, 64, 0),
            ];
            let seq = midi_take_sequence(&s, &MidiInput::default(), range, &events, &[]);
            let near = |got: i64, want: i64| (got - want).abs() <= 1;
            let n = notes_of(&seq);
            assert_eq!(n.len(), 2, "{hz} Hz, {bpm} bpm");
            assert!(near(n[0].2, 960) && near(n[0].3, 2880), "{hz} Hz, {bpm} bpm: the held note ends with the pass: {n:?}");
            assert!(near(n[1].2, 1920) && near(n[1].3, 960), "{hz} Hz, {bpm} bpm: {n:?}");
        }
    }

    #[test]
    fn each_input_filter_kind_keeps_its_own_messages_out_of_the_take() {
        let events = [
            ev(100, 0, 0x90, 60, 100),
            ev(200, 0, 0x80, 60, 0),
            ev(300, 0, 0xA0, 60, 50),
            ev(400, 0, 0xB0, 1, 60),
            ev(500, 0, 0xC0, 5, 0),
            ev(600, 0, 0xD0, 70, 0),
            ev(700, 0, 0xE0, 0, 80),
        ];
        let range = Range::new(0, 48_000);
        // What is left of (notes, poly pressure, controllers, programs, channel pressure, bends).
        let kept = |filter: &[&str]| {
            let mut e = Engine::default();
            let t = armed(&mut e, "Keys", "none");
            e.execute("setup.midi_input_filter", &json!({"filter": filter})).unwrap();
            if add_midi_recording(&mut e, range, &events, &[], false).clips.is_empty() {
                return [0; 6];
            }
            let seq = sequence(&e, t, range);
            let ctrls = |f: &dyn Fn(&CtrlKind) -> bool| seq.ctrls.iter().filter(|c| f(&c.kind)).count();
            [
                seq.notes.len(),
                ctrls(&|k| matches!(k, CtrlKind::PolyPressure(_))),
                ctrls(&|k| matches!(k, CtrlKind::Cc(_))),
                ctrls(&|k| matches!(k, CtrlKind::Program)),
                ctrls(&|k| matches!(k, CtrlKind::ChannelPressure)),
                ctrls(&|k| matches!(k, CtrlKind::PitchBend)),
            ]
        };
        assert_eq!(kept(&[]), [1; 6]);
        for (i, kind) in ["notes", "poly_aftertouch", "cc", "program", "mono_aftertouch", "pitch_bend"].into_iter().enumerate() {
            let mut want = [1; 6];
            want[i] = 0;
            assert_eq!(kept(&[kind]), want, "{kind}");
        }
        assert_eq!(kept(&["notes", "poly_aftertouch", "cc", "program", "mono_aftertouch", "pitch_bend"]), [0; 6], "nothing left: no clip");
        assert_eq!(kept(&["sysex", "realtime"]), [1; 6], "kinds that never reach a take change nothing");
    }

    #[test]
    fn midi_clips_join_the_undo_step_of_the_audio_takes_of_the_same_pass() {
        let mut e = Engine::default();
        let t = armed(&mut e, "Keys", "none");
        let range = Range::new(0, 48_000);
        let events = [ev(100, 0, 0x90, 60, 100), ev(200, 0, 0x80, 60, 0)];
        let clips = |e: &Engine| e.session().track(t).unwrap().clips().len();
        // On its own the take is one undo step called Record.
        let before = e.undo_label().map(str::to_string);
        assert_eq!(add_midi_recording(&mut e, range, &events, &[], false).clips.len(), 1);
        assert_eq!(e.undo_label(), Some("Record"));
        e.run("edit.undo").unwrap();
        assert_eq!((clips(&e), e.undo_label().map(str::to_string)), (0, before.clone()));
        // With audio takes of the same pass, it adds no step of its own: theirs covers it.
        assert_eq!(add_midi_recording(&mut e, range, &events, &[], true).clips.len(), 1);
        assert_eq!((clips(&e), e.undo_label().map(str::to_string)), (1, before));
        // A take that makes no clip makes no step either.
        let mut e = Engine::default();
        armed(&mut e, "Keys", "MIDI:*:Nothing");
        let before = e.undo_label().map(str::to_string);
        assert!(add_midi_recording(&mut e, range, &events, &["Keys".to_string()], false).clips.is_empty());
        assert_eq!(e.undo_label().map(str::to_string), before);
    }

    #[test]
    fn a_device_index_without_a_name_reaches_only_tracks_that_take_every_device() {
        let mut e = Engine::default();
        let all = armed(&mut e, "All", "none");
        let keys = armed(&mut e, "Keys", "MIDI:*:Keys");
        let channel = armed(&mut e, "Ch 3", "MIDI:3:*");
        let range = Range::new(0, 48_000);
        // Device 200 is past the name list; `u8::MAX` is a port that never got an index.
        let events = [ev(100, 200, 0x92, 60, 100), ev(200, 200, 0x82, 60, 0), ev(300, u8::MAX, 0x90, 62, 100), ev(400, u8::MAX, 0x80, 62, 0)];
        let take = add_midi_recording(&mut e, range, &events, &["Keys".to_string()], false);
        assert_eq!(take.clips.len(), 2);
        assert_eq!(notes_of(&sequence(&e, all, range)).len(), 2);
        assert_eq!(notes_of(&sequence(&e, channel, range)).iter().map(|n| n.1).collect::<Vec<_>>(), [60], "its channel, any device");
        assert!(e.session().track(keys).unwrap().clips().is_empty(), "not the named device");
    }

    #[test]
    fn events_past_the_limit_are_left_out_and_the_take_is_reported_incomplete() {
        let mut e = Engine::default();
        let t = armed(&mut e, "Keys", "none");
        let range = Range::new(0, 48_000);
        // The limit's worth of events from before the pass, then two notes: one inside the limit…
        let mut events = vec![ev(-1, 0, 0xB0, 1, 0); MAX_MIDI_TAKE_EVENTS - 2];
        events.extend([ev(100, 0, 0x90, 60, 100), ev(200, 0, 0x80, 60, 0)]);
        let take = add_midi_recording(&mut e, range, &events, &[], false);
        assert_eq!((take.clips.len(), take.gaps), (1, 0), "exactly the limit is complete");
        e.run("edit.undo").unwrap();
        // …and one past it.
        events.extend([ev(300, 0, 0x90, 72, 100), ev(400, 0, 0x80, 72, 0)]);
        let take = add_midi_recording(&mut e, range, &events, &[], false);
        assert_eq!((take.clips.len(), take.gaps), (1, 1), "the cut is reported");
        assert_eq!(notes_of(&sequence(&e, t, range)).iter().map(|n| n.1).collect::<Vec<_>>(), [60]);
    }
}
