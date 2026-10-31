//! MIDI note editing (MIDI Editor, pencil on MIDI tracks).
use super::{CommandSpec, always, bad, clip_id, has_tracks, i64_or, range_param, tracks_required};

use crate::{Engine, Result, cmd};
use serde_json::{Value, json};
use soundcraft_midi::{Note, Sequence};
use soundcraft_model::{Clip, ClipContent, ClipId};
use soundcraft_time::TICKS_PER_QUARTER;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(query "midi.notes", "List MIDI Notes", [], None, "{clip}", always, notes),
        cmd!(
            "midi.new_clip",
            "New MIDI Clip",
            ["Clip"],
            None,
            "{track, start?, end?|length?} (default: the edit selection, or 1 bar)",
            has_tracks,
            new_clip
        ),
        cmd!("midi.note_add", "Add Note", [], None, "{clip, pitch, start_ticks, length_ticks?: 480, velocity?: 100, channel?: 0}", always, note_add),
        cmd!("midi.note_edit", "Edit Note", [], None, "{clip, index, pitch?, start_ticks?, length_ticks?, velocity?}", always, note_edit),
        cmd!("midi.note_delete", "Delete Notes", [], None, "{clip, indices: [n]}", always, note_delete),
        cmd!("midi.notes_move", "Move Notes", [], None, "{clip, indices: [n], ticks?: 0, semitones?: 0}", always, notes_move),
    ]
}

fn seq_mut<'a>(e: &'a mut Engine, id: ClipId, cmd: &str) -> Result<&'a mut Sequence> {
    match e.session_mut().find_clip_mut(id).map(|c| &mut c.content) {
        Some(ClipContent::Midi { sequence }) => Ok(sequence),
        Some(_) => Err(bad(cmd, "not a MIDI clip")),
        None => Err(bad(cmd, "no such clip")),
    }
}

fn notes(e: &mut Engine, p: &Value) -> Result<Value> {
    let id = clip_id(p, "midi.notes")?;
    match e.session().find_clip(id).map(|(_, c)| &c.content) {
        Some(ClipContent::Midi { sequence }) => Ok(json!({"notes": sequence.notes})),
        _ => Err(bad("midi.notes", "not a MIDI clip")),
    }
}

fn new_clip(e: &mut Engine, p: &Value) -> Result<Value> {
    let t = tracks_required(e, "midi.new_clip", p)?.first().copied().ok_or_else(|| bad("midi.new_clip", "no track"))?;
    if !e.session().track(t).is_some_and(|x| x.kind.is_midi()) {
        return Err(bad("midi.new_clip", "not a MIDI or instrument track"));
    }
    let mut r = range_param(e, "midi.new_clip", p)?;
    if r.is_empty() {
        let s = e.session();
        let bar = soundcraft_time::GridValue::Note { value: soundcraft_time::NoteValue::Bar, dotted: false, triplet: false };
        let start = bar.line_at_or_before(r.start, s.sample_rate, &s.tempo, s.frame_rate);
        r = soundcraft_time::Range::new(start, bar.next_line(start, s.sample_rate, &s.tempo, s.frame_rate));
    }
    let s = e.session_mut();
    let id = s.new_clip_id();
    let name = s.track(t).map(|x| x.name.clone()).unwrap_or_default();
    crate::edit::place_clip(s, t, Clip::midi(id, name, r.start, r.len(), Sequence::default()));
    Ok(json!({"clip": id, "start": r.start, "end": r.end}))
}

fn note_add(e: &mut Engine, p: &Value) -> Result<Value> {
    let id = clip_id(p, "midi.note_add")?;
    let n = Note {
        pitch: i64_or(p, "pitch", 60).clamp(0, 127) as u8,
        velocity: i64_or(p, "velocity", 100).clamp(1, 127) as u8,
        release_velocity: 64,
        channel: i64_or(p, "channel", 0).clamp(0, 15) as u8,
        start: i64_or(p, "start_ticks", 0).max(0),
        length: i64_or(p, "length_ticks", TICKS_PER_QUARTER / 2).max(1),
    };
    let seq = seq_mut(e, id, "midi.note_add")?;
    seq.notes.push(n);
    seq.sort();
    let index = seq.notes.iter().position(|x| *x == n).unwrap_or(0);
    Ok(json!({"index": index, "notes": seq.notes.len()}))
}

fn note_edit(e: &mut Engine, p: &Value) -> Result<Value> {
    let id = clip_id(p, "midi.note_edit")?;
    let idx = p.get("index").and_then(Value::as_u64).ok_or_else(|| bad("midi.note_edit", "`index` required"))? as usize;
    let seq = seq_mut(e, id, "midi.note_edit")?;
    let n = seq.notes.get_mut(idx).ok_or_else(|| bad("midi.note_edit", "index out of range"))?;
    if let Some(v) = p.get("pitch").and_then(Value::as_i64) {
        n.pitch = v.clamp(0, 127) as u8;
    }
    if let Some(v) = p.get("start_ticks").and_then(Value::as_i64) {
        n.start = v.max(0);
    }
    if let Some(v) = p.get("length_ticks").and_then(Value::as_i64) {
        n.length = v.max(1);
    }
    if let Some(v) = p.get("velocity").and_then(Value::as_i64) {
        n.velocity = v.clamp(1, 127) as u8;
    }
    let edited = *n;
    seq.sort();
    Ok(json!({"note": edited, "index": seq.notes.iter().position(|x| *x == edited)}))
}

fn indices(p: &Value) -> Vec<usize> {
    p.get("indices").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_u64).map(|x| x as usize).collect()).unwrap_or_default()
}

fn note_delete(e: &mut Engine, p: &Value) -> Result<Value> {
    let id = clip_id(p, "midi.note_delete")?;
    let idx = indices(p);
    let seq = seq_mut(e, id, "midi.note_delete")?;
    let before = seq.notes.len();
    let mut i = 0;
    seq.notes.retain(|_| {
        let keep = !idx.contains(&i);
        i += 1;
        keep
    });
    Ok(json!({"deleted": before - seq.notes.len()}))
}

fn notes_move(e: &mut Engine, p: &Value) -> Result<Value> {
    let id = clip_id(p, "midi.notes_move")?;
    let idx = indices(p);
    let dt = i64_or(p, "ticks", 0);
    let ds = i64_or(p, "semitones", 0);
    let seq = seq_mut(e, id, "midi.notes_move")?;
    // Clamp the shared delta so the earliest selected note stops at tick 0 and the selection keeps its spacing.
    let earliest = seq.notes.iter().enumerate().filter(|(i, _)| idx.contains(i)).map(|(_, n)| n.start).min();
    let dt = match earliest {
        Some(m) => dt.max(0i64.saturating_sub(m).min(0)),
        None => dt,
    };
    for (i, n) in seq.notes.iter_mut().enumerate() {
        if idx.contains(&i) {
            n.start = n.start.saturating_add(dt).max(0);
            n.pitch = i64::from(n.pitch).saturating_add(ds).clamp(0, 127) as u8;
        }
    }
    let moved: Vec<Note> = seq.notes.iter().enumerate().filter(|(i, _)| idx.contains(i)).map(|(_, n)| *n).collect();
    seq.sort();
    // Where the moved notes ended up after sorting, so the editor can keep them selected.
    let mut taken = vec![false; seq.notes.len()];
    let mut indices = Vec::with_capacity(moved.len());
    for m in &moved {
        if let Some(j) = seq.notes.iter().enumerate().position(|(j, n)| !taken[j] && n == m) {
            taken[j] = true;
            indices.push(j);
        }
    }
    indices.sort_unstable();
    Ok(json!({"moved": idx.len(), "indices": indices}))
}

#[cfg(test)]
mod tests {
    use super::*;
    use soundcraft_model::{ChannelFormat, TrackKind};

    #[test]
    fn create_clip_add_edit_move_delete() {
        let mut e = Engine::default();
        let t = e.session_mut().add_track(TrackKind::Instrument, ChannelFormat::Stereo, Some("Keys"));
        let r = e.execute("midi.new_clip", &json!({"track": t.0})).unwrap();
        let clip = r["clip"].as_u64().unwrap();
        e.execute("midi.note_add", &json!({"clip": clip, "pitch": 60, "start_ticks": 0})).unwrap();
        e.execute("midi.note_add", &json!({"clip": clip, "pitch": 64, "start_ticks": 480})).unwrap();
        e.execute("midi.note_edit", &json!({"clip": clip, "index": 0, "velocity": 30})).unwrap();
        e.execute("midi.notes_move", &json!({"clip": clip, "indices": [1], "semitones": 3})).unwrap();
        let n = e.execute("midi.notes", &json!({"clip": clip})).unwrap();
        assert_eq!(n["notes"][0]["velocity"], 30);
        assert_eq!(n["notes"][1]["pitch"], 67);
        e.execute("midi.note_delete", &json!({"clip": clip, "indices": [0]})).unwrap();
        assert_eq!(e.execute("midi.notes", &json!({"clip": clip})).unwrap()["notes"].as_array().map(Vec::len), Some(1));
        assert!(e.execute("midi.note_edit", &json!({"clip": clip, "index": 99})).is_err());
    }

    #[test]
    fn notes_move_left_past_zero_keeps_spacing() {
        let mut e = Engine::default();
        let t = e.session_mut().add_track(TrackKind::Instrument, ChannelFormat::Stereo, Some("Keys"));
        let r = e.execute("midi.new_clip", &json!({"track": t.0})).unwrap();
        let clip = r["clip"].as_u64().unwrap();
        for s in [100, 200, 300] {
            e.execute("midi.note_add", &json!({"clip": clip, "pitch": 60, "start_ticks": s})).unwrap();
        }
        e.execute("midi.notes_move", &json!({"clip": clip, "indices": [0, 1], "ticks": -150})).unwrap();
        let n = e.execute("midi.notes", &json!({"clip": clip})).unwrap();
        let starts: Vec<i64> = n["notes"].as_array().unwrap().iter().map(|x| x["start"].as_i64().unwrap()).collect();
        assert_eq!(starts, [0, 100, 300]);
    }

    #[test]
    fn notes_move_without_left_overflow_does_not_shift_selection() {
        let mut e = Engine::default();
        let t = e.session_mut().add_track(TrackKind::Instrument, ChannelFormat::Stereo, Some("Keys"));
        let clip = e.execute("midi.new_clip", &json!({"track": t.0})).unwrap()["clip"].as_u64().unwrap();
        for s in [0, 100] {
            e.execute("midi.note_add", &json!({"clip": clip, "pitch": 60, "start_ticks": s})).unwrap();
        }
        seq_mut(&mut e, ClipId(clip), "test").unwrap().notes[0].start = -50;
        e.execute("midi.notes_move", &json!({"clip": clip, "indices": [0, 1], "ticks": 0})).unwrap();
        let n = e.execute("midi.notes", &json!({"clip": clip})).unwrap();
        let starts: Vec<i64> = n["notes"].as_array().unwrap().iter().map(|x| x["start"].as_i64().unwrap()).collect();
        assert_eq!(starts, [0, 100]);
    }

    #[test]
    fn notes_move_huge_params_do_not_overflow() {
        let mut e = Engine::default();
        let t = e.session_mut().add_track(TrackKind::Instrument, ChannelFormat::Stereo, Some("Keys"));
        let r = e.execute("midi.new_clip", &json!({"track": t.0})).unwrap();
        let clip = r["clip"].as_u64().unwrap();
        e.execute("midi.note_add", &json!({"clip": clip, "pitch": 60, "start_ticks": 480})).unwrap();
        e.execute("midi.notes_move", &json!({"clip": clip, "indices": [0], "ticks": i64::MAX})).unwrap();
        let n = e.execute("midi.notes", &json!({"clip": clip})).unwrap();
        assert_eq!(n["notes"][0]["start"], i64::MAX);
        e.execute("midi.notes_move", &json!({"clip": clip, "indices": [0], "semitones": i64::MAX})).unwrap();
        let n = e.execute("midi.notes", &json!({"clip": clip})).unwrap();
        assert_eq!(n["notes"][0]["pitch"], 127);
    }

    #[test]
    fn notes_move_reports_where_the_moved_notes_went() {
        let mut e = Engine::default();
        let t = e.session_mut().add_track(TrackKind::Instrument, ChannelFormat::Stereo, Some("Keys"));
        let clip = e.execute("midi.new_clip", &json!({"track": t.0})).unwrap()["clip"].as_u64().unwrap();
        for start in [0, 480, 960] {
            e.execute("midi.note_add", &json!({"clip": clip, "pitch": 60, "start_ticks": start})).unwrap();
        }
        let r = e.execute("midi.notes_move", &json!({"clip": clip, "indices": [0], "ticks": 720})).unwrap();
        assert_eq!(r["indices"], json!([1]));
        let notes = e.execute("midi.notes", &json!({"clip": clip})).unwrap();
        assert_eq!(notes["notes"][1]["start"], 720);
    }

    #[test]
    fn rejects_audio_tracks() {
        let mut e = Engine::default();
        let t = e.session_mut().add_track(TrackKind::Audio, ChannelFormat::Mono, None);
        assert!(e.execute("midi.new_clip", &json!({"track": t.0})).is_err());
    }
}
