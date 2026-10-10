//! More Clip menu commands: Regroup, Unloop, Clip Effects bypass/render, Conform to Tempo and
//! Remove Pitch Shift.

use super::more_util::{render_stretched, replace_clip};
use super::*;
use crate::cmd;
use serde_json::json;
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

fn same_material(a: &Clip, b: &Clip) -> bool {
    a.length == b.length
        && match (&a.content, &b.content) {
            (ClipContent::Audio { source: s1, offset: o1 }, ClipContent::Audio { source: s2, offset: o2 }) => s1 == s2 && o1 == o2,
            (ClipContent::Midi { sequence: q1 }, ClipContent::Midi { sequence: q2 }) => q1 == q2,
            (ClipContent::Video { source: s1, offset: o1 }, ClipContent::Video { source: s2, offset: o2 }) => s1 == s2 && o1 == o2,
            _ => false,
        }
}

/// The loop a clip belongs to: back-to-back copies of the same material. Origin first.
fn loop_chain(s: &Session, t: TrackId, id: ClipId) -> Vec<ClipId> {
    let Some(tr) = s.track(t) else { return Vec::new() };
    let clips = tr.clips();
    let Some(mut cur) = clips.iter().find(|c| c.id == id) else { return Vec::new() };
    let mut guard = 0;
    while let Some(prev) = clips.iter().find(|x| x.id != cur.id && x.end() == cur.start && same_material(x, cur)) {
        cur = prev;
        guard += 1;
        if guard > clips.len() {
            break;
        }
    }
    let mut chain = vec![cur.id];
    while let Some(next) = clips.iter().find(|x| !chain.contains(&x.id) && x.start == cur.end() && same_material(x, cur)) {
        chain.push(next.id);
        cur = next;
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
                pl.clips.retain(|c| !copies.contains(&c.id));
                gone.extend(copies.iter().copied());
            } else {
                for c in pl.clips.iter_mut().filter(|c| chain.contains(&c.id)) {
                    c.group = None;
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
    let vals: Vec<(String, f64)> =
        params.iter().filter_map(|(k, v)| v.as_f64().filter(|x| x.is_finite()).map(|x| (k.chars().take(64).collect(), x))).collect();
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
        if crate::io::render_clip_gain(e, *id)? {
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
    fn unloop_removes_copies() {
        let (mut e, t, ids) = engine_with(&[(0, 100, 0), (100, 100, 0), (200, 100, 0), (300, 100, 50)]);
        e.execute("clip.unloop", &json!({"clip": ids[1].0})).unwrap();
        let left: Vec<ClipId> = e.session().track(t).unwrap().clips().iter().map(|c| c.id).collect();
        assert_eq!(left, vec![ids[0], ids[3]]);
        assert!(e.execute("clip.unloop", &json!({"clip": ids[3].0})).is_err());
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
