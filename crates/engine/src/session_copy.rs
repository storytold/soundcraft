//! Explicit audio-portable copies. Ordinary saves and recovery remain able to preserve missing media.

use crate::{Engine, EngineError, Result, io};
use serde::Serialize;
use soundcraft_audio_io::{BitDepth, EncodeOptions, FileFormat};
use soundcraft_model::Session;
use std::collections::BTreeSet;
use std::io::Write;
use std::path::Path;

const MAX_SOURCES: usize = 9_999;
const MAX_TEXT_BYTES: u64 = 16 << 20;
const MAX_FIELD_BYTES: usize = 4_096;
const MAX_SAMPLE_VALUES: u64 = 1 << 30;
const MAX_BUNDLE_BYTES: u64 = 1 << 30;
const WAV_HEADER_BOUND: u64 = 128;

#[derive(Debug, Clone, Serialize)]
pub struct MediaRow {
    pub kind: &'static str,
    pub source: u64,
    pub name: String,
    pub source_path: String,
    pub status: &'static str,
    pub output_path: Option<String>,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CopyPreview {
    pub ready: bool,
    pub audio_source_count: usize,
    pub ready_audio: usize,
    pub missing_audio: usize,
    pub invalid_audio: usize,
    pub external_video_count: usize,
    pub sample_rate: u32,
    pub file_type: &'static str,
    pub bit_depth: &'static str,
    pub full_session_portability: &'static str,
    pub media: Vec<MediaRow>,
    pub issues: Vec<String>,
    #[serde(skip)]
    estimated_bytes: u64,
}

#[derive(Debug, Serialize)]
pub struct CopyResult {
    pub path: String,
    pub output_path: String,
    pub audio_files_written: usize,
    pub audio_sources_packaged: usize,
    pub audio_complete: bool,
    pub full_session_portability: &'static str,
    pub sample_rate: u32,
    pub file_type: &'static str,
    pub bit_depth: &'static str,
    pub media: Vec<MediaRow>,
}

/// Optional freshness guard, also checked against the original document before UI snapshot capture.
pub fn check_revision(e: &Engine, value: Option<&serde_json::Value>) -> Result<()> {
    if let Some(value) = value {
        let expected = value
            .as_u64()
            .ok_or_else(|| EngineError::BadParams("session.save_copy".into(), "expected_revision must be a nonnegative integer".into()))?;
        if expected != e.revision {
            return Err(EngineError::BadParams("session.save_copy".into(), "session changed; refresh the copy preview".into()));
        }
    }
    Ok(())
}

/// Read-only plan. `ready` concerns decoded audio and the destination, never all plugin/file assets.
pub fn preview(e: &Engine, path: Option<&str>) -> CopyPreview {
    let mut report = inspect(e.session());
    if let Some(path) = path {
        if path.trim().is_empty() || path.len() > MAX_FIELD_BYTES || path.contains('\0') {
            report.issues.push("choose a nonempty destination no longer than 4096 bytes, without NUL characters".into());
        } else if !cfg!(target_arch = "wasm32") {
            io::session_copy_files::destination_issues(e, &io::session_file(path), &mut report.issues);
        }
    }
    if cfg!(target_arch = "wasm32") && report.estimated_bytes > MAX_BUNDLE_BYTES {
        report.issues.push("audio and session exceed the browser bundle's 1 GiB limit".into());
    }
    report.ready &= report.issues.is_empty();
    report
}

fn inspect(s: &Session) -> CopyPreview {
    let mut report = CopyPreview {
        ready: true,
        audio_source_count: s.sources.len(),
        ready_audio: 0,
        missing_audio: 0,
        invalid_audio: 0,
        external_video_count: s.videos.len(),
        sample_rate: s.sample_rate.hz(),
        file_type: "wav",
        bit_depth: "32f",
        full_session_portability: "not_assessed",
        media: Vec::new(),
        issues: Vec::new(),
        estimated_bytes: 0,
    };
    if s.sources.len() > MAX_SOURCES || s.videos.len() > MAX_SOURCES {
        report.ready = false;
        report.issues.push("a copy supports at most 9999 audio sources and 9999 external video references".into());
        return report;
    }
    if soundcraft_time::SampleRate::new(s.sample_rate.hz()).is_err() {
        report.issues.push("session sample rate must be within 8000–768000 Hz".into());
    }
    let text_bytes = match text_size(s) {
        Ok(n) => n,
        Err(err) => {
            report.ready = false;
            report.issues.push(err);
            return report;
        }
    };
    let mut declared = BTreeSet::new();
    let mut duplicates = BTreeSet::new();
    for src in &s.sources {
        if !declared.insert(src.id) {
            duplicates.insert(src.id);
        }
    }
    for src in &s.sources {
        let mut row = MediaRow {
            kind: "audio",
            source: src.id.0,
            name: clipped(&src.name),
            source_path: clipped(&src.path),
            status: "ready",
            output_path: None,
            reason: None,
        };
        let invalid = if duplicates.contains(&src.id) {
            Some("duplicate audio source ID".to_string())
        } else if src.name.len() > MAX_FIELD_BYTES || src.path.len() > MAX_FIELD_BYTES {
            Some("source name or path exceeds 4096 bytes".into())
        } else if let Some(audio) = s.pool.get(src.id) {
            let buf = &audio.buffer;
            let values = src.frames.checked_mul(u64::from(src.channels));
            let bytes = values.and_then(|n| n.checked_mul(4)).and_then(|n| n.checked_add(WAV_HEADER_BOUND));
            if buf.num_channels() == 0 || buf.num_channels() > usize::from(u16::MAX / 4) || buf.frames() == 0 {
                Some("audio has no frames/channels or too many channels for Float32 WAV".into())
            } else if buf.channels.iter().any(|channel| channel.len() != buf.frames()) {
                Some("audio channels have different frame counts".into())
            } else if buf.num_channels() != usize::from(src.channels) || u64::try_from(buf.frames()).ok() != Some(src.frames) {
                Some("decoded channels/frames differ from source metadata".into())
            } else if soundcraft_time::SampleRate::new(buf.sample_rate).is_err()
                || buf.sample_rate != src.sample_rate
                || src.sample_rate != s.sample_rate.hz()
            {
                Some("source, decoded audio and session must have the same sample rate within 8000–768000 Hz".into())
            } else if src.sample_rate.checked_mul(u32::from(src.channels) * 4).is_none() {
                Some("WAV byte rate exceeds its supported field width".into())
            } else if values.is_none_or(|n| n > MAX_SAMPLE_VALUES) || bytes.and_then(|n| usize::try_from(n).ok()).is_none() {
                Some("audio output is too large".into())
            } else {
                report.estimated_bytes = report.estimated_bytes.saturating_add(bytes.unwrap_or(u64::MAX));
                None
            }
        } else {
            row.status = "missing";
            row.reason = Some("decoded audio is unavailable; restore or relink it before copying".into());
            None
        };
        if let Some(reason) = invalid {
            row.status = "invalid";
            row.reason = Some(reason);
        }
        match row.status {
            "ready" => report.ready_audio += 1,
            "missing" => report.missing_audio += 1,
            _ => report.invalid_audio += 1,
        }
        report.media.push(row);
    }
    let references = s.tracks.iter().flat_map(|t| {
        t.playlists
            .iter()
            .flat_map(|p| p.clips.iter().filter_map(|c| c.source()))
            .chain(t.mixer.inserts.iter().flatten().filter_map(|insert| insert.sample))
            .chain(t.instrument.iter().filter_map(|insert| insert.sample))
    });
    let dangling: BTreeSet<_> = references.filter(|id| !declared.contains(id)).collect();
    for id in dangling.iter().take(32) {
        report.issues.push(format!("clip or sampler references undeclared audio source {}", id.0));
    }
    if dangling.len() > 32 {
        report.issues.push(format!("{} more undeclared audio references", dangling.len() - 32));
    }
    for video in &s.videos {
        if video.name.len() > MAX_FIELD_BYTES || video.path.len() > MAX_FIELD_BYTES {
            report.issues.push("video reference name or path exceeds 4096 bytes".into());
        }
        report.media.push(MediaRow {
            kind: "video",
            source: video.id.0,
            name: clipped(&video.name),
            source_path: clipped(&video.path),
            status: "external_unverified",
            output_path: None,
            reason: Some("video is referenced externally and is not copied".into()),
        });
    }
    // Bound rewritten paths/name before any native output or bundle allocation.
    let text_bound = text_bytes.saturating_add((s.sources.len() as u64).saturating_mul(256)).saturating_add(256);
    if text_bound > MAX_TEXT_BYTES {
        report.issues.push("session metadata exceeds the copy's 16 MiB limit".into());
    }
    report.estimated_bytes = report.estimated_bytes.saturating_add(text_bound);
    report.ready = report.missing_audio == 0 && report.invalid_audio == 0 && report.issues.is_empty();
    report
}

fn clipped(text: &str) -> String {
    let mut end = text.len().min(MAX_FIELD_BYTES);
    while !text.is_char_boundary(end) {
        end = end.saturating_sub(1);
    }
    text.get(..end).unwrap_or_default().into()
}

fn copy_stem(name: &str) -> String {
    let mut stem = io::sanitize_name(name);
    let mut end = stem.len().min(180);
    while !stem.is_char_boundary(end) {
        end = end.saturating_sub(1);
    }
    stem.truncate(end);
    if stem.is_empty() { "Audio".into() } else { stem }
}

fn text_size(s: &Session) -> std::result::Result<u64, String> {
    struct Counter(u64);
    impl Write for Counter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0 = self.0.saturating_add(bytes.len() as u64);
            if self.0 > MAX_TEXT_BYTES {
                return Err(std::io::Error::other("session metadata exceeds the copy's 16 MiB limit"));
            }
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut count = Counter(0);
    serde_json::to_writer_pretty(&mut count, s).map_err(|err| err.to_string())?;
    Ok(count.0)
}

fn require_ready(report: &CopyPreview) -> Result<()> {
    if report.ready {
        return Ok(());
    }
    let mut reasons: Vec<_> = report
        .media
        .iter()
        .filter(|row| row.kind == "audio" && row.status != "ready")
        .take(16)
        .map(|row| format!("source {} ({}): {}", row.source, row.name, row.reason.as_deref().unwrap_or("unavailable")))
        .collect();
    reasons.extend(report.issues.iter().take(16).cloned());
    Err(EngineError::BadParams("session.save_copy".into(), format!("copy blocked: {}", reasons.join("; "))))
}

fn options() -> EncodeOptions {
    EncodeOptions { format: FileFormat::Wav, bit_depth: BitDepth::Float32, dither: false, bwf: None }
}

fn result(path: &str, output: String, written: usize, report: CopyPreview) -> CopyResult {
    CopyResult {
        path: path.into(),
        output_path: output,
        audio_files_written: written,
        audio_sources_packaged: report.audio_source_count,
        audio_complete: true,
        full_session_portability: "not_assessed",
        sample_rate: report.sample_rate,
        file_type: report.file_type,
        bit_depth: report.bit_depth,
        media: report.media,
    }
}

/// Strict, nonmutating copy; this is deliberately separate from permissive recovery writes.
pub fn save(e: &Engine, path: &str) -> Result<CopyResult> {
    let mut report = preview(e, Some(path));
    require_ready(&report)?;
    if cfg!(target_arch = "wasm32") {
        return deliver_bundle(e, path, io::write_file);
    }
    let file = io::session_file(path);
    let dir = file.parent().unwrap_or(Path::new(""));
    let audio_dir = dir.join("Audio Files");
    let mut s = e.session().clone();
    let mut written = 0;
    for (src, row) in s.sources.iter_mut().zip(report.media.iter_mut()) {
        let audio = s.pool.get(src.id).ok_or_else(|| EngineError::Io("validated audio became unavailable".into()))?;
        let bytes = soundcraft_audio_io::encode(&audio.buffer, &options()).map_err(|err| EngineError::Io(err.to_string()))?;
        io::make_dir(&audio_dir)?;
        let (name, new) = io::store_copy_wav(&audio_dir, &copy_stem(&src.name), &bytes)?;
        src.path = format!("Audio Files/{name}");
        src.unsaved = false;
        src.format = FileFormat::Wav;
        row.status = if new { "written" } else { "reused" };
        row.output_path = Some(src.path.clone());
        written += usize::from(new);
    }
    s.name = copy_stem(file.file_stem().and_then(|name| name.to_str()).unwrap_or("Untitled"));
    let text = s.to_json().map_err(|err| EngineError::Io(err.to_string()))?;
    io::make_dir(dir)?;
    io::session_copy_files::publish(&file, &audio_dir, text.as_bytes())?;
    Ok(result(path, file.to_string_lossy().into_owned(), written, report))
}

/// Build the browser copy without touching the document or a filesystem. Also useful to native hosts.
pub fn bundle(e: &Engine, path: &str) -> Result<(Vec<u8>, CopyResult)> {
    let mut report = inspect(e.session());
    if path.trim().is_empty() || path.len() > MAX_FIELD_BYTES || path.contains('\0') {
        report.issues.push("choose a nonempty destination no longer than 4096 bytes, without NUL characters".into());
        report.ready = false;
    }
    require_ready(&report)?;
    if report.estimated_bytes > MAX_BUNDLE_BYTES {
        return Err(EngineError::BadParams("session.save_copy".into(), "audio and session exceed the browser bundle's 1 GiB limit".into()));
    }
    let mut s = e.session().clone();
    s.name = copy_stem(Path::new(path).file_stem().and_then(|name| name.to_str()).unwrap_or("Untitled"));
    let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let opts = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored).large_file(true);
    for (src, row) in s.sources.iter_mut().zip(report.media.iter_mut()) {
        let audio = s.pool.get(src.id).ok_or_else(|| EngineError::Io("validated audio became unavailable".into()))?;
        let bytes = soundcraft_audio_io::encode(&audio.buffer, &options()).map_err(|err| EngineError::Io(err.to_string()))?;
        src.path = format!("Audio Files/{}_{}.wav", copy_stem(&src.name), src.id.0);
        src.unsaved = false;
        src.format = FileFormat::Wav;
        writer.start_file(&src.path, opts).map_err(|err| EngineError::Io(err.to_string()))?;
        writer.write_all(&bytes).map_err(|err| EngineError::Io(err.to_string()))?;
        row.status = "packaged";
        row.output_path = Some(src.path.clone());
    }
    writer.start_file(format!("{}.scraft", s.name), opts).map_err(|err| EngineError::Io(err.to_string()))?;
    writer.write_all(s.to_json().map_err(|err| EngineError::Io(err.to_string()))?.as_bytes()).map_err(|err| EngineError::Io(err.to_string()))?;
    let bytes = writer.finish().map_err(|err| EngineError::Io(err.to_string()))?.into_inner();
    let written = report.audio_source_count;
    Ok((bytes, result(path, format!("{}.{}", s.name, io::BUNDLE_EXTENSION), written, report)))
}

fn deliver_bundle(e: &Engine, path: &str, deliver: impl FnOnce(&str, &[u8]) -> Result<()>) -> Result<CopyResult> {
    let (bytes, report) = bundle(e, path)?;
    deliver(&report.output_path, &bytes)?;
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundle_is_validated_before_delivery_and_delivery_errors_leave_document_untouched() {
        let mut e = crate::demo::demo_engine();
        let id = e.session().sources[0].id;
        e.session_mut().pool.remove(id);
        let before = e.session_arc();
        let mut delivered = false;
        assert!(
            deliver_bundle(&e, "Copy.scraftz", |_, _| {
                delivered = true;
                Ok(())
            })
            .is_err()
        );
        assert!(!delivered);
        assert!(std::sync::Arc::ptr_eq(&before, &e.session_arc()));
        let e = Engine::default();
        let before = e.session_arc();
        assert!(
            deliver_bundle(&e, "Copy.scraftz", |name, bytes| {
                delivered = true;
                assert_eq!(name, "Copy.scraftz");
                let mut r = Engine::default();
                assert!(io::open_session_bytes(&mut r, name, bytes)?.is_empty());
                Err(EngineError::Io("synthetic delivery failure".into()))
            })
            .is_err()
        );
        assert!(delivered);
        assert!(std::sync::Arc::ptr_eq(&before, &e.session_arc()));
        assert!(e.path.is_none() && !e.is_dirty());
    }
}
