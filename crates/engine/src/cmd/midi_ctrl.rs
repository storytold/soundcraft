//! MIDI controller editing: the MIDI Editor's controller lanes (CC, pitch bend, aftertouch,
//! program change), what the Pencil draws in them, and which lanes each track shows.

use super::midi::{indices, seq_mut};
use super::*;
use crate::cmd;
use serde_json::json;
use soundcraft_midi::{CtrlEvent, CtrlKind, MAX_POINTS, PencilShape, Sequence, ShapeStroke};
use soundcraft_model::ClipContent;
use soundcraft_time::TICKS_PER_QUARTER;
use std::collections::HashSet;

/// How far apart a stroke's events are: a 64th note, about 30 ms at 120 BPM.
pub const DRAW_STEP: i64 = TICKS_PER_QUARTER / 16;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(query "midi.ctrls", "List MIDI Controllers", [], None, "{clip, kind?: cc0..cc127|pitchbend|aftertouch|program|polyat<pitch>}", always, ctrls),
        cmd!(
            "midi.ctrl_add",
            "Add Controller Event",
            [],
            None,
            "{clip, kind, tick, value, channel?} (replaces the kind's event at that tick)",
            always,
            ctrl_add
        ),
        cmd!(
            "midi.ctrls_move",
            "Move Controller Events",
            [],
            None,
            "{clip, indices: [n], ticks?: 0, value?: 0 (added to each value), copy?: false}",
            always,
            ctrls_move
        ),
        cmd!("midi.ctrl_delete", "Delete Controller Events", [], None, "{clip, indices: [n]}", always, ctrl_delete),
        cmd!(
            "midi.ctrls_draw",
            "Draw Controller Events",
            [],
            None,
            "{clip, kind, channel?, points: [[tick, value]] (Free Hand) | shape: line|triangle|square|random|parabolic|s_curve, from: [tick, value], to: [tick, value], period?: ticks (the grid; Triangle/Square/Random), step?: ticks (default a 64th), seed?} — replaces the kind's events between the first and last tick",
            always,
            ctrls_draw
        ),
        cmd!(noundo "midi.lanes", "MIDI Controller Lanes", [], None, "{track, lanes: [kind]} — the controller lanes the MIDI Editor shows for the track", always, lanes),
        cmd!(noundo "edit.pencil_shape", "Pencil Shape", [], None, "{shape: free_hand|line|triangle|square|random|parabolic|s_curve}", always, pencil_shape),
    ]
}

fn kind_param(p: &Value, cmd: &str) -> Result<CtrlKind> {
    str_param(p, "kind")
        .and_then(CtrlKind::from_id)
        .ok_or_else(|| bad(cmd, "`kind` must be cc0..cc127, pitchbend, aftertouch, program or polyat<pitch>"))
}

/// A value rounded into the kind's range (a non-number lands on the kind's resting value).
pub fn clamp_value(kind: CtrlKind, v: f64) -> i32 {
    let (lo, hi) = kind.range();
    if v.is_finite() { v.round().clamp(f64::from(lo), f64::from(hi)) as i32 } else { kind.rest_value() }
}

/// The channel new events of `kind` go on: that of the kind's events already there, else the
/// notes', else the first channel.
fn channel_for(seq: &Sequence, kind: CtrlKind, p: &Value) -> u8 {
    match p.get("channel").and_then(Value::as_i64) {
        Some(c) => c.clamp(0, 15) as u8,
        None => seq.ctrls.iter().find(|c| c.kind == kind).map(|c| c.channel).or_else(|| seq.notes.first().map(|n| n.channel)).unwrap_or(0),
    }
}

fn event_json(i: usize, c: &CtrlEvent) -> Value {
    json!({"index": i, "tick": c.tick, "channel": c.channel, "kind": c.kind.id(), "value": c.value})
}

fn ctrls(e: &mut Engine, p: &Value) -> Result<Value> {
    let id = clip_id(p, "midi.ctrls")?;
    let kind = if p.get("kind").is_some() { Some(kind_param(p, "midi.ctrls")?) } else { None };
    match e.session().find_clip(id).map(|(_, c)| &c.content) {
        Some(ClipContent::Midi { sequence }) => {
            let list: Vec<Value> =
                sequence.ctrls.iter().enumerate().filter(|(_, c)| kind.is_none_or(|k| c.kind == k)).map(|(i, c)| event_json(i, c)).collect();
            Ok(json!({"ctrls": list}))
        }
        Some(_) => Err(bad("midi.ctrls", "not a MIDI clip")),
        None => Err(bad("midi.ctrls", "no such clip")),
    }
}

/// Puts `new` into the sequence, each replacing any event of the same kind and channel at its
/// tick (among `new` the last one wins), and returns where they landed after sorting.
fn insert(seq: &mut Sequence, new: Vec<CtrlEvent>) -> Vec<usize> {
    let mut keys = HashSet::with_capacity(new.len());
    let mut unique: Vec<CtrlEvent> = Vec::with_capacity(new.len());
    for c in new.into_iter().rev() {
        if keys.insert((c.tick, c.channel, c.kind)) {
            unique.push(c);
        }
    }
    seq.ctrls.retain(|c| !keys.contains(&(c.tick, c.channel, c.kind)));
    seq.ctrls.extend(unique);
    seq.sort();
    seq.ctrls.iter().enumerate().filter(|(_, c)| keys.contains(&(c.tick, c.channel, c.kind))).map(|(i, _)| i).collect()
}

fn ctrl_add(e: &mut Engine, p: &Value) -> Result<Value> {
    let id = clip_id(p, "midi.ctrl_add")?;
    let kind = kind_param(p, "midi.ctrl_add")?;
    let tick = i64_or(p, "tick", 0).max(0);
    let value = clamp_value(kind, f64_or(p, "value", f64::from(kind.rest_value())));
    let seq = seq_mut(e, id, "midi.ctrl_add")?;
    let channel = channel_for(seq, kind, p);
    let at = insert(seq, vec![CtrlEvent { tick, channel, kind, value }]);
    Ok(json!({"index": at.first(), "ctrls": seq.ctrls.len()}))
}

fn ctrls_move(e: &mut Engine, p: &Value) -> Result<Value> {
    let id = clip_id(p, "midi.ctrls_move")?;
    let idx: HashSet<usize> = indices(p).into_iter().collect();
    let dt = i64_or(p, "ticks", 0);
    let dv = i64_or(p, "value", 0);
    let copy = bool_or(p, "copy", false);
    let seq = seq_mut(e, id, "midi.ctrls_move")?;
    let mut moved = Vec::with_capacity(idx.len());
    for (i, c) in seq.ctrls.iter().enumerate() {
        if idx.contains(&i) {
            let (lo, hi) = c.kind.range();
            let value = i64::from(c.value).saturating_add(dv).clamp(i64::from(lo), i64::from(hi)) as i32;
            moved.push(CtrlEvent { tick: c.tick.saturating_add(dt).max(0), value, ..*c });
        }
    }
    if !copy {
        let mut i = 0;
        seq.ctrls.retain(|_| {
            let keep = !idx.contains(&i);
            i += 1;
            keep
        });
    }
    let count = moved.len();
    Ok(json!({"moved": count, "indices": insert(seq, moved)}))
}

fn ctrl_delete(e: &mut Engine, p: &Value) -> Result<Value> {
    let id = clip_id(p, "midi.ctrl_delete")?;
    let idx: HashSet<usize> = indices(p).into_iter().collect();
    let seq = seq_mut(e, id, "midi.ctrl_delete")?;
    let before = seq.ctrls.len();
    let mut i = 0;
    seq.ctrls.retain(|_| {
        let keep = !idx.contains(&i);
        i += 1;
        keep
    });
    Ok(json!({"deleted": before - seq.ctrls.len()}))
}

/// `[tick, value]` from a two-element array.
fn tick_value(v: &Value) -> Option<(i64, f64)> {
    let a = v.as_array()?;
    Some((a.first()?.as_i64()?, a.get(1)?.as_f64()?))
}

/// The stroke a `midi.ctrls_draw` call describes, as sorted `(tick, value)` points with one per tick.
pub fn stroke_points(p: &Value, cmd: &str) -> Result<Vec<(i64, f64)>> {
    let mut pts: Vec<(i64, f64)> = if let Some(arr) = p.get("points").and_then(Value::as_array) {
        arr.iter().take(MAX_POINTS as usize).filter_map(tick_value).collect()
    } else {
        let shape = match str_param(p, "shape") {
            Some(s) => PencilShape::from_id(s).ok_or_else(|| bad(cmd, format!("unknown shape `{s}`")))?,
            None => PencilShape::Line,
        };
        let (Some(from), Some(to)) = (p.get("from").and_then(tick_value), p.get("to").and_then(tick_value)) else {
            return Err(bad(cmd, "`points`, or `from` and `to` as [tick, value], required"));
        };
        let seed = p.get("seed").and_then(Value::as_u64).unwrap_or(0);
        ShapeStroke { shape, from, to, period: i64_or(p, "period", TICKS_PER_QUARTER / 4), seed }.points(i64_or(p, "step", DRAW_STEP))
    };
    for pt in &mut pts {
        pt.0 = pt.0.max(0);
    }
    pts.sort_by_key(|pt| pt.0);
    // Several points on one tick: the last one drawn wins.
    pts.dedup_by(|later, kept| {
        let same = later.0 == kept.0;
        if same {
            *kept = *later;
        }
        same
    });
    if pts.is_empty() {
        return Err(bad(cmd, "no points to draw"));
    }
    Ok(pts)
}

/// Replaces the kind's events between the stroke's first and last tick (on every channel, as the
/// lane shows them all) with the stroke. Points that repeat the value before them are left out:
/// a controller holds its value until the next event.
fn ctrls_draw(e: &mut Engine, p: &Value) -> Result<Value> {
    const CMD: &str = "midi.ctrls_draw";
    let id = clip_id(p, CMD)?;
    let kind = kind_param(p, CMD)?;
    let pts = stroke_points(p, CMD)?;
    let (lo, hi) = (pts.first().map_or(0, |pt| pt.0), pts.last().map_or(0, |pt| pt.0));
    let seq = seq_mut(e, id, CMD)?;
    let channel = channel_for(seq, kind, p);
    seq.ctrls.retain(|c| c.kind != kind || c.tick < lo || c.tick > hi);
    let mut new = Vec::with_capacity(pts.len());
    let mut last = None;
    for (tick, v) in pts {
        let value = clamp_value(kind, v);
        if last != Some(value) {
            new.push(CtrlEvent { tick, channel, kind, value });
            last = Some(value);
        }
    }
    let count = new.len();
    Ok(json!({"drawn": count, "start": lo, "end": hi, "indices": insert(seq, new)}))
}

fn lanes(e: &mut Engine, p: &Value) -> Result<Value> {
    const CMD: &str = "midi.lanes";
    let t = track_param(e, CMD, p, "track")?.ok_or_else(|| bad(CMD, "`track` required"))?;
    let arr = p.get("lanes").and_then(Value::as_array).ok_or_else(|| bad(CMD, "`lanes` must be a list of controller kinds"))?;
    let mut lanes: Vec<CtrlKind> = Vec::new();
    for v in arr.iter().take(128) {
        let k = v.as_str().and_then(CtrlKind::from_id).ok_or_else(|| bad(CMD, format!("unknown controller {v}")))?;
        if !lanes.contains(&k) {
            lanes.push(k);
        }
    }
    let ids: Vec<String> = lanes.iter().map(|k| k.id()).collect();
    match e.session().track(t) {
        Some(track) if !track.kind.is_midi() => return Err(bad(CMD, "not a MIDI or instrument track")),
        Some(track) if track.midi_lanes == lanes => return Ok(json!({"lanes": ids})),
        _ => {}
    }
    let track = e.session_mut().track_mut(t).ok_or_else(|| bad(CMD, "no such track"))?;
    track.midi_lanes = lanes;
    Ok(json!({"lanes": ids}))
}

fn pencil_shape(e: &mut Engine, p: &Value) -> Result<Value> {
    let shape = str_param(p, "shape").and_then(PencilShape::from_id).ok_or_else(|| bad("edit.pencil_shape", "unknown shape"))?;
    if e.session().edit.pencil_shape != shape {
        e.session_mut().edit.pencil_shape = shape;
    }
    Ok(json!({"shape": shape.id()}))
}

#[cfg(test)]
mod tests {
    use super::*;
    use soundcraft_model::{ChannelFormat, TrackKind};

    fn clip(e: &mut Engine) -> (u64, u64) {
        let t = e.session_mut().add_track(TrackKind::Instrument, ChannelFormat::Stereo, Some("Keys"));
        let clip = e.execute("midi.new_clip", &json!({"track": t.0})).unwrap()["clip"].as_u64().unwrap();
        (t.0, clip)
    }

    fn events(e: &mut Engine, clip: u64, kind: &str) -> Vec<(i64, i64)> {
        let r = e.execute("midi.ctrls", &json!({"clip": clip, "kind": kind})).unwrap();
        r["ctrls"].as_array().unwrap().iter().map(|c| (c["tick"].as_i64().unwrap(), c["value"].as_i64().unwrap())).collect()
    }

    #[test]
    fn add_replaces_the_event_at_its_tick_and_values_stay_in_range() {
        let mut e = Engine::default();
        let (_, c) = clip(&mut e);
        e.execute("midi.ctrl_add", &json!({"clip": c, "kind": "cc1", "tick": 480, "value": 20})).unwrap();
        e.execute("midi.ctrl_add", &json!({"clip": c, "kind": "cc1", "tick": 0, "value": 300})).unwrap();
        let r = e.execute("midi.ctrl_add", &json!({"clip": c, "kind": "cc1", "tick": 480, "value": 90})).unwrap();
        assert_eq!(r["index"], 1);
        assert_eq!(events(&mut e, c, "cc1"), vec![(0, 127), (480, 90)]);
        e.execute("midi.ctrl_add", &json!({"clip": c, "kind": "pitchbend", "tick": 10, "value": -9000})).unwrap();
        assert_eq!(events(&mut e, c, "pitchbend"), vec![(10, -8192)]);
        assert!(e.execute("midi.ctrl_add", &json!({"clip": c, "kind": "cc200", "tick": 0})).is_err());
    }

    #[test]
    fn move_shifts_time_and_value_and_reports_the_new_indices() {
        let mut e = Engine::default();
        let (_, c) = clip(&mut e);
        for (tick, value) in [(0, 10), (480, 20), (960, 30)] {
            e.execute("midi.ctrl_add", &json!({"clip": c, "kind": "cc7", "tick": tick, "value": value})).unwrap();
        }
        let r = e.execute("midi.ctrls_move", &json!({"clip": c, "indices": [0], "ticks": 720, "value": 5})).unwrap();
        assert_eq!(r["indices"], json!([1]));
        assert_eq!(events(&mut e, c, "cc7"), vec![(480, 20), (720, 15), (960, 30)]);
        // Landing on another event's tick replaces it; a copy leaves the original.
        e.execute("midi.ctrls_move", &json!({"clip": c, "indices": [0], "ticks": 480, "copy": true})).unwrap();
        assert_eq!(events(&mut e, c, "cc7"), vec![(480, 20), (720, 15), (960, 20)]);
        e.execute("midi.ctrls_move", &json!({"clip": c, "indices": [0, 1, 2], "ticks": i64::MIN, "value": i64::MAX})).unwrap();
        assert_eq!(events(&mut e, c, "cc7"), vec![(0, 127)], "all three land on tick 0: one event remains");
    }

    #[test]
    fn a_line_replaces_only_its_span_and_skips_repeated_values() {
        let mut e = Engine::default();
        let (_, c) = clip(&mut e);
        for tick in [0, 600, 2000] {
            e.execute("midi.ctrl_add", &json!({"clip": c, "kind": "cc1", "tick": tick, "value": 5})).unwrap();
        }
        e.execute("midi.ctrl_add", &json!({"clip": c, "kind": "cc7", "tick": 600, "value": 100})).unwrap();
        let r =
            e.execute("midi.ctrls_draw", &json!({"clip": c, "kind": "cc1", "shape": "line", "from": [480, 0], "to": [960, 8], "step": 60})).unwrap();
        assert_eq!((r["start"].as_i64(), r["end"].as_i64()), (Some(480), Some(960)));
        assert_eq!(
            events(&mut e, c, "cc1"),
            vec![(0, 5), (480, 0), (540, 1), (600, 2), (660, 3), (720, 4), (780, 5), (840, 6), (900, 7), (960, 8), (2000, 5)]
        );
        assert_eq!(events(&mut e, c, "cc7"), vec![(600, 100)], "other controllers are untouched");
        let flat = e.execute("midi.ctrls_draw", &json!({"clip": c, "kind": "cc1", "from": [0, 64], "to": [2000, 64]})).unwrap();
        assert_eq!(flat["drawn"], 1);
        assert_eq!(events(&mut e, c, "cc1"), vec![(0, 64)]);
        e.execute("edit.undo", &json!({})).unwrap();
        assert_eq!(events(&mut e, c, "cc1").len(), 11, "one undo step per stroke");
    }

    #[test]
    fn free_hand_points_are_sorted_and_one_per_tick() {
        let mut e = Engine::default();
        let (_, c) = clip(&mut e);
        e.execute("midi.ctrls_draw", &json!({"clip": c, "kind": "aftertouch", "points": [[300, 40], [100, 10], [200, 20], [200, 30], [-5, 1]]}))
            .unwrap();
        assert_eq!(events(&mut e, c, "aftertouch"), vec![(0, 1), (100, 10), (200, 30), (300, 40)]);
        assert!(e.execute("midi.ctrls_draw", &json!({"clip": c, "kind": "cc1", "points": []})).is_err());
        assert!(e.execute("midi.ctrls_draw", &json!({"clip": c, "kind": "cc1", "shape": "spiral", "from": [0, 0], "to": [1, 1]})).is_err());
    }

    #[test]
    fn shapes_draw_at_the_grid() {
        let mut e = Engine::default();
        let (_, c) = clip(&mut e);
        e.execute("midi.ctrls_draw", &json!({"clip": c, "kind": "cc11", "shape": "square", "from": [0, 0], "to": [960, 127], "period": 480}))
            .unwrap();
        assert_eq!(events(&mut e, c, "cc11"), vec![(0, 0), (240, 127), (480, 0), (720, 127), (960, 0)]);
        let huge = json!({"clip": c, "kind": "cc11", "shape": "triangle", "from": [0, 0], "to": [i64::MAX, 127], "period": 1, "step": 1});
        let r = e.execute("midi.ctrls_draw", &huge).unwrap();
        assert!(r["drawn"].as_i64().unwrap() <= 2 * MAX_POINTS + 1);
    }

    #[test]
    fn delete_removes_the_listed_events() {
        let mut e = Engine::default();
        let (_, c) = clip(&mut e);
        for tick in [0, 480, 960] {
            e.execute("midi.ctrl_add", &json!({"clip": c, "kind": "cc64", "tick": tick, "value": 127})).unwrap();
        }
        let r = e.execute("midi.ctrl_delete", &json!({"clip": c, "indices": [0, 2, 99]})).unwrap();
        assert_eq!(r["deleted"], 2);
        assert_eq!(events(&mut e, c, "cc64"), vec![(480, 127)]);
    }

    #[test]
    fn lanes_belong_to_midi_tracks_and_are_not_an_undo_step() {
        let mut e = Engine::default();
        let (t, _) = clip(&mut e);
        let undo_before = e.undo_history();
        let r = e.execute("midi.lanes", &json!({"track": t, "lanes": ["cc1", "PB", "cc1", "cc 64"]})).unwrap();
        assert_eq!(r["lanes"], json!(["cc1", "pitchbend", "cc64"]));
        assert_eq!(e.session().track(TrackId(t)).unwrap().midi_lanes, vec![CtrlKind::Cc(1), CtrlKind::PitchBend, CtrlKind::Cc(64)]);
        assert_eq!(e.undo_history(), undo_before);
        assert!(e.execute("midi.lanes", &json!({"track": t, "lanes": ["volume knob"]})).is_err());
        let audio = e.session_mut().add_track(TrackKind::Audio, ChannelFormat::Mono, None);
        assert!(e.execute("midi.lanes", &json!({"track": audio.0, "lanes": ["cc1"]})).is_err());
    }

    #[test]
    fn pencil_shape_is_an_edit_setting() {
        let mut e = Engine::default();
        assert_eq!(e.session().edit.pencil_shape, PencilShape::FreeHand);
        e.execute("edit.pencil_shape", &json!({"shape": "s-curve"})).unwrap();
        assert_eq!(e.session().edit.pencil_shape, PencilShape::SCurve);
        assert!(e.execute("edit.pencil_shape", &json!({"shape": "zigzag"})).is_err());
    }
}
