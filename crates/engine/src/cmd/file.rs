//! File menu commands.

use super::*;
use crate::cmd;
use serde_json::json;
use soundcraft_model::Session;
use soundcraft_time::SampleRate;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(noundo "session.new", "New...", ["File"], Some("Cmd+N"), "{name?: 'Untitled', sample_rate?: 48000, bit_depth?: 24, template?: blank|demo}", always, new_session),
        cmd!(noundo "session.open", "Open Session...", ["File"], Some("Cmd+O"), "{path}", always, |e, p| {
            let path = str_param(p, "path").ok_or_else(|| bad("session.open", "`path` required"))?.to_string();
            let missing = crate::io::open_session(e, &path)?;
            if !missing.is_empty() {
                for m in &missing {
                    e.message(format!("missing media: {m}"));
                }
            }
            Ok(json!({"name": e.session().name, "tracks": e.session().tracks.len(), "missing_files": missing}))
        }),
        cmd!(noundo "session.close", "Close Session", ["File"], Some("Cmd+Shift+W"), "{}", always, |e, _| { e.replace_session(Session::default()); e.path = None; Ok(json!({})) }),
        cmd!(noundo "session.save", "Save Session", ["File"], Some("Cmd+S"), "{path?}", always, |e, p| {
            let path = str_param(p, "path").map(str::to_string).or_else(|| e.path.clone()).ok_or_else(|| bad("session.save", "no path yet: pass `path` (Save As)"))?;
            let written = crate::io::save_session(e, &path)?;
            Ok(json!({"path": path, "audio_files_written": written}))
        }),
        cmd!(noundo "session.save_as", "Save Session As...", ["File"], Some("Cmd+Shift+S"), "{path}", always, |e, p| {
            let path = str_param(p, "path").ok_or_else(|| bad("session.save_as", "`path` required"))?.to_string();
            let written = crate::io::save_session(e, &path)?;
            Ok(json!({"path": path, "audio_files_written": written}))
        }),
        cmd!(noundo "session.save_copy", "Save Session Copy In...", ["File"], None, "{path}", always, |e, p| {
            let path = str_param(p, "path").ok_or_else(|| bad("session.save_copy", "`path` required"))?.to_string();
            let keep = e.path.clone();
            let written = crate::io::save_session(e, &path)?;
            e.path = keep;
            Ok(json!({"path": path, "audio_files_written": written}))
        }),
        cmd!(noundo "session.save_template", "Save As Template...", ["File"], None, "{path}", always, |e, p| {
            let path = str_param(p, "path").ok_or_else(|| bad("session.save_template", "`path` required"))?.to_string();
            let mut s = e.session().clone();
            for t in &mut s.tracks { for pl in &mut t.playlists { pl.clips.clear() } }
            s.sources.clear();
            s.videos.clear();
            let text = s.to_json().map_err(|err| EngineError::Io(err.to_string()))?;
            std::fs::write(&path, text).map_err(|err| EngineError::Io(format!("{path}: {err}")))?;
            Ok(json!({"path": path}))
        }),
        cmd!(noundo "session.revert", "Revert Session to Saved...", ["File"], None, "{}", always, |e, _| {
            let path = e.path.clone().ok_or_else(|| bad("session.revert", "the session has not been saved"))?;
            crate::io::open_session(e, &path)?;
            Ok(json!({}))
        }),
        cmd!(
            "file.import_audio",
            "Audio...",
            ["File", "Import"],
            Some("Cmd+Shift+I"),
            "{path | paths: [..], track?: target track, at?: position, new_tracks?: true}",
            always,
            import_audio
        ),
        cmd!("file.import_midi", "MIDI...", ["File", "Import"], Some("Cmd+Alt+I"), "{path, at?}", always, |e, p| {
            let path = str_param(p, "path").ok_or_else(|| bad("file.import_midi", "`path` required"))?.to_string();
            let at = position_param(e, "file.import_midi", p, "at")?.unwrap_or(0);
            let bytes = std::fs::read(&path).map_err(|err| EngineError::Io(format!("{path}: {err}")))?;
            let tracks = crate::io::import_midi_bytes(e, &bytes, at, bool_or(p, "tempo_map", true))?;
            Ok(json!({"tracks": tracks}))
        }),
        cmd!(
            "file.import_video",
            "Video...",
            ["File", "Import"],
            Some("Cmd+Ctrl+I"),
            "{path, at?} — adds a Video track clip for the movie's picture (H.264, ProRes, Motion JPEG) and imports its audio onto a new track",
            always,
            |e, p| {
                let path = str_param(p, "path").ok_or_else(|| bad("file.import_video", "`path` required"))?.to_string();
                let at = position_param(e, "file.import_video", p, "at")?.unwrap_or(0).max(0);
                let bytes = std::fs::read(&path).map_err(|err| EngineError::Io(format!("{path}: {err}")))?;
                crate::io::import_video_bytes(e, &path, bytes, at)
            }
        ),
        cmd!(noundo "file.bounce_mix", "Bounce Mix...", ["File"], Some("Cmd+Alt+B"), "{path, format?: wav|aiff|flac, bit_depth?: 16|24|32f, start?, end?, source?: main|bus name, normalize?: false, dither?: true, fold_down?: stereo}", always, bounce),
        cmd!(noundo "file.bounce_stems", "Bounce Stems...", [], None, "{dir, format?: wav|aiff|flac, bit_depth?: 16|24|32f, start?, end?, fold_down?: stereo}", has_tracks, |e, p| {
            let dir = str_param(p, "dir").ok_or_else(|| bad("file.bounce_stems", "`dir` required"))?.to_string();
            let r = range_param(e, "file.bounce_stems", p)?;
            let r = if r.is_empty() { soundcraft_time::Range::new(0, e.session().content_end().max(1)) } else { r };
            let format = match str_param(p, "format") {
                Some("aiff") | Some("aif") => soundcraft_audio_io::FileFormat::Aiff,
                Some("flac") => soundcraft_audio_io::FileFormat::Flac,
                _ => soundcraft_audio_io::FileFormat::Wav,
            };
            let bit_depth = match p.get("bit_depth").map(|b| b.to_string().trim_matches('"').to_string()).as_deref() {
                Some("16") => soundcraft_audio_io::BitDepth::Int16,
                Some("32") | Some("32f") => soundcraft_audio_io::BitDepth::Float32,
                _ => soundcraft_audio_io::BitDepth::Int24,
            };
            let fold = fold_down_param(p, "file.bounce_stems")?;
            let files = crate::io::bounce_stems_with(e, &dir, r, &soundcraft_audio_io::EncodeOptions { format, bit_depth, dither: true, bwf: None }, fold)?;
            Ok(json!({"files": files}))
        }),
        cmd!(noundo "file.export_midi", "MIDI...", ["File", "Export"], None, "{path, tracks?}", always, |e, p| {
            let path = str_param(p, "path").ok_or_else(|| bad("file.export_midi", "`path` required"))?.to_string();
            let bytes = crate::io::export_midi(e)?;
            std::fs::write(&path, &bytes).map_err(|err| EngineError::Io(format!("{path}: {err}")))?;
            Ok(json!({"path": path, "bytes": bytes.len()}))
        }),
        cmd!(noundo "file.export_session_text", "Session Info as Text...", ["File", "Export"], None, "{path?}", always, |e, p| {
            let text = crate::inspect::session_text(e);
            if let Some(path) = str_param(p, "path") {
                std::fs::write(path, &text).map_err(|err| EngineError::Io(format!("{path}: {err}")))?;
            }
            Ok(json!({"text": text}))
        }),
        cmd!(noundo "file.export_clips", "Export Clips as Files...", [], None, "{dir, clips?, format?: wav}", always, |e, p| {
            let dir = str_param(p, "dir").ok_or_else(|| bad("file.export_clips", "`dir` required"))?.to_string();
            let ids = clip_ids_param(e, p);
            let n = crate::io::export_clips(e, &ids, &dir)?;
            Ok(json!({"written": n}))
        }),
        cmd!(noundo "file.export_selected_tracks_as_session", "Selected Tracks as New Session...", ["File", "Export"], None, "{path, tracks?}", has_selection, |e, p| {
            let path = str_param(p, "path").ok_or_else(|| bad("file.export_selected_tracks_as_session", "`path` required"))?.to_string();
            let keep = tracks_required(e, "file.export_selected_tracks_as_session", p)?;
            let mut s = e.session().clone();
            s.tracks.retain(|t| keep.contains(&t.id) || t.kind == soundcraft_model::TrackKind::Master);
            let mut tmp = crate::Engine::new(s);
            crate::io::save_session(&mut tmp, &path)?;
            Ok(json!({"path": path}))
        }),
        cmd!(noundo "file.export_sibelius", "Sibelius...", ["File", "Export"], None, "{path, tracks?} — MIDI tracks as MusicXML (.musicxml)", always, |e, p| {
            write_score(e, p, "file.export_sibelius", false)
        }),
        cmd!(noundo "file.send_to_sibelius", "Sibelius", ["File", "Send To..."], None, "{path?, tracks?} — writes MusicXML for a notation program (to the temp folder when no `path`)", always, |e, p| {
            write_score(e, p, "file.send_to_sibelius", false)
        }),
        cmd!(noundo "file.print_score", "Print Score...", ["File"], Some("Cmd+P"), "{path?, tracks?} — engraves the MIDI tracks as a printable SVG page", always, |e, p| {
            write_score(e, p, "file.print_score", true)
        }),
        cmd!(
            "file.score_setup",
            "Score Setup...",
            ["File"],
            None,
            "{title?, composer?, bars_per_system?: 1..16, show_track_names?: bool} — returns the current setup",
            always,
            |e, p| {
                let mut setup = crate::score::ScoreSetup::from_session(e.session());
                if let Some(t) = str_param(p, "title") {
                    setup.title = t.to_string();
                }
                if let Some(c) = str_param(p, "composer") {
                    setup.composer = c.to_string();
                }
                if let Some(b) = p.get("bars_per_system").and_then(serde_json::Value::as_f64) {
                    setup.bars_per_system = if b.is_finite() { b.clamp(1.0, 16.0) as u32 } else { 4 };
                }
                if let Some(b) = p.get("show_track_names").and_then(serde_json::Value::as_bool) {
                    setup.show_track_names = b;
                }
                setup.store(e.session_mut());
                Ok(
                    json!({"title": setup.title, "composer": setup.composer, "bars_per_system": setup.bars_per_system, "show_track_names": setup.show_track_names}),
                )
            }
        ),
        cmd!(noundo "file.export_clip_groups", "Export Clip Groups...", [], None, "{path, clips?} — writes a .scgrp clip group file with its audio", always, |e, p| {
            let path = str_param(p, "path").ok_or_else(|| bad("file.export_clip_groups", "`path` required"))?.to_string();
            let ids = clip_ids_param(e, p);
            let n = crate::clip_group_file::export(e, &ids, &path)?;
            Ok(json!({"path": path, "clips": n}))
        }),
        cmd!("file.import_clip_groups", "Clip Groups...", ["File", "Import"], None, "{path, track?: first target track, at?}", always, |e, p| {
            let id = "file.import_clip_groups";
            let path = str_param(p, "path").ok_or_else(|| bad(id, "`path` required"))?.to_string();
            let at = position_param(e, id, p, "at")?.unwrap_or(e.session().edit.selection.start).max(0);
            let first = tracks_param(e, id, p)?.first().copied();
            let placed = crate::clip_group_file::import(e, &path, first, at)?;
            e.session_mut().edit.selected_clips.clone_from(&placed);
            Ok(json!({"clips": placed.iter().map(|c| c.0).collect::<Vec<_>>()}))
        }),
        cmd!(query "file.get_info", "Get Info...", ["File"], None, "{}", always, |e, _| Ok(crate::inspect::session(e, false))),
        cmd!(noundo "app.quit", "Quit", [], Some("Cmd+Q"), "{}", always, |_, _| Ok(json!({"quit": true}))),
    ]
}

fn new_session(e: &mut Engine, p: &Value) -> Result<Value> {
    let name = str_param(p, "name").unwrap_or("Untitled").to_string();
    let sr =
        SampleRate::new(i64_or(p, "sample_rate", 48_000).clamp(0, i64::from(u32::MAX)) as u32).map_err(|err| bad("session.new", err.to_string()))?;
    let mut s = if str_param(p, "template") == Some("demo") { crate::demo::demo_session() } else { Session::new(name.clone(), sr) };
    s.name = name;
    if let Some(b) = p.get("bit_depth") {
        s.bit_depth = match b.to_string().trim_matches('"') {
            "16" => soundcraft_model::BitDepthSetting::Int16,
            "32" | "32f" => soundcraft_model::BitDepthSetting::Float32,
            _ => soundcraft_model::BitDepthSetting::Int24,
        };
    }
    e.replace_session(s);
    e.path = None;
    Ok(json!({"name": e.session().name, "sample_rate": e.session().sample_rate.hz()}))
}

fn import_audio(e: &mut Engine, p: &Value) -> Result<Value> {
    let mut paths: Vec<String> =
        p.get("paths").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect()).unwrap_or_default();
    if let Some(one) = str_param(p, "path") {
        paths.push(one.to_string());
    }
    if paths.is_empty() {
        return Err(bad("file.import_audio", "`path` or `paths` required"));
    }
    let at = position_param(e, "file.import_audio", p, "at")?.unwrap_or(0).max(0);
    let target = track_param(e, "file.import_audio", p, "track")?;
    let mut out = Vec::new();
    for path in &paths {
        let bytes = std::fs::read(path).map_err(|err| EngineError::Io(format!("{path}: {err}")))?;
        let name = std::path::Path::new(path).file_name().and_then(|n| n.to_str()).unwrap_or("Audio").to_string();
        let r = crate::io::import_audio_bytes(e, &name, &bytes, Some(path.as_str()), target, at)?;
        out.push(r);
    }
    Ok(json!({"imported": out}))
}

/// `fold_down: "stereo"` bounces an ITU stereo fold-down of a surround mix.
fn fold_down_param(p: &Value, id: &str) -> Result<bool> {
    match str_param(p, "fold_down").map(str::trim) {
        None | Some("") | Some("none") => Ok(false),
        Some(f) if f.eq_ignore_ascii_case("stereo") => Ok(true),
        Some(f) => Err(bad(id, format!("unknown fold_down `{f}` (use \"stereo\")"))),
    }
}

fn bounce(e: &mut Engine, p: &Value) -> Result<Value> {
    let path = str_param(p, "path").ok_or_else(|| bad("file.bounce_mix", "`path` required"))?.to_string();
    let r = range_param(e, "file.bounce_mix", p)?;
    let r = if r.is_empty() { soundcraft_time::Range::new(0, e.session().content_end().max(1)) } else { r };
    let source = str_param(p, "source").map(|s| s.to_string());
    let ext = std::path::Path::new(&path).extension().and_then(|x| x.to_str()).unwrap_or("wav").to_ascii_lowercase();
    let format = match str_param(p, "format").unwrap_or(ext.as_str()) {
        "aif" | "aiff" => soundcraft_audio_io::FileFormat::Aiff,
        "flac" => soundcraft_audio_io::FileFormat::Flac,
        _ => soundcraft_audio_io::FileFormat::Wav,
    };
    let bit_depth = match p.get("bit_depth").map(|b| b.to_string().trim_matches('"').to_string()).as_deref() {
        Some("16") => soundcraft_audio_io::BitDepth::Int16,
        Some("32") | Some("32f") => soundcraft_audio_io::BitDepth::Float32,
        _ => soundcraft_audio_io::BitDepth::Int24,
    };
    let opts = soundcraft_audio_io::EncodeOptions { format, bit_depth, dither: bool_or(p, "dither", true), bwf: None };
    let normalize = bool_or(p, "normalize", false);
    let fold = fold_down_param(p, "file.bounce_mix")?;
    let (bytes, stats) = crate::io::bounce_bytes_with(e, r, &opts, normalize, fold, source)?;
    std::fs::write(&path, &bytes).map_err(|err| EngineError::Io(format!("{path}: {err}")))?;
    Ok(
        json!({"path": path, "bytes": bytes.len(), "seconds": e.session().sample_rate.seconds(r.len()), "peak_db": stats.0, "lufs": stats.1, "true_peak_db": stats.2}),
    )
}

/// Write the score of the chosen MIDI tracks as MusicXML (or SVG when `svg`).
fn write_score(e: &mut Engine, p: &Value, id: &str, svg: bool) -> Result<Value> {
    let tracks = if p.get("tracks").is_some() || p.get("track").is_some() { tracks_param(e, id, p)? } else { Vec::new() };
    let parts = crate::score::layout(e.session(), &tracks);
    if parts.is_empty() {
        return Err(bad(id, "no MIDI or instrument tracks to notate"));
    }
    let setup = crate::score::ScoreSetup::from_session(e.session());
    let (text, ext) = if svg { (crate::score::svg(&parts, &setup), "svg") } else { (crate::score::musicxml(&parts, &setup), "musicxml") };
    let path = match str_param(p, "path") {
        Some(p) => p.to_string(),
        None => std::env::temp_dir().join(format!("{}.{ext}", crate::io::sanitize_name(&setup.title))).to_string_lossy().to_string(),
    };
    std::fs::write(&path, &text).map_err(|err| EngineError::Io(format!("{path}: {err}")))?;
    let measures = parts.iter().map(|p| p.measures.len()).max().unwrap_or(0);
    Ok(json!({"path": path, "parts": parts.len(), "measures": measures}))
}
