//! MIDI note editing (MIDI Editor, pencil on MIDI tracks).

use super::*;
use crate::cmd;
use serde_json::json;
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
        cmd!("midi.notes_move", "Move Notes", [], None, "{clip, indices: [n], ticks?: 0, semitones?: 0, copy?: false}", always, notes_move),
        cmd!(
            "midi.notes_trim",
            "Trim Notes",
            [],
            None,
            "{clip, indices: [n], start?: 0, end?: 0} (tick offsets for the note edges)",
            always,
            notes_trim
        ),
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
    let copy = p.get("copy").and_then(Value::as_bool).unwrap_or(false);
    let seq = seq_mut(e, id, "midi.notes_move")?;
    let mut moved = Vec::with_capacity(idx.len());
    for (i, n) in seq.notes.iter_mut().enumerate() {
        if idx.contains(&i) {
            let mut m = *n;
            m.start = (m.start + dt).max(0);
            m.pitch = (i64::from(m.pitch) + ds).clamp(0, 127) as u8;
            if !copy {
                *n = m;
            }
            moved.push(m);
        }
    }
    if copy {
        seq.notes.extend_from_slice(&moved);
    }
    seq.sort();
    // Where the moved notes (or the copies) ended up after sorting, so the editor can keep them selected.
    Ok(json!({"moved": moved.len(), "indices": relocate(seq, &moved)}))
}

/// Trim the start and/or end of several notes at once: `start` and `end` are tick offsets for the
/// note edges (a start trim keeps the note's end in place). Notes never shrink below one tick.
fn notes_trim(e: &mut Engine, p: &Value) -> Result<Value> {
    let id = clip_id(p, "midi.notes_trim")?;
    let idx = indices(p);
    let ds = i64_or(p, "start", 0);
    let de = i64_or(p, "end", 0);
    let seq = seq_mut(e, id, "midi.notes_trim")?;
    let mut trimmed = Vec::with_capacity(idx.len());
    for (i, n) in seq.notes.iter_mut().enumerate() {
        if idx.contains(&i) {
            let end = n.end().saturating_add(de).max(1);
            n.start = n.start.saturating_add(ds).clamp(0, end - 1);
            n.length = (end - n.start).max(1);
            trimmed.push(*n);
        }
    }
    seq.sort();
    Ok(json!({"trimmed": trimmed.len(), "indices": relocate(seq, &trimmed)}))
}

/// The sorted indices `notes` now sit at in `seq` (each matched once, so duplicates stay distinct).
fn relocate(seq: &Sequence, notes: &[Note]) -> Vec<usize> {
    let mut taken = vec![false; seq.notes.len()];
    let mut out = Vec::with_capacity(notes.len());
    for m in notes {
        if let Some(j) = seq.notes.iter().enumerate().position(|(j, n)| !taken.get(j).copied().unwrap_or(true) && n == m) {
            if let Some(t) = taken.get_mut(j) {
                *t = true;
            }
            out.push(j);
        }
    }
    out.sort_unstable();
    out
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

    fn keys_clip(e: &mut Engine, notes: &[(u8, i64, i64)]) -> u64 {
        let t = e.session_mut().add_track(TrackKind::Instrument, ChannelFormat::Stereo, Some("Keys"));
        let clip = e.execute("midi.new_clip", &json!({"track": t.0})).unwrap()["clip"].as_u64().unwrap();
        for &(pitch, start, length) in notes {
            e.execute("midi.note_add", &json!({"clip": clip, "pitch": pitch, "start_ticks": start, "length_ticks": length})).unwrap();
        }
        clip
    }

    fn starts_pitches_lengths(e: &mut Engine, clip: u64) -> Vec<(i64, i64, i64)> {
        let n = e.execute("midi.notes", &json!({"clip": clip})).unwrap();
        n["notes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|n| (n["start"].as_i64().unwrap(), n["pitch"].as_i64().unwrap(), n["length"].as_i64().unwrap()))
            .collect()
    }

    #[test]
    fn notes_move_with_copy_leaves_the_originals_and_reports_the_copies() {
        let mut e = Engine::default();
        let clip = keys_clip(&mut e, &[(60, 0, 240), (64, 480, 240)]);
        let r = e.execute("midi.notes_move", &json!({"clip": clip, "indices": [0, 1], "ticks": 960, "semitones": 2, "copy": true})).unwrap();
        assert_eq!(r["indices"], json!([2, 3]));
        assert_eq!(starts_pitches_lengths(&mut e, clip), vec![(0, 60, 240), (480, 64, 240), (960, 62, 240), (1440, 66, 240)]);
        e.execute("edit.undo", &json!({})).unwrap();
        assert_eq!(starts_pitches_lengths(&mut e, clip).len(), 2, "one undo step removes the copies");
    }

    #[test]
    fn notes_trim_moves_both_edges_of_every_note_in_one_step() {
        let mut e = Engine::default();
        let clip = keys_clip(&mut e, &[(60, 480, 480), (64, 960, 240)]);
        let r = e.execute("midi.notes_trim", &json!({"clip": clip, "indices": [0, 1], "end": 120})).unwrap();
        assert_eq!(r["indices"], json!([0, 1]));
        assert_eq!(starts_pitches_lengths(&mut e, clip), vec![(480, 60, 600), (960, 64, 360)]);
        // A start trim keeps the end; it stops one tick short of it and never goes below zero.
        e.execute("midi.notes_trim", &json!({"clip": clip, "indices": [0, 1], "start": -600})).unwrap();
        assert_eq!(starts_pitches_lengths(&mut e, clip), vec![(0, 60, 1080), (360, 64, 960)]);
        e.execute("midi.notes_trim", &json!({"clip": clip, "indices": [1], "start": 5000, "end": -5000})).unwrap();
        assert_eq!(starts_pitches_lengths(&mut e, clip)[1].2, 1);
    }

    #[test]
    fn rejects_audio_tracks() {
        let mut e = Engine::default();
        let t = e.session_mut().add_track(TrackKind::Audio, ChannelFormat::Mono, None);
        assert!(e.execute("midi.new_clip", &json!({"track": t.0})).is_err());
    }
}
