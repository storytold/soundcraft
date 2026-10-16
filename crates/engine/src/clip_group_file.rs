//! Clip group files (`.scgrp`): clips exported from a session (Clip List › Export Clip Groups)
//! and imported into another (File › Import › Clip Groups…).
//!
//! The file is JSON: clips per lane with starts relative to the group, plus the audio they play
//! as 32-bit float WAVs in a `<name> Audio/` folder next to it.

use crate::{Engine, EngineError, Result};
use serde::{Deserialize, Serialize};
use soundcraft_audio_io::{EncodeOptions, FileFormat};
use soundcraft_model::{Clip, ClipContent, ClipId, SourceId, TrackId};
use soundcraft_time::Samples;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub const EXTENSION: &str = "scgrp";
const FORMAT: &str = "soundcraft.clipgroup";
/// Refuse files that would place absurd numbers of clips, tracks or audio files.
const MAX_CLIPS: usize = 10_000;

#[derive(Debug, Serialize, Deserialize)]
struct GroupFile {
    format: String,
    version: u32,
    sample_rate: u32,
    length: Samples,
    /// Clips per lane (one lane per source track), starts relative to the group start.
    lanes: Vec<Vec<Clip>>,
    /// Source id (as written) → audio file path relative to the group file.
    audio: BTreeMap<u64, String>,
}

/// Write the clips `ids` (grouped by track) to `path`. Returns the number of clips written.
pub fn export(e: &Engine, ids: &[ClipId], path: &str) -> Result<usize> {
    let p = with_extension(path);
    // First, so a refused group file leaves nothing behind and is refused whatever else is wrong.
    crate::file_access::check_write(&p).map_err(EngineError::Io)?;
    let s = e.session();
    let mut lanes: Vec<(usize, Vec<Clip>)> = Vec::new();
    for id in ids {
        let Some((t, c)) = s.find_clip(*id) else { continue };
        let ti = s.tracks.iter().position(|x| x.id == t).unwrap_or(0);
        match lanes.iter_mut().find(|(i, _)| *i == ti) {
            Some((_, v)) => v.push(c.clone()),
            None => lanes.push((ti, vec![c.clone()])),
        }
    }
    let clips: Vec<&Clip> = lanes.iter().flat_map(|(_, v)| v.iter()).collect();
    let (Some(start), Some(end)) = (clips.iter().map(|c| c.start).min(), clips.iter().map(|c| c.start.saturating_add(c.length)).max()) else {
        return Err(EngineError::BadParams("file.export_clip_groups".into(), "no clips to export".into()));
    };
    let n = clips.len();
    lanes.sort_by_key(|(i, _)| *i);
    let stem = p.file_stem().and_then(|x| x.to_str()).unwrap_or("Clip Group").to_string();
    let audio_dir_name = format!("{} Audio", crate::io::sanitize_name(&stem));
    let dir = p.parent().map(Path::to_path_buf).unwrap_or_default();
    let mut audio = BTreeMap::new();
    let mut out_lanes = Vec::new();
    for (_, v) in lanes {
        let mut lane = Vec::new();
        for mut c in v {
            c.start -= start;
            if let ClipContent::Audio { source, .. } = c.content
                && let std::collections::btree_map::Entry::Vacant(slot) = audio.entry(source.0)
                && let (Some(buf), Some(src)) = (s.pool.get(source), s.sources.iter().find(|x| x.id == source))
            {
                let rel = format!("{audio_dir_name}/{}_{}.wav", crate::io::sanitize_name(&src.name), source.0);
                crate::io::make_dir(dir.join(&audio_dir_name))?;
                let opts = EncodeOptions {
                    format: FileFormat::Wav,
                    bit_depth: soundcraft_audio_io::BitDepth::Float32,
                    dither: false,
                    bwf: None,
                    bitrate_kbps: None,
                };
                let bytes = soundcraft_audio_io::encode(&buf.buffer, &opts).map_err(|err| EngineError::Io(err.to_string()))?;
                crate::io::write_path(dir.join(&rel), bytes)?;
                slot.insert(rel);
            }
            lane.push(c);
        }
        out_lanes.push(lane);
    }
    let file = GroupFile { format: FORMAT.into(), version: 1, sample_rate: s.sample_rate.hz(), length: end - start, lanes: out_lanes, audio };
    let text = serde_json::to_string_pretty(&file).map_err(|err| EngineError::Io(err.to_string()))?;
    crate::io::write_path(&p, text)?;
    Ok(n)
}

/// Import a clip group file at `at`, one lane per track starting at `first` (new audio tracks
/// are made when the session runs out). Returns the ids of the placed clips.
pub fn import(e: &mut Engine, path: &str, first: Option<TrackId>, at: Samples) -> Result<Vec<ClipId>> {
    let id = "file.import_clip_groups";
    let text = crate::io::read_text(path)?;
    let file: GroupFile = serde_json::from_str(&text).map_err(|err| EngineError::BadParams(id.into(), format!("not a clip group file: {err}")))?;
    if file.format != FORMAT {
        return Err(EngineError::BadParams(id.into(), "not a SoundCraft clip group file".into()));
    }
    if file.lanes.iter().map(|l| l.len().max(1)).sum::<usize>() > MAX_CLIPS || file.audio.len() > MAX_CLIPS {
        return Err(EngineError::BadParams(id.into(), "clip group file has too many clips".into()));
    }
    let dir = Path::new(path).parent().map(Path::to_path_buf).unwrap_or_default();
    // Decode audio first so a bad file changes nothing, and each file once however many ids name it.
    let (mut decoded, mut index, mut file_of) = (Vec::new(), BTreeMap::new(), BTreeMap::new());
    let rate = e.session().sample_rate.hz();
    for (old, rel) in &file.audio {
        let full = dir.join(rel);
        if !full.starts_with(&dir) || rel.contains("..") {
            return Err(EngineError::BadParams(id.into(), format!("audio path escapes the group folder: {rel}")));
        }
        let key = crate::io::file_key(&full);
        let n = match index.get(&key) {
            Some(n) => *n,
            None => {
                let bytes = crate::io::read_file(&full)?;
                let (_, buf) = soundcraft_audio_io::decode(&bytes, Some("wav")).map_err(|err| EngineError::Io(format!("{rel}: {err}")))?;
                let buf = crate::io::at_rate(buf, rate, crate::io::MAX_IMPORT_SAMPLES)
                    .ok_or_else(|| EngineError::Io(format!("{rel}: too large to import")))?;
                decoded.push((rel.clone(), buf));
                index.insert(key, decoded.len() - 1);
                decoded.len() - 1
            }
        };
        file_of.insert(*old, n);
    }
    let src_rate = f64::from(file.sample_rate.max(1));
    let s = e.session_mut();
    let ratio = f64::from(s.sample_rate.hz()) / src_rate;
    let scale = |v: Samples| -> Samples { (v as f64 * ratio).round() as Samples };
    let sources: Vec<SourceId> = decoded
        .into_iter()
        .map(|(rel, buf)| {
            let name = Path::new(&rel).file_name().and_then(|x| x.to_str()).unwrap_or("clip group").to_string();
            crate::io::add_source(s, &name, buf, None, FileFormat::Wav)
        })
        .collect();
    let map: BTreeMap<u64, SourceId> = file_of.into_iter().filter_map(|(old, n)| Some((old, *sources.get(n)?))).collect();
    let start_idx = first.and_then(|t| s.tracks.iter().position(|x| x.id == t));
    let mut placed = Vec::new();
    let group_id = s.alloc();
    for (li, lane) in file.lanes.into_iter().enumerate() {
        let audio_lane = lane.iter().any(|c| matches!(c.content, ClipContent::Audio { .. }));
        let target = start_idx.and_then(|i| s.tracks.get(i + li)).map(|t| t.id);
        let target = match target {
            Some(t) => t,
            None => {
                let channels = lane
                    .iter()
                    .find_map(|c| match c.content {
                        ClipContent::Audio { source, .. } => map.get(&source.0).and_then(|n| s.pool.get(*n)).map(|a| a.buffer.num_channels()),
                        _ => None,
                    })
                    .unwrap_or(2);
                let kind = if audio_lane { soundcraft_model::TrackKind::Audio } else { soundcraft_model::TrackKind::Midi };
                let fmt = if channels == 1 { soundcraft_model::ChannelFormat::Mono } else { soundcraft_model::ChannelFormat::Stereo };
                s.add_track(kind, fmt, Some("Clip Group"))
            }
        };
        for mut c in lane {
            if let ClipContent::Audio { source, offset } = &mut c.content {
                let Some(n) = map.get(&source.0) else { continue };
                *source = *n;
                *offset = scale(*offset);
            } else if !matches!(c.content, ClipContent::Midi { .. }) {
                continue;
            }
            c.id = s.new_clip_id();
            c.start = at.saturating_add(scale(c.start.max(0)));
            c.length = scale(c.length);
            c.group = Some(group_id);
            c.sanitize();
            placed.push(c.id);
            crate::edit::place_clip(s, target, c);
        }
    }
    Ok(placed)
}

fn with_extension(path: &str) -> PathBuf {
    let p = PathBuf::from(path);
    if p.extension().is_none() { p.with_extension(EXTENSION) } else { p }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    #[test]
    fn export_then_import_round_trips_audio_clips() {
        let dir = std::env::temp_dir().join(format!("sc-grp-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("drums.scgrp");
        let path = path.to_string_lossy().to_string();
        let mut e = crate::demo::demo_engine();
        let kick = e.session().tracks.iter().find(|t| t.name == "Kick").map(|t| t.id).unwrap();
        let ids: Vec<_> = e.session().track(kick).unwrap().clips().iter().take(2).map(|c| c.id).collect();
        let r = e.execute("file.export_clip_groups", &json!({"path": path, "clips": ids.iter().map(|c| c.0).collect::<Vec<_>>()})).unwrap();
        assert_eq!(r["clips"], 2);
        let before = e.session().tracks.len();
        let at = 30_000_000;
        let r = e.execute("file.import_clip_groups", &json!({"path": path, "at": at, "track": "Bass"})).unwrap();
        let placed: Vec<_> = r["clips"].as_array().unwrap().iter().filter_map(serde_json::Value::as_u64).collect();
        assert_eq!(placed.len(), 2);
        let bass = e.session().tracks.iter().find(|t| t.name == "Bass").map(|t| t.id).unwrap();
        for id in &placed {
            let (t, c) = e.session().find_clip(soundcraft_model::ClipId(*id)).unwrap();
            assert_eq!(t, bass);
            assert!(c.start >= at && c.group.is_some());
            assert!(matches!(c.content, soundcraft_model::ClipContent::Audio { source, .. } if e.session().pool.contains(source)));
        }
        e.undo();
        assert_eq!(e.session().tracks.len(), before);
        assert!(e.session().find_clip(soundcraft_model::ClipId(placed[0])).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn positions_in_a_group_file_are_kept_within_reach() {
        let dir = std::env::temp_dir().join(format!("sc-grp-far-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let p = dir.join("far.scgrp");
        let note = json!({"pitch": 60, "velocity": 100, "release_velocity": 64, "channel": 0, "start": i64::MAX, "length": i64::MAX});
        let clip =
            json!({"id": 1, "name": "far", "start": i64::MAX, "length": i64::MAX, "content": {"Midi": {"sequence": {"notes": [note], "ctrls": []}}}});
        let file = json!({"format": "soundcraft.clipgroup", "version": 1, "sample_rate": 48000, "length": 1, "lanes": [[clip]], "audio": {}});
        std::fs::write(&p, file.to_string()).unwrap();
        let mut e = crate::demo::demo_engine();
        let r = e.execute("file.import_clip_groups", &json!({"path": p.to_string_lossy()})).unwrap();
        let id = soundcraft_model::ClipId(r["clips"][0].as_u64().unwrap());
        let c = e.session().find_clip(id).unwrap().1;
        assert!(c.start <= crate::cmd::MAX_POSITION && c.length <= crate::cmd::MAX_POSITION);
        e.execute("edit.select_all", &json!({})).unwrap();
        for cmd in ["edit.duplicate", "event.extract_chords"] {
            assert!(!matches!(e.execute(cmd, &json!({})), Err(crate::EngineError::Internal(..))), "{cmd}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_file_named_by_many_ids_becomes_one_source() {
        let dir = std::env::temp_dir().join(format!("sc-grp-shared-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let audio = soundcraft_audio_io::AudioBuffer { sample_rate: 48_000, channels: vec![vec![0.25; 480]] };
        std::fs::write(dir.join("k.wav"), soundcraft_audio_io::encode(&audio, &soundcraft_audio_io::EncodeOptions::default()).unwrap()).unwrap();
        let clip = |id: u64| json!({"id": id, "name": "k", "start": id * 1000, "length": 480, "content": {"Audio": {"source": id, "offset": 0}}});
        let audio_paths = json!({"1": "k.wav", "2": "./k.wav", "3": "k.wav"});
        let file = json!({"format": "soundcraft.clipgroup", "version": 1, "sample_rate": 48000, "length": 4000, "lanes": [[clip(1), clip(2), clip(3)]], "audio": audio_paths});
        let p = dir.join("k.scgrp");
        std::fs::write(&p, file.to_string()).unwrap();
        let mut e = crate::demo::demo_engine();
        let before = e.session().sources.len();
        let r = e.execute("file.import_clip_groups", &json!({"path": p.to_string_lossy()})).unwrap();
        assert_eq!((r["clips"].as_array().unwrap().len(), e.session().sources.len()), (3, before + 1));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rejects_foreign_files_and_escaping_paths() {
        let dir = std::env::temp_dir().join(format!("sc-grp-bad-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let p = dir.join("x.scgrp");
        std::fs::write(
            &p,
            r#"{"format":"soundcraft.clipgroup","version":1,"sample_rate":48000,"length":1,"lanes":[],"audio":{"1":"../../etc/passwd"}}"#,
        )
        .unwrap();
        let mut e = crate::demo::demo_engine();
        assert!(e.execute("file.import_clip_groups", &json!({"path": p.to_string_lossy()})).is_err());
        std::fs::write(&p, "{}").unwrap();
        assert!(e.execute("file.import_clip_groups", &json!({"path": p.to_string_lossy()})).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
