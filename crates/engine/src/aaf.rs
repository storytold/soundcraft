//! AAF boundary: the reusable interchange crate constructs the source document, then this module
//! verifies and decodes media and maps new tracks into the destination as one undoable edit.
use crate::{EngineError, Result};
use serde_json::{Value, json};
use soundcraft_interchange::{Plan, build_session};
use soundcraft_model::{ClipContent, ClipId, Session, SourceAudio, SourceId, TrackId};
use soundcraft_time::MAX_POSITION;
use std::{
    collections::{HashMap, HashSet},
    path::Path,
    sync::Arc,
};

fn bad(message: impl Into<String>) -> EngineError {
    EngineError::Io(message.into())
}

pub fn preview(path: &str, media_dir: &str, composition: Option<&str>) -> Result<Value> {
    let plan = crate::io::read_aaf_plan(path, media_dir, composition)?;
    let parsed: Plan = serde_json::from_value(plan.clone()).map_err(|e| bad(e.to_string()))?;
    let built = build_session(parsed).map_err(|e| bad(e.to_string()))?;
    let s = built.session;
    let tracks: Vec<_> = s
        .tracks
        .iter()
        .enumerate()
        .map(|(index, t)| json!({"index": index, "name": t.name, "channels": t.channels(), "clips": t.clips().len()}))
        .collect();
    Ok(json!({"plan": plan, "name": s.name, "sample_rate": s.sample_rate.hz(), "frame_rate": s.frame_rate.label(),
        "timecode_start": s.timecode_start, "start_timecode": s.format_position(0, soundcraft_time::TimeFormat::Timecode),
        "sources": s.sources.len(), "tracks": tracks, "warnings": built.report.warnings}))
}

/// No destination mutation until all options, times and decoded source files have been checked.
pub fn import_into(destination: &Session, params: &Value) -> Result<(Session, Value)> {
    let plan_value = params.get("plan").ok_or_else(|| bad("`plan` from file.preview_aaf is required"))?;
    let mut plan: Plan = serde_json::from_value(plan_value.clone()).map_err(|e| bad(e.to_string()))?;
    let indices = params.get("selected").and_then(Value::as_array).ok_or_else(|| bad("`selected` track indices required"))?;
    let mut selected = HashSet::new();
    for v in indices {
        let n =
            v.as_u64().and_then(|n| usize::try_from(n).ok()).filter(|n| *n < plan.tracks.len()).ok_or_else(|| bad("invalid source track index"))?;
        if !selected.insert(n) {
            return Err(bad("duplicate source track selection"));
        }
    }
    if selected.is_empty() {
        return Err(bad("select at least one source track"));
    }
    let mut i = 0;
    plan.tracks.retain(|_| {
        let keep = selected.contains(&i);
        i += 1;
        keep
    });
    let used: HashSet<String> = plan.tracks.iter().flat_map(|t| t.clips.iter().map(|c| c.source.clone())).collect();
    plan.sources.retain(|s| used.contains(&s.key));
    let built = build_session(plan).map_err(|e| bad(e.to_string()))?;
    let mut incoming = built.session;
    let media = params.get("media").and_then(Value::as_str).unwrap_or("copy");
    if media != "copy" {
        return Err(bad("only full-source copy is supported; external linking is not preserved by Save Session"));
    }
    let mapping = params.get("mapping").and_then(Value::as_str).unwrap_or("absolute");
    if !matches!(mapping, "absolute" | "relative") {
        return Err(bad("mapping must be absolute or relative"));
    }
    let offset = match params.get("offset_samples") {
        None => 0,
        Some(v) => v.as_i64().filter(|n| n.abs_diff(0) <= MAX_POSITION as u64).ok_or_else(|| bad("invalid track offset in samples"))?,
    };
    let empty = destination.tracks.is_empty() && destination.sources.is_empty();
    if !empty && incoming.sample_rate != destination.sample_rate {
        return Err(bad("sample rates differ; sample-rate conversion is not implemented"));
    }
    let origin = if empty { incoming.timecode_start } else { destination.timecode_start };
    let shift = if mapping == "absolute" { incoming.timecode_start.checked_sub(origin) } else { Some(0) }
        .and_then(|n| n.checked_add(offset))
        .ok_or_else(|| bad("time mapping overflow"))?;
    for t in &mut incoming.tracks {
        for c in &mut t.playlist_mut().ok_or_else(|| bad("source playlist missing"))?.clips {
            c.start = c
                .start
                .checked_add(shift)
                .filter(|n| *n >= 0 && *n <= MAX_POSITION)
                .ok_or_else(|| bad("mapped clip precedes session start or exceeds time limit; choose relative mapping or another offset"))?;
            if c.start.checked_add(c.length).is_none_or(|n| n > MAX_POSITION) {
                return Err(bad("mapped end exceeds time limit"));
            }
        }
    }
    let mut total_samples = 0u64;
    for src in &incoming.sources {
        if !Path::new(&src.path).is_absolute() {
            return Err(bad("source paths must be absolute"));
        }
        total_samples = total_samples
            .checked_add(src.frames.saturating_mul(u64::from(src.channels)))
            .filter(|n| *n <= 1 << 27)
            .ok_or_else(|| bad("selected media exceeds the decoded import memory limit (512 MiB)"))?;
        let bytes = crate::io::read_interchange_file(Path::new(&src.path), 512 * 1024 * 1024)?;
        let info = soundcraft_audio_io::probe(&bytes, Some("wav")).map_err(|e| bad(e.to_string()))?;
        if (info.sample_rate, info.channels, info.frames) != (src.sample_rate, src.channels, src.frames)
            || info.format != soundcraft_audio_io::FileFormat::Wav
        {
            return Err(bad(format!("media changed since analysis: {}", src.path)));
        }
        let (_, buffer) = soundcraft_audio_io::decode(&bytes, Some("wav")).map_err(|e| bad(e.to_string()))?;
        incoming.pool.insert(src.id, Arc::new(SourceAudio::new(buffer)));
    }
    let mut target = destination.clone();
    if empty {
        target.sample_rate = incoming.sample_rate;
        target.frame_rate = incoming.frame_rate;
        target.timecode_start = incoming.timecode_start;
        if target.name == "Untitled" {
            target.name = incoming.name.clone();
        }
    }
    let mut source_ids = HashMap::new();
    for mut src in incoming.sources {
        let old = src.id;
        src.id = SourceId(target.alloc());
        let audio = incoming.pool.get(old).ok_or_else(|| bad("decoded media missing"))?.clone();
        target.pool.insert(src.id, audio);
        source_ids.insert(old, src.id);
        // Save writes the whole decoded source into the destination Audio Files folder.
        src.unsaved = true;
        target.sources.push(src);
    }
    let mut track_ids = Vec::new();
    for mut track in incoming.tracks {
        track.id = TrackId(target.alloc());
        track.name = target.unique_track_name(&track.name);
        let name = track.name.clone();
        for playlist in &mut track.playlists {
            playlist.name = name.clone();
            for clip in &mut playlist.clips {
                clip.id = ClipId(target.alloc());
                if let ClipContent::Audio { source, .. } = &mut clip.content {
                    *source = *source_ids.get(source).ok_or_else(|| bad("source mapping missing"))?;
                }
            }
        }
        track_ids.push(track.id);
        target.tracks.push(track);
    }
    let first_clip = target.tracks.iter().filter(|t| track_ids.contains(&t.id)).flat_map(|t| t.clips()).map(|c| c.start).min();
    let report = json!({"first_clip_sample":first_clip,"tracks":track_ids,"sources":source_ids.len(),"warnings":built.report.warnings,"handles":built.report.handles,"mapping":mapping,"shift_samples":shift,"media":media});
    Ok((target, report))
}
