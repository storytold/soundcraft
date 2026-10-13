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
}
