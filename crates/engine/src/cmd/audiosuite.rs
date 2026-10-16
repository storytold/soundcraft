//! AudioSuite-style offline processing of selected clips.

use super::*;
use crate::cmd;
use serde_json::json;
use soundcraft_audio_io::{AudioBuffer, FileFormat};
use soundcraft_model::{Clip, ClipContent, TrackKind};

pub fn specs() -> Vec<CommandSpec> {
    vec![cmd!(
        "audiosuite.process",
        "AudioSuite Process",
        [],
        None,
        "{process: <plugin id> | normalize | reverse | invert | gain | duplicate | time_stretch | pitch_shift | varispeed | signal_generator, params?: {..}, clips?, tracks?, start?, end?} — signal_generator fills start..end (default: the selection) with a new clip on each audio track in `tracks` (default: the selected tracks)",
        has_selection,
        process
    )]
}

/// Process each target clip's audio and replace the clip with one backed by a new source.
fn process(e: &mut Engine, p: &Value) -> Result<Value> {
    let name = str_param(p, "process").or_else(|| str_param(p, "plugin")).ok_or_else(|| bad("audiosuite.process", "`process` required"))?.to_string();
    let params: Vec<(String, f32)> = p
        .get("params")
        .and_then(Value::as_object)
        .map(|o| o.iter().filter_map(|(k, v)| v.as_f64().filter(|x| x.is_finite()).map(|x| (k.clone(), x as f32))).collect())
        .unwrap_or_default();
    let get = |k: &str, d: f32| params.iter().find(|(n, _)| n == k).map_or(d, |(_, v)| *v);
    let known = soundcraft_dsp::plugin_info(&name).is_some()
        || matches!(
            name.as_str(),
            "normalize" | "reverse" | "invert" | "gain" | "duplicate" | "time_stretch" | "pitch_shift" | "varispeed" | "dc_offset_removal"
        );
    if !known {
        return Err(bad("audiosuite.process", format!("unknown process `{name}`")));
    }
    if soundcraft_dsp::is_generator(&name) && p.get("clips").is_none() && p.get("clip").is_none() {
        let r = range_param(e, "audiosuite.process", p)?;
        if !r.is_empty() {
            return generate(e, p, &name, &params, r);
        }
    }
    let ids = clip_ids_param(e, p);
    if ids.is_empty() {
        return Err(bad("audiosuite.process", "select audio clips first"));
    }
    let sr = e.session().sample_rate;
    let ratio = f64::from(get("ratio", 1.0).clamp(0.25, 4.0));
    let semitones = get("semitones", 0.0).clamp(-24.0, 24.0);
    let to = (sr.as_f64() / f64::from(get("speed", 1.0).clamp(0.25, 4.0))).round().max(1.0) as u32;
    // These processes cut their output at MAX_OFFLINE_SAMPLES frames.
    let longest = |grows: f64| (soundcraft_dsp::offline::MAX_OFFLINE_SAMPLES as f64 / grows) as usize;
    let most = match name.as_str() {
        "time_stretch" => longest(ratio),
        "pitch_shift" => longest(2f64.powf(f64::from(semitones) / 12.0)),
        "varispeed" => longest(f64::from(to) / sr.as_f64()),
        _ => usize::MAX,
    };
    let mut done = 0;
    for id in ids {
        let Some((track, clip)) = e.session().find_clip(id).map(|(t, c)| (t, c.clone())) else { continue };
        if !clip.is_audio() {
            continue;
        }
        let channels = e.session().track(track).map_or(1, |t| t.channels());
        crate::io::refuse_longer(e.session(), "audiosuite.process", &clip.name, "process", clip.length, most.min(crate::io::render_limit(channels)))?;
        let mut audio = soundcraft_mix::render_clips(e.session(), track, clip.range());
        // Render without the clip's own fades: re-render a copy without fades so they stay editable.
        let mut tmp = e.session().clone();
        if let Some(c) = tmp.find_clip_mut(id) {
            c.fade_in = Default::default();
            c.fade_out = Default::default();
            c.gain_db = 0.0;
            c.gain_env.clear();
        }
        if let Some(pl) = tmp.track_mut(track).and_then(|t| t.playlist_mut()) {
            pl.clips.retain(|c| c.id == id);
            audio = soundcraft_mix::render_clips(&tmp, track, clip.range());
        }
        let srf = sr.as_f64() as f32;
        let out: Vec<Vec<f32>> = match name.as_str() {
            "normalize" => {
                soundcraft_dsp::offline::normalize(&mut audio, get("target_db", -0.1), false);
                audio
            }
            "reverse" => {
                soundcraft_dsp::offline::reverse(&mut audio);
                audio
            }
            "invert" => {
                soundcraft_dsp::offline::invert(&mut audio);
                audio
            }
            "gain" if soundcraft_dsp::plugin_info("gain").is_none() || params.iter().any(|(k, _)| k == "db") => {
                soundcraft_dsp::offline::gain(&mut audio, get("db", 0.0));
                audio
            }
            "duplicate" => audio,
            "time_stretch" => soundcraft_dsp::offline::time_stretch(&audio, ratio, srf),
            "pitch_shift" => soundcraft_dsp::offline::pitch_shift(&audio, semitones, srf),
            "varispeed" => soundcraft_dsp::offline::resample(&audio, sr.hz(), to),
            plugin => {
                apply(plugin, &params, &mut audio, srf)?;
                audio
            }
        };
        let frames = out.first().map_or(0, Vec::len);
        if frames == 0 {
            continue;
        }
        let buf = AudioBuffer { sample_rate: sr.hz(), channels: out };
        let new_name = format!("{}-{}", clip.name, short(&name));
        let s = e.session_mut();
        let src = crate::io::add_source(s, &new_name, buf, None, FileFormat::Wav);
        let mut nc: Clip = clip.clone();
        nc.name = new_name;
        nc.content = ClipContent::Audio { source: src, offset: 0 };
        nc.length = i64::try_from(frames).unwrap_or(nc.length);
        // The render already has the clip's stretch applied; keeping it would stretch the new file again.
        nc.stretch = 1.0;
        // Invert and Duplicate are phase/copy operations. The clip's gain stays a clip gain,
        // the same way its fades stay editable. Other processes bake a new file at unity gain.
        if !matches!(name.as_str(), "invert" | "duplicate") {
            nc.gain_db = 0.0;
            nc.gain_env.clear();
        }
        if let Some(pl) = s.track_mut(track).and_then(|t| t.playlist_mut()) {
            pl.clips.retain(|c| c.id != id);
        }
        crate::edit::place_clip(s, track, nc);
        done += 1;
    }
    Ok(json!({"processed": done, "process": name}))
}

/// Fill `r` on each selected audio track with a new clip from the generator `name`, over
/// whatever was there, like Pro Tools' Signal Generator.
fn generate(e: &mut Engine, p: &Value, name: &str, params: &[(String, f32)], r: Range) -> Result<Value> {
    let sr = e.session().sample_rate;
    let tracks = tracks_param(e, "audiosuite.process", p)?;
    // Refuse before generating anything, rather than cut the clip short.
    for t in tracks.iter().filter_map(|t| e.session().track(*t)).filter(|t| t.kind == TrackKind::Audio) {
        crate::io::refuse_longer(e.session(), "audiosuite.process", &t.name, "generate on", r.len(), crate::io::render_limit(t.channels()))?;
    }
    let mut clips = Vec::new();
    for track in tracks {
        let Some(t) = e.session().track(track).filter(|t| t.kind == TrackKind::Audio) else { continue };
        let channels = t.channels().max(1);
        let frames = usize::try_from(r.len()).unwrap_or(0).min(crate::io::render_limit(channels));
        let clip_name = format!("{}-{}", t.name, short(name));
        let mut audio = vec![vec![0.0; frames]; channels];
        apply(name, params, &mut audio, sr.as_f64() as f32)?;
        let s = e.session_mut();
        let src = crate::io::add_source(s, &clip_name, AudioBuffer { sample_rate: sr.hz(), channels: audio }, None, FileFormat::Wav);
        let id = s.new_clip_id();
        crate::edit::place_clip(s, track, Clip::audio(id, &clip_name, src, 0, r.start, i64::try_from(frames).unwrap_or(0)));
        clips.push(id.0);
    }
    if clips.is_empty() {
        return Err(bad("audiosuite.process", "select an audio track to generate on"));
    }
    Ok(json!({"processed": clips.len(), "process": name, "clips": clips}))
}

/// Run plugin `id` with `params` over `audio`, in place.
fn apply(id: &str, params: &[(String, f32)], audio: &mut [Vec<f32>], srf: f32) -> Result<()> {
    let Some(mut pl) = soundcraft_dsp::create(id) else { return Err(bad("audiosuite.process", format!("cannot create `{id}`"))) };
    pl.prepare(srf, 1024, audio.len().max(1));
    for (k, v) in params {
        pl.set_param(k, *v);
    }
    soundcraft_dsp::offline::apply_plugin(audio, pl.as_mut(), srf);
    Ok(())
}

fn short(n: &str) -> String {
    let s: String = n.split('_').map(|w| w.chars().take(4).collect::<String>()).collect::<Vec<_>>().join("");
    s.chars().take(8).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use soundcraft_audio_io::AudioBuffer;
    use soundcraft_model::{ChannelFormat, Session, TrackKind};

    fn gained_clip() -> (crate::Engine, soundcraft_model::ClipId) {
        let mut s = Session::default();
        let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, Some("Tone"));
        let buf = AudioBuffer { sample_rate: 48_000, channels: vec![vec![0.5; 64]] };
        let src = crate::io::add_source(&mut s, "tone", buf, None, FileFormat::Wav);
        let id = s.new_clip_id();
        let mut c = Clip::audio(id, "tone", src, 0, 0, 64);
        c.gain_db = -6.0;
        c.gain_env = vec![(0, 0.0), (32, -12.0)];
        crate::edit::place_clip(&mut s, t, c);
        s.edit.selected_clips = vec![id];
        (crate::Engine::new(s), id)
    }

    #[test]
    fn invert_and_duplicate_keep_clip_gain() {
        let (mut e, id) = gained_clip();
        e.execute("audiosuite.process", &json!({"process": "invert", "clips": [id.0]})).unwrap();
        let c = e.session().tracks[0].clips()[0].clone();
        assert!((c.gain_db + 6.0).abs() < 1e-6, "{}", c.gain_db);
        assert_eq!(c.gain_env, vec![(0, 0.0), (32, -12.0)]);
        let sample = e.session().pool.get(c.source().unwrap()).unwrap().buffer.channels[0][0];
        assert!((sample + 0.5).abs() < 1e-3, "invert flips the file and leaves the gain on the clip, got {sample}");

        let (mut e, id) = gained_clip();
        e.execute("audiosuite.process", &json!({"process": "duplicate", "clips": [id.0]})).unwrap();
        let c = e.session().tracks[0].clips()[0].clone();
        assert!((c.gain_db + 6.0).abs() < 1e-6, "{}", c.gain_db);
        assert_eq!(c.gain_env, vec![(0, 0.0), (32, -12.0)]);
        let sample = e.session().pool.get(c.source().unwrap()).unwrap().buffer.channels[0][0];
        assert!((sample - 0.5).abs() < 1e-3, "duplicate keeps the file level and the clip gain, got {sample}");
    }

    #[test]
    fn processed_clip_does_not_keep_the_stretch_it_baked() {
        for stretch in [0.5, 2.0] {
            for process in ["invert", "duplicate"] {
                let (mut e, id) = gained_clip();
                e.session_mut().find_clip_mut(id).unwrap().stretch = stretch;
                e.execute("audiosuite.process", &json!({"process": process, "clips": [id.0]})).unwrap();
                let c = e.session().tracks[0].clips()[0].clone();
                let frames = e.session().pool.get(c.source().unwrap()).unwrap().buffer.channels[0].len();
                assert_eq!(c.stretch, 1.0, "{process} at {stretch}");
                assert_eq!(c.length, i64::try_from(frames).unwrap(), "{process} at {stretch}");
            }
        }
    }

    #[test]
    fn signal_generator_fills_the_selection_with_a_clip() {
        let (mut e, _) = gained_clip();
        let tone = e.session().tracks[0].id;
        let t = e.session_mut().add_track(TrackKind::Audio, ChannelFormat::Stereo, Some("Empty"));
        e.execute("edit.select", &json!({"tracks": [t.0], "start": 48_000, "end": 96_000})).unwrap();
        assert!(matches!(e.execute("audiosuite.process", &json!({"process": "normalize"})), Err(crate::EngineError::BadParams(..))));

        let generator = json!({"process": "signal_generator", "params": {"freq": 440.0, "level": -6.0}});
        assert_eq!(e.execute("audiosuite.process", &generator).unwrap()["processed"], 1);
        let c = e.session().tracks[1].clips()[0].clone();
        assert_eq!((c.start, c.length), (48_000, 48_000));
        let buf = &e.session().pool.get(c.source().unwrap()).unwrap().buffer;
        assert_eq!(buf.channels.len(), 2);
        let peak = buf.channels[1].iter().fold(0.0f32, |m, x| m.max(x.abs()));
        assert!((peak - 0.5).abs() < 0.01, "-6 dB sine peaks near 0.5, got {peak}");

        // Over existing audio it replaces just the selection.
        e.execute("edit.select", &json!({"tracks": [tone.0], "start": 32, "end": 96})).unwrap();
        e.execute("audiosuite.process", &generator).unwrap();
        let ranges: Vec<_> = e.session().tracks[0].clips().iter().map(|c| (c.start, c.length)).collect();
        assert_eq!(ranges, vec![(0, 32), (32, 64)]);
    }
}
