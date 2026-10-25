//! More Event menu commands: the operations windows, Move Song Start, tempo curves, MIDI
//! operations (split notes, input quantize, step input, restore/flatten performance), MIDI track
//! offsets and real-time properties, chord extraction, Beat Detective, Identify Beat and
//! Retrospective Record.
use super::{
    CommandSpec, always, bad, bool_or, clip_ids_param, f32_or, f64_or, has_range, has_selection, has_tracks, i64_or, position_param, range_param,
    str_param, track_param, tracks_param, tracks_required,
};

use super::more_util::{store_value, track_key, values_with_prefix};
use crate::{Engine, Result, cmd};
use serde_json::{Value, json};
use soundcraft_midi::Note;
use soundcraft_midi::ops::QuantizeOptions;
use soundcraft_model::{ChannelFormat, Clip, ClipContent, ClipId, MarkerKind, Session, TrackId, TrackKind};
use soundcraft_time::{MAX_POSITION, Range, Samples, TICKS_PER_QUARTER, TempoMap};

/// Most tempo events one curve may write.
const MAX_TEMPO_EVENTS: i64 = 10_000;

#[derive(Clone, Copy)]
enum Curve {
    Linear,
    Parabolic,
    Sigmoid,
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(query "event.time_ops_window", "Time Operations Window", ["Event", "Time Operations"], None, "{} — tempo/meter map, song start and selection for the Time Operations window", always, |e, _| Ok(time_state(e.session()))),
        cmd!(query "event.tempo_ops_window", "Tempo Operations Window", ["Event", "Tempo Operations"], None, "{}", always, |e, _| Ok(time_state(e.session()))),
        cmd!(query "event.midi_ops_window", "MIDI Operations Window", ["Event", "MIDI Operations"], None, "{} — input quantize and stored performances", always, |e, _| {
            let s = e.session();
            Ok(json!({"input_quantize": input_quantize_state(s), "performances": s.midi_originals.len(), "rtp": values_with_prefix(&s.edit, "rtp.")}))
        }),
        cmd!(
            "event.move_song_start",
            "Move Song Start...",
            ["Event", "Time Operations"],
            None,
            "{to: position} — moves the song start; tick-based clips and later tempo/meter events move with it",
            always,
            move_song_start
        ),
        cmd!(
            "event.tempo_linear",
            "Linear...",
            ["Event", "Tempo Operations"],
            None,
            "{start?, end?, start_bpm?, end_bpm, resolution?: '1/8'|ticks}",
            always,
            |e, p| tempo_curve(e, p, Curve::Linear, "event.tempo_linear")
        ),
        cmd!(
            "event.tempo_parabolic",
            "Parabolic...",
            ["Event", "Tempo Operations"],
            None,
            "{start?, end?, start_bpm?, end_bpm, curvature?: -1..1 (0.5), resolution?}",
            always,
            |e, p| tempo_curve(e, p, Curve::Parabolic, "event.tempo_parabolic")
        ),
        cmd!(
            "event.tempo_s_curve",
            "S-Curve...",
            ["Event", "Tempo Operations"],
            None,
            "{start?, end?, start_bpm?, end_bpm, resolution?}",
            always,
            |e, p| tempo_curve(e, p, Curve::Sigmoid, "event.tempo_s_curve")
        ),
        cmd!(
            "event.tempo_stretch",
            "Stretch...",
            ["Event", "Tempo Operations"],
            None,
            "{start?, end?, factor? | length?: new length} — scales the tempo inside the selection so it lasts `factor` times as long",
            always,
            tempo_stretch
        ),
        cmd!(
            "event.select_split_notes",
            "Select/Split Notes...",
            ["Event", "MIDI Operations"],
            None,
            "{clips?, pitch_from?: 0, pitch_to?: 127, action?: select|split} — split moves the matching notes to a new MIDI track",
            has_selection,
            select_split
        ),
        cmd!(noundo "event.input_quantize", "Input Quantize...", ["Event", "MIDI Operations"], None, "{enabled?: bool, grid?: '1/16'|ticks, strength?: 0..100, swing?: 0..100}", always, input_quantize),
        cmd!(
            "event.step_input",
            "Step Input...",
            ["Event", "MIDI Operations"],
            None,
            "{track?, pitch?: 60 | pitches?: [..], duration?: '1/8'|ticks, velocity?: 100, rest?: bool, advance?: true}",
            has_tracks,
            step_input
        ),
        cmd!(
            "event.restore_performance",
            "Restore Performance...",
            ["Event", "MIDI Operations"],
            None,
            "{clips?} — returns MIDI clips to their stored original performance",
            has_selection,
            |e, p| performance(e, p, false)
        ),
        cmd!(
            "event.flatten_performance",
            "Flatten Performance...",
            ["Event", "MIDI Operations"],
            None,
            "{clips?} — makes the current notes the performance that Restore returns to",
            has_selection,
            |e, p| performance(e, p, true)
        ),
        cmd!(noundo "event.midi_track_offsets", "MIDI Track Offsets", ["Event"], None, "{track?, offset?: samples|{seconds}} — sets (or lists) per-track MIDI playback offsets", always, midi_offsets),
        cmd!(noundo "event.midi_rtp", "MIDI Real-Time Properties", ["Event"], None, "{tracks?, velocity?: ±127, transpose?: ±127, duration?: % (100), delay?: ticks, clear?: bool}", always, midi_rtp),
        cmd!(
            "event.extract_chords",
            "Extract Chords from Selection",
            ["Event"],
            None,
            "{tracks?, start?, end?} — names the chord in each bar and adds it as a marker on Markers 2",
            has_range,
            extract_chords
        ),
        cmd!(
            "event.beat_detective",
            "Beat Detective",
            ["Event"],
            None,
            "{tracks?, start?, end?, sensitivity?: 0.5, separate?: false, set_tempo?: false}",
            has_range,
            beat_detective
        ),
        cmd!(
            "event.identify_beat",
            "Identify Beat...",
            ["Event"],
            Some("Cmd+I"),
            "{start?, end?, bars?: 1, beats?: 0} — sets the tempo so the selection lasts that many bars",
            has_range,
            identify_beat
        ),
        cmd!(
            "event.retrospective_record",
            "Retrospective Record",
            ["Event"],
            None,
            "{track?, notes?: [{pitch, start?: position | start_ticks?, length_ticks?: 240, velocity?: 100}]} — turns captured MIDI into a clip (input quantize applies)",
            always,
            retro_record
        ),
    ]
}

fn time_state(s: &Session) -> Value {
    json!({
        "tempos": s.tempo.tempos(),
        "meters": s.tempo.meters(),
        "song_start": s.edit.value("song.start", 0.0),
        "selection": s.edit.selection,
        "sample_rate": s.sample_rate.hz(),
    })
}

/// Duration in ticks from a note-value string or a number of ticks.
fn note_ticks(v: Option<&Value>, default: i64) -> i64 {
    match v {
        Some(Value::Number(n)) => n.as_i64().unwrap_or(default).clamp(1, TICKS_PER_QUARTER * 64),
        Some(Value::String(s)) => match s.trim() {
            "1/1" | "1" | "bar" => TICKS_PER_QUARTER * 4,
            "1/2" => TICKS_PER_QUARTER * 2,
            "1/4" => TICKS_PER_QUARTER,
            "1/4t" => TICKS_PER_QUARTER * 2 / 3,
            "1/8" => TICKS_PER_QUARTER / 2,
            "1/8t" => TICKS_PER_QUARTER / 3,
            "1/16" => TICKS_PER_QUARTER / 4,
            "1/16t" => TICKS_PER_QUARTER / 6,
            "1/32" => TICKS_PER_QUARTER / 8,
            "1/64" => TICKS_PER_QUARTER / 16,
            _ => default,
        },
        _ => default,
    }
}

fn move_song_start(e: &mut Engine, p: &Value) -> Result<Value> {
    let id = "event.move_song_start";
    let to = position_param(e, id, p, "to")?.ok_or_else(|| bad(id, "`to` required"))?.max(0);
    let s = e.session_mut();
    let from = s.edit.value("song.start", 0.0).max(0.0) as Samples;
    let delta = to - from;
    if delta == 0 {
        return Ok(json!({"song_start": to, "moved": 0}));
    }
    let bpm0 = s.tempo.tempo_at_tick(0);
    let dticks = (delta as f64 * bpm0 * TICKS_PER_QUARTER as f64 / (60.0 * s.sample_rate.as_f64())).round() as i64;
    // Tempo and meter events after the start move with the song.
    let old = s.tempo.clone();
    let mut map = TempoMap::new(old.tempo_at_tick(0), old.meter_at_tick(0).numerator, old.meter_at_tick(0).denominator)
        .map_err(|err| bad(id, err.to_string()))?;
    for t in old.tempos().iter().filter(|t| t.tick > 0) {
        map.set_tempo((t.tick + dticks).max(1), t.bpm).map_err(|err| bad(id, err.to_string()))?;
    }
    for m in old.meters().iter().filter(|m| m.tick > 0) {
        map.set_meter((m.tick + dticks).max(1), m.numerator, m.denominator).map_err(|err| bad(id, err.to_string()))?;
    }
    s.tempo = map;
    let mut moved = 0;
    for tr in s.tracks.iter_mut().filter(|t| t.ticks_timebase && t.kind.has_playlist()) {
        if let Some(pl) = tr.playlist_mut() {
            for c in &mut pl.clips {
                c.start = c.start.saturating_add(delta).max(0);
                moved += 1;
            }
            pl.sort();
        }
    }
    s.edit.values.insert("song.start".into(), to as f64);
    Ok(json!({"song_start": to, "moved": moved}))
}

fn selection_ticks(e: &Engine, p: &Value, id: &str) -> Result<(Range, i64, i64)> {
    let r = range_param(e, id, p)?;
    if r.is_empty() {
        return Err(bad(id, "select a range on the timeline"));
    }
    let s = e.session();
    let t0 = s.tempo.samples_to_ticks(r.start, s.sample_rate);
    let t1 = s.tempo.samples_to_ticks(r.end, s.sample_rate);
    if t1 <= t0 {
        return Err(bad(id, "the selection is too short"));
    }
    Ok((r, t0, t1))
}

fn tempo_curve(e: &mut Engine, p: &Value, curve: Curve, id: &str) -> Result<Value> {
    let (_, t0, t1) = selection_ticks(e, p, id)?;
    let res = note_ticks(p.get("resolution"), TICKS_PER_QUARTER / 2);
    if (t1 - t0) / res > MAX_TEMPO_EVENTS {
        return Err(bad(id, "too many tempo events: use a coarser resolution"));
    }
    let s = e.session_mut();
    let a = f64_or(p, "start_bpm", s.tempo.tempo_at_tick(t0));
    let b = p.get("end_bpm").and_then(Value::as_f64).filter(|v| v.is_finite()).ok_or_else(|| bad(id, "`end_bpm` required"))?;
    let curvature = f64_or(p, "curvature", 0.5).clamp(-1.0, 1.0);
    let after = s.tempo.tempo_at_tick(t1);
    let had_end = s.tempo.tempos().iter().any(|x| x.tick == t1);
    let inside: Vec<i64> = s.tempo.tempos().iter().map(|x| x.tick).filter(|t| *t > t0 && *t < t1).collect();
    for t in inside {
        s.tempo.remove_tempo(t);
    }
    let mut n = 0;
    let mut tick = t0;
    while tick < t1 {
        let f = (tick - t0) as f64 / (t1 - t0) as f64;
        let g = match curve {
            Curve::Linear => f,
            Curve::Parabolic => f.powf(2f64.powf(curvature * 2.0)),
            Curve::Sigmoid => 0.5 - 0.5 * (f * std::f64::consts::PI).cos(),
        };
        s.tempo.set_tempo(tick, a + (b - a) * g).map_err(|err| bad(id, err.to_string()))?;
        n += 1;
        tick += res;
    }
    if !had_end {
        s.tempo.set_tempo(t1, after).map_err(|err| bad(id, err.to_string()))?;
    }
    Ok(json!({"events": n, "start_tick": t0, "end_tick": t1}))
}

fn tempo_stretch(e: &mut Engine, p: &Value) -> Result<Value> {
    let id = "event.tempo_stretch";
    let (r, t0, t1) = selection_ticks(e, p, id)?;
    let factor = match position_param(e, id, p, "length")? {
        Some(l) if l > 0 => l as f64 / r.len() as f64,
        _ => f64_or(p, "factor", f64::NAN),
    };
    if !factor.is_finite() || !(0.05..=20.0).contains(&factor) {
        return Err(bad(id, "give `factor` (0.05..20) or a new `length`"));
    }
    let s = e.session_mut();
    let after = s.tempo.tempo_at_tick(t1);
    let had_end = s.tempo.tempos().iter().any(|x| x.tick == t1);
    let mut events: Vec<(i64, f64)> = s.tempo.tempos().iter().filter(|x| x.tick > t0 && x.tick < t1).map(|x| (x.tick, x.bpm)).collect();
    events.push((t0, s.tempo.tempo_at_tick(t0)));
    for (tick, bpm) in &events {
        s.tempo.set_tempo(*tick, bpm / factor).map_err(|err| bad(id, err.to_string()))?;
    }
    if !had_end {
        s.tempo.set_tempo(t1, after).map_err(|err| bad(id, err.to_string()))?;
    }
    Ok(json!({"factor": factor, "events": events.len()}))
}

fn midi_clip_ids(e: &Engine, p: &Value) -> Vec<(TrackId, ClipId)> {
    let s = e.session();
    clip_ids_param(e, p)
        .into_iter()
        .filter_map(|id| s.find_clip(id).filter(|(_, c)| matches!(c.content, ClipContent::Midi { .. })).map(|(t, _)| (t, id)))
        .collect()
}

fn select_split(e: &mut Engine, p: &Value) -> Result<Value> {
    let id = "event.select_split_notes";
    let from = i64_or(p, "pitch_from", 0).clamp(0, 127) as u8;
    let to = i64_or(p, "pitch_to", 127).clamp(0, 127) as u8;
    let split = match str_param(p, "action") {
        None | Some("select") => false,
        Some("split") => true,
        Some(a) => return Err(bad(id, format!("unknown action `{a}`"))),
    };
    let clips = midi_clip_ids(e, p);
    if clips.is_empty() {
        return Err(bad(id, "select MIDI clips"));
    }
    let s = e.session_mut();
    let mut matched = 0;
    let mut new_tracks: std::collections::BTreeMap<u64, TrackId> = Default::default();
    for (t, cid) in clips {
        let Some(c) = s.find_clip(cid).map(|(_, c)| c.clone()) else { continue };
        let ClipContent::Midi { sequence } = &c.content else { continue };
        let (hit, rest) = soundcraft_midi::ops::split_notes(&sequence.notes, from, to);
        matched += hit.len();
        if !split || hit.is_empty() {
            continue;
        }
        if let Some(cl) = s.find_clip_mut(cid)
            && let ClipContent::Midi { sequence } = &mut cl.content
        {
            sequence.notes = rest;
        }
        let nt = match new_tracks.get(&t.0) {
            Some(x) => *x,
            None => {
                let name = s.track(t).map_or_else(|| "MIDI".to_string(), |x| format!("{} Split", x.name));
                let nt = s.add_track(TrackKind::Midi, ChannelFormat::Mono, Some(&name));
                new_tracks.insert(t.0, nt);
                nt
            }
        };
        let mut nc = c.clone();
        nc.id = s.new_clip_id();
        nc.content = ClipContent::Midi { sequence: soundcraft_midi::Sequence { notes: hit, ctrls: Vec::new() } };
        crate::edit::place_clip(s, nt, nc);
    }
    Ok(json!({"matched": matched, "split": split, "tracks": new_tracks.values().collect::<Vec<_>>()}))
}

fn input_quantize_state(s: &Session) -> Value {
    json!({
        "enabled": s.edit.flag("midi.input_quantize"),
        "grid_ticks": s.edit.value("midi.input_quantize.grid_ticks", (TICKS_PER_QUARTER / 4) as f64),
        "strength": s.edit.value("midi.input_quantize.strength", 100.0),
        "swing": s.edit.value("midi.input_quantize.swing", 0.0),
    })
}

fn input_quantize(e: &mut Engine, p: &Value) -> Result<Value> {
    let s = e.session_mut();
    if let Some(on) = p.get("enabled").and_then(Value::as_bool) {
        s.edit.set_flag("midi.input_quantize", on);
    }
    if p.get("grid").is_some() {
        let g = note_ticks(p.get("grid"), TICKS_PER_QUARTER / 4);
        s.edit.values.insert("midi.input_quantize.grid_ticks".into(), g as f64);
    }
    store_value(&mut s.edit, p, "strength", "midi.input_quantize.strength", 0.0, 100.0);
    store_value(&mut s.edit, p, "swing", "midi.input_quantize.swing", 0.0, 100.0);
    Ok(input_quantize_state(s))
}

fn quantize_opts(s: &Session) -> QuantizeOptions {
    QuantizeOptions {
        grid_ticks: (s.edit.value("midi.input_quantize.grid_ticks", (TICKS_PER_QUARTER / 4) as f64) as i64).clamp(1, TICKS_PER_QUARTER * 16),
        strength: (s.edit.value("midi.input_quantize.strength", 100.0) / 100.0).clamp(0.0, 1.0) as f32,
        swing: (s.edit.value("midi.input_quantize.swing", 0.0) / 100.0).clamp(0.0, 1.0) as f32,
        ..QuantizeOptions::default()
    }
}

fn midi_target(e: &Engine, p: &Value, id: &str) -> Result<TrackId> {
    if let Some(t) = track_param(e, id, p, "track")? {
        return if e.session().track(t).is_some_and(|x| x.kind.is_midi()) {
            Ok(t)
        } else {
            Err(bad(id, "`track` is not a MIDI or instrument track"))
        };
    }
    let s = e.session();
    s.edit
        .selected_tracks
        .iter()
        .copied()
        .find(|t| s.track(*t).is_some_and(|x| x.kind.is_midi()))
        .ok_or_else(|| bad(id, "select a MIDI or instrument track"))
}

fn step_input(e: &mut Engine, p: &Value) -> Result<Value> {
    let id = "event.step_input";
    let t = midi_target(e, p, id)?;
    let dur = note_ticks(p.get("duration"), TICKS_PER_QUARTER / 2);
    let vel = i64_or(p, "velocity", 100).clamp(1, 127) as u8;
    let rest = bool_or(p, "rest", false);
    let pitches: Vec<u8> = match p.get("pitches").and_then(Value::as_array) {
        Some(a) => a.iter().filter_map(Value::as_i64).map(|x| x.clamp(0, 127) as u8).take(16).collect(),
        None => vec![i64_or(p, "pitch", 60).clamp(0, 127) as u8],
    };
    let s = e.session_mut();
    let sr = s.sample_rate;
    let at = s.edit.selection.start.max(0);
    let tick = s.tempo.samples_to_ticks(at, sr);
    let end_tick = tick.saturating_add(dur);
    let end = s.tempo.tick_to_samples(end_tick, sr);
    // Continue the clip under (or ending at) the insertion point, else start a new one.
    let host = s.track(t).and_then(|tr| {
        let mut midi = tr.clips().iter().filter(|c| matches!(c.content, ClipContent::Midi { .. }));
        // A clip starting at `at` wins over one that merely ends there.
        midi.clone().find(|c| c.range().contains(at)).or_else(|| midi.find(|c| c.end() == at)).map(|c| c.id)
    });
    let host = match host {
        Some(h) => h,
        None => {
            let cid = s.new_clip_id();
            let name = s.track(t).map(|x| x.name.clone()).unwrap_or_default();
            crate::edit::place_clip(s, t, Clip::midi(cid, name, at, (end - at).max(1), soundcraft_midi::Sequence::default()));
            cid
        }
    };
    let next_start = s.track(t).and_then(|tr| tr.clips().iter().filter(|c| c.start > at && c.id != host).map(|c| c.start).min());
    let tempo = s.tempo.clone();
    let mut added = 0;
    if let Some(c) = s.find_clip_mut(host) {
        let base = tempo.samples_to_ticks(c.start, sr);
        if let ClipContent::Midi { sequence } = &mut c.content {
            if !rest {
                for pitch in &pitches {
                    sequence.notes.push(Note { pitch: *pitch, velocity: vel, release_velocity: 64, channel: 0, start: tick - base, length: dur });
                    added += 1;
                }
            }
            sequence.sort();
        }
        if end > c.end() {
            c.trim_end_to(end.min(next_start.unwrap_or(Samples::MAX)));
        }
    }
    if bool_or(p, "advance", true) {
        s.edit.selection = Range::point(end);
    }
    Ok(json!({"clip": host, "notes": added, "next": end}))
}

fn performance(e: &mut Engine, p: &Value, flatten: bool) -> Result<Value> {
    let clips = midi_clip_ids(e, p);
    let s = e.session_mut();
    let mut n = 0;
    for (_, cid) in clips {
        if flatten {
            if let Some(ClipContent::Midi { sequence }) = s.find_clip(cid).map(|(_, c)| c.content.clone()) {
                s.midi_originals.insert(cid.0, sequence);
                n += 1;
            }
        } else if let Some(orig) = s.midi_originals.get(&cid.0).cloned()
            && let Some(c) = s.find_clip_mut(cid)
            && let ClipContent::Midi { sequence } = &mut c.content
            && *sequence != orig
        {
            *sequence = orig;
            n += 1;
        }
    }
    Ok(json!({"clips": n}))
}

fn midi_offsets(e: &mut Engine, p: &Value) -> Result<Value> {
    let id = "event.midi_track_offsets";
    let t = track_param(e, id, p, "track")?;
    let off = position_param(e, id, p, "offset")?;
    let s = e.session_mut();
    if let (Some(t), Some(o)) = (t, off) {
        let lim = s.sample_rate.samples(10.0);
        s.edit.values.insert(format!("midi.offset.{}", t.0), o.clamp(-lim, lim) as f64);
    }
    let all: Vec<Value> = s
        .tracks
        .iter()
        .filter(|tr| tr.kind.is_midi())
        .map(|tr| json!({"track": tr.id, "name": tr.name, "offset": s.edit.value(&format!("midi.offset.{}", tr.id.0), 0.0)}))
        .collect();
    Ok(json!({"offsets": all}))
}

fn midi_rtp(e: &mut Engine, p: &Value) -> Result<Value> {
    let tracks = tracks_param(e, "event.midi_rtp", p)?;
    let s = e.session_mut();
    let clear = bool_or(p, "clear", false);
    for t in &tracks {
        if clear {
            let pre = format!("rtp.{}.", t.0);
            s.edit.values.retain(|k, _| !k.starts_with(&pre));
            continue;
        }
        store_value(&mut s.edit, p, "velocity", &track_key("rtp", *t, "velocity"), -127.0, 127.0);
        store_value(&mut s.edit, p, "transpose", &track_key("rtp", *t, "transpose"), -127.0, 127.0);
        store_value(&mut s.edit, p, "duration", &track_key("rtp", *t, "duration"), 1.0, 1000.0);
        store_value(&mut s.edit, p, "delay", &track_key("rtp", *t, "delay"), -1e7, 1e7);
    }
    Ok(json!({"rtp": values_with_prefix(&s.edit, "rtp.")}))
}

const NOTE_NAMES: [&str; 12] = ["C", "C#", "D", "Eb", "E", "F", "F#", "G", "Ab", "A", "Bb", "B"];
const CHORDS: [(&str, &[usize]); 12] = [
    ("", &[0, 4, 7]),
    ("m", &[0, 3, 7]),
    ("dim", &[0, 3, 6]),
    ("aug", &[0, 4, 8]),
    ("sus2", &[0, 2, 7]),
    ("sus4", &[0, 5, 7]),
    ("7", &[0, 4, 7, 10]),
    ("maj7", &[0, 4, 7, 11]),
    ("m7", &[0, 3, 7, 10]),
    ("m7b5", &[0, 3, 6, 10]),
    ("6", &[0, 4, 7, 9]),
    ("m6", &[0, 3, 7, 9]),
];

/// Name the chord formed by duration-weighted pitch classes (`bass` = lowest pitch class).
pub fn chord_name(weights: &[f64; 12], bass: usize) -> Option<String> {
    let present = weights.iter().filter(|w| **w > 0.0).count();
    if present < 3 {
        return None;
    }
    let total: f64 = weights.iter().sum();
    let maxw = weights.iter().copied().fold(0.0, f64::max);
    let mut best: Option<(f64, usize, &str)> = None;
    for root in 0..12 {
        for (suffix, tones) in CHORDS {
            let w = |i: usize| weights.get((root + i) % 12).copied().unwrap_or(0.0);
            let inside: f64 = tones.iter().map(|i| w(*i)).sum();
            let missing = tones.iter().filter(|i| w(**i) <= 0.0).count() as f64;
            let mut score = inside - (total - inside) - missing * 0.6 * maxw - tones.len() as f64 * 0.01 * maxw;
            if root == bass {
                score += 0.1 * maxw;
            }
            if best.is_none_or(|(b, _, _)| score > b) {
                best = Some((score, root, suffix));
            }
        }
    }
    let (_, root, suffix) = best?;
    let mut name = format!("{}{suffix}", NOTE_NAMES.get(root).copied().unwrap_or("C"));
    if bass != root {
        name.push('/');
        name.push_str(NOTE_NAMES.get(bass % 12).copied().unwrap_or("C"));
    }
    Some(name)
}

/// The most bars Extract Chords looks at (55 hours of 4/4 at 120 BPM), and notes sounding in them.
const MAX_CHORD_BARS: usize = 100_000;
const MAX_CHORD_NOTES: usize = 10_000_000;

fn extract_chords(e: &mut Engine, p: &Value) -> Result<Value> {
    let id = "event.extract_chords";
    let (_, t0, t1) = selection_ticks(e, p, id)?;
    let s = e.session();
    let picked = tracks_param(e, id, p)?;
    let mut tracks: Vec<TrackId> = picked.into_iter().filter(|t| s.track(*t).is_some_and(|x| x.kind.is_midi())).collect();
    if tracks.is_empty() {
        tracks = s.tracks.iter().filter(|t| t.kind.is_midi()).map(|t| t.id).collect();
    }
    // Absolute (tick) notes from every MIDI clip on those tracks, cut to their clip.
    let mut notes: Vec<(i64, i64, u8)> = Vec::new();
    for t in &tracks {
        for c in s.track(*t).map(|tr| tr.clips()).unwrap_or(&[]) {
            let ClipContent::Midi { sequence } = &c.content else { continue };
            let base = s.tempo.samples_to_ticks(c.start, s.sample_rate);
            let end = s.tempo.samples_to_ticks(c.end(), s.sample_rate);
            for n in &sequence.notes {
                let a = base.saturating_add(n.start);
                let b = base.saturating_add(n.end()).min(end);
                if b > a {
                    notes.push((a, b, n.pitch));
                }
            }
        }
    }
    notes.sort_unstable_by_key(|n| n.0);
    let mut bar = s.tempo.bar_beat_at_tick(t0).bar;
    let mut sounding: Vec<(i64, i64, u8)> = Vec::new();
    let (mut next, mut bars, mut heard) = (0, 0, 0);
    let mut chords: Vec<Value> = Vec::new();
    let mut marks: Vec<(Samples, String)> = Vec::new();
    let mut last = String::new();
    while chords.len() < 2000 && bars < MAX_CHORD_BARS && heard < MAX_CHORD_NOTES {
        let a = s.tempo.bar_start_tick(bar).max(t0);
        let b = s.tempo.bar_start_tick(bar + 1).min(t1);
        if a >= t1 {
            break;
        }
        while let Some(&n) = notes.get(next).filter(|n| n.0 < b) {
            sounding.push(n);
            next += 1;
        }
        sounding.retain(|n| n.1 > a);
        if sounding.is_empty() {
            match notes.get(next) {
                Some(n) => bar = (bar + 1).max(s.tempo.bar_beat_at_tick(n.0).bar),
                None => break,
            }
            continue;
        }
        bars += 1;
        heard += sounding.len();
        let mut w = [0.0f64; 12];
        let mut bass: Option<u8> = None;
        for (na, nb, pitch) in &sounding {
            let ov = (*nb).min(b) - (*na).max(a);
            if ov > 0 {
                if let Some(x) = w.get_mut(usize::from(*pitch) % 12) {
                    *x += ov as f64;
                }
                bass = Some(bass.map_or(*pitch, |bp| bp.min(*pitch)));
            }
        }
        if let Some(name) = bass.and_then(|bp| chord_name(&w, usize::from(bp) % 12)) {
            let at = s.tempo.tick_to_samples(a, s.sample_rate);
            chords.push(json!({"bar": bar, "chord": name, "at": at}));
            if name != last {
                marks.push((at, name.clone()));
                last = name;
            }
        }
        bar += 1;
    }
    let s = e.session_mut();
    for (at, name) in &marks {
        let mid = s.add_marker(name, MarkerKind::Marker, *at, *at);
        if let Some(m) = s.markers.iter_mut().find(|m| m.id == mid) {
            m.ruler = 2;
        }
    }
    Ok(json!({"chords": chords, "markers": marks.len()}))
}

fn beat_detective(e: &mut Engine, p: &Value) -> Result<Value> {
    let id = "event.beat_detective";
    let tracks = tracks_required(e, id, p)?;
    let r = range_param(e, id, p)?;
    if r.is_empty() {
        return Err(bad(id, "select a range to analyse"));
    }
    if r.len() > e.session().sample_rate.samples(20.0 * 60.0) {
        return Err(bad(id, "analyse at most 20 minutes at a time"));
    }
    let sens = f32_or(p, "sensitivity", 0.5).clamp(0.0, 1.0);
    let s = e.session();
    let sr = s.sample_rate;
    let mut hits: Vec<Samples> = Vec::new();
    for t in &tracks {
        if s.track(*t).is_some_and(|x| x.kind.has_playlist() && !x.kind.is_midi()) {
            hits.extend(crate::io::transients_in(s, *t, r, sens));
        }
    }
    hits.sort_unstable();
    let min_gap = sr.samples(0.03);
    let mut beats: Vec<Samples> = Vec::new();
    for h in hits {
        if beats.last().is_none_or(|l| h - l >= min_gap) {
            beats.push(h);
        }
    }
    let mut iois: Vec<f64> = beats.windows(2).filter_map(|w| Some((w.get(1)? - w.first()?) as f64)).filter(|d| *d > 0.0).collect();
    iois.sort_by(f64::total_cmp);
    let bpm = iois.get(iois.len() / 2).map(|d| {
        let mut b = 60.0 * sr.as_f64() / d;
        let mut guard = 0;
        while b < 70.0 && guard < 16 {
            b *= 2.0;
            guard += 1;
        }
        while b >= 180.0 && guard < 32 {
            b /= 2.0;
            guard += 1;
        }
        (b * 100.0).round() / 100.0
    });
    let separate = bool_or(p, "separate", false);
    let set_tempo = bool_or(p, "set_tempo", false);
    let s = e.session_mut();
    let mut separated = 0;
    if separate {
        for t in &tracks {
            for b in &beats {
                separated += crate::edit::separate_at(s, *t, *b);
            }
        }
    }
    if set_tempo && let Some(b) = bpm {
        let tick = s.tempo.samples_to_ticks(r.start, sr);
        s.tempo.set_tempo(tick, b).map_err(|err| bad(id, err.to_string()))?;
    }
    let shown: Vec<Samples> = beats.iter().take(2000).copied().collect();
    Ok(json!({"beats": shown, "count": beats.len(), "bpm": bpm, "separated": separated}))
}

fn identify_beat(e: &mut Engine, p: &Value) -> Result<Value> {
    let id = "event.identify_beat";
    let (r, t0, _) = selection_ticks(e, p, id)?;
    let s = e.session();
    let m = s.tempo.meter_at_tick(t0);
    let bars = f64_or(p, "bars", 1.0).max(0.0);
    let beats = f64_or(p, "beats", 0.0).max(0.0);
    let total_beats = bars * f64::from(m.numerator.max(1)) + beats;
    if total_beats <= 0.0 {
        return Err(bad(id, "`bars`/`beats` must describe a positive length"));
    }
    let quarters = total_beats * 4.0 / f64::from(m.denominator.max(1));
    let secs = s.sample_rate.seconds(r.len());
    let bpm = quarters * 60.0 / secs;
    let s = e.session_mut();
    s.tempo.set_tempo(t0, bpm).map_err(|err| bad(id, err.to_string()))?;
    Ok(json!({"bpm": bpm, "tick": t0}))
}

fn retro_record(e: &mut Engine, p: &Value) -> Result<Value> {
    let id = "event.retrospective_record";
    let Some(list) = p.get("notes").and_then(Value::as_array).filter(|a| !a.is_empty()) else {
        return Ok(json!({"recorded": 0}));
    };
    let t = midi_target(e, p, id)?;
    let s = e.session();
    let sr = s.sample_rate;
    // Latest start tick whose longest note still ends within MAX_POSITION.
    let max_tick = s.tempo.samples_to_ticks(MAX_POSITION, sr).saturating_sub(TICKS_PER_QUARTER * 1024).max(0);
    let mut raw: Vec<(i64, Note)> = Vec::new();
    for v in list.iter().take(100_000) {
        let tick = match (v.get("start_ticks").and_then(Value::as_i64), v.get("start")) {
            (Some(tk), _) => tk.max(0),
            (None, Some(_)) => s.tempo.samples_to_ticks(position_param(e, id, v, "start")?.unwrap_or(0).max(0), sr),
            _ => return Err(bad(id, "each note needs `start` or `start_ticks`")),
        }
        .min(max_tick);
        let note = Note {
            pitch: i64_or(v, "pitch", 60).clamp(0, 127) as u8,
            velocity: i64_or(v, "velocity", 100).clamp(1, 127) as u8,
            release_velocity: 64,
            channel: i64_or(v, "channel", 0).clamp(0, 15) as u8,
            start: 0,
            length: i64_or(v, "length_ticks", TICKS_PER_QUARTER / 4).clamp(1, TICKS_PER_QUARTER * 1024),
        };
        raw.push((tick, note));
    }
    let first = raw.iter().map(|x| x.0).min().unwrap_or(0);
    // Start the clip on the bar line so the quantize grid lines up with the music.
    let clip_tick = s.tempo.bar_start_tick(s.tempo.bar_beat_at_tick(first).bar).min(first).max(0);
    let end_tick = raw.iter().map(|(tk, n)| tk.saturating_add(n.length)).max().unwrap_or_else(|| first.saturating_add(1));
    let mut seq = soundcraft_midi::Sequence::default();
    for (tk, mut n) in raw {
        n.start = tk - clip_tick;
        seq.notes.push(n);
    }
    seq.sort();
    let performance = seq.clone();
    let quantized = s.edit.flag("midi.input_quantize");
    if quantized {
        soundcraft_midi::ops::quantize(&mut seq.notes, &quantize_opts(s));
        seq.sort();
    }
    let start = s.tempo.tick_to_samples(clip_tick, sr);
    let end = s.tempo.tick_to_samples(end_tick, sr).min(MAX_POSITION);
    let n = seq.notes.len();
    let s = e.session_mut();
    let cid = s.new_clip_id();
    let name = s.track(t).map(|x| x.name.clone()).unwrap_or_default();
    crate::edit::place_clip(s, t, Clip::midi(cid, name, start, (end - start).max(1), seq));
    s.midi_originals.insert(cid.0, performance);
    Ok(json!({"recorded": n, "clip": cid, "quantized": quantized}))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn midi_engine() -> (Engine, TrackId) {
        let mut e = Engine::default();
        e.execute("track.new", &json!({"kind": "midi"})).unwrap();
        let t = e.session().tracks[0].id;
        (e, t)
    }

    #[test]
    fn chord_names() {
        let mut w = [0.0; 12];
        for pc in [0, 4, 7] {
            w[pc] = 1.0;
        }
        assert_eq!(chord_name(&w, 0).as_deref(), Some("C"));
        let mut w = [0.0; 12];
        for pc in [9, 0, 4] {
            w[pc] = 1.0;
        }
        assert_eq!(chord_name(&w, 9).as_deref(), Some("Am"));
        let mut w = [0.0; 12];
        for pc in [7, 11, 2, 5] {
            w[pc] = 1.0;
        }
        assert_eq!(chord_name(&w, 7).as_deref(), Some("G7"));
        assert_eq!(chord_name(&[0.0; 12], 0), None);
    }

    #[test]
    fn retro_record_quantizes_and_restores() {
        let (mut e, t) = midi_engine();
        e.execute("event.input_quantize", &json!({"enabled": true, "grid": "1/4"})).unwrap();
        let r = e
            .execute(
                "event.retrospective_record",
                &json!({"track": t.0, "notes": [{"pitch": 60, "start_ticks": 1010}, {"pitch": 64, "start_ticks": 1930}]}),
            )
            .unwrap();
        assert_eq!(r["recorded"], 2);
        let cid = ClipId(r["clip"].as_u64().unwrap());
        let notes = |e: &Engine| match &e.session().find_clip(cid).unwrap().1.content {
            ClipContent::Midi { sequence } => sequence.notes.iter().map(|n| n.start).collect::<Vec<_>>(),
            _ => vec![],
        };
        assert_eq!(notes(&e), vec![960, 1920]);
        e.execute("edit.select", &json!({"clips": [cid.0]})).unwrap();
        e.execute("event.restore_performance", &json!({})).unwrap();
        assert_eq!(notes(&e), vec![1010, 1930]);
        assert_eq!(e.execute("event.retrospective_record", &json!({})).unwrap()["recorded"], 0);
    }

    #[test]
    fn flatten_then_restore() {
        let (mut e, t) = midi_engine();
        let r = e.execute("event.retrospective_record", &json!({"track": t.0, "notes": [{"pitch": 60, "start_ticks": 100}]})).unwrap();
        let cid = r["clip"].as_u64().unwrap();
        e.execute("edit.select", &json!({"clips": [cid]})).unwrap();
        e.execute("event.transpose", &json!({"semitones": 5})).unwrap();
        e.execute("event.flatten_performance", &json!({})).unwrap();
        e.execute("event.transpose", &json!({"semitones": 5})).unwrap();
        e.execute("event.restore_performance", &json!({})).unwrap();
        match &e.session().find_clip(ClipId(cid)).unwrap().1.content {
            ClipContent::Midi { sequence } => assert_eq!(sequence.notes[0].pitch, 65),
            _ => panic!(),
        }
    }

    #[test]
    fn step_input_appends_and_advances() {
        let (mut e, t) = midi_engine();
        e.execute("edit.select", &json!({"tracks": [t.0], "start": 0})).unwrap();
        e.execute("event.step_input", &json!({"pitch": 60, "duration": "1/4"})).unwrap();
        e.execute("event.step_input", &json!({"pitches": [64, 67], "duration": "1/4"})).unwrap();
        e.execute("event.step_input", &json!({"rest": true, "duration": "1/4"})).unwrap();
        let tr = e.session().track(t).unwrap();
        assert_eq!(tr.clips().len(), 1);
        let c = &tr.clips()[0];
        assert_eq!(c.length, 3 * 24_000);
        match &c.content {
            ClipContent::Midi { sequence } => assert_eq!(sequence.notes.iter().map(|n| n.start).collect::<Vec<_>>(), vec![0, 960, 960]),
            _ => panic!(),
        }
        assert_eq!(e.session().edit.selection.start, 3 * 24_000);
    }

    #[test]
    fn step_input_at_clip_boundary_uses_next_clip() {
        let (mut e, t) = midi_engine();
        let sr = 48_000;
        e.execute("midi.new_clip", &json!({"track": t.0, "start": 0, "end": sr})).unwrap();
        let second = e.execute("midi.new_clip", &json!({"track": t.0, "start": sr, "end": 2 * sr})).unwrap()["clip"].as_u64().unwrap();
        e.execute("edit.select", &json!({"tracks": [t.0], "start": sr})).unwrap();
        let r = e.execute("event.step_input", &json!({"track": t.0, "pitch": 60, "duration": "1/4"})).unwrap();
        assert_eq!(r["clip"].as_u64(), Some(second));
        let mut ranges: Vec<(i64, i64)> = e.session().track(t).unwrap().clips().iter().map(|c| (c.start, c.end())).collect();
        ranges.sort();
        assert_eq!(ranges, vec![(0, sr), (sr, 2 * sr)]);
        let c = e.session().find_clip(soundcraft_model::ClipId(second)).map(|(_, c)| c.clone()).unwrap();
        match &c.content {
            ClipContent::Midi { sequence } => assert_eq!(sequence.notes.iter().map(|n| n.start).collect::<Vec<_>>(), vec![0]),
            _ => panic!(),
        }
    }

    #[test]
    fn tempo_curves() {
        let mut e = Engine::default();
        e.execute("track.new", &json!({})).unwrap();
        e.execute("event.tempo_linear", &json!({"start": 0, "end": 96_000, "start_bpm": 120.0, "end_bpm": 160.0, "resolution": "1/4"})).unwrap();
        let tempos = e.session().tempo.tempos().to_vec();
        assert_eq!(tempos.first().unwrap().bpm, 120.0);
        assert!(tempos.windows(2).all(|w| w[1].bpm >= w[0].bpm || w[1].tick >= 3840));
        assert_eq!(e.session().tempo.tempo_at_tick(3840), 120.0, "tempo after the selection is kept");
        assert_eq!(tempos.len(), 5);
        let mut e = Engine::default();
        e.execute("event.tempo_stretch", &json!({"start": 0, "end": 96_000, "factor": 2.0})).unwrap();
        assert_eq!(e.session().tempo.tempo_at_tick(0), 60.0);
        assert_eq!(e.session().tempo.tick_to_samples(3840, soundcraft_time::SampleRate::HZ_48000), 192_000);
    }

    #[test]
    fn identify_beat_sets_tempo() {
        let mut e = Engine::default();
        e.execute("event.identify_beat", &json!({"start": 0, "end": 96_000, "bars": 1})).unwrap();
        assert!((e.session().tempo.tempo_at_tick(0) - 120.0).abs() < 1e-9);
        e.execute("event.identify_beat", &json!({"start": 0, "end": 192_000, "bars": 1})).unwrap();
        assert!((e.session().tempo.tempo_at_tick(0) - 60.0).abs() < 1e-9);
    }

    #[test]
    fn split_notes_to_new_track() {
        let (mut e, t) = midi_engine();
        let r = e
            .execute(
                "event.retrospective_record",
                &json!({"track": t.0, "notes": [{"pitch": 36, "start_ticks": 0}, {"pitch": 72, "start_ticks": 0}]}),
            )
            .unwrap();
        let cid = r["clip"].as_u64().unwrap();
        e.execute("edit.select", &json!({"clips": [cid]})).unwrap();
        let r = e.execute("event.select_split_notes", &json!({"pitch_from": 60, "pitch_to": 127, "action": "split"})).unwrap();
        assert_eq!(r["matched"], 1);
        assert_eq!(e.session().tracks.len(), 2);
        let nt = e.session().tracks.iter().find(|x| x.id != t).unwrap();
        match &nt.clips()[0].content {
            ClipContent::Midi { sequence } => assert_eq!(sequence.notes[0].pitch, 72),
            _ => panic!(),
        }
    }

    #[test]
    fn chords_from_demo_become_markers() {
        let mut e = crate::demo::demo_engine();
        let keys = e.session().track_by_name("Keys").unwrap().clone();
        let c = keys.clips()[0].clone();
        let before = e.session().markers.len();
        e.execute("edit.select", &json!({"tracks": [keys.id.0], "start": c.start, "end": c.end()})).unwrap();
        let r = e.execute("event.extract_chords", &json!({})).unwrap();
        assert!(!r["chords"].as_array().unwrap().is_empty(), "{r}");
        assert!(e.session().markers.len() > before);
        assert!(e.session().markers.iter().any(|m| m.ruler == 2));
    }

    #[test]
    fn chords_far_down_the_timeline_are_found_without_walking_every_bar() {
        let mut e = crate::demo::demo_engine();
        let keys = e.session().track_by_name("Keys").unwrap().id;
        let far = 1i64 << 39;
        if let Some(p) = e.session_mut().track_mut(keys).and_then(soundcraft_model::Track::playlist_mut) {
            p.clips.iter_mut().for_each(|c| c.start += far);
        }
        let r = e.execute("event.extract_chords", &json!({"tracks": [keys.0], "start": 0, "end": far + 48_000 * 600})).unwrap();
        assert!(!r["chords"].as_array().unwrap().is_empty(), "{r}");
    }

    #[test]
    fn beat_detective_finds_demo_tempo() {
        let mut e = crate::demo::demo_engine();
        let kick = e.session().track_by_name("Kick").unwrap().clone();
        let c = kick.clips()[0].clone();
        let r = e.execute("event.beat_detective", &json!({"tracks": [kick.id.0], "start": c.start, "end": c.start + 48_000 * 8})).unwrap();
        assert!(r["count"].as_u64().unwrap() > 4, "{r}");
        assert!(r["bpm"].as_f64().is_some());
    }

    #[test]
    fn extract_chords_survives_huge_note_start() {
        let (mut e, t) = midi_engine();
        let r = e.execute("midi.new_clip", &json!({"track": t.0, "start": 48_000, "end": 96_000})).unwrap();
        let clip = r["clip"].as_u64().unwrap();
        e.execute("midi.note_add", &json!({"clip": clip, "pitch": 60, "start_ticks": i64::MAX, "length_ticks": 480})).unwrap();
        e.execute("edit.select", &json!({"tracks": [t.0], "start": 0, "end": 96_000})).unwrap();
        let r = e.execute("event.extract_chords", &json!({}));
        assert!(r.is_ok(), "{r:?}");
    }

    #[test]
    fn retro_record_huge_start_ticks_stay_in_range() {
        for tk in [i64::MAX, 1_000_000_000_000_000] {
            let (mut e, t) = midi_engine();
            let notes = json!([{"pitch": 60, "start_ticks": tk, "length_ticks": TICKS_PER_QUARTER * 1024}]);
            e.execute("event.retrospective_record", &json!({"track": t.0, "notes": notes})).unwrap();
            let c = e.session().track(t).unwrap().clips()[0].clone();
            assert!(c.start >= 0 && c.length >= 1, "{tk}: {} {}", c.start, c.length);
            assert!(c.start.saturating_add(c.length) <= MAX_POSITION, "{tk}: start {} len {}", c.start, c.length);
        }
    }

    #[test]
    fn move_song_start_moves_tick_material() {
        let (mut e, t) = midi_engine();
        e.execute("event.retrospective_record", &json!({"track": t.0, "notes": [{"pitch": 60, "start_ticks": 0}]})).unwrap();
        e.execute("event.move_song_start", &json!({"to": 48_000})).unwrap();
        assert_eq!(e.session().track(t).unwrap().clips()[0].start, 48_000);
        assert_eq!(e.session().edit.value("song.start", 0.0), 48_000.0);
    }

    #[test]
    fn rtp_and_offsets_are_settings() {
        let (mut e, t) = midi_engine();
        let undo = e.undo_history().len();
        e.execute("event.midi_rtp", &json!({"track": t.0, "transpose": 7})).unwrap();
        assert_eq!(e.session().edit.value(&track_key("rtp", t, "transpose"), 0.0), 7.0);
        let r = e.execute("event.midi_track_offsets", &json!({"track": t.0, "offset": 480})).unwrap();
        assert_eq!(r["offsets"][0]["offset"], 480.0);
        assert_eq!(e.undo_history().len(), undo, "settings are not undo steps");
    }
}
