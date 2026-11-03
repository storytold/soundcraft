//! MIDI recording: what was played on MIDI input devices while recording becomes a MIDI clip on
//! each record-enabled instrument and MIDI track.
//!
//! The host stamps each incoming channel message with the timeline position it was played at
//! and hands the whole take over at stop; this command turns the messages into notes and
//! controller events and places the clip (replacing what lay underneath, like an audio take).
use super::{CommandSpec, always, bad, position_param, tracks_param};

use crate::{Engine, Result, cmd};
use serde_json::{Value, json};
use soundcraft_midi::{CtrlEvent, CtrlKind, Note, Sequence};
use soundcraft_model::{Clip, TrackId};
use soundcraft_time::{Range, Samples};
use std::collections::HashMap;

/// Messages kept from one take; a longer take keeps its first ones (hours of dense playing).
const MAX_EVENTS: usize = 500_000;

pub fn specs() -> Vec<CommandSpec> {
    vec![cmd!(
        "midi.record_take",
        "Record MIDI Take",
        [],
        None,
        "{start, end, events: [[sample, status, data1, data2], …], tracks?: [ids] (default: the record-enabled instrument and MIDI tracks)} — notes and controllers played between start and end become a clip on each track",
        always,
        record_take
    )]
}

fn record_take(e: &mut Engine, p: &Value) -> Result<Value> {
    let id = "midi.record_take";
    let start = position_param(e, id, p, "start")?.ok_or_else(|| bad(id, "`start` required"))?.max(0);
    let end = position_param(e, id, p, "end")?.ok_or_else(|| bad(id, "`end` required"))?;
    if end <= start {
        return Err(bad(id, "`end` must come after `start`"));
    }
    let range = Range::new(start, end);
    let events = parse_events(p.get("events")).ok_or_else(|| bad(id, "`events` must be a list of [sample, status, data1, data2]"))?;
    let tracks: Vec<TrackId> = if p.get("tracks").is_some() || p.get("track").is_some() {
        tracks_param(e, id, p)?
    } else {
        e.session().tracks.iter().filter(|t| t.kind.is_midi() && t.mixer.record_arm && !t.inactive).map(|t| t.id).collect()
    };
    let s = e.session();
    if let Some(t) = tracks.iter().find(|t| !s.track(**t).is_some_and(|x| x.kind.is_midi())) {
        return Err(bad(id, format!("track {} is not a MIDI or instrument track", t.0)));
    }
    if tracks.is_empty() {
        return Err(bad(id, "no record-enabled instrument or MIDI track"));
    }
    let seq = to_sequence(s, range, &events);
    if seq.notes.is_empty() && seq.ctrls.is_empty() {
        return Ok(json!({"clips": [], "notes": 0, "controllers": 0}));
    }
    let (notes, ctrls) = (seq.notes.len(), seq.ctrls.len());
    let s = e.session_mut();
    let mut clips = Vec::new();
    for t in tracks {
        let cid = s.new_clip_id();
        let name = s.track(t).map(|x| x.name.clone()).unwrap_or_default();
        crate::edit::place_clip(s, t, Clip::midi(cid, name, range.start, range.len(), seq.clone()));
        clips.push(cid);
    }
    Ok(json!({"clips": clips, "notes": notes, "controllers": ctrls}))
}

/// `[[sample, status, data1, data2], …]`, hostile input checked: anything malformed is refused.
fn parse_events(v: Option<&Value>) -> Option<Vec<(Samples, [u8; 3])>> {
    let arr = v?.as_array()?;
    let mut out = Vec::with_capacity(arr.len().min(MAX_EVENTS));
    for ev in arr.iter().take(MAX_EVENTS) {
        let a = ev.as_array()?;
        let at = a.first()?.as_i64()?;
        let byte = |i: usize| a.get(i).and_then(Value::as_u64).filter(|b| *b <= 0xFF).map(|b| b as u8);
        out.push((at, [byte(1)?, byte(2)? & 0x7F, byte(3)? & 0x7F]));
    }
    Some(out)
}

/// Turn time-stamped channel messages into a sequence whose ticks count from `range.start`.
/// A note still held at the end of the range ends there; events outside the range are dropped
/// (punch recording passes the punch range).
pub fn to_sequence(s: &soundcraft_model::Session, range: Range, events: &[(Samples, [u8; 3])]) -> Sequence {
    let sr = s.sample_rate;
    let base = s.tempo.samples_to_ticks(range.start, sr);
    let tick = |at: Samples| s.tempo.samples_to_ticks(at.clamp(range.start, range.end), sr).saturating_sub(base).max(0);
    let end_tick = tick(range.end).max(1);
    let mut sorted: Vec<(Samples, [u8; 3])> = events.iter().copied().filter(|(at, _)| *at >= range.start && *at < range.end).collect();
    // Stable: messages stamped with the same position keep the order they were played in.
    sorted.sort_by_key(|(at, _)| *at);
    let mut seq = Sequence::default();
    // Sounding notes by (channel, pitch): start tick and velocity.
    let mut open: HashMap<(u8, u8), (i64, u8)> = HashMap::new();
    let close = |seq: &mut Sequence, (ch, pitch): (u8, u8), (from, vel): (i64, u8), to: i64, release: u8| {
        let length = to.saturating_sub(from).max(1).min(end_tick.saturating_sub(from).max(1));
        seq.notes.push(Note { pitch, velocity: vel.max(1), release_velocity: release, channel: ch, start: from, length });
    };
    for &(at, [status, d1, d2]) in &sorted {
        let ch = status & 0x0F;
        let t = tick(at);
        let ctrl = |kind, value| CtrlEvent { tick: t, channel: ch, kind, value };
        match status & 0xF0 {
            0x90 if d2 > 0 => {
                // A repeated note-on ends the one sounding on the same key.
                if let Some(prev) = open.insert((ch, d1), (t, d2)) {
                    close(&mut seq, (ch, d1), prev, t, 64);
                }
            }
            0x80 | 0x90 => {
                if let Some(on) = open.remove(&(ch, d1)) {
                    close(&mut seq, (ch, d1), on, t, if status & 0xF0 == 0x80 { d2 } else { 64 });
                }
            }
            0xA0 => seq.ctrls.push(ctrl(CtrlKind::PolyPressure(d1), i32::from(d2))),
            0xB0 => seq.ctrls.push(ctrl(CtrlKind::Cc(d1), i32::from(d2))),
            0xC0 => seq.ctrls.push(ctrl(CtrlKind::Program, i32::from(d1))),
            0xD0 => seq.ctrls.push(ctrl(CtrlKind::ChannelPressure, i32::from(d1))),
            0xE0 => seq.ctrls.push(ctrl(CtrlKind::PitchBend, ((i32::from(d2) << 7) | i32::from(d1)) - 8192)),
            _ => {}
        }
    }
    let mut held: Vec<_> = open.into_iter().collect();
    held.sort_by_key(|(k, v)| (v.0, *k));
    for (k, v) in held {
        close(&mut seq, k, v, end_tick, 64);
    }
    seq.sort();
    seq
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use soundcraft_model::{ChannelFormat, ClipContent, TrackKind};
    use soundcraft_time::TICKS_PER_QUARTER;

    /// 120 bpm at 48 kHz: one quarter note is 24 000 samples.
    const Q: i64 = 24_000;

    fn engine() -> (Engine, TrackId, TrackId) {
        let mut e = Engine::new(soundcraft_model::Session::default());
        let s = e.session_mut();
        let keys = s.add_track(TrackKind::Instrument, ChannelFormat::Stereo, Some("Keys"));
        let other = s.add_track(TrackKind::Instrument, ChannelFormat::Stereo, Some("Lead"));
        s.track_mut(keys).unwrap().mixer.record_arm = true;
        (e, keys, other)
    }

    fn clip_seq(e: &Engine, t: TrackId) -> (i64, i64, Sequence) {
        let c = e.session().track(t).unwrap().clips().first().cloned().unwrap();
        let (start, len) = (c.start, c.range().len());
        let ClipContent::Midi { sequence } = c.content else { panic!("not MIDI") };
        (start, len, sequence)
    }

    #[test]
    fn played_notes_and_controllers_become_a_clip_on_the_armed_track() {
        let (mut e, keys, lead) = engine();
        let events = json!([
            [Q, 0x90, 60, 100],
            [Q + Q / 2, 0xB0, 64, 127],
            [2 * Q, 0x80, 60, 30],
            [2 * Q, 0x91, 64, 90],
            [3 * Q, 0x91, 64, 0],
            [3 * Q, 0xE0, 0, 0x40],
            [4 * Q, 0x90, 67, 80]
        ]);
        let r = e.execute("midi.record_take", &json!({"start": Q, "end": 5 * Q, "events": events})).unwrap();
        assert_eq!(r["notes"], 3);
        assert_eq!(r["controllers"], 2);
        let (start, len, seq) = clip_seq(&e, keys);
        assert_eq!((start, len), (Q, 4 * Q));
        let q = TICKS_PER_QUARTER;
        let got: Vec<(u8, u8, i64, i64, u8)> = seq.notes.iter().map(|n| (n.pitch, n.channel, n.start, n.length, n.velocity)).collect();
        // The last note was still held at stop: it ends with the take.
        assert_eq!(got, vec![(60, 0, 0, q, 100), (64, 1, q, q, 90), (67, 0, 3 * q, q, 80)]);
        assert_eq!(seq.notes[0].release_velocity, 30);
        assert_eq!(
            seq.ctrls.iter().map(|c| (c.tick, c.kind, c.value)).collect::<Vec<_>>(),
            vec![(q / 2, CtrlKind::Cc(64), 127), (2 * q, CtrlKind::PitchBend, 0)]
        );
        // Only the armed track records; undo takes the clip away.
        assert!(e.session().track(lead).unwrap().clips().is_empty());
        e.execute("edit.undo", &json!({})).unwrap();
        assert!(e.session().track(keys).unwrap().clips().is_empty());
    }

    #[test]
    fn punch_range_drops_what_was_played_outside_it() {
        let (mut e, keys, _) = engine();
        let events = json!([[0, 0x90, 50, 100], [Q / 2, 0x80, 50, 0], [Q, 0x90, 52, 100], [2 * Q, 0x80, 52, 0], [3 * Q, 0x90, 55, 1]]);
        e.execute("midi.record_take", &json!({"start": Q, "end": 2 * Q + 10, "events": events})).unwrap();
        let (_, _, seq) = clip_seq(&e, keys);
        assert_eq!(seq.notes.iter().map(|n| n.pitch).collect::<Vec<_>>(), vec![52]);
    }

    #[test]
    fn nothing_played_places_nothing_and_bad_input_is_refused() {
        let (mut e, keys, _) = engine();
        let r = e.execute("midi.record_take", &json!({"start": 0, "end": Q, "events": []})).unwrap();
        assert_eq!(r["clips"], json!([]));
        assert!(e.session().track(keys).unwrap().clips().is_empty());
        for bad_events in [json!("x"), json!([[0, 0x90, 60]]), json!([[0, 300, 60, 1]]), json!([["a", 0x90, 60, 1]])] {
            assert!(e.execute("midi.record_take", &json!({"start": 0, "end": Q, "events": bad_events})).is_err());
        }
        assert!(e.execute("midi.record_take", &json!({"start": Q, "end": Q, "events": []})).is_err());
        // Explicit tracks: an audio track is refused.
        let audio = e.session_mut().add_track(TrackKind::Audio, ChannelFormat::Mono, None);
        assert!(e.execute("midi.record_take", &json!({"start": 0, "end": Q, "events": [[0, 0x90, 60, 1]], "tracks": [audio.0]})).is_err());
        // Nothing armed.
        e.session_mut().track_mut(keys).unwrap().mixer.record_arm = false;
        assert!(e.execute("midi.record_take", &json!({"start": 0, "end": Q, "events": [[0, 0x90, 60, 1]]})).is_err());
    }
}
