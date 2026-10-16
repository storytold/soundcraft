//! Selected-track file delivery. Plans and exports do not mutate or journal the document.

use super::*;
use serde_json::json;
use soundcraft_audio_io::{BitDepth, EncodeOptions, FileFormat};
use soundcraft_mix::track_bounce::{self as render, Options};
use std::path::{Path, PathBuf};

const ID: &str = "track.bounce";
const PARAMS: &str = "{tracks?, path? (single track) | dir? (native batch), format?: wav|aiff|flac, bit_depth?: 16|24|32f, dither?: true, render_volume?: true, render_pan?: true, start?, end?, tail_seconds?: 0, expected_revision?}";

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec {
            id: ID,
            label: "Bounce...",
            menu: &["Track"],
            shortcut: Some("Cmd+Alt+Shift+B"),
            params: PARAMS,
            enabled: has_selection,
            run: bounce,
            journal: false,
            undoable: false,
            view: false,
        },
        CommandSpec {
            id: "track.bounce_preview",
            label: "Preview Track Bounce",
            menu: &[],
            shortcut: None,
            params: PARAMS,
            enabled: has_selection,
            run: preview,
            journal: false,
            undoable: false,
            view: false,
        },
    ]
}

struct FilePlan {
    render: render::Plan,
    path: PathBuf,
    name: String,
}

struct Plan {
    files: Vec<FilePlan>,
    issues: Vec<Value>,
    opts: EncodeOptions,
    revision: u64,
}

fn boolean(p: &Value, key: &str, default: bool) -> Result<bool> {
    p.get(key).map_or(Ok(default), |v| v.as_bool().ok_or_else(|| bad(ID, format!("`{key}` must be boolean"))))
}

fn text<'a>(p: &'a Value, key: &str) -> Result<Option<&'a str>> {
    p.get(key)
        .map(|v| {
            v.as_str()
                .filter(|s| !s.trim().is_empty() && s.len() <= 4096)
                .ok_or_else(|| bad(ID, format!("`{key}` must be nonempty text (at most 4096 bytes)")))
        })
        .transpose()
}

fn position(e: &Engine, p: &Value, key: &str) -> Result<Option<i64>> {
    let n = position_param_raw(e, ID, p, key)?;
    if n.is_some_and(|n| !(0..=MAX_POSITION).contains(&n)) {
        return Err(bad(ID, "range positions must be nonnegative and within the position limit"));
    }
    Ok(n)
}

fn source_end(e: &Engine, id: TrackId) -> Result<i64> {
    let s = e.session();
    let t = s.track(id).ok_or_else(|| bad(ID, "unknown source"))?;
    render::source_end(s, t).map_err(|r| bad(ID, r))
}

fn extension(f: FileFormat) -> &'static str {
    match f {
        FileFormat::Aiff => "aif",
        FileFormat::Flac => "flac",
        _ => "wav",
    }
}

fn stem(name: &str, id: TrackId) -> String {
    let clean = crate::io::sanitize_name(name);
    let mut out = String::new();
    for c in clean.chars() {
        if out.len() + c.len_utf8() > 96 {
            break;
        }
        out.push(c);
    }
    format!("{out}-{}", id.0)
}

fn plan(e: &Engine, p: &Value) -> Result<Plan> {
    if cfg!(all(not(unix), not(target_arch = "wasm32"))) {
        return Err(bad(
            ID,
            "native Track Bounce publication currently requires Unix file identity and a hard-link-capable filesystem; Windows delivery is unsupported",
        ));
    }
    let object = p.as_object().ok_or_else(|| bad(ID, "parameters must be an object"))?;
    let keys = [
        "tracks",
        "track",
        "path",
        "dir",
        "format",
        "bit_depth",
        "dither",
        "render_volume",
        "render_pan",
        "start",
        "end",
        "tail_seconds",
        "expected_revision",
    ];
    if let Some(key) = object.keys().find(|k| !keys.contains(&k.as_str())) {
        return Err(bad(ID, format!("unsupported option `{key}`")));
    }
    if let Some(v) = p.get("expected_revision")
        && v.as_u64() != Some(e.revision)
    {
        return Err(bad(ID, "Track Bounce preview is stale; reopen the dialog or preview the current revision"));
    }
    if p.get("tracks").is_some_and(|v| !v.is_array() || v.as_array().is_some_and(|v| v.len() > 128)) {
        return Err(bad(ID, "`tracks` must be an array of at most 128 sources"));
    }
    if p.get("tracks").is_none() && p.get("track").is_none() && e.session().edit.selected_tracks.len() > 128 {
        return Err(bad(ID, "at most 128 selected sources can be delivered in one batch"));
    }
    if p.get("tracks").is_some() && p.get("track").is_some() {
        return Err(bad(ID, "give track or tracks, exclusively"));
    }
    let mut tracks = tracks_required(e, ID, p)?;
    let mut seen = std::collections::HashSet::new();
    tracks.retain(|t| seen.insert(*t));
    if tracks.len() > 128 {
        return Err(bad(ID, "at most 128 sources can be delivered in one batch"));
    }
    let path = text(p, "path")?;
    let dir = text(p, "dir")?;
    if (path.is_some() == dir.is_some()) || (path.is_some() && tracks.len() != 1) {
        return Err(bad(ID, "give `path` for one source or `dir` for a batch, exclusively"));
    }
    if cfg!(target_arch = "wasm32") && (dir.is_some() || tracks.len() != 1) {
        return Err(bad(ID, "web Track Bounce supports one file destination; native batch directories are unsupported"));
    }
    let named_format = text(p, "format")?;
    let ext = path.and_then(|p| Path::new(p).extension()).and_then(|x| x.to_str());
    let format = soundcraft_audio_io::encode_format_for(named_format.or(ext).unwrap_or("wav"))
        .ok_or_else(|| bad(ID, "supported file formats: wav, aiff, flac"))?;
    if path.is_some() && ext.and_then(soundcraft_audio_io::encode_format_for) != Some(format) {
        return Err(bad(ID, "file extension must match the requested format"));
    }
    let bit_depth = match p.get("bit_depth") {
        None => BitDepth::Int24,
        Some(v) if v == 16 || v == "16" => BitDepth::Int16,
        Some(v) if v == 24 || v == "24" => BitDepth::Int24,
        Some(v) if v == "32f" => BitDepth::Float32,
        _ => return Err(bad(ID, "bit_depth must be 16, 24 or 32f")),
    };
    if format == FileFormat::Flac && bit_depth == BitDepth::Float32 {
        return Err(bad(ID, "FLAC cannot store 32-bit float"));
    }
    let dither = boolean(p, "dither", true)? && bit_depth != BitDepth::Float32;
    let options = Options { volume: boolean(p, "render_volume", true)?, pan: boolean(p, "render_pan", true)? };
    let start = position(e, p, "start")?;
    let end = position(e, p, "end")?;
    if start.is_some() != end.is_some() {
        return Err(bad(ID, "start and end must be supplied together"));
    }
    let edit = &e.session().edit;
    let timeline = if edit.link_timeline_edit { edit.selection } else { edit.timeline_selection };
    let selected = if let (Some(start), Some(end)) = (start, end) {
        Some(Range { start, end })
    } else if !timeline.is_empty() {
        Some(timeline)
    } else {
        None
    };
    if selected.is_some_and(|r| r.start < 0 || r.end <= r.start) {
        return Err(bad(ID, "range must have nonnegative start < end"));
    }
    let seconds = p.get("tail_seconds").map_or(Ok(0.0), |v| {
        v.as_f64()
            .filter(|v| v.is_finite() && (0.0..=14_400.0).contains(v))
            .ok_or_else(|| bad(ID, "tail_seconds must be finite and between 0 and 14400"))
    })?;
    let tail = e.session().sample_rate.samples(seconds);
    let mut files = Vec::new();
    let mut issues = Vec::new();
    let mut aggregate = 0usize;
    for id in tracks {
        let t = e.session().track(id).ok_or_else(|| bad(ID, "unknown track"))?;
        if t.name.len() > 4096 {
            return Err(bad(ID, "track name exceeds export metadata limit"));
        }
        let file =
            path.map_or_else(|| Path::new(dir.unwrap_or_default()).join(format!("{}.{}", stem(&t.name, id), extension(format))), PathBuf::from);
        let result = (|| {
            let range = match selected {
                Some(r) => r,
                None => Range { start: 0, end: source_end(e, id)? },
            };
            let end = range.end.checked_add(tail).filter(|v| *v <= MAX_POSITION).ok_or_else(|| bad(ID, "tail exceeds the position limit"))?;
            let render = render::plan(e.session(), id, Range { start: range.start, end }, range.end, options).map_err(|r| bad(ID, r))?;
            if format == FileFormat::Flac && (render.format.channels() > 8 || e.session().sample_rate.hz() > 655_350) {
                return Err(bad(ID, "FLAC supports at most eight channels and 655350 Hz"));
            }
            aggregate = aggregate
                .checked_add(render.memory_bytes)
                .filter(|n| *n <= render::MAX_BOUNCE_BYTES)
                .ok_or_else(|| bad(ID, "batch PCM, codec, workspace and Instrument preroll estimates exceed the 512 MiB aggregate policy budget"))?;
            crate::io::check_new_track_bounce_destination(&file)?;
            Ok(render)
        })();
        match result {
            Ok(render) => files.push(FilePlan { render, path: file, name: t.name.clone() }),
            Err(err) => issues.push(json!({"track": id, "name": t.name, "path": file, "reason": err.to_string()})),
        }
    }
    Ok(Plan { files, issues, opts: EncodeOptions { format, bit_depth, dither, bwf: None }, revision: e.revision })
}

fn manifest(e: &Engine, p: &Plan) -> Value {
    json!({"ready": p.issues.is_empty(), "revision": p.revision, "sample_rate": e.session().sample_rate.hz(), "format": p.opts.format,
        "bit_depth": p.opts.bit_depth, "dither": p.opts.dither, "issues": p.issues,
        "files": p.files.iter().map(|f| json!({"track": f.render.track, "name": f.name, "path": f.path, "layout": f.render.format,
            "channels": f.render.format.channels(), "capture_end": f.render.capture_end, "start": f.render.range.start, "end": f.render.range.end, "frames": f.render.frames,
            "processing_frames": f.render.processing_frames, "preroll_frames": f.render.processing_frames.saturating_sub(f.render.frames),
            "memory_bytes": f.render.memory_bytes})).collect::<Vec<_>>()})
}

fn preview(e: &mut Engine, p: &Value) -> Result<Value> {
    let p = plan(e, p)?;
    Ok(manifest(e, &p))
}

#[cfg(any(unix, target_arch = "wasm32"))]
fn encoded(e: &Engine, f: &FilePlan, opts: &EncodeOptions) -> Result<Vec<u8>> {
    let channels = render::render(e.session(), &f.render).map_err(|r| bad(ID, r))?;
    if channels.len() != f.render.format.channels() || channels.iter().any(|c| c.len() != f.render.frames) {
        return Err(bad(ID, "renderer did not return the complete planned shape"));
    }
    let buffer = soundcraft_audio_io::AudioBuffer { sample_rate: e.session().sample_rate.hz(), channels };
    soundcraft_audio_io::encode_with_channel_mask(&buffer, opts, f.render.format.channel_mask()).map_err(|err| EngineError::Io(err.to_string()))
}

fn bounce(e: &mut Engine, p: &Value) -> Result<Value> {
    let p = plan(e, p)?;
    if !p.issues.is_empty() {
        return Err(bad(ID, json!(p.issues).to_string()));
    }
    let mut result = manifest(e, &p);
    #[cfg(target_arch = "wasm32")]
    let sizes = {
        let f = p.files.first().ok_or_else(|| bad(ID, "no source files"))?;
        let bytes = encoded(e, f, &p.opts)?;
        crate::io::write_file(&f.path.to_string_lossy(), &bytes)?;
        vec![bytes.len()]
    };
    #[cfg(all(unix, not(target_arch = "wasm32")))]
    let sizes = {
        let paths: Vec<_> = p.files.iter().map(|f| f.path.clone()).collect();
        crate::io::track_bounce_files::deliver(&paths, |index| {
            let file = p.files.get(index).ok_or_else(|| bad(ID, "missing planned file"))?;
            encoded(e, file, &p.opts)
        })?
    };
    #[cfg(all(not(unix), not(target_arch = "wasm32")))]
    let sizes = {
        fn unsupported() -> Result<Vec<usize>> {
            Err(bad(ID, "native Track Bounce publication is unsupported on this platform"))
        }
        unsupported()?
    };
    if let Some(files) = result.get_mut("files").and_then(Value::as_array_mut) {
        for (f, size) in files.iter_mut().zip(sizes) {
            f["bytes"] = json!(size);
        }
    }
    result["delivered"] = json!(true);
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_preroll_saturates_if_processing_frames_are_below_delivered_frames() {
        let e = Engine::default();
        // The renderer normally guarantees processing >= delivered. Keep defensive manifest
        // arithmetic safe even if a future planner supplies a smaller processing count.
        let plan = Plan {
            files: vec![FilePlan {
                render: render::Plan {
                    track: TrackId(1),
                    range: Range::new(0, 2),
                    capture_end: 2,
                    format: soundcraft_model::ChannelFormat::Mono,
                    frames: 2,
                    processing_frames: 1,
                    memory_bytes: 0,
                    options: Options { volume: true, pan: false },
                },
                path: PathBuf::from("unused.wav"),
                name: "Manifest arithmetic".into(),
            }],
            issues: Vec::new(),
            opts: EncodeOptions::default(),
            revision: e.revision,
        };
        let value = manifest(&e, &plan);
        assert_eq!(value["files"][0]["frames"], 2);
        assert_eq!(value["files"][0]["processing_frames"], 1);
        assert_eq!(value["files"][0]["preroll_frames"], 0);
    }
}
