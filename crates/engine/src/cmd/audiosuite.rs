//! AudioSuite-style offline processing of selected clips.

use super::*;
use crate::cmd;
use serde_json::json;
use soundcraft_audio_io::{AudioBuffer, FileFormat};
use soundcraft_model::{Clip, ClipContent};

pub fn specs() -> Vec<CommandSpec> {
    vec![cmd!(
        "audiosuite.process",
        "AudioSuite Process",
        [],
        None,
        "{process: <plugin id> | normalize | reverse | invert | gain | duplicate | time_stretch | pitch_shift | varispeed | signal_generator, params?: {..}, clips?}",
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
    let ids = clip_ids_param(e, p);
    if ids.is_empty() {
        return Err(bad("audiosuite.process", "select audio clips first"));
    }
    let sr = e.session().sample_rate;
    let mut done = 0;
    for id in ids {
        let Some((track, clip)) = e.session().find_clip(id).map(|(t, c)| (t, c.clone())) else { continue };
        if !clip.is_audio() {
            continue;
        }
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
            "time_stretch" => soundcraft_dsp::offline::time_stretch(&audio, f64::from(get("ratio", 1.0).clamp(0.25, 4.0)), srf),
            "pitch_shift" => soundcraft_dsp::offline::pitch_shift(&audio, get("semitones", 0.0).clamp(-24.0, 24.0), srf),
            "varispeed" => {
                let speed = get("speed", 1.0).clamp(0.25, 4.0);
                let to = (sr.as_f64() / f64::from(speed)).round().max(1.0) as u32;
                soundcraft_dsp::offline::resample(&audio, sr.hz(), to)
            }
            plugin => {
                let Some(mut pl) = soundcraft_dsp::create(plugin) else { return Err(bad("audiosuite.process", format!("cannot create `{plugin}`"))) };
                pl.prepare(srf, 1024, audio.len().max(1));
                for (k, v) in &params {
                    pl.set_param(k, *v);
                }
                soundcraft_dsp::offline::apply_plugin(&mut audio, pl.as_mut(), srf);
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
}
