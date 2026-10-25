//! Event menu: time/tempo operations, MIDI operations, markers.
use super::{CommandSpec, always, bad, clip_ids_param, f32_or, f64_or, has_selection, has_tracks, i64_or, position_param, range_param, str_param};

use crate::{Engine, Result, cmd};
use serde_json::{Value, json};
use soundcraft_midi::ops::{DurationOp, QuantizeOptions, VelocityOp};
use soundcraft_model::{ClipContent, MarkerKind};
use soundcraft_time::TICKS_PER_QUARTER;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!("event.tempo", "Tempo Change", [], None, "{bpm, at?: position (default session start)}", always, tempo),
        cmd!("event.tempo_remove", "Remove Tempo Change", [], None, "{at}", always, |e, p| {
            let at = position_param(e, "event.tempo_remove", p, "at")?.unwrap_or(0);
            let s = e.session_mut();
            let tick = s.tempo.samples_to_ticks(at, s.sample_rate);
            Ok(json!({"removed": s.tempo.remove_tempo(tick)}))
        }),
        cmd!("event.meter", "Change Meter...", ["Event", "Time Operations"], None, "{numerator, denominator, at?: position}", always, meter),
        cmd!("event.tempo_scale", "Scale...", ["Event", "Tempo Operations"], None, "{factor}", always, |e, p| {
            let f = f64_or(p, "factor", 1.0);
            e.session_mut().tempo.scale_tempos(f).map_err(|err| bad("event.tempo_scale", err.to_string()))?;
            Ok(json!({}))
        }),
        cmd!("event.tempo_constant", "Constant...", ["Event", "Tempo Operations"], None, "{bpm, start?, end?}", always, tempo_constant),
        cmd!("event.insert_time", "Insert Time...", ["Event", "Time Operations"], None, "{start?, length | end}", has_tracks, insert_time),
        cmd!("event.cut_time", "Cut Time...", ["Event", "Time Operations"], None, "{start?, end?}", has_tracks, cut_time),
        cmd!(
            "event.quantize",
            "Quantize...",
            ["Event", "MIDI Operations"],
            Some("Alt+0"),
            "{grid?: 1/16 | ticks, strength?: 100, swing?: 0, clips?}",
            has_selection,
            quantize
        ),
        cmd!("event.transpose", "Transpose...", ["Event", "MIDI Operations"], Some("Alt+T"), "{semitones, clips?}", has_selection, |e, p| {
            let st = i64_or(p, "semitones", 0).clamp(-127, 127) as i32;
            midi_op(e, p, move |n| soundcraft_midi::ops::transpose(n, st))
        }),
        cmd!(
            "event.change_velocity",
            "Change Velocity...",
            ["Event", "MIDI Operations"],
            None,
            "{set?|add?|scale?|min?,max?, clips?}",
            has_selection,
            velocity
        ),
        cmd!(
            "event.change_duration",
            "Change Duration...",
            ["Event", "MIDI Operations"],
            Some("Alt+P"),
            "{set?|add?|scale?|legato?: gap, clips?}",
            has_selection,
            duration
        ),
        cmd!("event.remove_duplicate_notes", "Remove Duplicate Notes", ["Event"], None, "{clips?}", has_selection, |e, p| {
            let ids = clip_ids_param(e, p);
            let s = e.session_mut();
            let mut n = 0;
            for id in ids {
                if let Some(c) = s.find_clip_mut(id)
                    && let ClipContent::Midi { sequence } = &mut c.content
                {
                    n += sequence.remove_duplicates()
                }
            }
            Ok(json!({"removed": n}))
        }),
        cmd!("event.add_key_change", "Add Key Change...", ["Event"], None, "{key: 'C major', at?}", always, |e, p| {
            let key = str_param(p, "key").unwrap_or("C major").to_string();
            let at = position_param(e, "event.add_key_change", p, "at")?.unwrap_or(0);
            let s = e.session_mut();
            s.key_signatures.retain(|(t, _)| *t != at);
            s.key_signatures.push((at, key));
            s.key_signatures.sort_by_key(|k| k.0);
            Ok(json!({}))
        }),
        cmd!("event.renumber_bars", "Renumber Bars...", ["Event"], None, "{}", always, |_, _| Ok(json!({"note": "bars start at 1"}))),
        cmd!("event.all_notes_off", "All MIDI Notes Off", ["Event"], Some("Cmd+Shift+."), "{}", always, |e, _| {
            e.transport_requests.push(crate::TransportRequest::AllNotesOff);
            Ok(json!({}))
        }),
        cmd!("event.midi_note", "Add MIDI Note", [], None, "{clip, pitch, start_ticks, length_ticks, velocity?: 100}", has_selection, add_note),
        cmd!(
            "markers.add",
            "New Memory Location",
            [],
            Some("Enter"),
            "{name?, at?, start?, end?, kind?: marker|selection|none, ruler?: 1..5}",
            always,
            add_marker
        ),
        cmd!("markers.delete", "Delete Memory Location", [], None, "{number | id}", always, delete_marker),
        cmd!("markers.edit", "Edit Memory Location", [], None, "{number | id, name?, comments?, at?, color?: [r,g,b] | null}", always, edit_marker),
        cmd!(noundo "markers.recall", "Recall Memory Location", [], Some("Period + n + Period"), "{number}", always, recall_marker),
        cmd!(noundo "markers.next", "Go to Next Marker", [], None, "{}", always, |e, _| step_marker(e, 1)),
        cmd!(noundo "markers.previous", "Go to Previous Marker", [], None, "{}", always, |e, _| step_marker(e, -1)),
    ]
}

fn tempo(e: &mut Engine, p: &Value) -> Result<Value> {
    let bpm = f64_or(p, "bpm", f64::NAN);
    let at = position_param(e, "event.tempo", p, "at")?.or(position_param(e, "event.tempo", p, "start")?).unwrap_or(0);
    let s = e.session_mut();
    let tick = s.tempo.samples_to_ticks(at, s.sample_rate);
    s.tempo.set_tempo(tick, bpm).map_err(|err| bad("event.tempo", err.to_string()))?;
    Ok(json!({"tick": tick, "bpm": bpm}))
}

fn tempo_constant(e: &mut Engine, p: &Value) -> Result<Value> {
    let id = "event.tempo_constant";
    let r = range_param(e, id, p)?;
    let s = e.session();
    let (t0, t1) = (s.tempo.samples_to_ticks(r.start, s.sample_rate), s.tempo.samples_to_ticks(r.end, s.sample_rate));
    if t1 <= t0 {
        return tempo(e, p);
    }
    let bpm = f64_or(p, "bpm", f64::NAN);
    let s = e.session_mut();
    // Read before the edit: the end is a musical position, and the new tempo moves it in samples.
    let after = s.tempo.tempo_at_tick(t1);
    let had_end = s.tempo.tempos().iter().any(|x| x.tick == t1);
    s.tempo.set_tempo(t0, bpm).map_err(|err| bad(id, err.to_string()))?;
    let inside: Vec<i64> = s.tempo.tempos().iter().map(|x| x.tick).filter(|t| *t > t0 && *t < t1).collect();
    for t in inside {
        s.tempo.remove_tempo(t);
    }
    if !had_end {
        s.tempo.set_tempo(t1, after).map_err(|err| bad(id, err.to_string()))?;
    }
    Ok(json!({"tick": t0, "end_tick": t1, "bpm": bpm}))
}

fn meter(e: &mut Engine, p: &Value) -> Result<Value> {
    let num = i64_or(p, "numerator", 4).clamp(1, 64) as u32;
    let den = i64_or(p, "denominator", 4).clamp(1, 64) as u32;
    let at = position_param(e, "event.meter", p, "at")?.unwrap_or(0);
    let s = e.session_mut();
    let tick = s.tempo.samples_to_ticks(at, s.sample_rate);
    s.tempo.set_meter(tick, num, den).map_err(|err| bad("event.meter", err.to_string()))?;
    Ok(json!({"meter": format!("{num}/{den}")}))
}

fn insert_time(e: &mut Engine, p: &Value) -> Result<Value> {
    let r = range_param(e, "event.insert_time", p)?;
    if r.is_empty() {
        return Err(bad("event.insert_time", "give `start` and `length` or `end`"));
    }
    let s = e.session_mut();
    let ids: Vec<_> = s.tracks.iter().map(|t| t.id).collect();
    for t in ids {
        crate::edit::insert_silence(s, t, r);
        if let Some(tr) = s.track_mut(t) {
            for l in &mut tr.automation {
                l.shift_from(r.start, r.len());
            }
        }
    }
    for m in &mut s.markers {
        if m.start >= r.start {
            m.start += r.len();
            m.end += r.len();
        }
    }
    Ok(json!({"inserted": r.len()}))
}

fn cut_time(e: &mut Engine, p: &Value) -> Result<Value> {
    let r = range_param(e, "event.cut_time", p)?;
    if r.is_empty() {
        return Err(bad("event.cut_time", "selection is empty"));
    }
    let s = e.session_mut();
    let ids: Vec<_> = s.tracks.iter().map(|t| t.id).collect();
    for t in ids {
        crate::edit::clear_range(s, t, r, false);
        crate::edit::shift_from(s, t, r.end, -r.len());
        if let Some(tr) = s.track_mut(t) {
            for l in &mut tr.automation {
                l.clear_range(r.start, r.end);
                l.shift_from(r.end, -r.len());
            }
        }
    }
    s.markers.retain(|m| !r.contains(m.start));
    for m in &mut s.markers {
        if m.start >= r.end {
            m.start -= r.len();
            m.end -= r.len();
        }
    }
    Ok(json!({"removed": r.len()}))
}

fn midi_op(e: &mut Engine, p: &Value, f: impl Fn(&mut [soundcraft_midi::Note])) -> Result<Value> {
    let ids = clip_ids_param(e, p);
    let s = e.session_mut();
    let mut n = 0;
    for id in ids {
        if let Some(c) = s.find_clip_mut(id)
            && let ClipContent::Midi { sequence } = &mut c.content
        {
            f(&mut sequence.notes);
            sequence.sort();
            n += sequence.notes.len();
        }
    }
    Ok(json!({"notes": n}))
}

fn grid_ticks(p: &Value) -> i64 {
    match p.get("grid") {
        Some(Value::Number(n)) => n.as_i64().unwrap_or(240).clamp(1, TICKS_PER_QUARTER * 16),
        Some(Value::String(s)) => match s.trim() {
            "1/1" | "1" | "bar" => TICKS_PER_QUARTER * 4,
            "1/2" => TICKS_PER_QUARTER * 2,
            "1/4" => TICKS_PER_QUARTER,
            "1/8" => TICKS_PER_QUARTER / 2,
            "1/8t" => TICKS_PER_QUARTER / 3,
            "1/16t" => TICKS_PER_QUARTER / 6,
            "1/32" => TICKS_PER_QUARTER / 8,
            "1/64" => TICKS_PER_QUARTER / 16,
            _ => TICKS_PER_QUARTER / 4,
        },
        _ => TICKS_PER_QUARTER / 4,
    }
}

fn quantize(e: &mut Engine, p: &Value) -> Result<Value> {
    let opts = QuantizeOptions {
        grid_ticks: grid_ticks(p),
        strength: (f32_or(p, "strength", 100.0) / 100.0).clamp(0.0, 1.0),
        swing: (f32_or(p, "swing", 0.0) / 100.0).clamp(0.0, 1.0),
        ..QuantizeOptions::default()
    };
    midi_op(e, p, move |n| soundcraft_midi::ops::quantize(n, &opts))
}

fn velocity(e: &mut Engine, p: &Value) -> Result<Value> {
    let op = if let Some(v) = p.get("set").and_then(Value::as_u64) {
        VelocityOp::Set(v.min(127) as u8)
    } else if let Some(v) = p.get("add").and_then(Value::as_i64) {
        VelocityOp::Add(v.clamp(-127, 127) as i32)
    } else if let Some(v) = p.get("scale").and_then(Value::as_f64) {
        VelocityOp::Scale((v as f32).clamp(0.0, 10.0))
    } else {
        VelocityOp::Limit { min: i64_or(p, "min", 1).clamp(1, 127) as u8, max: i64_or(p, "max", 127).clamp(1, 127) as u8 }
    };
    midi_op(e, p, move |n| soundcraft_midi::ops::change_velocity(n, op))
}

fn duration(e: &mut Engine, p: &Value) -> Result<Value> {
    let op = if let Some(v) = p.get("set").and_then(Value::as_i64) {
        DurationOp::Set(v.max(1))
    } else if let Some(v) = p.get("add").and_then(Value::as_i64) {
        DurationOp::Add(v)
    } else if let Some(v) = p.get("scale").and_then(Value::as_f64) {
        DurationOp::Scale((v as f32).clamp(0.01, 100.0))
    } else {
        DurationOp::Legato { gap: i64_or(p, "legato", 0).max(0) }
    };
    midi_op(e, p, move |n| soundcraft_midi::ops::change_duration(n, op))
}

fn add_note(e: &mut Engine, p: &Value) -> Result<Value> {
    let ids = clip_ids_param(e, p);
    let id = *ids.first().ok_or_else(|| bad("event.midi_note", "`clip` required"))?;
    let note = soundcraft_midi::Note {
        pitch: i64_or(p, "pitch", 60).clamp(0, 127) as u8,
        velocity: i64_or(p, "velocity", 100).clamp(1, 127) as u8,
        release_velocity: 64,
        channel: i64_or(p, "channel", 0).clamp(0, 15) as u8,
        start: i64_or(p, "start_ticks", 0).max(0),
        length: i64_or(p, "length_ticks", TICKS_PER_QUARTER).max(1),
    };
    let s = e.session_mut();
    match s.find_clip_mut(id).map(|c| &mut c.content) {
        Some(ClipContent::Midi { sequence }) => {
            sequence.notes.push(note);
            sequence.sort();
            Ok(json!({"notes": sequence.notes.len()}))
        }
        _ => Err(bad("event.midi_note", "not a MIDI clip")),
    }
}

fn add_marker(e: &mut Engine, p: &Value) -> Result<Value> {
    let sel = e.session().edit.selection;
    let at = position_param(e, "markers.add", p, "at")?;
    let st = position_param(e, "markers.add", p, "start")?;
    let en = position_param(e, "markers.add", p, "end")?;
    let kind = match str_param(p, "kind") {
        Some("selection") => MarkerKind::Selection,
        Some("none") => MarkerKind::None,
        _ => MarkerKind::Marker,
    };
    let (a, b) = match (at, st, en) {
        (Some(a), _, _) => (a, a),
        (None, Some(a), Some(b)) => (a, b),
        (None, Some(p), None) | (None, None, Some(p)) => (p, p),
        _ if kind == MarkerKind::Selection => (sel.start, sel.end),
        _ => (if e.transport.playing { e.transport.position } else { sel.start }, sel.start),
    };
    let s = e.session_mut();
    let n = s.markers.len() + 1;
    let name = str_param(p, "name").map_or_else(|| format!("Marker {n}"), str::to_string);
    let id = s.add_marker(&name, kind, a.max(0), b.max(a).max(0));
    if let Some(m) = s.markers.iter_mut().find(|m| m.id == id) {
        m.ruler = i64_or(p, "ruler", 1).clamp(1, 5) as u8;
    }
    let number = s.marker(id).map_or(0, |m| m.number);
    Ok(json!({"id": id, "number": number}))
}

fn marker_index(e: &Engine, p: &Value, cmd: &str) -> Result<usize> {
    let s = e.session();
    let idx = if let Some(n) = p.get("number").and_then(Value::as_u64) {
        s.markers.iter().position(|m| u64::from(m.number) == n)
    } else if let Some(n) = p.get("id").and_then(Value::as_u64) {
        s.markers.iter().position(|m| m.id.0 == n)
    } else if let Some(name) = str_param(p, "name") {
        s.markers.iter().position(|m| m.name == name)
    } else {
        None
    };
    idx.ok_or_else(|| bad(cmd, "unknown memory location (give `number`, `id` or `name`)"))
}

fn delete_marker(e: &mut Engine, p: &Value) -> Result<Value> {
    let i = marker_index(e, p, "markers.delete")?;
    e.session_mut().markers.remove(i);
    Ok(json!({}))
}

fn edit_marker(e: &mut Engine, p: &Value) -> Result<Value> {
    let i = marker_index(e, p, "markers.edit")?;
    let at = position_param(e, "markers.edit", p, "at")?;
    let color = match p.get("color") {
        None => None,
        Some(Value::Null) => Some(None),
        Some(Value::Array(a)) if a.len() == 3 => {
            let ch = |i: usize| a.get(i).and_then(Value::as_u64).and_then(|v| u8::try_from(v).ok());
            match (ch(0), ch(1), ch(2)) {
                (Some(r), Some(g), Some(b)) => Some(Some([r, g, b])),
                _ => return Err(bad("markers.edit", "`color` components must be integers 0..=255")),
            }
        }
        Some(_) => return Err(bad("markers.edit", "`color` must be [r,g,b] or null")),
    };
    let s = e.session_mut();
    let Some(m) = s.markers.get_mut(i) else { return Err(bad("markers.edit", "gone")) };
    if let Some(c) = color {
        m.color = c;
    }
    // `name` is the documented new name; when it was the selector it already matches, so this is a no-op.
    if let Some(n) = str_param(p, "rename").or_else(|| str_param(p, "name")) {
        m.name = n.to_string();
    }
    if let Some(c) = str_param(p, "comments") {
        m.comments = c.to_string();
    }
    if let Some(a) = at {
        let len = m.end - m.start;
        m.start = a.max(0);
        m.end = m.start + len;
    }
    s.markers.sort_by_key(|m| m.start);
    Ok(json!({}))
}

fn recall_marker(e: &mut Engine, p: &Value) -> Result<Value> {
    let i = marker_index(e, p, "markers.recall")?;
    let s = e.session();
    let Some(m) = s.markers.get(i).cloned() else { return Err(bad("markers.recall", "gone")) };
    let s = e.session_mut();
    match m.kind {
        MarkerKind::Marker => s.edit.selection = soundcraft_time::Range::point(m.start),
        MarkerKind::Selection => s.edit.selection = soundcraft_time::Range::new(m.start, m.end),
        MarkerKind::None => {}
    }
    s.edit.playhead = m.start;
    e.transport_requests.push(crate::TransportRequest::Locate(m.start));
    Ok(json!({"at": m.start}))
}

fn step_marker(e: &mut Engine, dir: i64) -> Result<Value> {
    let cur = e.session().edit.selection.start;
    let s = e.session();
    let target = if dir > 0 {
        s.markers.iter().map(|m| m.start).filter(|t| *t > cur).min()
    } else {
        s.markers.iter().map(|m| m.start).filter(|t| *t < cur).max()
    };
    let Some(t) = target else { return Ok(json!({"moved": false})) };
    let s = e.session_mut();
    s.edit.selection = soundcraft_time::Range::point(t);
    s.edit.playhead = t;
    e.transport_requests.push(crate::TransportRequest::Locate(t));
    Ok(json!({"at": t}))
}

#[cfg(test)]
mod tests {
    use crate::demo::demo_engine;
    use serde_json::json;

    /// The tempo-ruler dialog runs `event.tempo` with `bpm` and `at`; the change must land at that position.
    #[test]
    fn tempo_change_at_position_is_applied() {
        let mut e = demo_engine();
        let sr = e.session().sample_rate;
        let at = sr.samples(4.0);
        e.execute("event.tempo", &json!({"bpm": 133.5, "at": at})).unwrap();
        let s = e.session();
        let tick = s.tempo.samples_to_ticks(at, s.sample_rate);
        assert!((s.tempo.tempo_at_tick(tick) - 133.5).abs() < 1e-9);
    }

    /// Changing the initial meter keeps a later meter change at its bar (issue #250).
    #[test]
    fn earlier_meter_change_keeps_later_meter_on_its_bar() {
        use soundcraft_time::BarBeat;
        let mut e = crate::Engine::default();
        assert_eq!(e.session().sample_rate.hz(), 48_000);
        e.execute("event.meter", &json!({"numerator": 3, "denominator": 4, "at": 192_000})).unwrap();
        e.execute("event.meter", &json!({"numerator": 6, "denominator": 4, "at": 0})).unwrap();
        let map = &e.session().tempo;
        let later = map.meters().get(1).copied().unwrap();
        assert_eq!((later.numerator, later.denominator), (3, 4));
        assert_eq!(map.bar_beat_at_tick(later.tick), BarBeat { bar: 3, beat: 1, tick: 0 });
        let bar2_beat3 = BarBeat { bar: 2, beat: 3, tick: 0 };
        assert_eq!(map.bar_beat_at_tick(map.tick_at_bar_beat(bar2_beat3)), bar2_beat3);
    }

    fn marker_name(e: &crate::Engine, id: u64) -> String {
        e.session().markers.iter().find(|m| m.id.0 == id).map(|m| m.name.clone()).unwrap_or_default()
    }

    /// `markers.edit` documents `name`: with `id` or `number` selecting the location, it is the new name.
    #[test]
    fn edit_marker_renames_with_documented_name() {
        let mut e = crate::Engine::default();
        let verse = e.execute("markers.add", &json!({"name": "Verse", "at": 1000})).unwrap();
        let outro = e.execute("markers.add", &json!({"name": "Outro", "at": 3000})).unwrap();
        let (verse_id, outro_id) = (verse["id"].as_u64().unwrap(), outro["id"].as_u64().unwrap());

        e.execute("markers.edit", &json!({"id": verse_id, "name": "Chorus"})).unwrap();
        assert_eq!((marker_name(&e, verse_id), marker_name(&e, outro_id)), ("Chorus".into(), "Outro".into()));

        e.execute("markers.edit", &json!({"number": outro["number"], "name": "Bridge"})).unwrap();
        assert_eq!((marker_name(&e, verse_id), marker_name(&e, outro_id)), ("Chorus".into(), "Bridge".into()));

        e.execute("markers.edit", &json!({"name": "Bridge", "comments": "last"})).unwrap();
        let s = e.session();
        let bridge = s.markers.iter().find(|m| m.id.0 == outro_id).unwrap();
        assert_eq!((bridge.name.as_str(), bridge.comments.as_str()), ("Bridge", "last"));
    }

    /// `markers.add` with only `start` or only `end` places the marker there, not at the edit selection.
    #[test]
    fn markers_add_start_or_end_alone_is_used() {
        let mut e = demo_engine();
        assert_ne!(e.session().edit.selection.start, 96_000);
        let a = e.execute("markers.add", &json!({"name": "A", "start": 96_000})).unwrap();
        let b = e.execute("markers.add", &json!({"name": "B", "end": 96_000})).unwrap();
        let c = e.execute("markers.add", &json!({"name": "C", "start": 1000, "end": 2000})).unwrap();
        let find = |r: &serde_json::Value| e.session().markers.iter().find(|m| Some(m.id.0) == r["id"].as_u64()).map(|m| (m.start, m.end));
        assert_eq!(find(&a), Some((96_000, 96_000)));
        assert_eq!(find(&b), Some((96_000, 96_000)));
        assert_eq!(find(&c), Some((1000, 2000)));
    }

    /// `markers.edit` documents `color`: it sets the memory location's RGB colour, undoably; `null` clears it.
    #[test]
    fn edit_marker_sets_documented_color() {
        let mut e = demo_engine();
        let n = e.session().markers.first().unwrap().number;
        let color = |e: &crate::Engine| e.session().markers.iter().find(|m| m.number == n).unwrap().color;
        assert_eq!(color(&e), None);

        e.execute("markers.edit", &json!({"number": n, "color": [255, 0, 0]})).unwrap();
        assert_eq!(color(&e), Some([255, 0, 0]));
        assert!(e.undo());
        assert_eq!(color(&e), None);
        assert!(e.redo());
        assert_eq!(color(&e), Some([255, 0, 0]));

        e.execute("markers.edit", &json!({"number": n, "color": null})).unwrap();
        assert_eq!(color(&e), None);

        for bad in [json!([1, 2]), json!([1, 2, 300]), json!("red"), json!([1, 2, -3])] {
            assert!(e.execute("markers.edit", &json!({"number": n, "color": bad})).is_err());
            assert_eq!(color(&e), None);
        }
    }

    /// Constant replaces the tempo over [start, end) only; the tempo at the old end tick carries on after it.
    #[test]
    fn tempo_constant_is_bounded_by_its_range() {
        let mut e = crate::Engine::default();
        e.execute("event.tempo", &json!({"bpm": 100.0, "at": 0})).unwrap();
        e.execute("event.tempo", &json!({"bpm": 140.0, "at": 48_000})).unwrap();
        let s = e.session();
        let (t0, t1) = (s.tempo.samples_to_ticks(24_000, s.sample_rate), s.tempo.samples_to_ticks(96_000, s.sample_rate));
        let mid = s.tempo.samples_to_ticks(72_000, s.sample_rate);
        let after = s.tempo.tempo_at_tick(t1);
        e.execute("event.tempo_constant", &json!({"bpm": 90.0, "start": 24_000, "end": 96_000})).unwrap();
        let tempo = &e.session().tempo;
        assert_eq!(tempo.tempo_at_tick(t0 - 1), 100.0, "before the range");
        assert_eq!(tempo.tempo_at_tick(t0), 90.0);
        assert_eq!(tempo.tempo_at_tick(mid), 90.0, "the interior change is replaced");
        assert_eq!(tempo.tempo_at_tick(t1), after, "the old tempo resumes at the end");
        assert_eq!(after, 140.0);
    }
}
