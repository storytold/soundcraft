//! File menu commands.
use super::{
    CommandSpec, always, bad, bool_or, clip_ids_param, has_selection, has_tracks, i64_or, position_param, range_param, str_param, track_param,
    tracks_param, tracks_required,
};

use crate::{Engine, EngineError, Result, cmd};
use serde_json::{Value, json};
use soundcraft_model::{Session, TrackId, TrackKind};
use soundcraft_time::SampleRate;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(noundo "session.new", "New...", ["File"], Some("Cmd+N"), "{name?: 'Untitled', sample_rate?: 48000, bit_depth?: 24, template?: blank|demo}", always, new_session),
        cmd!(noundo "session.open", "Open Session...", ["File"], Some("Cmd+O"), "{path}", always, |e, p| {
            let path = str_param(p, "path").ok_or_else(|| bad("session.open", "`path` required"))?.to_string();
            let missing = crate::io::open_session(e, &path)?;
            Ok(json!({"name": e.session().name, "tracks": e.session().tracks.len(), "missing": missing}))
        }),
        cmd!(noundo "session.close", "Close Session", ["File"], Some("Cmd+Shift+W"), "{}", always, |e, _| { e.replace_session(Session::default()); e.path = None; Ok(json!({})) }),
        cmd!(noundo "session.save", "Save Session", ["File"], Some("Cmd+S"), "{path?, replace?: false}", always, |e, p| {
            let path = str_param(p, "path").map(str::to_string).or_else(|| e.path.clone()).ok_or_else(|| bad("session.save", "no path yet: pass `path` (Save As)"))?;
            refuse_replacing(e, "session.save", &crate::io::session_file(&path), p)?;
            let written = crate::io::save_session(e, &path)?;
            Ok(json!({"path": path, "audio_files_written": written}))
        }),
        cmd!(noundo "session.save_as", "Save Session As...", ["File"], Some("Cmd+Shift+S"), "{path, replace?: false}", always, |e, p| {
            let path = str_param(p, "path").ok_or_else(|| bad("session.save_as", "`path` required"))?.to_string();
            refuse_replacing(e, "session.save_as", &crate::io::session_file(&path), p)?;
            let written = crate::io::save_session(e, &path)?;
            Ok(json!({"path": path, "audio_files_written": written}))
        }),
        cmd!(noundo "session.save_copy", "Save Session Copy In...", ["File"], None, "{path, replace?: false}", always, |e, p| {
            let path = str_param(p, "path").ok_or_else(|| bad("session.save_copy", "`path` required"))?.to_string();
            refuse_replacing(e, "session.save_copy", &crate::io::session_file(&path), p)?;
            let written = crate::io::save_session_copy(e, &path)?;
            Ok(json!({"path": path, "audio_files_written": written}))
        }),
        cmd!(noundo "session.save_template", "Save As Template...", ["File"], None, "{path, replace?: false}", always, |e, p| {
            let path = str_param(p, "path").ok_or_else(|| bad("session.save_template", "`path` required"))?.to_string();
            let file = crate::io::session_file(&path);
            refuse_replacing(e, "session.save_template", &file, p)?;
            let mut s = e.session().clone();
            for t in &mut s.tracks { for pl in &mut t.playlists { pl.clips.clear() } }
            s.sources.clear();
            s.videos.clear();
            let text = s.to_json().map_err(|err| EngineError::Io(err.to_string()))?;
            // A template too large to open is not saved (#892).
            crate::io::session_size(file.display(), text.len() as u64, true)?;
            if cfg!(target_arch = "wasm32") {
                crate::io::write_file(&file.to_string_lossy(), text.as_bytes())?;
            } else {
                crate::io::replace_file(&file.with_extension("scraft.tmp"), &file, text.as_bytes())?;
            }
            Ok(json!({"path": file}))
        }),
        cmd!(noundo "session.revert", "Revert Session to Saved...", ["File"], None, "{}", |e| if e.path.is_some() { Ok(()) } else { Err("the session has not been saved".into()) }, |e, _| {
            let path = e.path.clone().ok_or_else(|| bad("session.revert", "the session has not been saved"))?;
            crate::io::open_session(e, &path)?;
            Ok(json!({}))
        }),
        cmd!(
            "file.import_audio",
            "Audio...",
            ["File", "Import"],
            Some("Cmd+Shift+I"),
            "{path | paths: [..], track?: target track, at?: position, new_tracks?: true (default; false uses `track` or the selected audio track)}",
            always,
            import_audio
        ),
        cmd!("file.import_midi", "MIDI...", ["File", "Import"], Some("Cmd+Alt+I"), "{path, at?}", always, |e, p| {
            let path = str_param(p, "path").ok_or_else(|| bad("file.import_midi", "`path` required"))?.to_string();
            let at = position_param(e, "file.import_midi", p, "at")?.unwrap_or(0);
            let bytes = crate::io::read_file(&path)?;
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
                let bytes = crate::io::read_file(&path)?;
                crate::io::import_video_bytes(e, &path, bytes, at)
            }
        ),
        cmd!(noundo "file.bounce_mix", "Bounce Mix...", ["File"], Some("Cmd+Alt+B"), "{path, format?: wav|aiff|flac, bit_depth?: 16|24|32f, start?, end?, source?: main|bus name, normalize?: false (peak to -0.1 dBFS), target_lufs?: integrated loudness to normalize to (-70..0), true_peak_ceiling_db?: -1 (with target_lufs, -30..0; the gain is lowered to keep the true peak under it), dither?: true, fold_down?: stereo} → lufs and true_peak_db as written; with target_lufs also gain_db and a warning when the ceiling or silence kept it from the target", always, bounce),
        cmd!(noundo "file.bounce_stems", "Bounce Stems...", [], None, "{dir, format?: wav|aiff|flac, bit_depth?: 16|24|32f, start?, end?, dither?: true, fold_down?: stereo}", has_tracks, |e, p| {
            let dir = str_param(p, "dir").ok_or_else(|| bad("file.bounce_stems", "`dir` required"))?.to_string();
            let r = range_param(e, "file.bounce_stems", p)?;
            let r = if r.is_empty() { soundcraft_time::Range::new(0, e.session().content_end().max(1)) } else { r };
            let format_name = match p.get("format") {
                None => "wav",
                Some(value) => value.as_str().ok_or_else(|| bad("file.bounce_stems", "`format` must be a string"))?,
            };
            let format = craft_audio::encode_format_for(format_name).ok_or_else(|| {
                let supported = craft_audio::ENCODE_EXTENSIONS.join(", ");
                bad("file.bounce_stems", format!("cannot write `{format_name}` files (supported: {supported})"))
            })?;
            let bit_depth = match p.get("bit_depth").map(|b| b.to_string().trim_matches('"').to_string()).as_deref() {
                Some("16") => craft_audio::BitDepth::Int16,
                Some("32") | Some("32f") => craft_audio::BitDepth::Float32,
                _ => craft_audio::BitDepth::Int24,
            };
            let fold = fold_down_param(p, "file.bounce_stems")?;
            let files = crate::io::bounce_stems_with(e, &dir, r, &craft_audio::EncodeOptions::new(format).with_bit_depth(bit_depth).with_dither(bool_or(p, "dither", true)), fold)?;
            Ok(json!({"files": files}))
        }),
        cmd!(noundo "file.export_midi", "MIDI...", ["File", "Export"], None, "{path, tracks?}", always, |e, p| {
            let path = str_param(p, "path").ok_or_else(|| bad("file.export_midi", "`path` required"))?.to_string();
            let only = if p.get("tracks").is_some() || p.get("track").is_some() { Some(tracks_param(e, "file.export_midi", p)?) } else { None };
            let bytes = crate::io::export_midi(e, only.as_deref())?;
            crate::io::write_file(&path, &bytes)?;
            Ok(json!({"path": path, "bytes": bytes.len()}))
        }),
        cmd!(noundo "file.export_session_text", "Session Info as Text...", ["File", "Export"], None, "{path?, replace?: false}", always, |e, p| {
            let text = crate::inspect::session_text(e);
            if let Some(path) = str_param(p, "path") {
                refuse_replacing(e, "file.export_session_text", std::path::Path::new(path), p)?;
                crate::io::write_file(path, text.as_bytes())?;
            }
            Ok(json!({"text": text}))
        }),
        cmd!(noundo "file.export_clips", "Export Clips as Files...", [], Some("Cmd+Shift+K"), "{dir, clips?, format?: wav}", always, |e, p| {
            let dir = str_param(p, "dir").ok_or_else(|| bad("file.export_clips", "`dir` required"))?.to_string();
            let ids = clip_ids_param(e, p);
            if ids.is_empty() {
                return Err(bad("file.export_clips", "no clips selected"));
            }
            for (t, c) in ids.iter().filter_map(|id| e.session().find_clip(*id)) {
                let channels = e.session().track(t).map_or(1, |x| x.channels());
                crate::io::refuse_longer(e.session(), "file.export_clips", &c.name, "export", c.length, crate::io::render_limit(channels))?;
            }
            let n = crate::io::export_clips(e, &ids, &dir)?;
            Ok(json!({"written": n, "dir": dir}))
        }),
        cmd!(noundo "file.export_selected_tracks_as_session", "Selected Tracks as New Session...", ["File", "Export"], None, "{path, tracks?, replace?: false}", has_selection, |e, p| {
            let path = str_param(p, "path").ok_or_else(|| bad("file.export_selected_tracks_as_session", "`path` required"))?.to_string();
            refuse_replacing(e, "file.export_selected_tracks_as_session", &crate::io::session_file(&path), p)?;
            let keep = tracks_required(e, "file.export_selected_tracks_as_session", p)?;
            let mut s = e.session().clone();
            s.tracks.retain(|t| keep.contains(&t.id) || t.kind == soundcraft_model::TrackKind::Master);
            let mut tmp = crate::Engine::new(s);
            crate::io::save_session(&mut tmp, &path)?;
            Ok(json!({"path": path}))
        }),
        cmd!(noundo "file.export_sibelius", "MusicXML...", ["File", "Export"], None, "{path, tracks?} — MIDI tracks as MusicXML (.musicxml)", always, |e, p| {
            write_score(e, p, "file.export_sibelius", false)
        }),
        cmd!(noundo "file.send_to_sibelius", "MusicXML", ["File", "Send To..."], None, "{path?, tracks?} — writes MusicXML for a notation program (to the temp folder when no `path`)", always, |e, p| {
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

/// Writing a session over an existing file needs `replace: true` (see `io::replaces`).
fn refuse_replacing(e: &Engine, id: &str, file: &std::path::Path, p: &Value) -> Result<()> {
    // First, so a path outside the write root is refused as such and never reported as existing.
    crate::file_access::check_write(file).map_err(EngineError::Io)?;
    if crate::io::replaces(e, id, file) && !bool_or(p, "replace", false) {
        return Err(bad(id, format!("{} already exists: pass `replace: true` to replace it", file.display())));
    }
    Ok(())
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
    let target = import_audio_target(e, p)?;
    let mut out = Vec::new();
    for path in &paths {
        let name = std::path::Path::new(path).file_name().and_then(std::ffi::OsStr::to_str).unwrap_or("Audio").to_string();
        // Audio the app decoded on a worker thread (`Engine::offer_import`) is only placed here;
        // the path is checked the same way either way.
        crate::file_access::check_read(std::path::Path::new(path)).map_err(crate::EngineError::Io)?;
        let r = match e.take_import(std::path::Path::new(path)) {
            Some(d) => crate::io::place_decoded(e, &name, d, Some(path.as_str()), target, at)?,
            None => {
                let bytes = crate::io::read_file(path)?;
                crate::io::import_audio_bytes(e, &name, &bytes, Some(path.as_str()), target, at)?
            }
        };
        out.push(r);
    }
    Ok(json!({"imported": out}))
}

/// Destination for `file.import_audio`: an explicit `track` wins; otherwise `new_tracks`
/// (default true) creates a new track per file, and `new_tracks: false` uses the selected
/// audio track or errors when none is selected.
fn import_audio_target(e: &Engine, p: &Value) -> Result<Option<TrackId>> {
    if let Some(t) = track_param(e, "file.import_audio", p, "track")? {
        return Ok(Some(t));
    }
    if bool_or(p, "new_tracks", true) {
        return Ok(None);
    }
    let s = e.session();
    let selected = s.edit.selected_tracks.iter().find_map(|id| s.track(*id).filter(|t| t.kind == TrackKind::Audio).map(|t| t.id));
    selected.map(Some).ok_or_else(|| bad("file.import_audio", "`new_tracks: false` needs `track` or a selected audio track"))
}

/// `source` omitted or `main` is the main mix. Any other name is a bus, and an unknown bus is an error.
fn bounce_bus(e: &Engine, p: &Value) -> Result<Option<soundcraft_model::BusId>> {
    let Some(name) = str_param(p, "source").map(str::trim).filter(|s| !s.is_empty()) else {
        return Ok(None);
    };
    if name.eq_ignore_ascii_case("main") {
        return Ok(None);
    }
    e.session().bus_by_name(name).map(|b| Some(b.id)).ok_or_else(|| bad("file.bounce_mix", format!("unknown bus `{name}`")))
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
    let job = bounce_job(e, p)?;
    let (path, seconds) = (job.path().to_string_lossy().into_owned(), e.session().sample_rate.seconds(job.range().len()));
    let target = loudness_target(p)?;
    let done = job.run()?;
    let mut out = json!({
        "path": path,
        "bytes": done.bytes,
        "seconds": seconds,
        "peak_db": done.peak_db,
        "lufs": done.lufs,
        "true_peak_db": done.true_peak_db,
    });
    if let (Some(t), Some(outcome), Some(o)) = (target, done.loudness, out.as_object_mut()) {
        o.insert("gain_db".into(), json!(done.gain_db));
        let warning = match outcome {
            crate::io::LoudnessOutcome::Reached => None,
            crate::io::LoudnessOutcome::CeilingHeld => Some(format!(
                "the {} dBTP true-peak ceiling held the gain back: {:.1} LUFS, short of the {} LUFS target",
                t.true_peak_ceiling_db, done.lufs, t.lufs
            )),
            crate::io::LoudnessOutcome::Unmeasured => Some(unmeasured("silent, or too short to measure loudness (under 400 ms)", done.gain_db, t)),
            crate::io::LoudnessOutcome::TooQuiet => {
                Some(unmeasured("too quiet to measure loudness (every 400 ms block is under the -70 LUFS gate)", done.gain_db, t))
            }
            crate::io::LoudnessOutcome::Missed if done.lufs <= soundcraft_dsp::MIN_DB => Some(format!(
                "the gain took every 400 ms block under the -70 LUFS gate, so the file's loudness cannot be measured (target {} LUFS)",
                t.lufs
            )),
            crate::io::LoudnessOutcome::Missed => Some(format!(
                "{:.1} LUFS, not the {} LUFS target: the gain took the quieter parts under the -70 LUFS gate, so they no longer count",
                done.lufs, t.lufs
            )),
        };
        if let Some(w) = warning {
            o.insert("warning".into(), json!(w));
        }
    }
    Ok(out)
}

/// The warning for a bounce whose loudness could not be measured: not normalized, but lowered to the
/// true-peak ceiling when it was over it.
fn unmeasured(why: &str, gain_db: f32, t: crate::io::LoudnessTarget) -> String {
    if gain_db < 0.0 {
        format!("{why}: not normalized, but lowered {:.1} dB to the {} dBTP true-peak ceiling", -gain_db, t.true_peak_ceiling_db)
    } else {
        format!("{why}: written at unity gain")
    }
}

/// `target_lufs` and `true_peak_ceiling_db` (default -1 dBTP), when a loudness was asked for.
fn loudness_target(p: &Value) -> Result<Option<crate::io::LoudnessTarget>> {
    let num = |k: &str| {
        p.get(k)
            .filter(|v| !v.is_null())
            .map(|v| v.as_f64().filter(|x| x.is_finite()).ok_or_else(|| bad("file.bounce_mix", format!("`{k}` must be a number"))))
    };
    let Some(lufs) = num("target_lufs").transpose()? else {
        if p.get("true_peak_ceiling_db").is_some_and(|v| !v.is_null()) {
            return Err(bad("file.bounce_mix", "`true_peak_ceiling_db` goes with `target_lufs`"));
        }
        return Ok(None);
    };
    let ceiling = num("true_peak_ceiling_db").transpose()?.unwrap_or(-1.0);
    if !(-70.0..=0.0).contains(&lufs) {
        return Err(bad("file.bounce_mix", "`target_lufs` must be between -70 and 0"));
    }
    if !(-30.0..=0.0).contains(&ceiling) {
        return Err(bad("file.bounce_mix", "`true_peak_ceiling_db` must be between -30 and 0"));
    }
    if bool_or(p, "normalize", false) {
        return Err(bad("file.bounce_mix", "`normalize` (peak) and `target_lufs` (loudness) can't be used together"));
    }
    Ok(Some(crate::io::LoudnessTarget { lufs: lufs as f32, true_peak_ceiling_db: ceiling as f32 }))
}

/// The bounce `file.bounce_mix` runs for `p`, not rendered yet.
pub fn bounce_job(e: &Engine, p: &Value) -> Result<crate::io::Bounce> {
    let path = str_param(p, "path").ok_or_else(|| bad("file.bounce_mix", "`path` required"))?.to_string();
    let r = range_param(e, "file.bounce_mix", p)?;
    let r = if r.is_empty() { soundcraft_time::Range::new(0, e.session().content_end().max(1)) } else { r };
    let ext = std::path::Path::new(&path).extension().and_then(std::ffi::OsStr::to_str).unwrap_or("wav").to_ascii_lowercase();
    let format_name = str_param(p, "format").unwrap_or(ext.as_str());
    let format = craft_audio::encode_format_for(format_name).ok_or_else(|| {
        let supported = craft_audio::ENCODE_EXTENSIONS.join(", ");
        bad("file.bounce_mix", format!("cannot write `{format_name}` files (supported: {supported})"))
    })?;
    let bit_depth = match p.get("bit_depth").map(|b| b.to_string().trim_matches('"').to_string()).as_deref() {
        Some("16") => craft_audio::BitDepth::Int16,
        Some("32") | Some("32f") => craft_audio::BitDepth::Float32,
        _ => craft_audio::BitDepth::Int24,
    };
    let opts = craft_audio::EncodeOptions::new(format).with_bit_depth(bit_depth).with_dither(bool_or(p, "dither", true));
    let normalize = bool_or(p, "normalize", false);
    let fold = fold_down_param(p, "file.bounce_mix")?;
    let bus = bounce_bus(e, p)?;
    let target = loudness_target(p)?;
    let job = crate::io::Bounce::new(e.session_arc(), std::path::Path::new(&path), r, &opts, bus, normalize, fold)?;
    Ok(match target {
        Some(t) => job.with_loudness(t),
        None => job,
    })
}

/// Write the score of the chosen MIDI tracks as MusicXML (or SVG when `svg`).
fn write_score(e: &mut Engine, p: &Value, id: &str, svg: bool) -> Result<Value> {
    let tracks = if p.get("tracks").is_some() || p.get("track").is_some() { tracks_param(e, id, p)? } else { Vec::new() };
    let parts = crate::score::layout(e.session(), &tracks);
    if parts.is_empty() {
        return Err(bad(id, "no MIDI or instrument tracks to notate"));
    }
    let setup = crate::score::ScoreSetup::from_session(e.session());
    let (text, ext) = if svg {
        (crate::score::svg(&parts, &setup).map_err(|m| bad(id, m))?, "svg")
    } else {
        (crate::score::musicxml(&parts, &setup).map_err(|m| bad(id, m))?, "musicxml")
    };
    let path = match str_param(p, "path") {
        Some(p) => p.to_string(),
        None => std::env::temp_dir().join(format!("{}.{ext}", crate::io::sanitize_name(&setup.title))).to_string_lossy().to_string(),
    };
    crate::io::write_file(&path, text.as_bytes())?;
    let measures = parts.iter().map(|p| p.measures.len()).max().unwrap_or(0);
    Ok(json!({"path": path, "parts": parts.len(), "measures": measures}))
}

#[cfg(test)]
mod tests;
