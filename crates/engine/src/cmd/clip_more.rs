//! More Clip menu commands: Regroup, Unloop, Clip Effects bypass/render, Conform to Tempo and
//! Remove Pitch Shift.
use super::{CommandSpec, bad, clip_ids_param, has_selection, str_param};

use super::more_util::{render_stretched, replace_clip};
use crate::{Engine, Result, cmd};
use serde_json::{Value, json};
use soundcraft_model::{Clip, ClipContent, ClipId, Session, TrackId};
use soundcraft_time::Samples;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "clip.regroup",
            "Regroup",
            ["Clip"],
            Some("Cmd+Alt+R"),
            "{clips?} — puts the clips back into one clip group (reusing their most common group)",
            has_selection,
            regroup
        ),
        cmd!(
            "clip.unloop",
            "Unloop...",
            ["Clip"],
            None,
            "{clips?, mode?: remove|flatten} — remove = keep only the original clip; flatten = keep the copies as ordinary clips",
            has_selection,
            unloop
        ),
        cmd!(
            "clip.effects_set",
            "Set Clip Effects",
            [],
            None,
            "{clips?, params: {name: number}} — stores clip-effect settings on the clips",
            has_selection,
            effects_set
        ),
        cmd!(
            "clip.effects_bypass",
            "Bypass",
            ["Clip", "Clip Effects"],
            None,
            "{clips?, value?: bool} (toggles when omitted)",
            has_selection,
            effects_bypass
        ),
        cmd!(
            "clip.effects_render",
            "Render",
            ["Clip", "Clip Effects"],
            None,
            "{clips?} — renders clip gain and fades into new audio and clears the clip effects",
            has_selection,
            effects_render
        ),
        cmd!(
            "clip.conform_to_tempo",
            "Conform to Tempo",
            ["Clip"],
            None,
            "{clips?, source_bpm?} — stretches audio clips so their length in bars follows the session tempo; the first conform records the clip's tempo",
            has_selection,
            conform
        ),
        cmd!(
            "clip.remove_pitch_shift",
            "Remove Pitch Shift",
            ["Clip"],
            None,
            "{clips?} — re-renders vari-speed (stretched) clips with pitch preserved",
            has_selection,
            remove_pitch_shift
        ),
    ]
}

fn regroup(e: &mut Engine, p: &Value) -> Result<Value> {
    let ids = clip_ids_param(e, p);
    if ids.len() < 2 {
        return Err(bad("clip.regroup", "select two or more clips"));
    }
    let s = e.session_mut();
    let mut counts: std::collections::BTreeMap<u64, usize> = Default::default();
    for id in &ids {
        if let Some(g) = s.find_clip(*id).and_then(|(_, c)| c.group) {
            *counts.entry(g).or_default() += 1;
        }
    }
    let g = counts.iter().max_by_key(|(_, n)| **n).map(|(g, _)| *g).unwrap_or_else(|| s.alloc());
    let mut n = 0;
    for id in &ids {
        if let Some(c) = s.find_clip_mut(*id) {
            c.group = Some(g);
            n += 1;
        }
    }
    Ok(json!({"group": g, "clips": n}))
}

fn same_content(a: &Clip, b: &Clip) -> bool {
    match (&a.content, &b.content) {
        (ClipContent::Audio { source: s1, offset: o1 }, ClipContent::Audio { source: s2, offset: o2 }) => s1 == s2 && o1 == o2,
        (ClipContent::Midi { sequence: q1 }, ClipContent::Midi { sequence: q2 }) => q1 == q2,
        (ClipContent::Video { source: s1, offset: o1 }, ClipContent::Video { source: s2, offset: o2 }) => s1 == s2 && o1 == o2,
        _ => false,
    }
}

fn same_material(a: &Clip, b: &Clip) -> bool {
    a.length == b.length && (a.loop_id.is_some() && a.loop_id == b.loop_id || same_content(a, b))
}

fn same_source(a: &Clip, b: &Clip) -> bool {
    match (&a.content, &b.content) {
        (ClipContent::Audio { source: s1, .. }, ClipContent::Audio { source: s2, .. }) => s1 == s2,
        (ClipContent::Midi { sequence: q1 }, ClipContent::Midi { sequence: q2 }) => q1 == q2,
        (ClipContent::Video { source: s1, .. }, ClipContent::Video { source: s2, .. }) => s1 == s2,
        _ => false,
    }
}

/// Whether `next` is the iteration after `prev`, crossfaded into it by Clip Looping: the two share
/// the loop and overlap, and `next` starts its material at most the overlap before `prev`'s (it was
/// pulled back over its handle), where the second half of a crossfaded split clip starts later.
fn crossfaded(prev: &Clip, next: &Clip) -> bool {
    let overlap = prev.end().saturating_sub(next.start);
    let pulled_back = prev.source_offset().saturating_sub(next.source_offset());
    prev.loop_id.is_some()
        && prev.loop_id == next.loop_id
        && same_source(prev, next)
        && prev.start < next.start
        && overlap > 0
        && prev.end() <= next.end()
        && (0..=overlap).contains(&pulled_back)
}

/// Whether `short` is the last iteration of `full`'s loop, cut short at its end. It shares the loop,
/// it is shorter, and no clip of the loop starts where it ends, as the other half of a split clip
/// does. When `full` is a whole copy (an iteration of the same length ends where it starts), that is
/// enough. When `full` is the loop's origin alone, `short` also has to hold `full`'s material.
fn cut_short(clips: &[Clip], full: &Clip, short: &Clip) -> bool {
    let whole_copy = || clips.iter().any(|x| x.id != full.id && x.end() == full.start && same_material(x, full));
    full.loop_id.is_some()
        && full.loop_id == short.loop_id
        && short.length < full.length
        && !clips.iter().any(|x| x.loop_id == short.loop_id && x.start == short.end())
        && (whole_copy() || same_content(full, short))
}

/// The loop a clip belongs to: back-to-back copies of the same material, the last of which can be
/// cut short. Origin first.
fn loop_chain(s: &Session, t: TrackId, id: ClipId) -> Vec<ClipId> {
    let Some(tr) = s.track(t) else { return Vec::new() };
    let clips = tr.clips();
    let Some(mut cur) = clips.iter().find(|c| c.id == id) else { return Vec::new() };
    let mut guard = 0;
    while let Some(prev) =
        clips.iter().find(|x| x.id != cur.id && (x.end() == cur.start && (same_material(x, cur) || cut_short(clips, x, cur)) || crossfaded(x, cur)))
    {
        cur = prev;
        guard += 1;
        if guard > clips.len() {
            break;
        }
    }
    let mut chain = vec![cur.id];
    while let Some(next) = clips.iter().find(|x| !chain.contains(&x.id) && (x.start == cur.end() && same_material(x, cur) || crossfaded(cur, x))) {
        chain.push(next.id);
        cur = next;
    }
    if let Some(last) = clips.iter().find(|x| !chain.contains(&x.id) && x.start == cur.end() && cut_short(clips, cur, x)) {
        chain.push(last.id);
    }
    chain
}

fn unloop(e: &mut Engine, p: &Value) -> Result<Value> {
    let id = "clip.unloop";
    let remove = match str_param(p, "mode") {
        None | Some("remove") => true,
        Some("flatten") => false,
        Some(m) => return Err(bad(id, format!("unknown mode `{m}`"))),
    };
    let ids = clip_ids_param(e, p);
    let s = e.session_mut();
    let mut done: Vec<ClipId> = Vec::new();
    let mut gone: Vec<ClipId> = Vec::new();
    let mut affected = 0;
    for cid in ids {
        if done.contains(&cid) {
            continue;
        }
        let Some(t) = s.find_clip(cid).map(|(t, _)| t) else { continue };
        let chain = loop_chain(s, t, cid);
        done.extend(chain.iter().copied());
        if chain.len() < 2 {
            continue;
        }
        let copies: Vec<ClipId> = chain.iter().skip(1).copied().collect();
        if let Some(pl) = s.track_mut(t).and_then(|tr| tr.playlist_mut()) {
            if remove {
                // A crossfade into the first copy goes with it: the origin ends at the seam again.
                let first = copies.first().and_then(|id| pl.clips.iter().find(|c| c.id == *id)).cloned();
                if let (Some(next), Some(origin)) = (first, chain.first().and_then(|id| pl.clips.iter_mut().find(|c| c.id == *id)))
                    && crossfaded(origin, &next)
                {
                    let seam = next.start.saturating_add(origin.source_offset().saturating_sub(next.source_offset()));
                    origin.trim_end_to(seam);
                    origin.fade_out = soundcraft_model::Fade::default();
                }
                pl.clips.retain(|c| !copies.contains(&c.id));
                gone.extend(copies.iter().copied());
            } else {
                for c in pl.clips.iter_mut().filter(|c| chain.contains(&c.id)) {
                    c.group = None;
                    c.loop_id = None;
                }
            }
        }
        affected += copies.len();
    }
    if affected == 0 {
        return Err(bad(id, "the selection is not a looped clip"));
    }
    s.edit.selected_clips.retain(|c| !gone.contains(c));
    Ok(json!({"copies": affected, "removed": remove}))
}

fn effects_set(e: &mut Engine, p: &Value) -> Result<Value> {
    let ids = clip_ids_param(e, p);
    let params = p.get("params").and_then(Value::as_object).ok_or_else(|| bad("clip.effects_set", "`params` must be an object of numbers"))?;
    let vals: Vec<(String, f64)> = params
        .iter()
        .filter_map(|(k, v)| v.as_f64().filter(|x| x.is_finite()).map(|x| (k.chars().take(64).collect(), x)))
        .map(|(k, v)| {
            let v = if k == "gain" { v.clamp(f64::from(soundcraft_dsp::MIN_DB), f64::from(soundcraft_mix::CLIP_FX_MAX_GAIN_DB)) } else { v };
            (k, v)
        })
        .collect();
    let s = e.session_mut();
    for c in &ids {
        for (k, v) in &vals {
            s.edit.values.insert(format!("clip_fx.{}.{k}", c.0), *v);
        }
    }
    Ok(json!({"clips": ids.len(), "params": vals.len()}))
}

fn effects_bypass(e: &mut Engine, p: &Value) -> Result<Value> {
    let ids = clip_ids_param(e, p);
    if ids.is_empty() {
        return Err(bad("clip.effects_bypass", "select a clip"));
    }
    let s = e.session_mut();
    let key = |c: &ClipId| format!("clip_fx.bypass.{}", c.0);
    let on = p.get("value").and_then(Value::as_bool).unwrap_or_else(|| !ids.iter().all(|c| s.edit.flag(&key(c))));
    for c in &ids {
        s.edit.set_flag(&key(c), on);
    }
    Ok(json!({"bypass": on, "clips": ids.len()}))
}

fn effects_render(e: &mut Engine, p: &Value) -> Result<Value> {
    let ids = clip_ids_param(e, p);
    let mut n = 0;
    for id in &ids {
        if crate::io::render_clip_gain(e, "clip.effects_render", *id)? {
            n += 1;
        }
        let s = e.session_mut();
        let pre = format!("clip_fx.{}.", id.0);
        s.edit.values.retain(|k, _| !k.starts_with(&pre));
        s.edit.set_flag(&format!("clip_fx.bypass.{}", id.0), false);
        if let Some(c) = s.find_clip_mut(*id) {
            c.fade_in.len = c.fade_in.len.min(c.length);
            c.fade_out.len = c.fade_out.len.min(c.length);
        }
    }
    Ok(json!({"rendered": n}))
}

fn conform(e: &mut Engine, p: &Value) -> Result<Value> {
    let id = "clip.conform_to_tempo";
    let ids = clip_ids_param(e, p);
    let given = p.get("source_bpm").and_then(Value::as_f64).filter(|b| b.is_finite());
    if let Some(b) = given
        && !(5.0..=1000.0).contains(&b)
    {
        return Err(bad(id, "source_bpm must be 5..1000"));
    }
    let s = e.session_mut();
    let mut out = Vec::new();
    for cid in ids {
        let Some((_, c)) = s.find_clip(cid).map(|(t, c)| (t, c.clone())) else { continue };
        if !c.is_audio() || c.edit_locked {
            continue;
        }
        let bpm_key = format!("clip.bpm.{}", cid.0);
        let len_key = format!("clip.len0.{}", cid.0);
        let stretch = if c.stretch.is_finite() && c.stretch > 0.0 { c.stretch } else { 1.0 };
        let now = s.tempo.tempo_at_tick(s.tempo.samples_to_ticks(c.start, s.sample_rate));
        let src_bpm = given.unwrap_or_else(|| s.edit.value(&bpm_key, now));
        let len0 = s.edit.value(&len_key, c.length as f64 / stretch).max(1.0);
        s.edit.values.insert(bpm_key, src_bpm);
        s.edit.values.insert(len_key, len0);
        let ratio = (src_bpm / now).clamp(0.05, 20.0);
        let new_len = ((len0 * ratio).round() as Samples).max(1);
        if let Some(cl) = s.find_clip_mut(cid) {
            cl.stretch = ratio;
            cl.length = new_len;
            cl.fade_in.len = cl.fade_in.len.min(new_len);
            cl.fade_out.len = cl.fade_out.len.min(new_len);
        }
        out.push(json!({"clip": cid, "source_bpm": src_bpm, "bpm": now, "stretch": ratio}));
    }
    Ok(json!({"clips": out}))
}

fn remove_pitch_shift(e: &mut Engine, p: &Value) -> Result<Value> {
    let id = "clip.remove_pitch_shift";
    let ids = clip_ids_param(e, p);
    let s = e.session_mut();
    let mut n = 0;
    for cid in ids {
        let Some((t, c)) = s.find_clip(cid).map(|(t, c)| (t, c.clone())) else { continue };
        s.edit.values.remove(&format!("clip.pitch.{}", cid.0));
        if !c.is_audio() || (c.stretch - 1.0).abs() < 1e-9 || !c.stretch.is_finite() {
            continue;
        }
        let Some(src) = render_stretched(s, id, &c, c.length)? else { continue };
        let mut nc = c.clone();
        nc.content = ClipContent::Audio { source: src, offset: 0 };
        nc.stretch = 1.0;
        replace_clip(s, t, cid, nc);
        n += 1;
    }
    Ok(json!({"clips": n}))
}

#[cfg(test)]
mod tests {
    use super::*;
    use soundcraft_model::{ChannelFormat, SourceId, TrackKind};

    fn engine_with(clips: &[(Samples, Samples, Samples)]) -> (Engine, TrackId, Vec<ClipId>) {
        let mut s = Session::default();
        let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, None);
        let mut ids = Vec::new();
        for (st, len, off) in clips {
            let id = s.new_clip_id();
            crate::edit::place_clip(&mut s, t, Clip::audio(id, "c", SourceId(9), *off, *st, *len));
            ids.push(id);
        }
        s.edit.selected_tracks = vec![t];
        (Engine::new(s), t, ids)
    }

    #[test]
    fn effects_set_caps_gain() {
        let (mut e, _t, ids) = engine_with(&[(0, 100, 0)]);
        e.execute("clip.effects_set", &json!({"clip": ids[0].0, "params": {"gain": 500.0}})).unwrap();
        assert_eq!(e.session().edit.values.get(&format!("clip_fx.{}.gain", ids[0].0)).copied(), Some(24.0));
    }

    #[test]
    fn unloop_removes_copies() {
        let (mut e, t, ids) = engine_with(&[(0, 100, 0), (100, 100, 0), (200, 100, 0), (300, 100, 50)]);
        e.execute("clip.unloop", &json!({"clip": ids[1].0})).unwrap();
        let left: Vec<ClipId> = e.session().track(t).unwrap().clips().iter().map(|c| c.id).collect();
        assert_eq!(left, vec![ids[0], ids[3]]);
        assert!(e.execute("clip.unloop", &json!({"clip": ids[3].0})).is_err());
    }

    /// A 100-sample MIDI clip with one note, on a fresh instrument-less MIDI track.
    fn midi_engine() -> (Engine, TrackId, ClipId) {
        let mut s = Session::default();
        let t = s.add_track(TrackKind::Midi, ChannelFormat::Mono, None);
        let id = s.new_clip_id();
        let mut seq = soundcraft_midi::Sequence::default();
        seq.notes.push(soundcraft_midi::Note { pitch: 60, velocity: 100, release_velocity: 64, channel: 0, start: 0, length: 240 });
        crate::edit::place_clip(&mut s, t, Clip::midi(id, "m", 0, 100, seq));
        s.edit.selected_tracks = vec![t];
        (Engine::new(s), t, id)
    }

    #[test]
    fn unloop_recognises_loops_after_source_edit() {
        let (mut e, t, id) = midi_engine();
        assert_eq!(e.execute("clip.loop", &json!({"clip": id.0, "count": 4})).unwrap()["loops"], 3);
        // Editing the source note makes the copies' content differ from it.
        if let Some(c) = e.session_mut().find_clip_mut(id)
            && let ClipContent::Midi { sequence } = &mut c.content
        {
            sequence.notes[0].length = 1100;
        }
        e.execute("clip.unloop", &json!({"clip": id.0})).unwrap();
        let left: Vec<ClipId> = e.session().track(t).unwrap().clips().iter().map(|c| c.id).collect();
        assert_eq!(left, vec![id]);
    }

    #[test]
    fn unloop_flatten_turns_loop_into_independent_clips() {
        let (mut e, t, id) = midi_engine();
        e.execute("clip.loop", &json!({"clip": id.0, "count": 3})).unwrap();
        e.execute("clip.unloop", &json!({"clip": id.0, "mode": "flatten"})).unwrap();
        assert_eq!(e.session().track(t).unwrap().clips().len(), 3);
        assert!(e.session().track(t).unwrap().clips().iter().all(|c| c.loop_id.is_none() && c.group.is_none()));
    }

    fn spans(e: &Engine, t: TrackId) -> Vec<(Samples, Samples)> {
        e.session().track(t).unwrap().clips().iter().map(|c| (c.start, c.length)).collect()
    }

    #[test]
    fn loop_length_cuts_the_last_iteration_short() {
        let (mut e, t, id) = midi_engine();
        assert_eq!(e.execute("clip.loop", &json!({"clip": id.0, "length": 250})).unwrap()["loops"], 2);
        assert_eq!(spans(&e, t), vec![(0, 100), (100, 100), (200, 50)]);
        // A length that is a whole number of loops leaves no short one.
        let (mut e, t, id) = midi_engine();
        assert_eq!(e.execute("clip.loop", &json!({"clip": id.0, "length": 300})).unwrap()["loops"], 2);
        assert_eq!(spans(&e, t), vec![(0, 100), (100, 100), (200, 100)]);
    }

    #[test]
    fn a_short_last_iteration_fits_its_fades_and_drops_a_sync_point_past_the_cut() {
        let (mut e, t, id) = midi_engine();
        if let Some(c) = e.session_mut().find_clip_mut(id) {
            c.sync_point = 80;
            c.fade_out.len = 70;
        }
        e.execute("clip.loop", &json!({"clip": id.0, "length": 150})).unwrap();
        let clips = e.session().track(t).unwrap().clips();
        let last = clips.last().unwrap();
        assert_eq!((last.start, last.length, last.sync_point, last.fade_out.len), (100, 50, 0, 50));
        assert_eq!((clips[0].sync_point, clips[0].fade_out.len), (80, 70));
    }

    #[test]
    fn unloop_removes_a_short_last_iteration_too() {
        let (mut e, t, id) = midi_engine();
        e.execute("clip.loop", &json!({"clip": id.0, "length": 250})).unwrap();
        e.execute("clip.unloop", &json!({"clip": id.0})).unwrap();
        assert_eq!(spans(&e, t), vec![(0, 100)]);
        // An edit to the source note or to the short loop's note.
        for edited in [0, 2] {
            let (mut e, t, id) = midi_engine();
            e.execute("clip.loop", &json!({"clip": id.0, "length": 250})).unwrap();
            let target = e.session().track(t).unwrap().clips()[edited].id;
            if let Some(c) = e.session_mut().find_clip_mut(target)
                && let ClipContent::Midi { sequence } = &mut c.content
            {
                sequence.notes[0].length = 1100;
            }
            e.execute("clip.unloop", &json!({"clip": id.0})).unwrap();
            assert_eq!(spans(&e, t), vec![(0, 100)], "edited loop {edited}");
        }
    }

    #[test]
    fn unloop_keeps_both_halves_of_a_split_clip() {
        let (mut e, t, ids) = engine_with(&[(0, 100, 0)]);
        e.execute("clip.loop", &json!({"clip": ids[0].0, "count": 3})).unwrap();
        crate::edit::separate_at(e.session_mut(), t, 230);
        e.execute("clip.unloop", &json!({"clip": ids[0].0})).unwrap();
        assert_eq!(spans(&e, t), vec![(0, 100), (200, 30), (230, 70)]);
        // A split loop origin is not a loop.
        let (mut e, t, ids) = engine_with(&[(0, 100, 0)]);
        e.execute("clip.loop", &json!({"clip": ids[0].0, "count": 2})).unwrap();
        crate::edit::separate_at(e.session_mut(), t, 30);
        assert!(e.execute("clip.unloop", &json!({"clip": ids[0].0})).is_err());
        assert_eq!(spans(&e, t), vec![(0, 30), (30, 70), (100, 100)]);
    }

    /// One 1000-sample clip at 0 playing from `offset` in a file of `frames` frames; (1000, 10_000)
    /// leaves handles on both sides.
    fn engine_with_handles(offset: Samples, frames: u64) -> (Engine, TrackId, ClipId) {
        let mut s = Session::default();
        let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, None);
        s.sources.push(soundcraft_model::Source {
            id: SourceId(9),
            name: "src".into(),
            path: "src.wav".into(),
            channels: 1,
            frames,
            sample_rate: 48_000,
            format: soundcraft_audio_io::FileFormat::Wav,
            time_reference: 0,
            unsaved: false,
        });
        let id = s.new_clip_id();
        crate::edit::place_clip(&mut s, t, Clip::audio(id, "c", SourceId(9), offset, 0, 1000));
        s.edit.selected_tracks = vec![t];
        (Engine::new(s), t, id)
    }

    fn fades(e: &Engine, t: TrackId) -> Vec<(Samples, Samples)> {
        e.session().track(t).unwrap().clips().iter().map(|c| (c.fade_in.len, c.fade_out.len)).collect()
    }

    #[test]
    fn loop_crossfade_overlaps_each_seam_over_the_handles() {
        let (mut e, t, id) = engine_with_handles(1000, 10_000);
        let r = e.execute("clip.loop", &json!({"clip": id.0, "count": 3, "crossfade": 200, "crossfade_shape": "linear"})).unwrap();
        assert_eq!(r["loops"], 2);
        // Each seam (1000, 2000) is crossfaded 100 samples either side.
        assert_eq!(spans(&e, t), vec![(0, 1100), (900, 1200), (1900, 1100)]);
        assert_eq!(fades(&e, t), vec![(0, 200), (200, 200), (200, 0)]);
        let clips = e.session().track(t).unwrap().clips().to_vec();
        assert!(clips.iter().all(|c| c.fade_out.len == 0 || c.fade_out.shape == soundcraft_model::FadeShape::Linear));
        // Every iteration still plays from the same point in its period.
        assert_eq!(clips.iter().map(Clip::source_offset).collect::<Vec<_>>(), vec![1000, 900, 900]);
        // Unloop still finds the loop and puts the origin back to its own end without the crossfade.
        e.execute("clip.unloop", &json!({"clip": clips[1].id.0})).unwrap();
        assert_eq!(spans(&e, t), vec![(0, 1000)]);
        assert_eq!(fades(&e, t), vec![(0, 0)]);
        // Undo brings the crossfaded loop back.
        e.execute("edit.undo", &json!({})).unwrap();
        assert_eq!(spans(&e, t), vec![(0, 1100), (900, 1200), (1900, 1100)]);
    }

    #[test]
    fn loop_crossfade_never_reaches_the_clip_after_the_loop() {
        // Fill stops at a clip 50 samples into the last loop, shorter than half the crossfade:
        // that clip must not be pulled back or faded in.
        let (mut e, t, id) = engine_with_handles(1000, 10_000);
        let next_id = e.session_mut().new_clip_id();
        crate::edit::place_clip(e.session_mut(), t, Clip::audio(next_id, "next", SourceId(9), 5000, 2050, 500));
        e.execute("clip.loop", &json!({"clip": id.0, "fill": true, "crossfade": 200})).unwrap();
        let clips = e.session().track(t).unwrap().clips().to_vec();
        let next = clips.iter().find(|c| c.id == next_id).unwrap();
        assert_eq!((next.start, next.length, next.source_offset(), next.fade_in.len), (2050, 500, 5000, 0));
        // The last copy still crossfades with the one before it and ends where the next clip starts.
        assert_eq!(spans(&e, t), vec![(0, 1100), (900, 1150), (1900, 150), (2050, 500)]);
        assert_eq!(fades(&e, t), vec![(0, 200), (200, 150), (150, 0), (0, 0)]);
        // Unloop leaves the next clip as it was.
        e.execute("clip.unloop", &json!({"clip": id.0})).unwrap();
        assert_eq!(spans(&e, t), vec![(0, 1000), (2050, 500)]);
        assert_eq!(fades(&e, t), vec![(0, 0), (0, 0)]);
    }

    #[test]
    fn loop_crossfade_reaches_a_short_last_loop_and_flatten_keeps_it() {
        let (mut e, t, id) = engine_with_handles(1000, 10_000);
        e.execute("clip.loop", &json!({"clip": id.0, "length": 2500, "crossfade": 200})).unwrap();
        assert_eq!(spans(&e, t), vec![(0, 1100), (900, 1200), (1900, 600)]);
        assert_eq!(fades(&e, t), vec![(0, 200), (200, 200), (200, 0)]);
        e.execute("clip.unloop", &json!({"clip": id.0, "mode": "flatten"})).unwrap();
        assert_eq!(spans(&e, t), vec![(0, 1100), (900, 1200), (1900, 600)]);
        assert!(e.session().track(t).unwrap().clips().iter().all(|c| c.loop_id.is_none()));
    }

    #[test]
    fn loop_crossfade_without_handles_fades_at_the_seam() {
        // The clip plays the whole file: nothing to overlap, so each side fades over its half.
        let (mut e, t, id) = engine_with_handles(0, 1000);
        e.execute("clip.loop", &json!({"clip": id.0, "count": 2, "crossfade": 200})).unwrap();
        assert_eq!(spans(&e, t), vec![(0, 1000), (1000, 1000)]);
        assert_eq!(fades(&e, t), vec![(0, 100), (100, 0)]);
        e.execute("clip.unloop", &json!({"clip": id.0})).unwrap();
        assert_eq!(spans(&e, t), vec![(0, 1000)]);
    }

    #[test]
    fn loop_crossfade_is_capped_and_checked() {
        // At most half the clip: 250 either side of each seam, so neighbouring crossfades never meet.
        let (mut e, t, id) = engine_with_handles(1000, 10_000);
        e.execute("clip.loop", &json!({"clip": id.0, "count": 3, "crossfade": 5000})).unwrap();
        assert_eq!(fades(&e, t), vec![(0, 500), (500, 500), (500, 0)]);
        let (mut e, t, id) = engine_with_handles(1000, 10_000);
        for bad in [json!({"crossfade": -1}), json!({"crossfade_shape": "wobbly"}), json!({"crossfade_shape": 3})] {
            let mut p = json!({"clip": id.0, "count": 3});
            p.as_object_mut().unwrap().extend(bad.as_object().unwrap().clone());
            assert!(e.execute("clip.loop", &p).is_err(), "{p}");
            assert_eq!(spans(&e, t), vec![(0, 1000)]);
        }
        // No crossfade is the old butt-spliced loop.
        e.execute("clip.loop", &json!({"clip": id.0, "count": 3, "crossfade": 0})).unwrap();
        assert_eq!(spans(&e, t), vec![(0, 1000), (1000, 1000), (2000, 1000)]);
    }

    #[test]
    fn a_crossfaded_split_inside_a_loop_is_not_a_loop_crossfade() {
        // Splitting the loop origin and crossfading the halves keeps both halves through Unloop.
        let (mut e, t, id) = engine_with_handles(1000, 10_000);
        e.execute("clip.loop", &json!({"clip": id.0, "count": 2})).unwrap();
        crate::edit::separate_at(e.session_mut(), t, 500);
        crate::edit::create_fades(e.session_mut(), t, soundcraft_time::Range::new(450, 550), soundcraft_model::FadeShape::Linear);
        assert_eq!(spans(&e, t), vec![(0, 550), (450, 550), (1000, 1000)]);
        let second_half = e.session().track(t).unwrap().clips()[1].id;
        let _ = e.execute("clip.unloop", &json!({"clip": id.0}));
        assert!(e.session().track(t).unwrap().clips().iter().any(|c| c.id == second_half));
    }

    #[test]
    fn loop_fill_stops_at_the_next_clip() {
        let (mut e, t, ids) = engine_with(&[(0, 100, 0), (430, 100, 0)]);
        assert_eq!(e.execute("clip.loop", &json!({"clip": ids[0].0, "fill": true})).unwrap()["loops"], 4);
        assert_eq!(spans(&e, t), vec![(0, 100), (100, 100), (200, 100), (300, 100), (400, 30), (430, 100)]);
    }

    #[test]
    fn loop_fill_without_a_next_clip_runs_to_the_session_end() {
        let (mut e, t, ids) = engine_with(&[(0, 100, 0)]);
        let other = e.session_mut().add_track(TrackKind::Audio, ChannelFormat::Mono, None);
        let far = e.session_mut().new_clip_id();
        crate::edit::place_clip(e.session_mut(), other, Clip::audio(far, "far", SourceId(9), 0, 200, 150));
        assert_eq!(e.execute("clip.loop", &json!({"clip": ids[0].0, "fill": true})).unwrap()["loops"], 3);
        assert_eq!(spans(&e, t), vec![(0, 100), (100, 100), (200, 100), (300, 50)]);
        // The last clip in the session has nothing to fill up to.
        assert_eq!(e.execute("clip.loop", &json!({"clip": far.0, "fill": true})).unwrap()["loops"], 0);
    }

    #[test]
    fn a_fill_or_length_past_the_loop_limit_is_refused_without_an_edit() {
        let (mut e, t, ids) = engine_with(&[(0, 100, 0), (100_150, 100, 0)]);
        let err = e.execute("clip.loop", &json!({"clip": ids[0].0, "fill": true})).unwrap_err().to_string();
        assert!(err.contains("1001 loops"), "{err}");
        assert!(e.execute("clip.loop", &json!({"clip": ids[0].0, "length": 100_200})).is_err());
        assert_eq!(spans(&e, t), vec![(0, 100), (100_150, 100)]);
        // 1000 loops still fit.
        assert_eq!(e.execute("clip.loop", &json!({"clip": ids[0].0, "length": 100_100})).unwrap()["loops"], 1000);
    }

    #[test]
    fn a_count_reaches_the_same_loop_limit_as_length_and_fill() {
        // Number of Loops used to stop at 512 (511 loops) while length and fill made 1000.
        let (mut e, t, ids) = engine_with(&[(0, 100, 0)]);
        let err = e.execute("clip.loop", &json!({"clip": ids[0].0, "count": 1002})).unwrap_err().to_string();
        assert!(err.contains("at most 1000"), "{err}");
        assert_eq!(spans(&e, t), vec![(0, 100)]);
        assert_eq!(e.execute("clip.loop", &json!({"clip": ids[0].0, "count": 1001})).unwrap()["loops"], 1000);
        assert_eq!(spans(&e, t).len(), 1001);
        assert_eq!(spans(&e, t).last(), Some(&(100_000, 100)));
        // Hostile counts: zero, negative and huge do not edit past the limit or panic.
        let (mut e, t, ids) = engine_with(&[(0, 100, 0)]);
        assert_eq!(e.execute("clip.loop", &json!({"clip": ids[0].0, "count": -5})).unwrap()["loops"], 0);
        assert!(e.execute("clip.loop", &json!({"clip": ids[0].0, "count": i64::MAX})).is_err());
        assert!(e.execute("clip.loop", &json!({"clip": ids[0].0, "count": 1e300})).is_err());
        assert_eq!(spans(&e, t), vec![(0, 100)]);
    }

    #[test]
    fn regroup_reuses_group() {
        let (mut e, _, ids) = engine_with(&[(0, 100, 0), (500, 100, 0), (900, 100, 0)]);
        let all: Vec<u64> = ids.iter().map(|c| c.0).collect();
        e.execute("clip.group", &json!({"clips": all})).unwrap();
        let g = e.session().find_clip(ids[0]).unwrap().1.group;
        e.execute("clip.ungroup", &json!({"clips": [ids[2].0]})).unwrap();
        e.execute("clip.regroup", &json!({"clips": all})).unwrap();
        assert!(ids.iter().all(|id| e.session().find_clip(*id).unwrap().1.group == g));
    }

    #[test]
    fn effects_bypass_and_clear() {
        let (mut e, _, ids) = engine_with(&[(0, 100, 0)]);
        e.execute("clip.effects_set", &json!({"clip": ids[0].0, "params": {"eq_gain_db": 3.0}})).unwrap();
        e.execute("clip.effects_bypass", &json!({"clip": ids[0].0})).unwrap();
        assert!(e.session().edit.flag(&format!("clip_fx.bypass.{}", ids[0].0)));
        e.execute("edit.copy_clip_effects", &json!({"clip": ids[0].0})).unwrap();
        assert_eq!(e.clipboard.clip_effects, vec![("eq_gain_db".to_string(), 3.0)]);
        e.execute("edit.clear_clip_effects", &json!({"clip": ids[0].0})).unwrap();
        assert!(e.session().edit.values.is_empty());
        e.execute("edit.paste_clip_effects", &json!({"clip": ids[0].0})).unwrap();
        assert_eq!(e.session().edit.values.len(), 1);
    }

    #[test]
    fn conform_follows_tempo() {
        let (mut e, _, ids) = engine_with(&[(0, 48_000, 0)]);
        e.execute("clip.conform_to_tempo", &json!({"clip": ids[0].0})).unwrap();
        assert_eq!(e.session().find_clip(ids[0]).unwrap().1.length, 48_000);
        e.execute("event.tempo", &json!({"bpm": 60.0})).unwrap();
        e.execute("clip.conform_to_tempo", &json!({"clip": ids[0].0})).unwrap();
        let c = e.session().find_clip(ids[0]).unwrap().1.clone();
        assert_eq!(c.length, 96_000);
        assert!((c.stretch - 2.0).abs() < 1e-9);
    }

    #[test]
    fn remove_pitch_shift_renders() {
        let mut s = Session::default();
        let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, None);
        let tone: Vec<f32> = (0..12_000).map(|i| (i as f32 * 0.07).sin() * 0.5).collect();
        let buf = soundcraft_audio_io::AudioBuffer { sample_rate: 48_000, channels: vec![tone] };
        let src = crate::io::add_source(&mut s, "tone", buf, None, soundcraft_audio_io::FileFormat::Wav);
        let id = s.new_clip_id();
        let mut c = Clip::audio(id, "tone", src, 0, 0, 18_000);
        c.stretch = 1.5;
        crate::edit::place_clip(&mut s, t, c);
        s.edit.selected_clips = vec![id];
        s.edit.selected_tracks = vec![t];
        let mut e = Engine::new(s);
        let r = e.execute("clip.remove_pitch_shift", &json!({})).unwrap();
        assert_eq!(r["clips"], 1);
        let c = e.session().find_clip(id).unwrap().1.clone();
        assert_eq!(c.stretch, 1.0);
        assert_eq!(c.length, 18_000);
        assert_ne!(c.source(), Some(src));
    }

    fn peak_from(ch: &[f32], start: usize) -> f32 {
        ch.get(start..).unwrap_or(&[]).iter().fold(0.0f32, |m, x| m.max(x.abs()))
    }

    fn max_abs_diff(a: &[Vec<f32>], b: &[Vec<f32>]) -> f32 {
        a.iter().zip(b.iter()).flat_map(|(x, y)| x.iter().zip(y.iter()).map(|(p, q)| (p - q).abs())).fold(0.0f32, f32::max)
    }

    /// Gain and effects renders bake the current varispeed. The clip must not keep stretching the result.
    #[test]
    fn render_preserves_stretched_audio() {
        for command in ["clip.gain_render", "clip.effects_render"] {
            for ratio in [0.5_f64, 1.0, 2.0] {
                let mut s = Session::default();
                let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, None);
                let tone: Vec<f32> = (0..9_600).map(|i| (i as f32 * 0.05).sin() * 0.4).collect();
                let buf = soundcraft_audio_io::AudioBuffer { sample_rate: 48_000, channels: vec![tone] };
                let src = crate::io::add_source(&mut s, "tone", buf, None, soundcraft_audio_io::FileFormat::Wav);
                let id = s.new_clip_id();
                crate::edit::place_clip(&mut s, t, Clip::audio(id, "tone", src, 0, 0, 9_600));
                s.edit.selected_clips = vec![id];
                let mut e = Engine::new(s);
                e.execute("clip.gain", &json!({"clip": id.0, "db": -6.0})).unwrap();
                e.execute("clip.conform_to_tempo", &json!({"clip": id.0, "source_bpm": 120.0 * ratio})).unwrap();
                if command == "clip.effects_render" {
                    e.execute("clip.effects_set", &json!({"clip": id.0, "params": {"gain": 3.0}})).unwrap();
                }
                let range = e.session().find_clip(id).unwrap().1.range();
                let before = soundcraft_mix::render_clips(e.session(), t, range);
                e.execute(command, &json!({"clip": id.0})).unwrap();
                let c = e.session().find_clip(id).unwrap().1.clone();
                assert!((c.stretch - 1.0).abs() < 1e-9, "{command} ratio {ratio} left stretch {}", c.stretch);
                assert_eq!(c.gain_db, 0.0);
                let after = soundcraft_mix::render_clips(e.session(), t, range);
                let diff = max_abs_diff(&before, &after);
                assert!(diff < 1e-5, "{command} ratio {ratio} changed the audio by {diff}");
                if ratio < 1.0 {
                    let tail = before.first().map(|ch| peak_from(ch, ch.len() / 2)).unwrap_or(0.0);
                    assert!(tail > 0.02, "{command} conformed tail was silent before render: {tail}");
                }
            }
        }

        // Varispeed on a non-elastic track does not change the clip length. Render still matches it.
        let mut s = Session::default();
        let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, None);
        let tone: Vec<f32> = (0..9_600).map(|i| (i as f32 * 0.05).sin() * 0.4).collect();
        let buf = soundcraft_audio_io::AudioBuffer { sample_rate: 48_000, channels: vec![tone] };
        let src = crate::io::add_source(&mut s, "tone", buf, None, soundcraft_audio_io::FileFormat::Wav);
        let id = s.new_clip_id();
        crate::edit::place_clip(&mut s, t, Clip::audio(id, "tone", src, 0, 0, 9_600));
        s.edit.selected_clips = vec![id];
        let mut e = Engine::new(s);
        e.execute("clip.elastic_properties", &json!({"clip": id.0, "ratio": 0.5})).unwrap();
        e.execute("clip.gain", &json!({"clip": id.0, "db": -6.0})).unwrap();
        let range = e.session().find_clip(id).unwrap().1.range();
        let before = soundcraft_mix::render_clips(e.session(), t, range);
        e.execute("clip.gain_render", &json!({"clip": id.0})).unwrap();
        let c = e.session().find_clip(id).unwrap().1.clone();
        assert_eq!(c.length, 9_600);
        assert!((c.stretch - 1.0).abs() < 1e-9);
        let after = soundcraft_mix::render_clips(e.session(), t, range);
        assert!(max_abs_diff(&before, &after) < 1e-5);
    }
}
