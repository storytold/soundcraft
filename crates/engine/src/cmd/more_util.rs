//! Helpers shared by the newer command modules: settings kept in `EditState.flags`/`values`,
//! per-track settings, target playlists, clip-gain envelopes and stretch rendering.
use super::bad;
use crate::{Engine, Result};

use serde_json::{Value, json};
use soundcraft_audio_io::{AudioBuffer, FileFormat};
use soundcraft_model::{Clip, ClipContent, EditState, Session, SourceId, TrackId};
use soundcraft_time::Samples;

/// Toggle (or set from `value`) a flag in `edit.flags`; returns the new state.
pub fn toggle_flag(e: &mut Engine, p: &Value, id: &str) -> Result<Value> {
    let s = e.session_mut();
    let on = p.get("value").and_then(Value::as_bool).unwrap_or(!s.edit.flag(id));
    s.edit.set_flag(id, on);
    Ok(json!({"flag": id, "value": on}))
}

/// Turn `id` on and every other flag of `group` off (radio-style menu items).
pub fn exclusive_flag(e: &mut Engine, id: &str, group: &[&str]) -> Result<Value> {
    let s = e.session_mut();
    for g in group {
        s.edit.set_flag(g, false);
    }
    s.edit.set_flag(id, true);
    Ok(json!({"flag": id, "value": true}))
}

/// Set several flags at once.
pub fn set_flags(e: &mut Engine, ids: &[&str], on: bool) -> Value {
    let s = e.session_mut();
    for id in ids {
        s.edit.set_flag(id, on);
    }
    json!({"flags": ids, "value": on})
}

/// String settings live in `edit.flags` as `key=value` (one per key).
pub fn set_text(edit: &mut EditState, key: &str, value: Option<&str>) {
    let prefix = format!("{key}=");
    edit.flags.retain(|f| !f.starts_with(&prefix));
    if let Some(v) = value.map(str::trim).filter(|v| !v.is_empty()) {
        let v: String = v.chars().take(200).collect();
        edit.flags.insert(format!("{prefix}{v}"));
    }
}

pub fn text(edit: &EditState, key: &str) -> Option<String> {
    let prefix = format!("{key}=");
    edit.flags.iter().find_map(|f| f.strip_prefix(&prefix).map(str::to_string))
}

/// All `values` whose key starts with `prefix`, as a JSON object (prefix stripped).
pub fn values_with_prefix(edit: &EditState, prefix: &str) -> Value {
    let m: serde_json::Map<String, Value> =
        edit.values.iter().filter_map(|(k, v)| k.strip_prefix(prefix).map(|rest| (rest.to_string(), json!(v)))).collect();
    Value::Object(m)
}

/// Store a finite number from `p[key]` into `values[target]`, clamped. Returns whether it was given.
pub fn store_value(edit: &mut EditState, p: &Value, key: &str, target: &str, lo: f64, hi: f64) -> bool {
    match p.get(key).and_then(Value::as_f64).filter(|v| v.is_finite()) {
        Some(v) => {
            edit.values.insert(target.to_string(), v.clamp(lo, hi));
            true
        }
        None => false,
    }
}

/// Key for a per-track value, e.g. `rtp.12.velocity`.
pub fn track_key(prefix: &str, t: TrackId, name: &str) -> String {
    format!("{prefix}.{}.{name}", t.0)
}

/// The track's designated target playlist index, if valid.
pub fn target_playlist(s: &Session, t: TrackId) -> Option<usize> {
    let v = s.edit.values.get(&format!("playlist.target.{}", t.0)).copied()?;
    if !v.is_finite() || v < 0.0 {
        return None;
    }
    let i = v as usize;
    s.track(t).filter(|tr| i < tr.playlists.len()).map(|_| i)
}

/// Make `idx` the active playlist, remembering the previous one for Toggle Recent Playlist.
pub fn activate_playlist(s: &mut Session, t: TrackId, idx: usize) -> bool {
    let Some(tr) = s.track_mut(t) else { return false };
    if idx >= tr.playlists.len() || idx == tr.active_playlist {
        return false;
    }
    let prev = tr.active_playlist;
    tr.active_playlist = idx;
    s.edit.values.insert(format!("playlist.recent.{}", t.0), prev as f64);
    true
}

/// Interpolated value of a `(offset, dB)` envelope (0 when empty), like the model's clip gain.
pub fn env_db(env: &[(Samples, f32)], at: Samples) -> f32 {
    let mut prev: Option<(Samples, f32)> = None;
    for &(t, v) in env {
        if t >= at {
            return match prev {
                Some((pt, pv)) if t > pt => pv + (v - pv) * ((at - pt) as f32 / (t - pt) as f32),
                _ => v,
            };
        }
        prev = Some((t, v));
    }
    prev.map_or(0.0, |(_, v)| v)
}

/// Absolute clip gain (static gain + envelope) in dB at `rel` samples from the clip start.
pub fn clip_db(c: &Clip, rel: Samples) -> f32 {
    c.gain_db + if c.gain_env.is_empty() { 0.0 } else { env_db(&c.gain_env, rel) }
}

/// Replace a clip's gain with an absolute envelope (sorted, deduplicated, clamped).
pub fn set_clip_env(c: &mut Clip, mut pts: Vec<(Samples, f32)>) {
    pts.retain(|(o, v)| *o >= 0 && *o <= c.length && v.is_finite());
    pts.sort_by_key(|p| p.0);
    pts.dedup_by_key(|p| p.0);
    for p in &mut pts {
        p.1 = p.1.clamp(-144.0, 36.0);
    }
    c.gain_db = 0.0;
    let flat = pts.windows(2).all(|w| w.first().map(|a| a.1) == w.get(1).map(|b| b.1));
    if flat {
        c.gain_db = pts.first().map_or(0.0, |p| p.1);
        c.gain_env.clear();
    } else {
        c.gain_env = pts;
    }
}

/// Longest audio we render in one stretch: one hour at the session rate, and no more than
/// `time_stretch` makes.
fn max_render(s: &Session) -> Samples {
    s.sample_rate.samples(3600.0).min(soundcraft_dsp::offline::MAX_OFFLINE_SAMPLES as Samples)
}

/// Render the audio a clip plays, time-stretched (pitch preserved) to `out_len` samples, into a new
/// source. Returns `Ok(None)` for MIDI clips or missing audio.
pub fn render_stretched(s: &mut Session, cmd: &str, c: &Clip, out_len: Samples) -> Result<Option<SourceId>> {
    let ClipContent::Audio { source, offset } = c.content else { return Ok(None) };
    let Some(audio) = s.pool.get(source).cloned() else { return Ok(None) };
    let max = max_render(s);
    if out_len <= 0 || out_len > max {
        let secs = max as f64 / s.sample_rate.as_f64();
        return Err(bad(cmd, format!("the target length must be between 1 sample and {secs:.0} s")));
    }
    let stretch = if c.stretch.is_finite() && c.stretch > 0.0 { c.stretch } else { 1.0 };
    let src_len = ((c.length as f64) / stretch).round().max(1.0);
    let ratio = out_len as f64 / src_len;
    if !(0.1..=10.0).contains(&ratio) {
        return Err(bad(cmd, format!("stretch ratio {ratio:.3} is outside 0.1..10")));
    }
    // The source span read in can be 10× the output, and both are held for every channel: bound
    // each like any other offline render (a session's clip length and stretch are not trusted).
    let channels = audio.buffer.channels.len().max(1);
    let limit = crate::io::render_limit(channels);
    if src_len > limit as f64 || usize::try_from(out_len).map_or(true, |n| n > limit) {
        let secs = limit as f64 / s.sample_rate.as_f64();
        return Err(bad(cmd, format!("the clip is too long to stretch: one render holds at most {secs:.0} s of {channels}-channel audio")));
    }
    let a = usize::try_from(offset.max(0)).unwrap_or(0);
    let n = src_len as usize;
    let ch: Vec<Vec<f32>> = audio
        .buffer
        .channels
        .iter()
        .map(|x| {
            let mut v: Vec<f32> = x.iter().skip(a).take(n).copied().collect();
            v.resize(n, 0.0);
            v
        })
        .collect();
    let sr = s.sample_rate;
    let mut out = soundcraft_dsp::offline::time_stretch(&ch, ratio, sr.as_f64() as f32);
    let want = usize::try_from(out_len).unwrap_or(0);
    for o in &mut out {
        o.resize(want, 0.0);
    }
    let buf = AudioBuffer { sample_rate: sr.hz(), channels: out };
    Ok(Some(crate::io::add_source(s, &format!("{}-TCE", c.name), buf, None, FileFormat::Wav)))
}

/// Replace clip `old` on track `t` with `new` (overwriting whatever `new` covers).
pub fn replace_clip(s: &mut Session, t: TrackId, old: soundcraft_model::ClipId, new: Clip) {
    if let Some(pl) = s.track_mut(t).and_then(|tr| tr.playlist_mut()) {
        pl.clips.retain(|c| c.id != old);
    }
    crate::edit::place_clip(s, t, new);
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn text_settings_round_trip() {
        let mut ed = EditState::default();
        set_text(&mut ed, "engine.device", Some("Built-in"));
        set_text(&mut ed, "engine.device", Some("Interface"));
        assert_eq!(text(&ed, "engine.device").as_deref(), Some("Interface"));
        set_text(&mut ed, "engine.device", None);
        assert_eq!(text(&ed, "engine.device"), None);
    }

    #[test]
    fn envelope_interpolates() {
        let env = [(0, 0.0f32), (100, -10.0)];
        assert!((env_db(&env, 50) + 5.0).abs() < 1e-6);
        assert_eq!(env_db(&[], 50), 0.0);
        assert_eq!(env_db(&env, 500), -10.0);
    }

    #[test]
    fn stretches_are_no_longer_than_time_stretch_makes() {
        let mut s = Session::default();
        assert_eq!(max_render(&s), s.sample_rate.samples(3600.0));
        s.sample_rate = soundcraft_time::SampleRate::new(96_000).unwrap();
        assert_eq!(max_render(&s), soundcraft_dsp::offline::MAX_OFFLINE_SAMPLES as Samples);
    }

    /// A stereo session with a one-frame source and one clip on it.
    fn stretched_clip(length: Samples, stretch: f64) -> (Session, Clip) {
        use soundcraft_model::{ChannelFormat, TrackKind};
        let mut s = Session::default();
        let t = s.add_track(TrackKind::Audio, ChannelFormat::Stereo, Some("Long"));
        let buf = soundcraft_audio_io::AudioBuffer { sample_rate: 48_000, channels: vec![vec![0.5]; 2] };
        let src = crate::io::add_source(&mut s, "One", buf, None, FileFormat::Wav);
        let cid = s.new_clip_id();
        let mut clip = Clip::audio(cid, "Clip", src, 0, 0, length);
        clip.stretch = stretch;
        crate::edit::place_clip(&mut s, t, clip.clone());
        (s, clip)
    }

    /// #895: a session's clip length and stretch made the source span read in 10× the one-hour
    /// output limit, for every channel (about 14 GB here), and the allocation aborted the app.
    #[test]
    fn a_stretch_over_the_render_limit_is_refused_before_allocating() {
        let hour = Session::default().sample_rate.samples(3600.0);
        let (mut s, clip) = stretched_clip(hour, 0.1);
        let err = render_stretched(&mut s, "clip.remove_pitch_shift", &clip, clip.length).unwrap_err();
        assert!(err.to_string().contains("too long to stretch"), "{err}");

        let mut e = crate::Engine::new(s);
        assert!(e.execute("clip.remove_pitch_shift", &serde_json::json!({"clips": [clip.id.0]})).is_err());
        let (_, kept) = e.session().find_clip(clip.id).unwrap();
        assert_eq!((kept.stretch, kept.length), (0.1, hour), "the clip is left as it was");

        // An ordinary stretch still renders, at the length asked for.
        let (mut s, clip) = stretched_clip(1_000, 0.5);
        let src = render_stretched(&mut s, "clip.remove_pitch_shift", &clip, clip.length).unwrap().unwrap();
        assert!(s.pool.get(src).unwrap().buffer.channels.iter().all(|c| c.len() == 1_000));
    }
}
