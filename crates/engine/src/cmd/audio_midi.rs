//! Edit › Copy Special › Audio as MIDI: pitch-track the selected audio into MIDI notes on the
//! clipboard (paste them onto a MIDI or instrument track).
use super::{CommandSpec, bad, has_range, range_param, tracks_required};

use crate::{Clipboard, Engine, Result, cmd};
use serde_json::{Value, json};
use soundcraft_midi::{Note, Sequence};
use soundcraft_model::{Clip, ClipId};

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(noundo "edit.copy_audio_as_midi", "Audio as MIDI", ["Edit", "Copy Special"], None, "{track?, start?, end?} — monophonic pitch tracking of the selected audio", has_range, audio_as_midi),
    ]
}

fn audio_as_midi(e: &mut Engine, p: &Value) -> Result<Value> {
    let id = "edit.copy_audio_as_midi";
    let t = tracks_required(e, id, p)?.first().copied().ok_or_else(|| bad(id, "no track"))?;
    let r = range_param(e, id, p)?;
    if r.is_empty() {
        return Err(bad(id, "select some audio first"));
    }
    let s = e.session();
    if s.track(t).is_none_or(|x| x.kind != soundcraft_model::TrackKind::Audio) {
        return Err(bad(id, "select audio on an audio track"));
    }
    let r = crate::io::bounded_to_track(s, t, r);
    let audio = soundcraft_mix::render_clips(s, t, r);
    let n = audio.first().map_or(0, Vec::len);
    let k = audio.len().max(1) as f32;
    let mono: Vec<f32> = (0..n).map(|i| audio.iter().map(|c| c.get(i).copied().unwrap_or(0.0)).sum::<f32>() / k).collect();
    let detected = soundcraft_dsp::pitch_detect::audio_to_notes(&mono, s.sample_rate.as_f64() as f32);
    let base = s.tempo.samples_to_ticks(r.start, s.sample_rate);
    let mut seq = Sequence::default();
    for d in &detected {
        let a = s.tempo.samples_to_ticks(r.start + d.start as i64, s.sample_rate) - base;
        let b = s.tempo.samples_to_ticks(r.start + d.end as i64, s.sample_rate) - base;
        seq.notes.push(Note { pitch: d.pitch, velocity: d.velocity, release_velocity: 64, channel: 0, start: a.max(0), length: (b - a).max(1) });
    }
    seq.sort();
    let count = seq.notes.len();
    let clip = Clip::midi(ClipId(0), "Audio as MIDI", 0, r.len(), seq);
    e.clipboard = Clipboard { tracks: vec![vec![clip]], automation: vec![Vec::new()], ..Clipboard::of_range(e.session(), r) };
    Ok(json!({"notes": count, "length": r.len()}))
}

#[cfg(test)]
mod tests {
    use super::*;
    use soundcraft_audio_io::AudioBuffer;
    use soundcraft_model::{ChannelFormat, TrackKind};

    #[test]
    fn copies_a_melody_as_midi_and_pastes_it() {
        let mut e = Engine::default();
        let sr = 48_000.0f32;
        let tone = |f: f32| -> Vec<f32> { (0..(0.4 * sr) as usize).map(|i| (i as f32 * f * std::f32::consts::TAU / sr).sin() * 0.5).collect() };
        let mut a = tone(220.0);
        a.extend(tone(330.0));
        let frames = a.len() as i64;
        let s = e.session_mut();
        let src = crate::io::add_source(s, "mel", AudioBuffer { sample_rate: 48_000, channels: vec![a] }, None, soundcraft_audio_io::FileFormat::Wav);
        let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, None);
        let cid = s.new_clip_id();
        crate::edit::place_clip(s, t, Clip::audio(cid, "mel", src, 0, 0, frames));
        let r = e.execute("edit.copy_audio_as_midi", &json!({"track": t.0, "start": 0, "end": frames})).unwrap();
        assert_eq!(r["notes"], 2, "{r}");
        let m = e.session_mut().add_track(TrackKind::Midi, ChannelFormat::Mono, None);
        e.execute("edit.paste", &json!({"track": m.0, "at": 0})).unwrap();
        let clip = e.session().track(m).unwrap().clips()[0].clone();
        match clip.content {
            soundcraft_model::ClipContent::Midi { sequence } => assert_eq!(sequence.notes.iter().map(|n| n.pitch).collect::<Vec<_>>(), vec![57, 64]),
            _ => panic!("expected MIDI"),
        }
    }
}
