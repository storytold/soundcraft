//! Tab and Tab to Transient: the insertion point or selection end to the next or previous clip boundary, transient or note.
use super::{CommandSpec, always, bool_or};

use crate::{Engine, Result, TransportRequest, cmd};
use serde_json::{Value, json};
use soundcraft_model::{Clip, ClipContent, Session, SourceAudio, TrackId};
use soundcraft_time::{Range, SampleRate, Samples};
use std::sync::{Arc, Mutex, PoisonError, Weak};

/// Transients are found in fixed blocks of each clip, so a long clip costs a block per press and
/// Tab and Alt+Tab always see the same transients.
const BLOCK_SECS: f64 = 10.0;
const SCAN_LIMIT: i64 = 64;
const CACHE_LIMIT: usize = 64;

/// Blocks already analysed, reused while their clips, clip effects and audio stay the same.
static ANALYSED: Mutex<Vec<Analysed>> = Mutex::new(Vec::new());

struct Analysed {
    track: TrackId,
    around: Range,
    rate: SampleRate,
    clips: Vec<Clip>,
    audio: Vec<Weak<SourceAudio>>,
    fx: Vec<(String, f64)>,
    hits: Vec<Samples>,
}

pub fn specs() -> Vec<CommandSpec> {
    vec![cmd!(noundo "edit.tab", "Tab", [], None, "{back?: bool, extend?: bool (extend the selection)}", always, tab)]
}

fn tab(e: &mut Engine, p: &Value) -> Result<Value> {
    let (back, extend) = (bool_or(p, "back", false), bool_or(p, "extend", false));
    let s = e.session();
    let sel = s.edit.selection;
    let to = |from| stop(s, from, back);
    let next = match (extend, back) {
        (false, _) => to(sel.start).map(Range::point),
        (true, false) => to(sel.end).map(|x| Range::new(sel.start, x)),
        // Alt+Shift+Tab pulls the end back while a stop lies inside the selection, then grows the start.
        (true, true) => match to(sel.end) {
            Some(x) if x > sel.start => Some(Range::new(sel.start, x)),
            _ => to(sel.start).map(|x| Range::new(x, sel.end)),
        },
    };
    let Some(r) = next else { return Ok(json!({"selection": sel})) };
    let s = e.session_mut();
    s.edit.selection = r;
    s.edit.selected_clips.clear();
    if s.edit.link_timeline_edit {
        s.edit.timeline_selection = r;
    }
    if !extend {
        s.edit.playhead = r.start;
        e.transport_requests.push(TransportRequest::Locate(r.start));
    }
    Ok(json!({"selection": r}))
}

/// The nearest stop after `from` (before it when `back`) on the selected tracks.
fn stop(s: &Session, from: Samples, back: bool) -> Option<Samples> {
    let nearer = |x: Samples, best: Option<Samples>| {
        if back { x < from && best.is_none_or(|b| x > b) } else { x > from && best.is_none_or(|b| x < b) }
    };
    let transients = s.edit.tab_to_transient;
    let tracks: Vec<_> = s.tracks.iter().filter(|t| s.edit.selected_tracks.contains(&t.id)).collect();
    let mut stops = Vec::new();
    for c in tracks.iter().flat_map(|t| t.clips()) {
        let sync = (c.sync_point > 0 && c.sync_point < c.length).then_some(c.start.saturating_add(c.sync_point));
        let notes: Vec<Samples> = match &c.content {
            ClipContent::Midi { sequence } if transients => {
                let base = s.tempo.samples_to_ticks(c.start, s.sample_rate);
                let ticks = sequence.notes.iter().flat_map(|n| {
                    let on = base.saturating_add(n.start);
                    [on, on.saturating_add(n.length)]
                });
                ticks.map(|tick| s.tempo.tick_to_samples(tick, s.sample_rate)).filter(|x| c.range().contains(*x)).collect()
            }
            _ => Vec::new(),
        };
        stops.extend([c.start, c.end()].into_iter().chain(sync).chain(notes));
    }
    let mut best = None;
    for &x in &stops {
        if nearer(x, best) {
            best = Some(x);
        }
    }
    if transients {
        // A hit right at a clip start or note is the same stop, not a second one a few samples later.
        let gap = s.sample_rate.samples(0.01).unsigned_abs();
        let apart = |x: Samples| stops.iter().all(|b| b.abs_diff(x) > gap);
        for t in &tracks {
            for c in t.clips().iter().filter(|c| matches!(c.content, ClipContent::Audio { .. })) {
                if let Some(x) = transient(s, t.id, c.range(), from, back, best, apart) {
                    best = Some(x);
                }
            }
        }
    }
    best
}

/// The transient in `clip` nearest to `from` on its side, if it is nearer than `best`.
fn transient(
    s: &Session,
    t: TrackId,
    clip: Range,
    from: Samples,
    back: bool,
    best: Option<Samples>,
    apart: impl Fn(Samples) -> bool,
) -> Option<Samples> {
    let (lo, hi) = if back {
        (best.map_or(clip.start, |b| b.saturating_add(1)).max(clip.start), from.min(clip.end))
    } else {
        (from.saturating_add(1).max(clip.start), best.unwrap_or(clip.end).min(clip.end))
    };
    if lo >= hi {
        return None;
    }
    let block = s.sample_rate.samples(BLOCK_SECS).max(1);
    let margin = s.sample_rate.samples(0.25);
    let first = lo.saturating_sub(clip.start) / block;
    let last = hi.saturating_sub(1).saturating_sub(clip.start) / block;
    for i in 0..=last.saturating_sub(first).min(SCAN_LIMIT) {
        let k = if back { last - i } else { first + i };
        let start = clip.start.saturating_add(k.saturating_mul(block));
        let b = Range::new(start, start.saturating_add(block).min(clip.end));
        let around = Range::new(b.start.saturating_sub(margin), b.end.saturating_add(margin));
        let hits = analysed(s, t, around).into_iter().filter(|x| b.contains(*x) && (lo..hi).contains(x) && apart(*x));
        if let Some(x) = if back { hits.max() } else { hits.min() } {
            return Some(x);
        }
    }
    None
}

/// The transients of track `t` in `around`, from the cache when nothing there has changed.
fn analysed(s: &Session, t: TrackId, around: Range) -> Vec<Samples> {
    let clips: Vec<Clip> = s.track(t).map_or_else(Vec::new, |tr| tr.clips().iter().filter(|c| c.range().overlaps(&around)).cloned().collect());
    let audio: Vec<Weak<SourceAudio>> = clips
        .iter()
        .filter_map(|c| if let ClipContent::Audio { source, .. } = c.content { s.pool.get(source).map(Arc::downgrade) } else { None })
        .collect();
    let fx: Vec<(String, f64)> = clips
        .iter()
        .flat_map(|c| {
            let pre = format!("clip_fx.{}.", c.id.0);
            s.edit.values.range(pre.clone()..).take_while(move |(k, _)| k.starts_with(&pre)).map(|(k, v)| (k.clone(), *v))
        })
        .collect();
    let same = |a: &Analysed| {
        a.track == t
            && a.around == around
            && a.rate == s.sample_rate
            && a.clips == clips
            && a.fx == fx
            && a.audio.len() == audio.len()
            && a.audio.iter().zip(&audio).all(|(x, y)| x.ptr_eq(y))
    };
    if let Some(a) = ANALYSED.lock().unwrap_or_else(PoisonError::into_inner).iter().find(|a| same(a)) {
        return a.hits.clone();
    }
    let hits = crate::io::transients_in(s, t, around, 0.5);
    let mut kept = ANALYSED.lock().unwrap_or_else(PoisonError::into_inner);
    if kept.len() >= CACHE_LIMIT {
        kept.remove(0);
    }
    kept.push(Analysed { track: t, around, rate: s.sample_rate, clips, audio, fx, hits: hits.clone() });
    hits
}

#[cfg(test)]
mod tests {
    use crate::Engine;
    use serde_json::json;
    use soundcraft_audio_io::{AudioBuffer, FileFormat};
    use soundcraft_midi::{Note, Sequence};
    use soundcraft_model::{ChannelFormat, Clip, Session, TrackId, TrackKind};
    use soundcraft_time::{SampleRate, TICKS_PER_QUARTER};

    const SR: f64 = 48_000.0;

    fn at(secs: f64) -> i64 {
        (secs * SR) as i64
    }

    fn hits(times: &[f64], secs: f64) -> Vec<f32> {
        let mut v = vec![0.0f32; at(secs) as usize];
        let mut seed = 7u32;
        for &t in times {
            for i in 0..at(0.2) as usize {
                seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                let noise = (seed >> 8) as f32 / (1u32 << 24) as f32 - 0.5;
                if let Some(x) = v.get_mut(at(t) as usize + i) {
                    *x = if i == 0 { 0.9 } else { noise * (-(i as f32) / 1500.0).exp() };
                }
            }
        }
        v
    }

    fn audio_track(s: &mut Session, name: &str, clips: &[(f64, f64)], audio: Vec<f32>) -> TrackId {
        let src = crate::io::add_source(s, name, AudioBuffer { sample_rate: 48_000, channels: vec![audio] }, None, FileFormat::Wav);
        let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, Some(name));
        for &(start, len) in clips {
            let id = s.new_clip_id();
            crate::edit::place_clip(s, t, Clip::audio(id, name, src, 0, at(start), at(len)));
        }
        t
    }

    fn tab(e: &mut Engine, back: bool, extend: bool) -> (i64, i64) {
        e.execute("edit.tab", &json!({"back": back, "extend": extend})).unwrap();
        let r = e.session().edit.selection;
        (r.start, r.end)
    }

    fn walk(e: &mut Engine, back: bool) -> Vec<i64> {
        let mut out = Vec::new();
        loop {
            let (x, _) = tab(e, back, false);
            if out.last() == Some(&x) {
                return out;
            }
            out.push(x);
        }
    }

    #[test]
    fn tab_steps_through_clip_boundaries_and_sync_points_and_extends_the_selection() {
        let mut s = Session::new("t", SampleRate::HZ_48000);
        let t = audio_track(&mut s, "A", &[(1.0, 2.0), (5.0, 3.0)], vec![0.0; at(10.0) as usize]);
        if let Some(c) = s.track_mut(t).and_then(|t| t.playlist_mut()).and_then(|p| p.clips.get_mut(1)) {
            c.sync_point = at(1.0);
        }
        let mut e = Engine::new(s);
        e.execute("edit.select", &json!({"tracks": [t.0], "start": 0})).unwrap();
        assert_eq!(walk(&mut e, false), [1.0, 3.0, 5.0, 6.0, 8.0].map(at));
        assert_eq!(walk(&mut e, true), [6.0, 5.0, 3.0, 1.0].map(at));
        assert_eq!(e.transport_requests.last(), Some(&crate::TransportRequest::Locate(at(1.0))));

        assert_eq!(tab(&mut e, false, true), (at(1.0), at(3.0)));
        assert_eq!(tab(&mut e, false, true), (at(1.0), at(5.0)));
        assert_eq!(tab(&mut e, true, true), (at(1.0), at(3.0)));
        e.execute("edit.select", &json!({"start": at(5.5), "end": at(7.0)})).unwrap();
        assert_eq!(tab(&mut e, true, true), (at(5.5), at(6.0)));
        assert_eq!(tab(&mut e, true, true), (at(5.0), at(6.0)));
    }

    #[test]
    fn tab_to_transient_finds_the_same_hits_both_ways_across_analysis_blocks() {
        let planted = [0.5, 1.7, 9.9, 10.1, 15.0, 19.95, 23.0];
        let mut s = Session::new("t", SampleRate::HZ_48000);
        let t = audio_track(&mut s, "Drums", &[(2.0, 25.0)], hits(&[&[0.003][..], &planted].concat(), 25.0));
        let mut e = Engine::new(s);
        e.execute("options.tab_to_transient", &json!({"value": true})).unwrap();
        e.execute("edit.select", &json!({"tracks": [t.0], "start": 0})).unwrap();
        let forward = walk(&mut e, false);
        let mut backward = walk(&mut e, true);
        backward.reverse();
        backward.push(at(27.0));
        assert_eq!(forward, [&[at(2.0)][..], &backward[1..]].concat());
        assert_eq!(forward.len(), planted.len() + 2, "{forward:?}");
        for (found, want) in forward[1..=planted.len()].iter().zip(planted) {
            assert!((found - at(2.0 + want)).abs() < 256, "{found} vs {want} s");
        }
    }

    #[test]
    fn tab_to_transient_stops_at_the_first_hit_on_any_selected_track_and_at_midi_notes() {
        let mut s = Session::new("t", SampleRate::HZ_48000);
        let keys = s.add_track(TrackKind::Instrument, ChannelFormat::Stereo, Some("Keys"));
        let mut seq = Sequence::default();
        seq.notes.push(Note { pitch: 60, velocity: 100, release_velocity: 64, channel: 0, start: 4 * TICKS_PER_QUARTER, length: TICKS_PER_QUARTER });
        let id = s.new_clip_id();
        crate::edit::place_clip(&mut s, keys, Clip::midi(id, "Keys", 0, at(4.0), seq));
        let a = audio_track(&mut s, "A", &[(0.0, 4.0)], hits(&[1.0, 3.0], 4.0));
        let b = audio_track(&mut s, "B", &[(0.0, 4.0)], hits(&[2.2], 4.0));
        let note = |tick| s.tempo.tick_to_samples(tick, s.sample_rate);
        let (on, off) = (note(4 * TICKS_PER_QUARTER), note(5 * TICKS_PER_QUARTER));
        let mut e = Engine::new(s);
        e.execute("edit.select", &json!({"tracks": [keys.0, a.0, b.0], "start": 0})).unwrap();
        assert_eq!(walk(&mut e, false), [at(4.0)]);
        e.execute("options.tab_to_transient", &json!({"value": true})).unwrap();
        e.execute("edit.select", &json!({"start": 0})).unwrap();
        let stops = walk(&mut e, false);
        assert_eq!(stops.len(), 6, "{stops:?}");
        for (i, want) in [(0, 1.0), (2, 2.2), (4, 3.0)] {
            assert!((stops[i] - at(want)).abs() < 256, "{stops:?}");
        }
        assert_eq!([stops[1], stops[3], stops[5]], [on, off, at(4.0)]);
    }
}
