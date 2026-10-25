//! Surround mixing: the main output format and the surround panners of tracks and sends.
use super::{CommandSpec, always, bad, bool_or, f32_or, has_selection, str_param, tracks_required};

use crate::{Engine, Result, cmd};
use serde_json::{Value, json};
use soundcraft_model::{ChannelFormat, Mixer, OutputPath, Route, SEND_SLOTS, Session, SurroundPan, TrackKind};

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "mix.surround_pan",
            "Set Surround Pan",
            [],
            None,
            "{tracks?, x?: -1..1 (left..right), y?: -1..1 (back..front), z?: 0..1 (height), divergence?: 0..1, center?: 0..100, lfe_db?: -144..12, clear?: bool}",
            has_selection,
            surround_pan
        ),
        cmd!(
            "mix.send_surround_pan",
            "Set Send Surround Pan",
            [],
            None,
            "{track?, slot: 0..9|a..j, x?, y?, z?, divergence?, center?, lfe_db?, clear?: bool}",
            has_selection,
            send_surround_pan
        ),
        cmd!(
            "setup.main_format",
            "Main Output Format",
            [],
            None,
            "{format: stereo|LCR|Quad|5.0|5.1|6.1|7.0|7.1|7.1.2|5.1.4|7.1.4|9.1.6|1st Order Ambisonics…}",
            always,
            main_format
        ),
    ]
}

/// Apply the provided fields of `p` on top of `base`, clamped.
fn apply_fields(base: SurroundPan, p: &Value) -> SurroundPan {
    let mut sp = base;
    sp.x = f32_or(p, "x", sp.x);
    sp.y = f32_or(p, "y", sp.y);
    sp.z = f32_or(p, "z", sp.z);
    sp.divergence = f32_or(p, "divergence", sp.divergence);
    sp.center = f32_or(p, "center", sp.center);
    sp.lfe_db = f32_or(p, "lfe_db", sp.lfe_db);
    sp.sanitize();
    sp
}

/// The surround pan a mixer starts from: its own, else its stereo pan position on the front edge.
fn current(m: &Mixer) -> SurroundPan {
    m.surround.unwrap_or_else(|| SurroundPan::from_stereo(if m.pan.len() == 1 { m.pan.first().copied().unwrap_or(0.0) } else { 0.0 }))
}

fn surround_pan(e: &mut Engine, p: &Value) -> Result<Value> {
    let id = "mix.surround_pan";
    let tracks = tracks_required(e, id, p)?;
    let clear = bool_or(p, "clear", false);
    let s = e.session_mut();
    let mut out = Vec::new();
    for t in tracks {
        let Some(tr) = s.track_mut(t) else { continue };
        if matches!(tr.kind, TrackKind::Master | TrackKind::Vca) {
            continue;
        }
        tr.mixer.surround = if clear { None } else { Some(apply_fields(current(&tr.mixer), p)) };
        out.push(json!({"track": t, "surround": tr.mixer.surround}));
    }
    Ok(json!({"tracks": out}))
}

fn send_surround_pan(e: &mut Engine, p: &Value) -> Result<Value> {
    let id = "mix.send_surround_pan";
    let t = tracks_required(e, id, p)?.first().copied().ok_or_else(|| bad(id, "no track"))?;
    let slot = send_slot(p).ok_or_else(|| bad(id, format!("`slot` must be 0..{} or a..j", SEND_SLOTS - 1)))?;
    let clear = bool_or(p, "clear", false);
    let s = e.session_mut();
    let tr = s.track_mut(t).ok_or_else(|| bad(id, "no track"))?;
    let base = current(&tr.mixer);
    let snd = tr.mixer.sends.get_mut(slot).and_then(Option::as_mut).ok_or_else(|| bad(id, "empty send slot"))?;
    let start = snd.surround.unwrap_or(if snd.follow_main_pan { base } else { SurroundPan::from_stereo(snd.pan) });
    snd.surround = if clear { None } else { Some(apply_fields(start, p)) };
    Ok(json!({"track": t, "slot": slot, "surround": snd.surround}))
}

fn send_slot(p: &Value) -> Option<usize> {
    let v = p.get("slot")?;
    let i = match (v.as_u64(), v.as_str()) {
        (Some(n), _) => usize::try_from(n).ok()?,
        (_, Some(s)) => {
            let s = s.trim().to_ascii_lowercase();
            match s.as_bytes() {
                [c @ b'a'..=b'j'] => usize::from(c - b'a'),
                _ => s.parse().ok()?,
            }
        }
        _ => return None,
    };
    (i < SEND_SLOTS).then_some(i)
}

/// Set the main output path's format (creating it if missing) and resize master faders to match.
pub fn set_main_format(s: &mut Session, fmt: ChannelFormat) {
    let n = fmt.channels();
    let new_name = format!("Out 1-{n}");
    match s.outputs.first_mut() {
        Some(o) => {
            o.format = fmt;
            // Rename the default-style "Out 1-N" path to the new width; custom names stay.
            let default_style = o.name.strip_prefix("Out 1-").is_some_and(|x| x.parse::<usize>().is_ok());
            if default_style && o.name != new_name {
                let old = std::mem::replace(&mut o.name, new_name.clone());
                for t in &mut s.tracks {
                    if t.mixer.output == Route::Hardware(old.clone()) {
                        t.mixer.output = Route::Hardware(new_name.clone());
                    }
                }
            }
        }
        None => s.outputs.push(OutputPath { name: new_name, first_channel: 0, format: fmt }),
    }
    for t in s.tracks.iter_mut().filter(|t| t.kind == TrackKind::Master) {
        if t.format != fmt {
            t.format = fmt;
            t.mixer.pan = Mixer::new(fmt).pan;
        }
    }
}

fn main_format(e: &mut Engine, p: &Value) -> Result<Value> {
    let id = "setup.main_format";
    let name = str_param(p, "format").ok_or_else(|| bad(id, "`format` required"))?;
    let fmt = ChannelFormat::from_id(name).ok_or_else(|| bad(id, format!("unknown format `{name}`")))?;
    if !(2..=16).contains(&fmt.channels()) {
        return Err(bad(id, format!("the main output must have 2 to 16 channels (`{}` has {})", fmt.label(), fmt.channels())));
    }
    let s = e.session_mut();
    set_main_format(s, fmt);
    Ok(json!({"format": fmt.label(), "channels": fmt.channels(), "output": s.outputs.first().map(|o| o.name.clone())}))
}

#[cfg(test)]
mod tests {
    use super::*;
    use soundcraft_audio_io::AudioBuffer;
    use soundcraft_model::{Clip, SourceAudio, SourceId};
    use std::sync::Arc;

    #[test]
    fn main_format_resizes_masters_and_renames_the_path() {
        let mut e = crate::demo::demo_engine();
        assert_eq!(e.session().main_format(), ChannelFormat::Stereo);
        let r = e.execute("setup.main_format", &json!({"format": "5.1"})).unwrap();
        assert_eq!(r["channels"], 6);
        assert_eq!(e.session().main_format(), ChannelFormat::Surround51);
        assert_eq!(e.session().outputs[0].name, "Out 1-6");
        assert!(e.session().tracks.iter().filter(|t| t.kind == TrackKind::Master).all(|t| t.format == ChannelFormat::Surround51));
        let out = soundcraft_mix::render_range(e.session(), soundcraft_time::Range::new(0, 4800), 512);
        assert_eq!(out.len(), 6);
        assert!(e.execute("setup.main_format", &json!({"format": "mono"})).is_err());
        assert!(e.execute("setup.main_format", &json!({"format": "nope"})).is_err());
        assert!(e.undo());
        assert_eq!(e.session().main_format(), ChannelFormat::Stereo);
        let insp = crate::inspect::session(&e, false);
        assert_eq!(insp["main_format"], "Stereo");
    }

    #[test]
    fn surround_pan_commands() {
        let mut e = Engine::default();
        e.execute("track.new", &json!({"count": 1, "format": "mono"})).unwrap();
        let t = e.session().tracks[0].id;
        e.execute("mix.surround_pan", &json!({"track": t.0, "x": -0.5, "divergence": 2.0})).unwrap();
        let sp = e.session().track(t).unwrap().mixer.surround.unwrap();
        assert_eq!((sp.x, sp.y, sp.divergence), (-0.5, 1.0, 1.0));
        e.execute("mix.surround_pan", &json!({"track": t.0, "y": f64::MAX})).unwrap();
        assert_eq!(e.session().track(t).unwrap().mixer.surround.unwrap().x, -0.5, "fields not given are kept");
        let insp = crate::inspect::session(&e, false);
        assert_eq!(insp["tracks"][0]["surround"]["x"], -0.5);
        e.execute("mix.surround_pan", &json!({"track": t.0, "clear": true})).unwrap();
        assert!(e.session().track(t).unwrap().mixer.surround.is_none());
        // Sends.
        assert!(e.execute("mix.send_surround_pan", &json!({"track": t.0, "slot": "a", "x": 1.0})).is_err(), "empty slot");
        e.execute("mix.new_bus", &json!({"name": "FX"})).unwrap();
        e.execute("mix.send", &json!({"track": t.0, "slot": 0, "bus": "FX"})).unwrap();
        e.execute("mix.send_surround_pan", &json!({"track": t.0, "slot": "a", "x": 1.0, "y": -1.0})).unwrap();
        let snd = e.session().track(t).unwrap().mixer.sends[0].clone().unwrap();
        assert_eq!(snd.surround.map(|s| (s.x, s.y)), Some((1.0, -1.0)));
        assert!(e.execute("mix.send_surround_pan", &json!({"track": t.0, "slot": 12})).is_err());
    }

    #[test]
    fn surround_bounce_writes_every_channel_with_its_mask() {
        let mut e = Engine::default();
        e.execute("setup.main_format", &json!({"format": "7.1"})).unwrap();
        e.execute("track.new", &json!({"count": 1, "format": "mono"})).unwrap();
        let t = e.session().tracks[0].id;
        {
            let s = e.session_mut();
            s.pool.insert(SourceId(77), Arc::new(SourceAudio::new(AudioBuffer { sample_rate: 48_000, channels: vec![vec![0.25; 4800]] })));
            let cid = s.new_clip_id();
            s.track_mut(t).unwrap().playlist_mut().unwrap().clips.push(Clip::audio(cid, "dc", SourceId(77), 0, 0, 4800));
        }
        e.execute("mix.surround_pan", &json!({"track": t.0, "x": 0.0, "y": 1.0})).unwrap();
        let dir = std::env::temp_dir().join(format!("sc-surround-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("mix.wav");
        let ps = path.to_string_lossy().to_string();
        e.execute("file.bounce_mix", &json!({"path": ps, "start": 0, "end": 4800, "bit_depth": "32f"})).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(soundcraft_audio_io::wav_channel_mask(&bytes), Some(0x63F));
        let (_, buf) = soundcraft_audio_io::decode(&bytes, Some("wav")).unwrap();
        assert_eq!(buf.channels.len(), 8);
        assert!((buf.channels[2][1000] - 0.25).abs() < 1e-5, "front centre lands on C");
        // ITU stereo fold-down.
        e.execute("file.bounce_mix", &json!({"path": ps, "start": 0, "end": 4800, "bit_depth": "32f", "fold_down": "stereo"})).unwrap();
        let (_, buf) = soundcraft_audio_io::decode(&std::fs::read(&path).unwrap(), Some("wav")).unwrap();
        assert_eq!(buf.channels.len(), 2);
        assert!((buf.channels[0][1000] - 0.25 * std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-4);
        assert!(e.execute("file.bounce_mix", &json!({"path": ps, "fold_down": "quad"})).is_err());
        // Stems follow the main format too.
        let sdir = dir.join("stems");
        let r = e.execute("file.bounce_stems", &json!({"dir": sdir.to_string_lossy(), "start": 0, "end": 4800})).unwrap();
        let f = r["files"][0].as_str().unwrap().to_string();
        let (_, buf) = soundcraft_audio_io::decode(&std::fs::read(&f).unwrap(), Some("wav")).unwrap();
        assert_eq!(buf.channels.len(), 8);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
