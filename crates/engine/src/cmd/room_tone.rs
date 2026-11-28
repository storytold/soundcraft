//! Room tone: choose a stretch of recorded silence and fill gaps with it instead of digital silence.
//!
//! The fill is made of ordinary audio clips that play the chosen window, overlapped and
//! crossfaded where the gap is longer than the tone, so it can be edited like any other clip.

use super::{CommandSpec, always, bad, clip_ids_param, f64_or, has_range, range_param, tracks_required};
use crate::{Engine, Result, cmd};
use serde_json::{Value, json};
use soundcraft_model::{Clip, ClipContent, Fade, FadeShape, RoomTone, Session, TrackId, TrackKind};
use soundcraft_time::{Range, Samples};

const SET: &str = "edit.room_tone_set";
const FILL: &str = "edit.room_tone_fill";

/// The shortest room tone accepted, in seconds: shorter windows tile into a buzz and many clips.
const MIN_TONE_SECONDS: f64 = 0.1;
/// The most clips one fill makes, so a huge range can't flood the playlist.
const MAX_FILL_CLIPS: usize = 20_000;
/// The name the fill clips get.
const CLIP_NAME: &str = "Room Tone";

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "edit.room_tone_set",
            "Set Room Tone",
            ["Edit", "Room Tone"],
            None,
            "{clip?} — the clip (default: the selected clip) becomes the session's room tone; with no `clip`, an edit selection inside the clip picks just that part. At least 0.1 s, not elastic. Returns the window",
            always,
            set
        ),
        cmd!(
            "edit.room_tone_fill",
            "Fill Gaps with Room Tone",
            ["Edit", "Room Tone"],
            None,
            "{tracks?, start?, end?, fade_ms?: 10, edge_fade_ms?: 2, min_gap_ms?: 0} — fills every empty stretch of the range on each audio track with clips of the room tone, tiled with crossfades of `fade_ms` where the gap is longer than the tone, and faded in and out over `edge_fade_ms` at the edges of each gap (short, so the background does not dip next to speech); gaps shorter than `min_gap_ms` are left alone. Returns {clips, gaps}",
            fillable,
            fill
        ),
        cmd!(
            "edit.room_tone_clear",
            "Clear Room Tone",
            ["Edit", "Room Tone"],
            None,
            "{} — forgets the room tone; clips already filled stay",
            has_tone,
            |e, _| {
                let was = e.session_mut().room_tone.take();
                Ok(json!({"cleared": was.is_some()}))
            }
        ),
        cmd!(query "edit.room_tone", "Room Tone", [], None, "{} — the room tone window {source, offset, length, gain_db}, or null", always, |e, _| {
            Ok(e.session().room_tone.map_or(Value::Null, |t| describe(e.session(), t)))
        }),
    ]
}

fn has_tone(e: &Engine) -> std::result::Result<(), String> {
    if e.session().room_tone.is_some() { Ok(()) } else { Err("set a room tone first (Edit › Room Tone › Set Room Tone)".into()) }
}

fn fillable(e: &Engine) -> std::result::Result<(), String> {
    has_tone(e)?;
    has_range(e)
}

fn describe(s: &Session, t: RoomTone) -> Value {
    let name = s.source(t.source).map(|f| f.name.clone());
    json!({"source": t.source.0, "source_name": name, "offset": t.offset, "length": t.length, "gain_db": t.gain_db})
}

fn set(e: &mut Engine, p: &Value) -> Result<Value> {
    let explicit = p.get("clip").is_some() || p.get("clips").is_some();
    let ids = clip_ids_param(e, p);
    let s = e.session();
    let id = match ids.as_slice() {
        [id] => *id,
        [] => return Err(bad(SET, "select the clip of room tone, or pass `clip`")),
        _ => return Err(bad(SET, "select one clip of room tone, not several")),
    };
    let (_, clip) = s.find_clip(id).ok_or_else(|| bad(SET, format!("no clip with id {}", id.0)))?;
    let ClipContent::Audio { source, .. } = clip.content else {
        return Err(bad(SET, format!("“{}” is not an audio clip", clip.name)));
    };
    if (clip.stretch - 1.0).abs() > f64::EPSILON {
        return Err(bad(SET, format!("“{}” is time-stretched; render it first", clip.name)));
    }
    // An edit selection inside the clip picks part of it.
    let sel = s.edit.selection;
    let window = if explicit || sel.is_empty() {
        clip.range()
    } else {
        sel.intersect(&clip.range()).ok_or_else(|| bad(SET, "the edit selection is outside the clip"))?
    };
    let offset = clip.source_offset().saturating_add(window.start - clip.start);
    let stored = s.source(source).map(|f| f.frames).ok_or_else(|| bad(SET, format!("the audio file of “{}” is missing", clip.name)))?;
    // The stored count isn't refreshed when a session reopens; the decoded audio is what plays.
    let frames = s.pool.get(source).map_or(stored, |a| a.frames() as u64);
    let tone = RoomTone { source, offset, length: window.len(), gain_db: clip.gain_db }
        .sanitized(frames)
        .ok_or_else(|| bad(SET, format!("“{}” plays no audio from its file", clip.name)))?;
    if tone.length < s.sample_rate.samples(MIN_TONE_SECONDS) {
        return Err(bad(SET, format!("room tone must be at least {MIN_TONE_SECONDS} s long")));
    }
    let s = e.session_mut();
    // A session reopens with the room tone cut to the stored count, so record the decoded one.
    if let Some(f) = s.sources.iter_mut().find(|f| f.id == source && f.frames < frames) {
        f.frames = frames;
    }
    s.room_tone = Some(tone);
    Ok(describe(e.session(), tone))
}

/// The empty stretches of `range` on `track`'s active playlist, at least `min` samples long.
fn gaps(s: &Session, track: TrackId, range: Range, min: Samples) -> Vec<Range> {
    let Some(t) = s.track(track) else { return Vec::new() };
    let mut out = Vec::new();
    let mut at = range.start;
    let mut clips: Vec<Range> = t.clips().iter().map(Clip::range).filter(|r| r.end > range.start && r.start < range.end).collect();
    clips.sort_by_key(|r| r.start);
    for c in clips {
        if c.start > at {
            out.push(Range::new(at, c.start));
        }
        at = at.max(c.end);
    }
    if at < range.end {
        out.push(Range::new(at, range.end));
    }
    out.retain(|g| g.len() >= min.max(1));
    out
}

/// The clips that fill `gap` with `tone`: whole tone windows, each overlapping the one before by
/// `fade` samples with a crossfade, the last cut to the gap's end; the first fades in and the last
/// fades out over `edge` (or less, in a short gap). `None` when that takes more than `budget` clips.
fn tiles(tone: RoomTone, gap: Range, fade: Samples, edge: Samples, budget: usize) -> Option<Vec<Clip>> {
    let edge = edge.max(0);
    // The overlap must leave each tile moving forward.
    let fade = fade.clamp(0, tone.length / 2);
    let step = tone.length - fade;
    if step <= 0 {
        return Some(Vec::new());
    }
    // Count before allocating: a long gap and a short tone could otherwise make millions.
    let needed = gap.len().saturating_sub(fade).max(1).saturating_add(step - 1) / step;
    if usize::try_from(needed).map_or(true, |n| n > budget) {
        return None;
    }
    let mut out: Vec<Clip> = Vec::new();
    let mut at = gap.start;
    while at < gap.end && out.len() <= budget {
        let length = tone.length.min(gap.end - at);
        let mut c = Clip::audio(soundcraft_model::ClipId(0), CLIP_NAME, tone.source, tone.offset, at, length);
        c.gain_db = tone.gain_db;
        let crossfade = Fade { len: fade.min(length), shape: FadeShape::EqualPower, curvature: 0.0 };
        if let Some(prev) = out.last_mut() {
            prev.fade_out = crossfade;
            c.fade_in = crossfade;
        } else {
            c.fade_in = Fade { len: edge.min(length / 2), ..crossfade };
        }
        if at + length >= gap.end {
            // The last tile also fades out to the gap's edge, leaving room for its fade in.
            c.fade_out = Fade { len: edge.min(length - c.fade_in.len).max(0), ..crossfade };
            out.push(c);
            break;
        }
        out.push(c);
        at = at.saturating_add(step);
    }
    (out.len() <= budget).then_some(out)
}

fn fill(e: &mut Engine, p: &Value) -> Result<Value> {
    let tone = e.session().room_tone.ok_or_else(|| bad(FILL, "set a room tone first"))?;
    let tracks = tracks_required(e, FILL, p)?;
    let range = range_param(e, FILL, p)?;
    if range.is_empty() {
        return Err(bad(FILL, "select a range to fill"));
    }
    let s = e.session();
    let name = s.source(tone.source).map_or_else(|| "the room tone's audio file".to_string(), |f| format!("`{}`", f.name));
    let Some(audio) = s.pool.get(tone.source) else {
        return Err(bad(FILL, format!("{name} is not loaded; relink it before filling")));
    };
    // The window was checked against the session file's frame count, which can be stale when the
    // file was replaced by a shorter one: tile only audio that decoded.
    let tone = tone
        .sanitized(audio.frames() as u64)
        .filter(|t| t.length >= s.sample_rate.samples(MIN_TONE_SECONDS))
        .ok_or_else(|| bad(FILL, format!("{name} is now shorter than the room tone; set the room tone again")))?;
    let ms = |k: &str, d: f64| s.sample_rate.samples(f64_or(p, k, d).clamp(0.0, 10_000.0) / 1000.0);
    let (fade, edge, min_gap) = (ms("fade_ms", 10.0), ms("edge_fade_ms", 2.0), ms("min_gap_ms", 0.0));
    // Plan every track first, so a fill that would be too big changes nothing.
    let mut plan: Vec<(TrackId, Vec<Clip>)> = Vec::new();
    let (mut count, mut gap_count) = (0usize, 0usize);
    for t in tracks {
        if s.track(t).is_none_or(|tr| tr.kind != TrackKind::Audio) {
            continue;
        }
        let mut clips = Vec::new();
        for gap in gaps(s, t, range, min_gap) {
            let budget = MAX_FILL_CLIPS.saturating_sub(count.saturating_add(clips.len()));
            let made = tiles(tone, gap, fade, edge, budget).ok_or_else(|| {
                bad(FILL, format!("that would make more than {MAX_FILL_CLIPS} clips; fill a shorter range or use a longer room tone"))
            })?;
            clips.extend(made);
            gap_count += 1;
        }
        count += clips.len();
        plan.push((t, clips));
    }
    if gap_count == 0 {
        return Err(bad(FILL, "no empty space to fill on the audio tracks in that range"));
    }
    let s = e.session_mut();
    for (t, mut clips) in plan {
        for c in &mut clips {
            c.id = s.new_clip_id();
        }
        if let Some(pl) = s.track_mut(t).and_then(|tr| tr.playlist_mut()) {
            pl.clips.extend(clips);
            pl.sort();
        }
    }
    Ok(json!({"clips": count, "gaps": gap_count}))
}

#[cfg(test)]
mod tests {
    use super::*;
    use craft_audio::AudioBuffer;
    use soundcraft_model::{ChannelFormat, ClipId, FileFormat, SourceAudio, SourceId};
    use std::sync::Arc;

    const RATE: Samples = 48_000;

    /// A session with one mono track holding `clips` (start, length) of a 10 s source, the first
    /// of which plays 1 s of room tone from the start of the file.
    fn engine(clips: &[(Samples, Samples)]) -> (Engine, TrackId, Vec<ClipId>) {
        let mut e = Engine::default();
        let s = e.session_mut();
        let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, Some("Narration"));
        let src = SourceId(s.alloc());
        let frames = 10 * RATE as usize;
        s.sources.push(soundcraft_model::Source {
            id: src,
            name: "Take".into(),
            path: "Take.wav".into(),
            channels: 1,
            frames: frames as u64,
            sample_rate: 48_000,
            format: FileFormat::Wav,
            time_reference: 0,
            unsaved: false,
        });
        // Quiet white noise, like a room: uncorrelated from one moment to the next.
        let mut seed = 0x2545_f491_u32;
        let noise: Vec<f32> = (0..frames)
            .map(|_| {
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                (seed as f32 / u32::MAX as f32 - 0.5) * 0.02
            })
            .collect();
        let buffer = AudioBuffer { sample_rate: 48_000, channels: vec![noise] };
        s.pool.insert(src, Arc::new(SourceAudio::new(buffer)));
        let mut ids = Vec::new();
        for (i, (start, length)) in clips.iter().enumerate() {
            let id = s.new_clip_id();
            let offset = if i == 0 { 0 } else { RATE * 2 };
            if let Some(pl) = s.track_mut(t).and_then(|tr| tr.playlist_mut()) {
                pl.clips.push(Clip::audio(id, format!("Clip {i}"), src, offset, *start, *length));
                pl.sort();
            }
            ids.push(id);
        }
        s.edit.selected_tracks = vec![t];
        (e, t, ids)
    }

    fn clips(e: &Engine, t: TrackId) -> Vec<Clip> {
        e.session().track(t).map(|tr| tr.clips().to_vec()).unwrap_or_default()
    }

    #[test]
    fn set_takes_the_selected_clip_or_the_selected_part_of_it() {
        let (mut e, _, ids) = engine(&[(0, RATE)]);
        e.session_mut().edit.selected_clips = vec![ids[0]];
        let r = e.execute(SET, &json!({})).unwrap();
        assert_eq!((r["offset"].as_i64(), r["length"].as_i64()), (Some(0), Some(RATE)));
        e.session_mut().edit.selected_clips.clear();
        e.session_mut().edit.selection = Range::new(RATE / 4, RATE / 2);
        let r = e.execute(SET, &json!({})).unwrap();
        assert_eq!((r["offset"].as_i64(), r["length"].as_i64()), (Some(RATE / 4), Some(RATE / 4)));
        e.execute("edit.undo", &json!({})).unwrap();
        assert_eq!(e.session().room_tone.map(|t| t.length), Some(RATE), "undo restores the previous tone");
    }

    #[test]
    fn a_tone_past_a_stale_stored_length_survives_a_reload() {
        // The session file says 3 s, but 10 s decoded: the tone at 4..5 s plays.
        let (mut e, t, _) = engine(&[]);
        let s = e.session_mut();
        let src = s.sources.first().map(|f| f.id).unwrap();
        s.sources.iter_mut().for_each(|f| f.frames = 3 * RATE as u64);
        let id = s.new_clip_id();
        if let Some(pl) = s.track_mut(t).and_then(|tr| tr.playlist_mut()) {
            pl.clips.push(Clip::audio(id, "Late", src, 4 * RATE, 0, RATE));
        }
        e.execute(SET, &json!({"clip": id.0})).unwrap();
        let tone = e.session().room_tone;
        assert_eq!(tone.map(|t| (t.offset, t.length)), Some((4 * RATE, RATE)));
        let loaded = Session::from_json(&e.session().to_json().unwrap()).unwrap();
        assert_eq!(loaded.room_tone, tone, "reopening keeps the tone that was set");
    }

    #[test]
    fn set_refuses_what_cannot_be_a_room_tone() {
        let (mut e, t, ids) = engine(&[(0, RATE / 20)]);
        assert!(e.execute(SET, &json!({"clip": ids[0].0})).is_err(), "50 ms is too short");
        assert!(e.execute(SET, &json!({"clip": 9999})).is_err(), "unknown clip");
        e.session_mut().edit.selected_tracks = vec![t];
        e.session_mut().edit.selection = Range::new(RATE * 5, RATE * 6);
        assert!(e.execute(SET, &json!({})).is_err(), "nothing selected");
        assert_eq!(e.session().room_tone, None);
        assert!(e.execute(FILL, &json!({"start": 0, "end": RATE})).is_err(), "no tone to fill with");
    }

    #[test]
    fn fill_puts_crossfaded_tone_clips_in_every_gap_and_nowhere_else() {
        // Tone 0..1 s, speech 3..4 s; fill 0..6 s: gaps 1..3 s (2 s) and 4..6 s (2 s).
        let (mut e, t, ids) = engine(&[(0, RATE), (3 * RATE, RATE)]);
        e.execute(SET, &json!({"clip": ids[0].0})).unwrap();
        let fade = RATE / 100;
        let r = e.execute(FILL, &json!({"start": 0, "end": 6 * RATE})).unwrap();
        assert_eq!(r["gaps"], 2);
        let fills: Vec<Clip> = clips(&e, t).into_iter().filter(|c| !ids.contains(&c.id)).collect();
        assert_eq!(r["clips"].as_u64(), Some(fills.len() as u64));
        for gap in [Range::new(RATE, 3 * RATE), Range::new(4 * RATE, 6 * RATE)] {
            let mut in_gap: Vec<&Clip> = fills.iter().filter(|c| gap.contains(c.start)).collect();
            in_gap.sort_by_key(|c| c.start);
            // 2 s of a 1 s tone with 10 ms overlaps: three tiles, the last cut short.
            assert_eq!(in_gap.len(), 3, "{gap:?}");
            assert_eq!(in_gap.first().map(|c| c.start), Some(gap.start));
            assert_eq!(in_gap.last().map(|c| c.end()), Some(gap.end), "the fill ends at the gap's edge");
            for pair in in_gap.windows(2) {
                assert_eq!(pair[0].end() - pair[1].start, fade, "neighbours overlap by the crossfade");
                assert_eq!((pair[0].fade_out.len, pair[1].fade_in.len), (fade, fade));
            }
            let edge = RATE / 500;
            assert_eq!(in_gap[0].fade_in.len, edge, "a short de-click fade at the gap's edges");
            assert!(in_gap.last().is_some_and(|c| c.fade_out.len == edge));
            for c in &in_gap {
                assert_eq!(c.name, CLIP_NAME);
                assert_eq!(c.source_offset(), 0);
                assert!(c.length <= RATE);
            }
        }
        // The original clips are untouched and nothing covers them.
        let all = clips(&e, t);
        for id in &ids {
            let c = all.iter().find(|c| c.id == *id).unwrap();
            assert!(fills.iter().all(|f| !f.range().overlaps(&c.range())), "fill over {}", c.name);
        }
        e.execute("edit.undo", &json!({})).unwrap();
        assert_eq!(clips(&e, t).len(), 2, "undo removes the whole fill");
    }

    #[test]
    fn the_fill_plays_at_the_tones_level_through_every_crossfade() {
        let (mut e, t, ids) = engine(&[(0, RATE)]);
        e.execute(SET, &json!({"clip": ids[0].0})).unwrap();
        e.execute(FILL, &json!({"tracks": [t.0], "start": 0, "end": 5 * RATE, "fade_ms": 50})).unwrap();
        let out = soundcraft_mix::render_range(e.session(), Range::new(0, 5 * RATE), 512);
        let left = &out[0];
        let rms = |a: usize, b: usize| (left[a..b].iter().map(|x| f64::from(*x).powi(2)).sum::<f64>() / (b - a) as f64).sqrt();
        let tone = rms(0, RATE as usize);
        assert!(tone > 0.0);
        // 20 ms windows from where the tone clip ends to the end of the fill: the edge fades are
        // too short to dip a window, and the crossfades keep the power.
        let win = (RATE / 50) as usize;
        for w in (RATE as usize..(5 * RATE) as usize - win).step_by(win) {
            let db = 20.0 * (rms(w, w + win) / tone).log10();
            assert!(db.abs() < 1.5, "level {db:.2} dB off the tone at {} s", w as f64 / RATE as f64);
        }
    }

    #[test]
    fn fill_skips_short_gaps_and_refuses_when_nothing_is_empty() {
        let (mut e, t, ids) = engine(&[(0, RATE), (RATE + 100, RATE)]);
        e.execute(SET, &json!({"clip": ids[0].0})).unwrap();
        assert!(e.execute(FILL, &json!({"start": 0, "end": 2 * RATE + 100, "min_gap_ms": 5})).is_err(), "the 100-sample gap is under 5 ms");
        let r = e.execute(FILL, &json!({"start": 0, "end": 2 * RATE + 100})).unwrap();
        assert_eq!((r["gaps"].as_u64(), r["clips"].as_u64()), (Some(1), Some(1)));
        let short = clips(&e, t).into_iter().find(|c| !ids.contains(&c.id)).unwrap();
        assert_eq!((short.start, short.length), (RATE, 100));
        assert!(short.fade_in.len + short.fade_out.len <= short.length, "fades fit a short gap");
        assert_eq!((short.fade_in.len, short.fade_out.len), (50, 50));
    }

    #[test]
    fn a_fill_too_big_changes_nothing() {
        let (mut e, t, ids) = engine(&[(0, RATE / 5)]);
        e.execute(SET, &json!({"clip": ids[0].0})).unwrap();
        // 200 ms tiles over 4 hours is far more than the cap.
        let r = e.execute(FILL, &json!({"start": 0, "end": RATE * 4 * 3600}));
        assert!(r.is_err());
        assert_eq!(clips(&e, t).len(), 1);
    }

    #[test]
    fn fill_ignores_non_audio_tracks_and_needs_loaded_audio() {
        let (mut e, t, ids) = engine(&[(0, RATE)]);
        e.execute(SET, &json!({"clip": ids[0].0})).unwrap();
        let aux = e.session_mut().add_track(TrackKind::Aux, ChannelFormat::Stereo, None);
        let r = e.execute(FILL, &json!({"tracks": [t.0, aux.0], "start": 0, "end": 2 * RATE})).unwrap();
        assert_eq!(r["gaps"], 1);
        let tone = e.session().room_tone.unwrap();
        e.session_mut().pool.remove(tone.source);
        assert!(e.execute(FILL, &json!({"start": 0, "end": 3 * RATE})).is_err(), "unloaded audio is not filled with");
    }

    /// Replaces the tone's pooled audio with `frames` frames of silence, as when its file was
    /// swapped for a shorter one: the stored `Source.frames` keeps the old length.
    fn shorten_audio(e: &mut Engine, frames: usize) {
        let src = e.session().sources.first().map(|f| f.id).unwrap();
        let buffer = AudioBuffer { sample_rate: 48_000, channels: vec![vec![0.0; frames]] };
        e.session_mut().pool.insert(src, Arc::new(SourceAudio::new(buffer)));
    }

    #[test]
    fn fill_reads_only_the_audio_that_decoded() {
        // Tone 0..1 s; the file now decodes to 0.5 s.
        let (mut e, t, ids) = engine(&[(0, RATE)]);
        e.execute(SET, &json!({"clip": ids[0].0})).unwrap();
        shorten_audio(&mut e, (RATE / 2) as usize);
        e.execute(FILL, &json!({"start": 0, "end": 3 * RATE})).unwrap();
        let fills: Vec<Clip> = clips(&e, t).into_iter().filter(|c| !ids.contains(&c.id)).collect();
        assert!(!fills.is_empty());
        for c in &fills {
            assert!(c.source_offset() + c.length <= RATE / 2, "a fill clip reads past the decoded audio: {c:?}");
        }
        // Less than the minimum left: refused, nothing placed.
        shorten_audio(&mut e, (RATE / 20) as usize);
        let before = clips(&e, t).len();
        assert!(e.execute(FILL, &json!({"start": 0, "end": 6 * RATE})).is_err());
        assert_eq!(clips(&e, t).len(), before);
    }

    #[test]
    fn set_measures_the_window_against_the_decoded_audio() {
        let (mut e, _, ids) = engine(&[(0, RATE)]);
        shorten_audio(&mut e, (RATE / 2) as usize);
        let r = e.execute(SET, &json!({"clip": ids[0].0})).unwrap();
        assert_eq!(r["length"].as_i64(), Some(RATE / 2));
        shorten_audio(&mut e, (RATE / 20) as usize);
        assert!(e.execute(SET, &json!({"clip": ids[0].0})).is_err(), "only 50 ms decoded");
    }

    #[test]
    fn clear_forgets_the_tone_but_keeps_the_fill() {
        let (mut e, t, ids) = engine(&[(0, RATE)]);
        e.execute(SET, &json!({"clip": ids[0].0})).unwrap();
        e.execute(FILL, &json!({"start": 0, "end": 2 * RATE})).unwrap();
        e.execute("edit.room_tone_clear", &json!({})).unwrap();
        assert_eq!(e.session().room_tone, None);
        assert_eq!(e.execute("edit.room_tone", &json!({})).unwrap(), Value::Null);
        assert_eq!(clips(&e, t).len(), 2);
    }

    #[test]
    fn tiles_never_stall_or_overrun_on_odd_lengths() {
        let tone = RoomTone { source: SourceId(1), offset: 0, length: 3, gain_db: 0.0 };
        for (gap, fade) in [(Range::new(0, 1), 10), (Range::new(5, 17), 2), (Range::new(0, 1000), 0), (Range::new(0, 7), 1000)] {
            let t = tiles(tone, gap, fade, fade, MAX_FILL_CLIPS).unwrap();
            assert_eq!(t.first().map(|c| c.start), Some(gap.start));
            assert_eq!(t.last().map(Clip::end), Some(gap.end));
            assert!(t.iter().all(|c| c.length > 0 && c.fade_in.len + c.fade_out.len <= c.length), "{gap:?} {fade}");
        }
        let huge = Range::new(0, soundcraft_time::MAX_POSITION);
        assert!(tiles(tone, huge, 0, 0, MAX_FILL_CLIPS).is_none(), "refused before allocating");
        assert_eq!(tiles(tone, Range::new(0, 9), 0, 0, 3).map(|t| t.len()), Some(3), "exactly the budget is fine");
    }
}
