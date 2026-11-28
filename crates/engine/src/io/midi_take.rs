//! MIDI played during a record pass, turned into MIDI clips.

use super::{Clip, Engine, Range, Samples, Session, TrackId, TrackKind};
use soundcraft_midi::{CtrlEvent, CtrlKind, Note, Sequence};
use soundcraft_model::{ClipId, MidiInput, Route};
use std::collections::HashMap;

/// A MIDI message played during a record pass, at its timeline position; or, with the status
/// [`MidiTakeEvent::LOST`], the point where the input queues dropped events; or, with
/// [`MidiTakeEvent::WRAP`], the point where loop recording went back to the start of the loop.
/// Entries are in the order they were played.
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

    /// The `status` of an entry that says loop recording went round: the entries after it were
    /// played in the next round of the loop. A key held across it sounds to the end of the loop
    /// and on from its start, also when its release is later on the timeline than its press.
    pub const WRAP: u8 = 0xFE;
}

/// How a record pass went over its range.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MidiPass {
    /// Once, from its start to its end. Positions only go forward: a release that is stamped
    /// earlier than its key press (the audio callback ran late) ends the note at the press.
    #[default]
    Linear,
    /// Loop recording: over the range again and again. `stopped` is where inside the loop the
    /// pass stopped: the keys still down end there. When that is not known they count as held
    /// to the end of the range. `marked`: the take has a [`MidiTakeEvent::WRAP`] entry at every
    /// round, as the audio engine writes them, and only those say that the loop went round (a
    /// release stamped before its press in one round is a late callback, as in a linear pass).
    /// A take without those entries is not `marked`: there a release earlier on the timeline
    /// than its key press is read as the loop having gone round.
    Loop { stopped: Option<Samples>, marked: bool },
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
/// pitch: the same pitch held on two devices gives two notes. A key held while loop recording
/// goes round ([`MidiTakeEvent::WRAP`]) is two notes: to the end of the loop, and from its start;
/// that holds for a key still down when the pass stops too ([`MidiPass::Loop`]). In a pass that
/// did not loop ([`MidiPass::Linear`]) nothing is read as the loop going round.
fn midi_take_sequence(s: &Session, input: &MidiInput<'_>, range: Range, pass: MidiPass, events: &[MidiTakeEvent], devices: &[String]) -> Sequence {
    let sr = s.sample_rate;
    let base = s.tempo.samples_to_ticks(range.start, sr);
    let tick = |at: Samples| s.tempo.samples_to_ticks(at, sr).saturating_sub(base).max(0);
    let end_tick = tick(range.end).max(1);
    let looped = matches!(pass, MidiPass::Loop { .. });
    // Whether a release earlier on the timeline than its press says that the loop went round.
    let inferred = matches!(pass, MidiPass::Loop { marked: false, .. });
    // The last tick a note can start at and still be played: a key pressed within the last
    // samples of the range may round to the tick of its end, where the clip no longer plays.
    let last_start = end_tick.saturating_sub(1).max(0);
    let mut seq = Sequence::default();
    // The round of the loop being played: it goes up at every `WRAP`.
    let mut round = 0u32;
    // Keys down: (device, channel, pitch) -> (start tick, velocity, round, the sample the key was
    // pressed at). Whether the loop went round between a press and its release is judged by
    // their samples: two positions a few samples apart may share a tick, and a press in the
    // last samples of the range starts its note a tick earlier so that it plays.
    let mut down: HashMap<(u8, u8, u8), (i64, u8, u32, Samples)> = HashMap::new();
    let close = |seq: &mut Sequence,
                 (_, channel, pitch): (u8, u8, u8),
                 (start, velocity, since, pressed): (i64, u8, u32, Samples),
                 end: i64,
                 release: u8,
                 round: u32,
                 end_at: Samples| {
        let note = |start: i64, end: i64, release: u8| Note {
            pitch,
            velocity,
            release_velocity: release,
            channel,
            start,
            length: end.saturating_sub(start).max(1),
        };
        let rounds = round.saturating_sub(since);
        // A pass in a line never goes round, and neither does a loop pass between two
        // entries of one round when its rounds are marked: a release stamped before its
        // press ends the note at the press (a note is at least a tick long).
        if !looped || (rounds == 0 && (end_at >= pressed || !inferred)) {
            seq.notes.push(note(start, end, release));
            return;
        }
        // The pass went round between the key press and the release (loop recording goes back to
        // the start of the loop): the capture said so, or the release is earlier on the timeline
        // than the press. The note is held to the end of the pass and goes on from its start, as
        // it was played. Held for a whole round or more, it covers the loop: the second note
        // then ends where the first begins, so the two do not overlap.
        seq.notes.push(note(start, end_tick, 64));
        let end = if rounds > 1 || (rounds == 1 && end_at >= pressed) { start } else { end };
        if end > 0 {
            seq.notes.push(note(0, end, release));
        }
    };
    for e in events.iter().take(MAX_MIDI_TAKE_EVENTS) {
        // The mark sits at the end of the loop, which is the end of the range.
        if e.status == MidiTakeEvent::WRAP {
            round = round.saturating_add(1);
            continue;
        }
        if e.at < range.start || e.at >= range.end {
            continue;
        }
        let at = tick(e.at);
        if e.status == MidiTakeEvent::LOST {
            let ended: Vec<(u8, u8, u8)> = down.keys().copied().filter(|k| lost_covers(e.device, k.0)).collect();
            for key in ended {
                if let Some(open) = down.remove(&key) {
                    close(&mut seq, key, open, at, 64, round, e.at);
                }
            }
            continue;
        }
        let (channel, d1, d2) = (e.status & 0x0F, e.data1 & 0x7F, e.data2 & 0x7F);
        if midi_filtered(s, e.status) || !input.accepts(devices.get(usize::from(e.device)).map(String::as_str), channel) {
            continue;
        }
        let key = (e.device, channel, d1);
        // Like a note start, a controller in the last samples of the range stays where it plays.
        let ctrl = |kind, value| CtrlEvent { tick: at.min(last_start), channel, kind, value };
        match e.status & 0xF0 {
            0x90 if d2 > 0 => {
                // The same key again without a release: the first note ends here.
                if let Some(open) = down.insert(key, (at.min(last_start), d2, round, e.at)) {
                    close(&mut seq, key, open, at, 64, round, e.at);
                }
            }
            0x80 | 0x90 => {
                if let Some(open) = down.remove(&key) {
                    close(&mut seq, key, open, at, if e.status & 0xF0 == 0x80 { d2 } else { 64 }, round, e.at);
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
    // Still down when the pass stopped. A loop pass stops somewhere inside the loop: the keys end
    // there, in the round the stop is in. Otherwise, or when nothing says where it stopped, they
    // are held to the end of the range, in the round that was playing (after a round or more,
    // that is the whole loop).
    // A take cut at the event cap may have lost the marks of its last rounds: the stop position
    // belongs to a round the entries that are left do not reach, and is not used.
    let stop_at = match pass {
        MidiPass::Loop { stopped: Some(at), .. } if events.len() <= MAX_MIDI_TAKE_EVENTS => Some(at.clamp(range.start, range.end)),
        _ => None,
    };
    for (key, open) in down {
        close(&mut seq, key, open, stop_at.map_or(end_tick, tick), 64, round, stop_at.unwrap_or(range.end));
    }
    seq.sort();
    seq
}

/// Most events of one MIDI record pass that are kept.
pub const MAX_MIDI_TAKE_EVENTS: usize = 1 << 20;

/// Turn the MIDI played during a record pass that went once over `range` into a clip on every
/// MIDI and instrument track that is record-enabled now, each taking the events of its MIDI input
/// (device and channel). `devices` names the devices by [`MidiTakeEvent::device`]. A track that
/// received nothing gets no clip. One undo step ("Record"); with `same_pass` the clips join the
/// step the audio takes of this pass already made. The result says where input was dropped
/// during the pass.
pub fn add_midi_recording(e: &mut Engine, range: Range, events: &[MidiTakeEvent], devices: &[String], same_pass: bool) -> MidiTake {
    let inputs = midi_record_inputs(e);
    add_midi_recording_from(e, range, MidiPass::Linear, events, devices, &inputs, same_pass)
}

/// The MIDI inputs of the record-enabled MIDI and instrument tracks, to keep from the start of a
/// record pass and hand to [`add_midi_recording_from`] at its end.
pub fn midi_record_inputs(e: &Engine) -> Vec<(TrackId, Route)> {
    e.session().tracks.iter().filter(|t| records_midi_on(t)).map(|t| (t.id, t.mixer.input.clone())).collect()
}

/// [`add_midi_recording`] for the tracks that were in record when the pass started, with the MIDI
/// inputs they had then (`inputs`, from [`midi_record_inputs`]), and for a pass that went over
/// `range` as `pass` says. What a pass records is settled when it starts: a track listed takes
/// the events of that input for the whole pass, so changing its device or channel while recording
/// does not throw away what it already recorded, and neither does taking it out of record. A
/// track that is not listed gets nothing, also when it was record-enabled during the pass: it
/// would get what was played before that, and the caller that waits for the end of the take
/// decides whether to wait by this same list.
pub fn add_midi_recording_from(
    e: &mut Engine,
    range: Range,
    pass: MidiPass,
    events: &[MidiTakeEvent],
    devices: &[String],
    inputs: &[(TrackId, Route)],
    same_pass: bool,
) -> MidiTake {
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
            .filter_map(|t| {
                let (_, route) = inputs.iter().find(|(id, _)| *id == t.id)?;
                Some((t.id, t.name.clone(), midi_take_sequence(s, &MidiInput::of(route), range, pass, events, devices)))
            })
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
            let seq = midi_take_sequence(&s, &MidiInput::default(), range, MidiPass::Linear, &events, &[]);
            assert_eq!(notes_of(&seq), want, "{what}");
            assert!(seq.notes.iter().all(|n| n.length > 0 && n.pitch < 128 && n.velocity < 128 && n.channel < 16), "{what}");
        }
        // Controller data is masked too: a pitch bend of two 0xFF bytes is full up, not more.
        let seq =
            midi_take_sequence(&s, &MidiInput::default(), range, MidiPass::Linear, &[ev(0, 0, 0xE0, 0xFF, 0xFF), ev(0, 0, 0xB0, 0x87, 0xFF)], &[]);
        assert_eq!(seq.ctrls.iter().map(|c| (c.kind, c.value)).collect::<Vec<_>>(), [(CtrlKind::PitchBend, 8191), (CtrlKind::Cc(7), 127)]);
    }

    #[test]
    fn a_note_held_across_the_loop_point_is_split_at_it() {
        // A loop of four quarter notes (3840 ticks), recorded over several times: the timeline
        // position goes back to the start, so a release can be earlier than its key press.
        let s = Session::default();
        let (q, range) = (24_000, Range::new(0, 96_000));
        let events = [
            ev(q / 2, 0, 0x90, 64, 100), // within one time round: an ordinary note
            ev(q, 0, 0x80, 64, 0),
            ev(3 * q + q / 2, 0, 0x90, 60, 100), // pressed half a quarter before the loop point…
            ev(q / 2, 0, 0x80, 60, 40),          // …released half a quarter after it
            ev(3 * q, 0, 0x90, 67, 100),         // pressed, then pressed again after the loop point
            ev(q, 0, 0x90, 67, 90),
            ev(2 * q, 0, 0x80, 67, 0),
            ev(3 * q, 1, 0x90, 72, 100), // its device loses input after the loop point
            ev(q, 1, MidiTakeEvent::LOST, 0, 0),
            ev(3 * q, 0, 0x90, 48, 100), // released exactly at the loop start: nothing after it
            ev(0, 0, 0x80, 48, 0),
        ];
        let seq = midi_take_sequence(&s, &MidiInput::default(), range, MidiPass::Loop { stopped: None, marked: false }, &events, &[]);
        let mut got = notes_of(&seq);
        got.sort_by_key(|n| (n.1, n.2));
        let want = [
            (0, 48, 2880, 960, 100, 64),
            (0, 60, 0, 480, 100, 40),    // from the loop start to the release
            (0, 60, 3360, 480, 100, 64), // from the key press to the loop point
            (0, 64, 480, 480, 100, 0),
            (0, 67, 0, 960, 100, 64), // the first press, carried over the loop point…
            (0, 67, 960, 960, 90, 0), // …ends where the key is pressed again
            (0, 67, 2880, 960, 100, 64),
            (0, 72, 0, 960, 100, 64),
            (0, 72, 2880, 960, 100, 64),
        ];
        assert_eq!(got, want);
        assert!(seq.notes.iter().all(|n| n.start >= 0 && n.start + n.length <= 3840), "every note is inside the loop");
    }

    #[test]
    fn a_note_held_into_the_next_round_is_split_even_when_its_release_is_later_on_the_timeline() {
        // The same loop. The capture says where it went round; positions alone cannot.
        let s = Session::default();
        let (q, range) = (24_000, Range::new(0, 96_000));
        let wrap = ev(4 * q, u8::MAX, MidiTakeEvent::WRAP, 0, 0);
        let events = [
            ev(q / 2, 0, 0x90, 60, 100), // pressed at half a quarter in the first round…
            ev(q, 0, 0x90, 64, 100),     // (an ordinary note within the first round)
            ev(2 * q, 0, 0x80, 64, 0),
            ev(3 * q, 0, 0x90, 67, 100), // (held over two rounds)
            wrap,
            ev(q, 0, 0x80, 60, 40),      // …released at one quarter in the second
            ev(2 * q, 0, 0x90, 72, 100), // an ordinary note within the second round
            ev(3 * q, 0, 0x80, 72, 0),
            wrap,
            ev(q, 0, 0x80, 67, 0),
            ev(2 * q, 0, 0x90, 48, 100), // still down when the pass ends: to the end of the loop
        ];
        let held = events;
        let seq = midi_take_sequence(&s, &MidiInput::default(), range, MidiPass::Loop { stopped: None, marked: true }, &events, &[]);
        let mut got = notes_of(&seq);
        got.sort_by_key(|n| (n.1, n.2));
        let want = [
            (0, 48, 1920, 1920, 100, 64),
            (0, 60, 0, 480, 100, 40),    // from the loop start: up to its own start, so the two
            (0, 60, 480, 3360, 100, 64), // parts cover what was held and do not overlap
            (0, 64, 960, 960, 100, 0),
            (0, 67, 0, 2880, 100, 0), // held for more than a round: the whole loop
            (0, 67, 2880, 960, 100, 64),
            (0, 72, 1920, 960, 100, 0),
        ];
        assert_eq!(got, want);
        assert!(seq.notes.iter().all(|n| n.start >= 0 && n.start + n.length <= 3840), "every note is inside the loop");
        // Keys still down when the pass stops, after the loop went round. One was pressed at half
        // a quarter in the first round, one at three quarters in the second.
        let keys = [ev(q / 2, 0, 0x90, 60, 100), wrap, ev(3 * q, 0, 0x90, 64, 100), wrap];
        let stopped = |at: i64| {
            let mut got =
                notes_of(&midi_take_sequence(&s, &MidiInput::default(), range, MidiPass::Loop { stopped: Some(at), marked: true }, &keys, &[]));
            got.sort_by_key(|n| (n.1, n.2));
            got.iter().map(|n| (n.1, n.2, n.3)).collect::<Vec<_>>()
        };
        // Stopped at one quarter in the third round: the first key was held for two rounds and
        // covers the loop; the second went round once and is held from the loop start to the stop.
        assert_eq!(stopped(q), [(60, 0, 480), (60, 480, 3360), (64, 0, 960), (64, 2880, 960)]);
        // Stopped past both presses: both cover the loop, without overlapping themselves.
        assert_eq!(stopped(3 * q + q / 2), [(60, 0, 480), (60, 480, 3360), (64, 0, 2880), (64, 2880, 960)]);
        // Without the stop position a key still down counts as held to the end of the loop.
        let got = notes_of(&midi_take_sequence(&s, &MidiInput::default(), range, MidiPass::Loop { stopped: None, marked: true }, &keys, &[]));
        assert_eq!(got.len(), 4, "{got:?}");
        // In one round a stop ends the key where it is, like the end of the range does.
        let one_round = [ev(q, 0, 0x90, 60, 100)];
        let got =
            notes_of(&midi_take_sequence(&s, &MidiInput::default(), range, MidiPass::Loop { stopped: Some(2 * q), marked: true }, &one_round, &[]));
        assert_eq!(got.iter().map(|n| (n.2, n.3)).collect::<Vec<_>>(), [(960, 960)]);
        let events = held;
        // The marks are not gaps: the take is complete.
        let mut e = Engine::default();
        armed(&mut e, "Keys", "none");
        assert_eq!(add_midi_recording(&mut e, range, &events, &[], false).gaps, 0);
    }

    #[test]
    fn a_release_stamped_before_its_press_in_a_pass_that_did_not_loop_is_one_short_note() {
        // The audio callback ran late: the key press was written down at 1 024, and the release,
        // taken by a later callback, at 672. Nothing went round.
        let s = Session::default();
        let range = Range::new(0, 96_000);
        let events = [ev(24_000, 0, 0x90, 60, 100), ev(23_000, 0, 0x80, 60, 40), ev(48_000, 0, 0x90, 64, 100), ev(72_000, 0, 0x80, 64, 0)];
        let got = notes_of(&midi_take_sequence(&s, &MidiInput::default(), range, MidiPass::Linear, &events, &[]));
        assert_eq!(got, [(0, 60, 960, 1, 100, 40), (0, 64, 1920, 960, 100, 0)], "one note at the press, not two around the clip's ends");
        // The same entries from a loop take without marks are a note held across the loop point.
        let got = notes_of(&midi_take_sequence(&s, &MidiInput::default(), range, MidiPass::Loop { stopped: None, marked: false }, &events, &[]));
        assert_eq!(got.iter().filter(|n| n.1 == 60).count(), 2);
        // In a loop take with marks no mark lies between the two: a late callback again.
        let got = notes_of(&midi_take_sequence(&s, &MidiInput::default(), range, MidiPass::Loop { stopped: None, marked: true }, &events, &[]));
        assert_eq!(got, [(0, 60, 960, 1, 100, 40), (0, 64, 1920, 960, 100, 0)]);
        // A key pressed in the last sample of the range starts at the last tick that plays.
        let unmarked = MidiPass::Loop { stopped: None, marked: false };
        // In a take without marks, a key pressed there and released a tick's worth earlier in
        // the next round went round the loop: judged by where it was pressed, not where it starts.
        let round = [ev(95_999, 0, 0x90, 60, 100), ev(95_975, 0, 0x80, 60, 0)];
        let got = notes_of(&midi_take_sequence(&s, &MidiInput::default(), range, unmarked, &round, &[]));
        assert_eq!(got.iter().map(|n| (n.2, n.3)).collect::<Vec<_>>(), [(0, 3839), (3839, 1)], "the tail and the round after it");
        // Two positions within one tick: a release 22 samples before the press went round too,
        // and a release 22 samples after it is one short note.
        let near = [ev(24_011, 0, 0x90, 60, 100), ev(23_989, 0, 0x80, 60, 0)];
        let got = notes_of(&midi_take_sequence(&s, &MidiInput::default(), range, unmarked, &near, &[]));
        assert_eq!(got.iter().map(|n| (n.2, n.3)).collect::<Vec<_>>(), [(0, 960), (960, 2880)]);
        let forward = [ev(23_989, 0, 0x90, 60, 100), ev(24_011, 0, 0x80, 60, 0)];
        let got = notes_of(&midi_take_sequence(&s, &MidiInput::default(), range, unmarked, &forward, &[]));
        assert_eq!(got.iter().map(|n| (n.2, n.3)).collect::<Vec<_>>(), [(960, 1)]);
        // A controller there stays inside the clip too.
        let cc = [ev(95_999, 0, 0xB0, 7, 90)];
        let seq = midi_take_sequence(&s, &MidiInput::default(), range, MidiPass::Linear, &cc, &[]);
        assert_eq!(seq.ctrls.iter().map(|c| c.tick).collect::<Vec<_>>(), [3839]);
        let late = [ev(95_999, 0, 0x90, 60, 100)];
        let got = notes_of(&midi_take_sequence(&s, &MidiInput::default(), range, MidiPass::Linear, &late, &[]));
        assert_eq!(got.iter().map(|n| (n.2, n.3)).collect::<Vec<_>>(), [(3839, 1)], "not at the tick of the clip's end");
        // A mark of the loop going round means nothing in a pass that did not loop.
        let marked = [ev(24_000, 0, 0x90, 60, 100), ev(96_000, u8::MAX, MidiTakeEvent::WRAP, 0, 0), ev(48_000, 0, 0x80, 60, 0)];
        let got = notes_of(&midi_take_sequence(&s, &MidiInput::default(), range, MidiPass::Linear, &marked, &[]));
        assert_eq!(got.iter().map(|n| (n.2, n.3)).collect::<Vec<_>>(), [(960, 960)]);
    }

    #[test]
    fn the_stop_position_of_a_loop_pass_is_not_one_of_the_capped_entries() {
        // Exactly as many entries as a take keeps, the last a key that is still down at the stop.
        let s = Session::default();
        let range = Range::new(0, 96_000);
        let mut events = vec![ev(-1, 0, 0xB0, 1, 0); MAX_MIDI_TAKE_EVENTS - 1];
        events.push(ev(24_000, 0, 0x90, 60, 100));
        let got =
            notes_of(&midi_take_sequence(&s, &MidiInput::default(), range, MidiPass::Loop { stopped: Some(48_000), marked: true }, &events, &[]));
        assert_eq!(got.iter().map(|n| (n.2, n.3)).collect::<Vec<_>>(), [(960, 960)], "the key ends at the stop, not at the loop end");
        let mut e = Engine::default();
        armed(&mut e, "Keys", "none");
        let inputs = midi_record_inputs(&e);
        let take = add_midi_recording_from(&mut e, range, MidiPass::Loop { stopped: Some(48_000), marked: true }, &events, &[], &inputs, false);
        assert_eq!((take.clips.len(), take.gaps), (1, 0), "and a full take is not reported as cut");
        // One entry more and the take is cut: the stop may be in a round whose mark was cut off,
        // so the key is held to the end of the loop instead of ending early.
        events.push(ev(96_000, u8::MAX, MidiTakeEvent::WRAP, 0, 0));
        let got =
            notes_of(&midi_take_sequence(&s, &MidiInput::default(), range, MidiPass::Loop { stopped: Some(48_000), marked: true }, &events, &[]));
        assert_eq!(got.iter().map(|n| (n.2, n.3)).collect::<Vec<_>>(), [(960, 2880)]);
    }

    #[test]
    fn a_track_taken_out_of_record_during_the_pass_keeps_what_it_recorded() {
        let mut e = Engine::default();
        let t = armed(&mut e, "Keys", "none");
        let range = Range::new(0, 48_000);
        let events = [ev(100, 0, 0x90, 60, 100), ev(200, 0, 0x80, 60, 0)];
        let inputs = midi_record_inputs(&e);
        e.execute("mix.record_arm", &json!({"tracks": [t.0], "value": false})).unwrap();
        let take = add_midi_recording_from(&mut e, range, MidiPass::Linear, &events, &[], &inputs, false);
        assert_eq!(take.clips.len(), 1, "it was record-enabled when the pass started");
        assert_eq!(notes_of(&sequence(&e, t, range)).iter().map(|n| n.1).collect::<Vec<_>>(), [60]);
        // A track that was not record-enabled at the start, and is not now, records nothing.
        e.run("edit.undo").unwrap();
        assert!(add_midi_recording_from(&mut e, range, MidiPass::Linear, &events, &[], &[], false).clips.is_empty());
        // Nor does one that was put in record during the pass: it would get what was played
        // before that, and nobody waited for the end of its take.
        e.execute("mix.record_arm", &json!({"tracks": [t.0], "value": true})).unwrap();
        assert!(add_midi_recording_from(&mut e, range, MidiPass::Linear, &events, &[], &[], false).clips.is_empty());
        assert!(e.session().track(t).unwrap().clips().is_empty());
    }

    #[test]
    fn a_track_records_the_input_it_had_when_the_pass_started() {
        let mut e = Engine::default();
        let t = armed(&mut e, "Keys", "MIDI:*:A");
        let other = armed(&mut e, "Other", "MIDI:*:B");
        let devices = ["A".to_string(), "B".to_string()];
        let range = Range::new(0, 48_000);
        let events = [ev(100, 0, 0x90, 60, 100), ev(200, 0, 0x80, 60, 0), ev(300, 1, 0x90, 72, 100), ev(400, 1, 0x80, 72, 0)];
        let inputs = midi_record_inputs(&e);
        assert_eq!(inputs.len(), 2);
        // During the pass the first track is switched to device B.
        e.execute("track.input", &json!({"tracks": [t.0], "input": "MIDI:*:B"})).unwrap();
        let take = add_midi_recording_from(&mut e, range, MidiPass::Linear, &events, &devices, &inputs, false);
        assert_eq!(take.clips.len(), 2);
        assert_eq!(notes_of(&sequence(&e, t, range)).iter().map(|n| n.1).collect::<Vec<_>>(), [60], "what it recorded from A is kept");
        assert_eq!(notes_of(&sequence(&e, other, range)).iter().map(|n| n.1).collect::<Vec<_>>(), [72]);
        // The next pass uses the input as it is when that pass starts.
        e.run("edit.undo").unwrap();
        let inputs = midi_record_inputs(&e);
        add_midi_recording_from(&mut e, range, MidiPass::Linear, &events, &devices, &inputs, false);
        assert_eq!(notes_of(&sequence(&e, t, range)).iter().map(|n| n.1).collect::<Vec<_>>(), [72]);
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
            let seq = midi_take_sequence(&s, &MidiInput::default(), range, MidiPass::Linear, &events, &[]);
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
