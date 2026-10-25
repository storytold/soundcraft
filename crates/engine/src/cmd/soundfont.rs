//! The SoundFont Player's bank: `mix.soundfont_load` reads a SoundFont 2 (`.sf2`) file and makes
//! it what an instrument track plays (the SoundFont Player becomes its instrument);
//! `mix.soundfont_presets` lists the bank's presets. The file is referenced by path, like a movie,
//! not copied into the session: a session reopens it from there (see [`reload_banks`]). Which
//! preset plays is the instrument's `preset` parameter (`mix.instrument_param`), or a Program
//! Change played or recorded on the track.
use super::{CommandSpec, always, bad, has_selection, str_param, tracks_required};

use crate::{Engine, EngineError, Result, cmd};
use serde_json::{Value, json};
use soundcraft_dsp::soundfont::SoundFont;
use soundcraft_model::{Insert, Session, TrackId, TrackKind};
use std::path::Path;
use std::sync::Arc;

const PLAYER: &str = "soundfont";

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "mix.soundfont_load",
            "Load SoundFont",
            [],
            None,
            "{track?, path, preset?: index or name}: the instrument track plays the .sf2 file through the SoundFont Player (made its instrument if it is not already) → {track, name, presets: [{index, name, bank, program}], preset}",
            has_selection,
            load
        ),
        cmd!(query "mix.soundfont_presets", "List SoundFont Presets", [], None, "{track?} → {path, name, presets: [{index, name, bank, program}], preset}", always, presets),
    ]
}

fn instrument_track(e: &Engine, id: &str, p: &Value) -> Result<TrackId> {
    let t = tracks_required(e, id, p)?.first().copied().ok_or_else(|| bad(id, "no track"))?;
    if e.session().track(t).map(|tr| tr.kind) != Some(TrackKind::Instrument) {
        return Err(bad(id, "only instrument tracks play a SoundFont"));
    }
    Ok(t)
}

/// Read and parse the bank at `path` (a file the person picked, or one a session names).
pub(crate) fn read_bank(path: &Path) -> Result<Arc<SoundFont>> {
    let bytes = crate::io::read_file(path)?;
    SoundFont::parse(&bytes).map(Arc::new).map_err(|why| EngineError::Io(format!("{}: {why}", path.display())))
}

fn preset_list(sf: &SoundFont) -> Value {
    json!(sf.presets.iter().enumerate().map(|(i, p)| json!({"index": i, "name": p.name, "bank": p.bank, "program": p.program})).collect::<Vec<_>>())
}

/// The bank a track's SoundFont Player plays, with its path.
pub fn track_bank(s: &Session, t: TrackId) -> Option<(String, Arc<SoundFont>)> {
    let path = s.track(t)?.instrument.as_ref().filter(|i| i.plugin == PLAYER)?.soundfont.clone()?;
    let bank = Arc::clone(s.pool.bank(&path)?).downcast::<SoundFont>().ok()?;
    Some((path, bank))
}

fn load(e: &mut Engine, p: &Value) -> Result<Value> {
    let id = "mix.soundfont_load";
    let path = str_param(p, "path").filter(|x| !x.trim().is_empty()).ok_or_else(|| bad(id, "`path` required"))?.to_string();
    let t = instrument_track(e, id, p)?;
    let info = soundcraft_dsp::plugin_info(PLAYER).ok_or_else(|| bad(id, "the SoundFont Player is not available"))?;
    let bank = read_bank(Path::new(&path))?;
    let preset = match p.get("preset") {
        None => 0,
        Some(v) => match (v.as_u64(), v.as_str()) {
            (Some(n), _) if (n as usize) < bank.presets.len() => n as usize,
            (_, Some(name)) => bank
                .presets
                .iter()
                .position(|x| x.name.eq_ignore_ascii_case(name.trim()))
                .ok_or_else(|| bad(id, format!("no preset named `{name}`")))?,
            _ => return Err(bad(id, format!("`preset` must be an index below {} or a preset name", bank.presets.len()))),
        },
    };
    let (name, list) = (bank.name.clone(), preset_list(&bank));
    let s = e.session_mut();
    s.pool.insert_bank(path.clone(), bank);
    let tr = s.track_mut(t).ok_or_else(|| bad(id, "no track"))?;
    if !tr.instrument.as_ref().is_some_and(|i| i.plugin == PLAYER) {
        let mut ins = Insert::new(PLAYER);
        for pi in info.params {
            ins.params.insert(pi.id.to_string(), pi.default);
        }
        tr.instrument = Some(ins);
    }
    if let Some(ins) = tr.instrument.as_mut() {
        ins.soundfont = Some(path.clone());
        ins.params.insert("preset".into(), preset as f32);
    }
    drop_unused_banks(e.session_mut());
    Ok(json!({"track": t, "path": path, "name": name, "presets": list, "preset": preset}))
}

fn presets(e: &mut Engine, p: &Value) -> Result<Value> {
    let id = "mix.soundfont_presets";
    let t = instrument_track(e, id, p)?;
    let s = e.session();
    let (path, bank) = track_bank(s, t).ok_or_else(|| bad(id, "the track plays no SoundFont (load one with mix.soundfont_load)"))?;
    let preset = s.track(t).and_then(|tr| tr.instrument.as_ref()).and_then(|i| i.params.get("preset")).map_or(0.0, |v| *v);
    Ok(json!({"path": path, "name": bank.name, "presets": preset_list(&bank), "preset": preset}))
}

/// Forget banks no track plays any more (they can be hundreds of megabytes).
fn drop_unused_banks(s: &mut Session) {
    let used: Vec<String> = s.tracks.iter().filter_map(|t| t.instrument.as_ref()?.soundfont.clone()).collect();
    s.pool.retain_banks(|k| used.iter().any(|u| u == k));
}

/// Load the banks the tracks of a session just read name (`links`: the session file their paths
/// are relative to). Returns the files that could not be loaded, for the missing-media report.
pub(crate) fn reload_banks(s: &mut Session, links: &Path) -> Vec<String> {
    let mut missing = Vec::new();
    let paths: Vec<String> = s.tracks.iter().filter_map(|t| t.instrument.as_ref().filter(|i| i.plugin == PLAYER)?.soundfont.clone()).collect();
    for path in paths {
        if s.pool.bank(&path).is_some() {
            continue;
        }
        let loaded = crate::io::stored_path(&path, links)
            .map_err(|why| format!("{path}: {why}"))
            .and_then(|file| read_bank(&file).map_err(|err| err.to_string()));
        match loaded {
            Ok(bank) => s.pool.insert_bank(path, bank),
            Err(err) => missing.push(err),
        }
    }
    missing
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use soundcraft_model::ChannelFormat;

    /// Writes the generated test bank (a sine, no third-party audio) to a scratch file.
    fn bank_file(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("sc-sf2-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let f = d.join("test.sf2");
        std::fs::write(&f, soundcraft_dsp::soundfont::fixture::test_bank()).unwrap();
        f
    }

    fn engine() -> (Engine, TrackId) {
        let mut e = Engine::new(Session::default());
        let t = e.session_mut().add_track(TrackKind::Instrument, ChannelFormat::Stereo, Some("Keys"));
        (e, t)
    }

    #[test]
    fn loading_a_bank_makes_the_player_the_instrument_and_lists_its_presets() {
        let (mut e, t) = engine();
        let f = bank_file("load");
        let r = e.execute("mix.soundfont_load", &json!({"track": t.0, "path": f.to_str().unwrap(), "preset": "test"})).unwrap();
        assert_eq!(r["presets"][0]["name"], "Test");
        assert_eq!(r["presets"][0]["program"], 5);
        let ins = e.session().track(t).unwrap().instrument.clone().unwrap();
        assert_eq!(ins.plugin, PLAYER);
        assert_eq!(ins.soundfont.as_deref(), f.to_str());
        assert_eq!(ins.params.get("preset"), Some(&0.0));
        assert!(track_bank(e.session(), t).is_some());
        let q = e.execute("mix.soundfont_presets", &json!({"track": t.0})).unwrap();
        assert_eq!(q["presets"].as_array().unwrap().len(), 1);
        // Undo puts the synth back and the bank goes with it.
        e.execute("edit.undo", &json!({})).unwrap();
        assert!(track_bank(e.session(), t).is_none());
        let _ = std::fs::remove_dir_all(f.parent().unwrap());
    }

    #[test]
    fn a_midi_clip_plays_through_the_loaded_bank() {
        let (mut e, t) = engine();
        let f = bank_file("play");
        e.execute("mix.soundfont_load", &json!({"track": t.0, "path": f.to_str().unwrap()})).unwrap();
        let mut seq = soundcraft_midi::Sequence::default();
        seq.notes.push(soundcraft_midi::Note { pitch: 69, velocity: 110, release_velocity: 64, channel: 0, start: 0, length: 960 });
        let s = e.session_mut();
        let cid = s.new_clip_id();
        crate::edit::place_clip(s, t, soundcraft_model::Clip::midi(cid, "n", 0, 48_000, seq));
        let out = soundcraft_mix::render_range(e.session(), soundcraft_time::Range::new(0, 24_000), 512);
        let peak = out.iter().flatten().fold(0.0f32, |m, x| m.max(x.abs()));
        assert!(peak > 0.05, "silent: {peak}");
        let _ = std::fs::remove_dir_all(f.parent().unwrap());
    }

    #[test]
    fn bad_files_and_bad_presets_are_refused_before_touching_the_session() {
        let (mut e, t) = engine();
        let f = bank_file("bad");
        let before = e.session().track(t).unwrap().instrument.clone();
        let wav = f.with_file_name("x.sf2");
        std::fs::write(&wav, b"RIFF\x04\0\0\0WAVE").unwrap();
        assert!(e.execute("mix.soundfont_load", &json!({"track": t.0, "path": wav.to_str().unwrap()})).is_err());
        assert!(e.execute("mix.soundfont_load", &json!({"track": t.0, "path": f.to_str().unwrap(), "preset": 9})).is_err());
        assert!(e.execute("mix.soundfont_load", &json!({"track": t.0, "path": f.to_str().unwrap(), "preset": "nope"})).is_err());
        assert_eq!(e.session().track(t).unwrap().instrument, before);
        let audio = e.session_mut().add_track(TrackKind::Audio, ChannelFormat::Mono, None);
        assert!(e.execute("mix.soundfont_load", &json!({"track": audio.0, "path": f.to_str().unwrap()})).is_err());
        let _ = std::fs::remove_dir_all(f.parent().unwrap());
    }

    #[test]
    fn a_saved_session_reopens_with_its_bank_or_reports_it_missing() {
        let (mut e, t) = engine();
        let f = bank_file("reopen");
        e.execute("mix.soundfont_load", &json!({"track": t.0, "path": f.to_str().unwrap()})).unwrap();
        let song = f.with_file_name("Song.scraft");
        crate::io::save_session(&mut e, song.to_str().unwrap()).unwrap();
        let mut again = Engine::new(Session::default());
        let missing = crate::io::open_session(&mut again, song.to_str().unwrap()).unwrap();
        assert!(missing.is_empty(), "{missing:?}");
        let t2 = again.session().tracks.iter().find(|x| x.kind == TrackKind::Instrument).unwrap().id;
        assert!(track_bank(again.session(), t2).is_some());
        std::fs::remove_file(&f).unwrap();
        let mut third = Engine::new(Session::default());
        let missing = crate::io::open_session(&mut third, song.to_str().unwrap()).unwrap();
        assert_eq!(missing.len(), 1, "{missing:?}");
        let _ = std::fs::remove_dir_all(f.parent().unwrap());
    }
}
