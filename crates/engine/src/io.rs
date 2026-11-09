//! Import, export, save/load, rendering helpers (bounce, consolidate, commit) and analysis.

use crate::{Engine, EngineError, Result};
use serde_json::{Value, json};
use soundcraft_audio_io::{AudioBuffer, EncodeOptions, FileFormat};
use soundcraft_model::{BusId, ChannelFormat, Clip, ClipContent, Session, Source, SourceAudio, SourceId, TrackId, TrackKind};
use soundcraft_time::{Range, Samples};
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

mod bundle;
mod import;
mod midi;
mod record;
mod render;
mod session;
mod strip;

use bundle::BundleLimits;
pub use bundle::{BUNDLE_EXTENSION, open_session_bytes, session_bundle};
pub use import::{DecodedImport, add_source, decode_import, import_audio_bytes, load_sampler_bytes, relink_videos};
pub(crate) use import::{MAX_IMPORT_SAMPLES, at_rate, place_decoded};
pub use midi::{export_midi, import_midi_bytes};
pub use record::{Recording, Take, add_recording, live_span, place_recording};
pub use render::{commit_track, consolidate, refuse_longer, render_clip_gain, render_limit, sanitize_name};
pub(crate) use session::{DecodedFiles, decode_file, open_session_linked};
pub use session::{open_session, save_copy, save_session, save_session_copy};
pub use strip::{StripMode, StripNames, StripScan, bounded_to_track, name_stripped_clips, strip_silence, strip_silence_ranges, transients_in};

// ─── Checked file access ──────────────────────────────────────────────────────────────────────
//
// Every file a command names by path is read and written through the helpers below, so the
// automation roots (`crate::file_access`, `soundcraft-cli mcp --automation-read-root` and
// `--automation-write-root`) are checked in one place. A test in `file_access` keeps the rest of
// the engine from calling `std::fs` itself. Audio is decoded into the session's pool the moment it
// enters it (import, open, sampler load), so playback and bounce read nothing from disk and
// need no check of their own.

/// The bytes of the file at `path`, which must be inside the automation read root when there is
/// one. Only a regular file qualifies: a device or a pipe named by a session or a command would
/// never end or never answer. Errors name the file.
pub fn read_file(path: impl AsRef<Path>) -> Result<Vec<u8>> {
    read_file_within(path.as_ref(), false)
}

/// [`read_file`]; a `session` file is refused ([`session_size`]) past [`session_limit`].
fn read_file_within(path: &Path, session: bool) -> Result<Vec<u8>> {
    use std::io::Read as _;
    crate::file_access::check_read(path).map_err(EngineError::Io)?;
    let err = |err: std::io::Error| EngineError::Io(format!("{}: {err}", path.display()));
    let meta = std::fs::metadata(path).map_err(err)?;
    if !meta.is_file() {
        return Err(EngineError::Io(format!("{}: not a regular file", path.display())));
    }
    if !session {
        return std::fs::read(path).map_err(err);
    }
    session_size(path.display(), meta.len(), false)?;
    // The file may have grown since: never read more than one byte past the limit.
    let mut data = Vec::new();
    std::fs::File::open(path).map_err(err)?.take(session_limit().saturating_add(1)).read_to_end(&mut data).map_err(err)?;
    session_size(path.display(), data.len() as u64, false)?;
    Ok(data)
}

/// [`read_file`] as UTF-8 text.
pub fn read_text(path: impl AsRef<Path>) -> Result<String> {
    let path = path.as_ref();
    utf8(path, read_file(path)?)
}

fn utf8(path: &Path, bytes: Vec<u8>) -> Result<String> {
    String::from_utf8(bytes).map_err(|err| EngineError::Io(format!("{}: {err}", path.display())))
}

/// The largest session (`.scraft`) SoundCraft opens or saves. A session is read whole and parsed
/// in one go, so a larger one is refused by its size before it is read, and never written (it
/// could not be opened again); real sessions are far smaller.
pub const MAX_SESSION_BYTES: u64 = 256 << 20;

#[cfg(test)]
thread_local! {
    /// A smaller limit than [`MAX_SESSION_BYTES`] for the tests on this thread ([`SessionLimit`]).
    static TEST_SESSION_LIMIT: std::cell::Cell<Option<u64>> = const { std::cell::Cell::new(None) };
}

/// [`MAX_SESSION_BYTES`] (in tests, the limit a [`SessionLimit`] set on this thread).
pub(crate) fn session_limit() -> u64 {
    #[cfg(test)]
    if let Some(limit) = TEST_SESSION_LIMIT.with(std::cell::Cell::get) {
        return limit;
    }
    MAX_SESSION_BYTES
}

/// While it lives, sessions on this thread are opened and saved up to the limit given to
/// [`SessionLimit::set`] instead of [`MAX_SESSION_BYTES`], so tests reach it with small sessions.
#[cfg(test)]
pub(crate) struct SessionLimit;

#[cfg(test)]
impl SessionLimit {
    pub(crate) fn set(limit: u64) -> SessionLimit {
        TEST_SESSION_LIMIT.with(|l| l.set(Some(limit)));
        SessionLimit
    }
}

#[cfg(test)]
impl Drop for SessionLimit {
    fn drop(&mut self) {
        TEST_SESSION_LIMIT.with(|l| l.set(None));
    }
}

/// Refuses the session `name` of `len` bytes when it is larger than [`session_limit`]: too large
/// to open, or, when `saving`, to save.
pub(crate) fn session_size(name: impl std::fmt::Display, len: u64, saving: bool) -> Result<()> {
    let limit = session_limit();
    if len <= limit {
        return Ok(());
    }
    let most = if limit >= 1 << 20 { format!("{} MiB", limit >> 20) } else { format!("{limit} bytes") };
    Err(EngineError::Io(if saving {
        format!("{name} is too large to save: over {most}, more than SoundCraft opens")
    } else {
        format!("{name} is too large to open (over {most})")
    }))
}

/// [`read_text`] for a session file: refused, unread, past [`session_limit`].
pub(crate) fn read_session_text(path: impl AsRef<Path>) -> Result<String> {
    let path = path.as_ref();
    utf8(path, read_file_within(path, true)?)
}

/// A bare `.scraft` handed over as bytes (the browser) as text: refused, uncopied, past
/// [`session_limit`].
pub(crate) fn session_bytes_text(name: &str, bytes: &[u8]) -> Result<String> {
    session_size(name, bytes.len() as u64, false)?;
    String::from_utf8(bytes.to_vec()).map_err(|_| EngineError::Io(format!("{name}: not a SoundCraft session")))
}

/// How many bytes [`Session::to_json`] makes of `s`, counted without keeping them.
fn json_len(s: &Session) -> u64 {
    struct Count(u64);
    impl std::io::Write for Count {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0 = self.0.saturating_add(buf.len() as u64);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut n = Count(0);
    // A session that cannot be written counts as nothing here: `to_json` reports that error.
    serde_json::to_writer_pretty(&mut n, s).map_or(0, |()| n.0)
}

/// Is there a file at `path` the automation read root lets us see? A path outside the root is
/// reported as not found, so a command can't probe for files beyond it.
pub fn file_found(path: impl AsRef<Path>) -> bool {
    let path = path.as_ref();
    crate::file_access::check_read(path).is_ok() && path.is_file()
}

/// The file a path stored in the session file `session` refers to, or why it is not opened.
///
/// Every path a session names (audio sources, movies) goes through here before it reaches the
/// file system. A relative path is taken from the session's folder, and only files on this
/// computer or on the network share the session itself is on are opened (`craft_docpath`).
/// Paths the person picks in this session (imports) are not stored paths and skip this.
pub fn stored_path(stored: &str, session: &Path) -> std::result::Result<PathBuf, String> {
    let session = std::path::absolute(session).unwrap_or_else(|_| session.to_path_buf());
    craft_docpath::resolve_absolute(stored, Some(&session)).map_err(|why| format!("not opened: {why}"))
}

/// Create `dir` and its parents, inside the automation write root when there is one. The browser
/// has no folders: there each file is delivered by its name ([`write_file`]) and this does nothing.
pub(crate) fn make_dir(dir: impl AsRef<Path>) -> Result<()> {
    if cfg!(target_arch = "wasm32") {
        return Ok(());
    }
    let dir = dir.as_ref();
    // The empty folder of a bare file name ("t.scraft") is the working directory, which the root
    // check would refuse as an empty path.
    let checked = if dir.as_os_str().is_empty() { Path::new(".") } else { dir };
    crate::file_access::check_write(checked).map_err(EngineError::Io)?;
    std::fs::create_dir_all(dir).map_err(|err| EngineError::Io(format!("{}: {err}", dir.display())))
}

/// Write `bytes` to the file at `path`, inside the automation write root when there is one.
pub(crate) fn write_path(path: impl AsRef<Path>, bytes: impl AsRef<[u8]>) -> Result<()> {
    let path = path.as_ref();
    crate::file_access::check_write(path).map_err(EngineError::Io)?;
    std::fs::write(path, bytes).map_err(|err| EngineError::Io(format!("{}: {err}", path.display())))
}

/// Import a movie: a Video track clip spanning the picture (the movie is referenced by `path`,
/// never copied) plus its audio on a new audio track when it has a usable audio stream. Fails
/// only when the file has neither a readable picture nor audio.
pub fn import_video_bytes(e: &mut Engine, path: &str, bytes: Vec<u8>, at: Samples) -> Result<Value> {
    let stem = Path::new(path).file_stem().and_then(OsStr::to_str).unwrap_or("Video").to_string();
    let ext = Path::new(path).extension().and_then(OsStr::to_str).unwrap_or("mp4").to_ascii_lowercase();
    let bytes: Arc<[u8]> = bytes.into();
    let movie = soundcraft_video::Movie::open(Arc::clone(&bytes)).map(|m| m.info().clone());
    let audio_name = format!("{stem} audio.{}", if ext == "mov" { "mp4" } else { ext.as_str() });
    let audio = import_audio_bytes(e, &audio_name, &bytes, Some(path), None, at);
    let info = match (movie, &audio) {
        (Ok(info), _) => info,
        (Err(verr), Ok(a)) => {
            e.message(format!("{stem}: imported the audio; no picture ({verr})"));
            return Ok(json!({"audio": a, "video": Value::Null, "video_error": verr.to_string()}));
        }
        (Err(verr), Err(aerr)) => return Err(EngineError::Io(format!("{path}: {verr}; no usable audio stream ({aerr})"))),
    };
    let abs = std::fs::canonicalize(path).map_or_else(|_| path.to_string(), |p| p.to_string_lossy().into_owned());
    let s = e.session_mut();
    let sr = s.sample_rate.as_f64();
    let len = (info.duration * sr).round();
    let len = if len.is_finite() { (len as Samples).clamp(1, sr as Samples * 3600 * 48) } else { 1 };
    let vid = SourceId(s.alloc());
    s.videos.push(soundcraft_model::VideoSource {
        id: vid,
        name: Path::new(path).file_name().and_then(OsStr::to_str).unwrap_or("movie").to_string(),
        path: abs,
        width: info.width,
        height: info.height,
        frame_rate: info.frame_rate,
        duration: info.duration,
        codec: info.codec.clone(),
        offline: None,
    });
    // Reuse the first Video track when the new clip does not overlap its clips; else add one at the top.
    let range = Range::new(at, at.saturating_add(len));
    let track = match s.tracks.iter().find(|t| t.kind == TrackKind::Video) {
        Some(t) if !t.clips().iter().any(|c| c.range().overlaps(&range)) => t.id,
        _ => {
            let id = s.add_track(TrackKind::Video, ChannelFormat::Mono, Some(&stem));
            if let Some(i) = s.track_index(id) {
                let t = s.tracks.remove(i);
                s.tracks.insert(0, t);
            }
            id
        }
    };
    let cid = s.new_clip_id();
    crate::edit::place_clip(s, track, Clip::video(cid, stem.clone(), vid, 0, at, len));
    if !info.decodable {
        e.message(format!("{stem}: {} pictures cannot be shown yet; the Video track shows the clip only", info.codec_detail));
    }
    if let Err(err) = &audio {
        e.message(format!("{stem}: no usable audio stream ({err})"));
    }
    Ok(json!({
        "video": {"track": track, "clip": cid, "source": vid, "length": len, "info": info},
        "audio": audio.as_ref().ok(),
        "audio_error": audio.as_ref().err().map(ToString::to_string),
    }))
}

/// A bounce of the main mix, or of one bus, over a range, written to its file block by block so no
/// bounce has to fit in memory, and rendered a slice at a time ([`Bounce::step`]) so a UI can show
/// progress. The file appears at its path only once complete; a bounce that fails, or is dropped
/// unfinished, leaves any older file there alone.
pub struct Bounce {
    session: Arc<Session>,
    path: PathBuf,
    part: PathBuf,
    range: Range,
    bus: Option<BusId>,
    fold: Option<ChannelFormat>,
    normalize: bool,
    /// Loudness normalization instead (see [`Bounce::with_loudness`]).
    loudness: Option<LoudnessTarget>,
    /// Measuring the peak or the loudness to normalize to, the first of two passes.
    measuring: bool,
    /// The normalization gain applied (dB) and what a loudness target came to.
    gain_db: f32,
    outcome: Option<LoudnessOutcome>,
    render: soundcraft_mix::RangeRender,
    block: Vec<Vec<f32>>,
    done: usize,
    gain: f32,
    peak: f32,
    meter: soundcraft_dsp::meter::LoudnessMeter,
    enc: Option<soundcraft_audio_io::FileEncoder<Out>>,
}

/// The Broadcast Wave stamp of timeline position `at`: the session's timecode start plus `at`, in
/// samples since midnight (`bext` TimeReference; 0 before midnight).
fn time_stamp(s: &Session, at: Samples) -> soundcraft_audio_io::BwfInfo {
    let time_reference = u64::try_from(s.timecode_start.saturating_add(at)).unwrap_or(0);
    soundcraft_audio_io::BwfInfo { time_reference, ..Default::default() }
}

/// A loudness to normalize a bounce to: integrated loudness (LUFS, ITU-R BS.1770 / EBU R128) and
/// a true-peak ceiling (dBTP) the gain may not push past.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LoudnessTarget {
    pub lufs: f32,
    pub true_peak_ceiling_db: f32,
}

/// What a loudness-normalized bounce came to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoudnessOutcome {
    /// The gain brought it to the target.
    Reached,
    /// The true-peak ceiling held the gain back, so the file is quieter than the target.
    CeilingHeld,
    /// Silence, or too short to measure (under one 400 ms gating block): written at unity gain.
    Unmeasured,
}

/// A finished bounce: the size of its file and its levels (`lufs` and `true_peak_db` as written).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bounced {
    pub bytes: u64,
    pub peak_db: f32,
    pub lufs: f32,
    pub true_peak_db: f32,
    /// The normalization gain applied (0 without normalization).
    pub gain_db: f32,
    /// Set when the bounce was normalized to a loudness.
    pub loudness: Option<LoudnessOutcome>,
}

impl Bounce {
    /// Starts a bounce of `s`'s main mix, or of `bus`, over `r` to `path`: every channel (SMPTE/WAV
    /// order, with the WAV speaker mask of its format), or an ITU stereo fold-down when
    /// `fold_stereo`, peak-normalized to -0.1 dBFS when `normalize` (which renders twice).
    pub fn new(
        s: Arc<Session>,
        path: &Path,
        r: Range,
        opts: &EncodeOptions,
        bus: Option<BusId>,
        normalize: bool,
        fold_stereo: bool,
    ) -> Result<Bounce> {
        if r.len() > s.sample_rate.samples(MAX_BOUNCE_SECS) {
            return Err(EngineError::BadParams("file.bounce_mix".into(), "bounces are limited to 4 hours".into()));
        }
        let (fmt, channels, fold) = bounce_layout(&s, bus, fold_stereo);
        let name = path.file_name().map_or_else(|| "bounce".into(), |n| n.to_string_lossy().into_owned());
        let part = path.with_file_name(format!("{name}.part"));
        crate::file_access::check_write(path).map_err(EngineError::Io)?;
        crate::file_access::check_write(&part).map_err(EngineError::Io)?;
        // A WAV bounce says where it starts on the session's timecode, as other DAWs' bounces do,
        // so post and broadcast tools can place it (only WAV has a `bext` chunk to say it in).
        let opts = &EncodeOptions { bwf: opts.bwf.clone().or_else(|| Some(time_stamp(&s, r.start))), ..opts.clone() };
        let enc = (|| -> std::result::Result<_, soundcraft_audio_io::AudioError> {
            let enc = soundcraft_audio_io::FileEncoder::new(Out::create(&part)?, opts, channels, s.sample_rate.hz(), Some(fmt.channel_mask()))?;
            enc.check_room(u64::try_from(r.len()).unwrap_or(0))?;
            Ok(enc)
        })()
        .map_err(|err| {
            let _ = std::fs::remove_file(&part);
            EngineError::Io(format!("{}: {err}", path.display()))
        })?;
        let render = soundcraft_mix::RangeRender::new(&s, bus, r, 1024);
        let meter = soundcraft_dsp::meter::LoudnessMeter::new(s.sample_rate.as_f64() as f32, channels);
        Ok(Bounce {
            session: s,
            path: path.to_path_buf(),
            part,
            range: r,
            bus,
            fold,
            normalize,
            loudness: None,
            measuring: normalize,
            gain_db: 0.0,
            outcome: None,
            render,
            block: Vec::new(),
            done: 0,
            gain: 1.0,
            peak: 0.0,
            meter,
            enc: Some(enc),
        })
    }

    /// Normalizes to `target` instead of the peak: a first pass measures integrated loudness and
    /// true peak, and the gain moves the loudness to the target, lowered if needed so the true peak
    /// stays at or under the ceiling (no limiter: nothing but the level changes). Call before the
    /// first step.
    pub fn with_loudness(mut self, target: LoudnessTarget) -> Self {
        if self.done == 0 && !self.normalize {
            self.loudness = Some(target);
            self.measuring = true;
        }
        self
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn range(&self) -> Range {
        self.range
    }

    /// How far along the bounce is, from 0 to 1.
    pub fn progress(&self) -> f32 {
        let pass = (self.done as f64 / self.range.len().max(1) as f64).min(1.0) as f32;
        match (self.normalize || self.loudness.is_some(), self.measuring) {
            (false, _) => pass,
            (true, true) => pass / 2.0,
            (true, false) => 0.5 + pass / 2.0,
        }
    }

    /// Renders blocks until `more` returns false (at least one per call). Returns the finished
    /// bounce once its file is complete.
    pub fn step(&mut self, mut more: impl FnMut() -> bool) -> Result<Option<Bounced>> {
        loop {
            match self.render.next_block(&self.session) {
                Some((chunk, n)) => {
                    fill(&mut self.block, chunk, n, self.fold, self.gain);
                    self.done += n;
                    self.peak = self.peak.max(soundcraft_dsp::offline::peak(&self.block));
                    if !self.measuring || self.loudness.is_some() {
                        self.meter.process(&self.block, n);
                    }
                    if !self.measuring
                        && let Some(enc) = &mut self.enc
                    {
                        enc.write(&self.block, n).map_err(|err| EngineError::Io(format!("{}: {err}", self.path.display())))?;
                    }
                }
                None if self.measuring => {
                    self.gain = match self.loudness {
                        Some(t) => self.loudness_gain(t),
                        None if self.peak > 0.0 => soundcraft_dsp::db_to_gain(-0.1) / self.peak,
                        None => 1.0,
                    };
                    if self.normalize {
                        self.gain_db = soundcraft_dsp::gain_to_db(self.gain);
                    }
                    self.meter.reset();
                    self.peak = 0.0;
                    self.done = 0;
                    self.measuring = false;
                    self.render = soundcraft_mix::RangeRender::new(&self.session, self.bus, self.range, 1024);
                }
                None => return self.finish().map(Some),
            }
            if !more() {
                return Ok(None);
            }
        }
    }

    /// The gain that brings the measured loudness to `t`, held back by the true-peak ceiling.
    fn loudness_gain(&mut self, t: LoudnessTarget) -> f32 {
        let (lufs, true_peak) = (self.meter.integrated_lufs(), self.meter.true_peak_db());
        if lufs <= soundcraft_dsp::MIN_DB {
            self.outcome = Some(LoudnessOutcome::Unmeasured);
            return 1.0;
        }
        let mut gain_db = t.lufs - lufs;
        self.outcome = Some(LoudnessOutcome::Reached);
        if true_peak + gain_db > t.true_peak_ceiling_db {
            gain_db = t.true_peak_ceiling_db - true_peak;
            self.outcome = Some(LoudnessOutcome::CeilingHeld);
        }
        self.gain_db = gain_db;
        soundcraft_dsp::db_to_gain(gain_db)
    }

    /// Renders the whole bounce (see [`Bounce::step`]).
    pub fn run(mut self) -> Result<Bounced> {
        loop {
            if let Some(done) = self.step(|| true)? {
                return Ok(done);
            }
        }
    }

    fn finish(&mut self) -> Result<Bounced> {
        let bytes = match self.enc.take() {
            Some(enc) => enc.finish().map_err(|err| EngineError::Io(format!("{}: {err}", self.path.display())))?.complete(&self.part, &self.path)?,
            None => 0,
        };
        Ok(Bounced {
            bytes,
            peak_db: soundcraft_dsp::gain_to_db(self.peak),
            lufs: self.meter.integrated_lufs(),
            true_peak_db: self.meter.true_peak_db(),
            gain_db: self.gain_db,
            loudness: self.outcome,
        })
    }
}

impl Drop for Bounce {
    /// Removes the part file unless the bounce completed (which renamed it).
    fn drop(&mut self) {
        drop(self.enc.take());
        let _ = std::fs::remove_file(&self.part);
    }
}

/// Where a bounce is written: its part file, or memory in the browser, which has no file system.
enum Out {
    File(std::io::BufWriter<std::fs::File>),
    Memory(std::io::Cursor<Vec<u8>>),
}

impl Out {
    fn create(part: &Path) -> std::io::Result<Out> {
        if cfg!(target_arch = "wasm32") {
            return Ok(Out::Memory(std::io::Cursor::new(Vec::new())));
        }
        Ok(Out::File(std::io::BufWriter::with_capacity(1 << 20, std::fs::File::create(part)?)))
    }

    /// Puts the finished file at `path` and returns its size: the part file is synced and renamed
    /// over it; in the browser the bytes are delivered ([`write_file`]).
    fn complete(self, part: &Path, path: &Path) -> Result<u64> {
        match self {
            Out::File(out) => {
                let mut len = 0;
                let synced = out.into_inner().map_err(std::io::IntoInnerError::into_error).and_then(|f| {
                    f.sync_all()?;
                    len = f.metadata()?.len();
                    Ok(())
                });
                replace_with(part, path, synced).map(|()| len)
            }
            Out::Memory(m) => {
                let bytes = m.into_inner();
                write_file(&path.to_string_lossy(), &bytes)?;
                Ok(bytes.len() as u64)
            }
        }
    }
}

impl std::io::Write for Out {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        match self {
            Out::File(f) => f.write(buf),
            Out::Memory(m) => {
                // A long bounce may not fit in the browser's memory: fail instead of aborting.
                let end = usize::try_from(m.position()).unwrap_or(usize::MAX).saturating_add(buf.len());
                let v = m.get_mut();
                v.try_reserve(end.saturating_sub(v.len())).map_err(|_| std::io::ErrorKind::OutOfMemory)?;
                m.write(buf)
            }
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        match self {
            Out::File(f) => f.flush(),
            Out::Memory(_) => Ok(()),
        }
    }
}

impl std::io::Seek for Out {
    fn seek(&mut self, pos: std::io::SeekFrom) -> std::io::Result<u64> {
        match self {
            Out::File(f) => f.seek(pos),
            Out::Memory(m) => m.seek(pos),
        }
    }
}

const MAX_BOUNCE_SECS: f64 = 4.0 * 3600.0;

/// The format, channel count and fold-down source of a bounce of the main mix or `bus`: every
/// channel (a mono or stereo main mix as stereo), or an ITU stereo fold-down of a surround source
/// when `fold_stereo`.
fn bounce_layout(s: &Session, bus: Option<BusId>, fold_stereo: bool) -> (ChannelFormat, usize, Option<ChannelFormat>) {
    let nch = soundcraft_mix::source_channels(s, bus);
    let fmt = match bus {
        Some(id) => s.bus(id).map_or(ChannelFormat::Stereo, |b| b.format),
        None if nch <= 2 => ChannelFormat::Stereo,
        None => s.main_format(),
    };
    if fold_stereo && nch > 2 { (ChannelFormat::Stereo, 2, Some(fmt)) } else { (fmt, nch, None) }
}

/// The first `n` frames of `chunk`, folded down from `fold` when set and scaled by `gain`.
fn fill(block: &mut Vec<Vec<f32>>, chunk: &[Vec<f32>], n: usize, fold: Option<ChannelFormat>, gain: f32) {
    let folded = fold.map(|from| soundcraft_mix::fold_down_stereo(from, chunk));
    let src = folded.as_deref().unwrap_or(chunk);
    block.resize_with(src.len(), Vec::new);
    for (d, c) in block.iter_mut().zip(src) {
        d.clear();
        d.extend(c.iter().take(n).map(|v| v * gain));
    }
}

/// Bounce one file per audio/instrument track (each soloed, so its sends and auxes are included)
/// into `dir`. Returns the written paths.
pub fn bounce_stems(e: &Engine, dir: &str, r: Range, opts: &EncodeOptions) -> Result<Vec<String>> {
    bounce_stems_with(e, dir, r, opts, false)
}

/// [`bounce_stems`] in the main format, or folded down to stereo when `fold_stereo`.
pub fn bounce_stems_with(e: &Engine, dir: &str, r: Range, opts: &EncodeOptions, fold_stereo: bool) -> Result<Vec<String>> {
    let s = e.session();
    if r.len() > s.sample_rate.samples(MAX_BOUNCE_SECS) {
        return Err(EngineError::BadParams("file.bounce_stems".into(), "bounces are limited to 4 hours".into()));
    }
    make_dir(dir)?;
    let ext = opts.format.default_extension();
    let mut out = Vec::new();
    let stems: Vec<(TrackId, String)> = s
        .tracks
        .iter()
        .filter(|t| matches!(t.kind, TrackKind::Audio | TrackKind::Instrument | TrackKind::Midi) && !t.inactive && !t.mixer.mute)
        .map(|t| (t.id, t.name.clone()))
        .collect();
    for (id, name) in stems {
        let mut solo = s.clone();
        for t in &mut solo.tracks {
            t.mixer.solo = t.id == id;
        }
        let path = Path::new(dir).join(format!("{} - {}.{ext}", sanitize_name(&s.name), sanitize_name(&name)));
        Bounce::new(Arc::new(solo), &path, r, opts, None, false, fold_stereo)?.run()?;
        out.push(path.to_string_lossy().into_owned());
    }
    Ok(out)
}

impl From<StripNames> for soundcraft_model::AutoName {
    fn from(n: StripNames) -> Self {
        Self { name: n.name, number: n.number, zeros: n.zeros, suffix: n.suffix, id: 0 }
    }
}

/// Write each clip as a WAV file named after it into `dir` (in the browser, delivered by name
/// through [`write_file`]). Returns how many were written.
pub fn export_clips(e: &Engine, ids: &[soundcraft_model::ClipId], dir: &str) -> Result<usize> {
    let s = e.session();
    make_dir(dir)?;
    let mut names = ClipNames::default();
    for id in ids {
        let Some((t, c)) = s.find_clip(*id) else { continue };
        let audio = soundcraft_mix::render_clips(s, t, c.range());
        let buf = AudioBuffer { sample_rate: s.sample_rate.hz(), channels: audio };
        let bytes = soundcraft_audio_io::encode(&buf, &EncodeOptions::default()).map_err(|err| EngineError::Io(err.to_string()))?;
        let path = clip_file(dir, &sanitize_name(&c.name), &mut names, Path::exists)?;
        write_file(&path.to_string_lossy(), &bytes)?;
    }
    Ok(names.taken.len())
}

/// The files an export has named so far, and for each name the suffix to try next, so a run of
/// same-named clips doesn't count up from 2 again for every clip.
#[derive(Default)]
struct ClipNames {
    taken: std::collections::HashSet<PathBuf>,
    next: std::collections::HashMap<String, u32>,
}

/// The first of `<stem>.wav`, `<stem>-2.wav`, … in `dir` that is neither `on_disk` nor already
/// named by this export, recorded in `names`: the browser has no disk to look at, so same-named
/// clips are kept apart by `names`.
fn clip_file(dir: &str, stem: &str, names: &mut ClipNames, on_disk: impl Fn(&Path) -> bool) -> Result<PathBuf> {
    let file = |k: u32| Path::new(dir).join(if k < 2 { format!("{stem}.wav") } else { format!("{stem}-{k}.wav") });
    let mut k = names.next.get(stem).copied().unwrap_or(1);
    let mut path = file(k);
    while on_disk(&path) || names.taken.contains(&path) {
        k = k.saturating_add(1);
        if k > 10_000 {
            return Err(EngineError::Io(format!("{dir}: too many files named {stem}")));
        }
        path = file(k);
    }
    names.next.insert(stem.to_owned(), k.saturating_add(1));
    names.taken.insert(path.clone());
    Ok(path)
}

/// Write `session` to `path`; returns the session with updated media paths, the number of audio
/// files written and the final path.
fn write_session(session: &Session, path: &str, overwrite: bool) -> Result<(Session, usize, PathBuf)> {
    use std::collections::{HashMap, HashSet};
    let p = session_file(path);
    // Before any audio is written: a session saved outside the write root changes nothing.
    crate::file_access::check_write(&p).map_err(EngineError::Io)?;
    let dir = p.parent().map(Path::to_path_buf).unwrap_or_default();
    let audio_dir = dir.join("Audio Files");
    let mut s = session.clone();
    let from = s.folder.clone();
    let same_dir = from.as_deref().is_some_and(|f| same_path(f, &dir));
    // A file can stay put for all the sources that name it, unless their audio differs.
    let shared_paths: Vec<bool> = {
        let mut audio_by_path: HashMap<&str, HashSet<Option<*const SourceAudio>>> = HashMap::new();
        for src in &s.sources {
            audio_by_path.entry(&src.path).or_default().insert(s.pool.get(src.id).map(Arc::as_ptr));
        }
        s.sources.iter().map(|src| audio_by_path.get(src.path.as_str()).is_some_and(|a| a.len() > 1)).collect()
    };
    // Files for audio in this save: sources playing the same audio share one. The files still to
    // write (their stem, audio and the sources naming them) are written once the size is checked.
    let mut files: HashMap<*const SourceAudio, String> = HashMap::new();
    let mut queued: HashMap<*const SourceAudio, usize> = HashMap::new();
    let mut pending: Vec<(String, Arc<SourceAudio>, Vec<usize>)> = Vec::new();
    for (i, src) in s.sources.iter_mut().enumerate() {
        if !same_dir
            && s.pool.get(src.id).is_none()
            && Path::new(&src.path).is_relative()
            && let Some(f) = &from
        {
            src.path = f.join(&src.path).to_string_lossy().into_owned();
        }
        let exists_rel = same_dir && !Path::new(&src.path).is_absolute() && dir.join(&src.path).exists();
        let shared = shared_paths.get(i).copied().unwrap_or(false);
        if exists_rel && !src.unsaved && !shared {
            continue;
        }
        let in_folder = Path::new(&src.path).strip_prefix(&audio_dir).ok().and_then(Path::to_str).map(str::to_string);
        if let Some(f) = in_folder.filter(|_| !src.unsaved && Path::new(&src.path).is_file()) {
            src.path = format!("Audio Files/{f}");
            continue;
        }
        let Some(audio) = s.pool.get(src.id) else { continue };
        if let Some(path) = files.get(&Arc::as_ptr(audio)) {
            src.path.clone_from(path);
            src.unsaved = false;
            src.format = FileFormat::Wav;
            continue;
        }
        if let Some((stem, _, sources)) = queued.get(&Arc::as_ptr(audio)).and_then(|&k| pending.get_mut(k)) {
            // As long as any name the file can get, so the size checked is an upper bound.
            src.path = format!("Audio Files/{stem}_9999.wav");
            src.unsaved = false;
            src.format = FileFormat::Wav;
            sources.push(i);
            continue;
        }
        let mut stem = sanitize_name(&src.name);
        if overwrite {
            // One file per source id so autosaves never collide or pile up.
            stem = format!("{stem}_{}", src.id.0);
            if audio_dir.join(format!("{stem}.wav")).exists() {
                src.path = format!("Audio Files/{stem}.wav");
                src.unsaved = false;
                files.insert(Arc::as_ptr(audio), src.path.clone());
                continue;
            }
        }
        src.path = format!("Audio Files/{stem}_9999.wav");
        src.unsaved = false;
        src.format = FileFormat::Wav;
        queued.insert(Arc::as_ptr(audio), pending.len());
        pending.push((stem, Arc::clone(audio), vec![i]));
    }
    s.folder = Some(dir.clone());
    if !overwrite && let Some(stem) = p.file_stem().and_then(OsStr::to_str) {
        s.name = stem.to_string();
    }
    // Before any audio is written: a session too large to open again is not saved (#892). With no
    // audio to write, the text checked is the text saved.
    let checked = if pending.is_empty() {
        let text = s.to_json().map_err(|err| EngineError::Io(err.to_string()))?;
        session_size(p.display(), text.len() as u64, true)?;
        Some(text)
    } else {
        session_size(p.display(), json_len(&s), true)?;
        None
    };
    let mut written = 0;
    for (stem, audio, sources) in &pending {
        make_dir(&audio_dir)?;
        let opts = EncodeOptions { format: FileFormat::Wav, bit_depth: soundcraft_audio_io::BitDepth::Float32, dither: false, bwf: None };
        let bytes = soundcraft_audio_io::encode(&audio.buffer, &opts).map_err(|err| EngineError::Io(err.to_string()))?;
        let (name, new) = store_wav(&audio_dir, stem, &bytes)?;
        for &i in sources {
            if let Some(src) = s.sources.get_mut(i) {
                src.path = format!("Audio Files/{name}");
            }
        }
        written += usize::from(new);
    }
    if written > 0 {
        sync_dir(&audio_dir);
    }
    let text = match checked {
        Some(text) => text,
        None => s.to_json().map_err(|err| EngineError::Io(err.to_string()))?,
    };
    make_dir(&dir)?;
    // Atomic-ish write: temp file then rename.
    let tmp = p.with_extension("scraft.tmp");
    if !overwrite && read_file(&p).is_ok_and(|old| old != text.as_bytes()) {
        back_up(&p);
    }
    replace_file(&tmp, &p, text.as_bytes())?;
    Ok((s, written, p))
}

/// Write `bytes` to `tmp`, flush it to disk and rename it over `file`, so `file` holds either the
/// old or the new contents, never a mix.
pub fn replace_file(tmp: &Path, file: &Path, bytes: &[u8]) -> Result<()> {
    crate::file_access::check_write(file).map_err(EngineError::Io)?;
    crate::file_access::check_write(tmp).map_err(EngineError::Io)?;
    replace_with(tmp, file, write_synced(tmp, bytes))
}

/// Renames `tmp` over `file` once it is `written`, or removes it when writing or renaming failed.
/// The caller has checked both paths ([`crate::file_access::check_write`]).
fn replace_with(tmp: &Path, file: &Path, written: std::io::Result<()>) -> Result<()> {
    if let Err(err) = written.and_then(|()| std::fs::rename(tmp, file)) {
        let _ = std::fs::remove_file(tmp);
        return Err(EngineError::Io(format!("{}: {err}", file.display())));
    }
    sync_dir(file.parent().unwrap_or(Path::new("")));
    Ok(())
}

fn sync_dir(dir: &Path) {
    let dir = if dir.as_os_str().is_empty() { Path::new(".") } else { dir };
    let _ = std::fs::File::open(dir).and_then(|d| d.sync_all());
}

fn write_synced(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let mut f = std::fs::File::create(path)?;
    f.write_all(bytes)?;
    f.sync_all()
}

/// Copies `from` to `to`, which must not exist yet; an existing entry, even a link, is never followed.
fn copy_new(from: &Path, to: &Path) -> std::io::Result<()> {
    let mut src = std::fs::File::open(from)?;
    let mut dst = std::fs::OpenOptions::new().write(true).create_new(true).open(to)?;
    if let Err(err) = std::io::copy(&mut src, &mut dst) {
        #[cfg(not(target_arch = "wasm32"))]
        drop(dst);
        let _ = std::fs::remove_file(to);
        return Err(err);
    }
    Ok(())
}

/// Copy the session file about to be replaced to `Backups/<name> backup <n>.<ext>`, keeping the
/// newest ten.
fn back_up(file: &Path) {
    let (Some(dir), Some(stem), Some(ext)) = (file.parent(), file.file_stem().and_then(OsStr::to_str), file.extension().and_then(OsStr::to_str))
    else {
        return;
    };
    let backups = dir.join("Backups");
    if let Err(err) = crate::file_access::check_write(&backups) {
        log::warn!("no backup of {}: {err}", file.display());
        return;
    }
    let prefix = format!("{stem} backup ");
    let suffix = format!(".{ext}");
    let numbers: Vec<u32> = std::fs::read_dir(&backups)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|f| f.file_name().to_str()?.strip_prefix(&prefix)?.strip_suffix(&suffix)?.parse().ok())
        .collect();
    let Some(next) = numbers.iter().max().map_or(Some(1), |n| n.checked_add(1)) else {
        log::warn!("no backup of {}: backup numbers are exhausted", file.display());
        return;
    };
    let dest = backups.join(format!("{prefix}{next}{suffix}"));
    if let Err(err) = crate::file_access::check_write(&dest) {
        log::warn!("no backup of {}: {err}", file.display());
        return;
    }
    if let Err(err) = std::fs::create_dir_all(&backups).and_then(|()| copy_new(file, &dest)) {
        log::warn!("no backup of {}: {err}", file.display());
        return;
    }
    for n in numbers.into_iter().filter(|n| n.saturating_add(10) <= next) {
        let _ = std::fs::remove_file(backups.join(format!("{prefix}{n}{suffix}")));
    }
}

/// The file a session saved to `path` goes to: `path`, with `.scraft` added when it has no
/// extension and isn't an existing file.
pub fn session_file(path: &str) -> PathBuf {
    let p = PathBuf::from(path);
    // Only a path the write root allows is probed, so a refused one can't be used to test for files.
    let is_file = crate::file_access::check_write(&p).is_ok() && p.is_file();
    if p.extension().is_none() && !is_file { p.with_extension(soundcraft_model::SESSION_EXTENSION) } else { p }
}

/// True when `command` would write a session over the existing `file`. Save, Save As and Save
/// Copy In may write the open session's own file; other commands never ask.
pub fn replaces(e: &Engine, command: &str, file: &Path) -> bool {
    match command {
        "session.save" | "session.save_as" | "session.save_copy" => {
            file.exists() && !e.path.as_deref().is_some_and(|own| same_path(Path::new(own), file))
        }
        "session.save_template" | "file.export_selected_tracks_as_session" | "file.export_session_text" => file.exists(),
        _ => false,
    }
}

fn same_path(a: &Path, b: &Path) -> bool {
    let full = |p: &Path| if p.as_os_str().is_empty() { Path::new(".").canonicalize() } else { p.canonicalize() };
    a == b || matches!((full(a), full(b)), (Ok(x), Ok(y)) if x == y)
}

/// Store `bytes` as the first of `<stem>.wav`, `<stem>_01.wav`, … in `dir` that is free or already
/// holds exactly these bytes, so saving never replaces audio; returns the name and whether it was
/// written.
fn store_wav(dir: &Path, stem: &str, bytes: &[u8]) -> Result<(String, bool)> {
    use std::io::Write;
    for k in 0..10_000u32 {
        let name = if k == 0 { format!("{stem}.wav") } else { format!("{stem}_{k:02}.wav") };
        let path = dir.join(&name);
        match std::fs::OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(mut f) => {
                if let Err(err) = f.write_all(bytes).and_then(|()| f.sync_all()) {
                    let _ = std::fs::remove_file(&path);
                    return Err(EngineError::Io(format!("{}: {err}", path.display())));
                }
                return Ok((name, true));
            }
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
                if std::fs::metadata(&path).is_ok_and(|m| m.len() == bytes.len() as u64) && std::fs::read(&path).is_ok_and(|b| b == bytes) {
                    return Ok((name, false));
                }
            }
            Err(err) => return Err(EngineError::Io(format!("{}: {err}", path.display()))),
        }
    }
    Err(EngineError::Io(format!("{}: no free file name for {stem}", dir.display())))
}

/// Audio files decoded so far, by path: a session may name one file thousands of times.
/// The one name of the file at `p` (links followed), so the same file named two ways is read once.
pub(crate) fn file_key(p: &Path) -> PathBuf {
    std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf())
}

/// Hands a finished file (name, bytes) to the person on a host without a filesystem: the web app
/// installs one that starts a download. Native builds write files and never install it.
pub type DeliverHook = fn(&str, &[u8]) -> std::result::Result<(), String>;

static DELIVER: std::sync::OnceLock<DeliverHook> = std::sync::OnceLock::new();

/// Install the delivery hook (once, at start-up of the web app).
pub fn set_deliver(h: DeliverHook) {
    let _ = DELIVER.set(h);
}

/// Write one output file: to `path` natively; in the browser, delivered under its file name.
/// Without this, every export on the web failed with «operation not supported on this platform».
pub fn write_file(path: &str, bytes: &[u8]) -> Result<()> {
    if cfg!(target_arch = "wasm32") {
        let deliver = DELIVER.get().ok_or_else(|| EngineError::Io("saving files is not available in this build".into()))?;
        let name = Path::new(path).file_name().and_then(OsStr::to_str).filter(|n| !n.is_empty()).unwrap_or("Untitled");
        return deliver(name, bytes).map_err(EngineError::Io);
    }
    crate::file_access::check_write(path).map_err(EngineError::Io)?;
    std::fs::write(path, bytes).map_err(|err| EngineError::Io(format!("{path}: {err}")))
}

/// Unpack a bundle: the one `.scraft` at its top level and the files beside it, by relative path.
fn read_bundle(bytes: &[u8], limits: BundleLimits) -> Result<(String, std::collections::HashMap<String, Vec<u8>>)> {
    use std::io::Read as _;
    let zerr = |err: zip::result::ZipError| EngineError::Io(format!("bundle: {err}"));
    let mut z = zip::ZipArchive::new(std::io::Cursor::new(bytes)).map_err(zerr)?;
    if z.len() > limits.entries {
        return Err(EngineError::Io("bundle: too many files".into()));
    }
    // Entries are stored, so the files we keep can only outgrow the bundle when several entries
    // point at the same bytes: a small bundle naming one payload thousands of times would be
    // copied out once per name.
    if z.has_overlapping_files().map_err(zerr)? {
        return Err(EngineError::Io("bundle: files share data".into()));
    }
    let (mut text, mut files, mut total) = (None, std::collections::HashMap::new(), 0u64);
    for i in 0..z.len() {
        let mut f = z.by_index(i).map_err(zerr)?;
        if f.is_dir() {
            continue;
        }
        let rel = f.name().map_err(zerr)?.to_string();
        let size = f.size();
        total = total.saturating_add(size);
        if total > limits.bytes {
            return Err(EngineError::Io("bundle: too large".into()));
        }
        let is_session = !rel.contains('/') && Path::new(&rel).extension().and_then(OsStr::to_str) == Some(soundcraft_model::SESSION_EXTENSION);
        if is_session && text.is_none() {
            session_size(format_args!("bundle: {rel}"), size, false)?;
        }
        let mut data = Vec::new();
        // The declared size is hostile too: never read past it.
        f.by_ref().take(size).read_to_end(&mut data).map_err(|err| EngineError::Io(format!("bundle: {rel}: {err}")))?;
        if is_session && text.is_none() {
            text = Some(String::from_utf8(data).map_err(|_| EngineError::Io(format!("bundle: {rel} is not text")))?);
        } else {
            files.insert(rel, data);
        }
    }
    let text = text.ok_or_else(|| EngineError::Io("bundle: no .scraft session inside".into()))?;
    Ok((text, files))
}

/// The record-enabled audio tracks, each with the input channels it records (those its input
/// route names, "In 3" → 2, wrapped to the device's `input_channels`) and a new take file
/// `<track>_<NN>.wav` in the `Audio Files` of the session's folder (`unsaved_dir` until it is saved).
pub fn plan_takes(e: &Engine, unsaved_dir: &Path, input_channels: usize) -> Vec<(TrackId, Vec<usize>, PathBuf)> {
    let dir = e.path.as_deref().and_then(|p| Path::new(p).parent()).unwrap_or(unsaved_dir);
    let s = e.session();
    let take_no = s.sources.len() + 1;
    s.tracks
        .iter()
        .filter(|t| t.mixer.record_arm && t.kind == TrackKind::Audio && !t.inactive)
        .map(|t| {
            let first = match &t.mixer.input {
                soundcraft_model::Route::Hardware(h) => {
                    h.trim_start_matches("In ").split(['-', ' ']).next().and_then(|n| n.parse::<usize>().ok()).map_or(0, |n| n.saturating_sub(1))
                }
                _ => 0,
            };
            let inputs = (0..t.channels()).map(|k| first.saturating_add(k) % input_channels.max(1)).collect();
            let file = |n: usize| dir.join("Audio Files").join(format!("{}_{n:02}.wav", sanitize_name(&t.name)));
            let path = (take_no..take_no + 1_000).map(file).find(|p| !p.exists()).unwrap_or_else(|| file(take_no));
            (t.id, inputs, path)
        })
        .collect()
}

#[cfg(test)]
mod stem_tests {
    #[test]
    fn stems_one_file_per_track() {
        let mut e = crate::demo::demo_engine();
        let dir = std::env::temp_dir().join(format!("soundcraft-stems-{}", std::process::id()));
        let r = e
            .execute("file.bounce_stems", &serde_json::json!({"dir": dir.to_string_lossy(), "start": {"seconds": 10.0}, "end": {"seconds": 11.0}}))
            .unwrap();
        let n = r["files"].as_array().map_or(0, Vec::len);
        assert_eq!(n, 7, "{r}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The largest gap between the 16-bit stem in `dir16` and the 32-bit float one in `dir32`, in LSBs.
    fn max_error_lsb(dir16: &std::path::Path, dir32: &std::path::Path) -> f64 {
        let mut worst = 0.0f64;
        let mut names: Vec<_> = std::fs::read_dir(dir32).unwrap().map(|f| f.unwrap().file_name()).collect();
        names.sort();
        assert!(!names.is_empty());
        for name in names {
            let (_, float) = soundcraft_audio_io::decode(&std::fs::read(dir32.join(&name)).unwrap(), None).unwrap();
            let (_, int) = soundcraft_audio_io::decode(&std::fs::read(dir16.join(&name)).unwrap(), None).unwrap();
            for (f, i) in float.channels.iter().zip(&int.channels) {
                for (a, b) in f.iter().zip(i) {
                    worst = worst.max((f64::from(a.clamp(-1.0, 1.0)) - f64::from(*b)).abs() * 32768.0);
                }
            }
        }
        worst
    }

    #[test]
    fn stems_dither_only_when_asked() {
        let mut e = crate::demo::demo_engine();
        let root = std::env::temp_dir().join(format!("soundcraft-stems-dither-{}", std::process::id()));
        let mut bounce = |name: &str, extra: serde_json::Value| {
            let dir = root.join(name);
            let mut p = serde_json::json!({"dir": dir.to_string_lossy(), "start": {"seconds": 10.0}, "end": {"seconds": 10.5}});
            for (k, v) in extra.as_object().unwrap() {
                p[k] = v.clone();
            }
            e.execute("file.bounce_stems", &p).unwrap();
            dir
        };
        let float = bounce("float", serde_json::json!({"bit_depth": "32f"}));
        let plain = bounce("plain", serde_json::json!({"bit_depth": "16", "dither": false}));
        let dithered = bounce("dithered", serde_json::json!({"bit_depth": "16"}));
        // Without dither every sample is the float one rounded to the nearest step; TPDF dither of
        // ±1 LSB moves some of them further than half a step.
        let plain_error = max_error_lsb(&plain, &float);
        let dithered_error = max_error_lsb(&dithered, &float);
        assert!(plain_error <= 0.5 + 1e-3, "{plain_error}");
        assert!(dithered_error > 0.6, "{dithered_error}");
        let _ = std::fs::remove_dir_all(&root);
    }
}

#[cfg(test)]
mod clip_tests {
    /// The browser has no disk to find earlier files on: same-named clips get their own names
    /// from the export itself, as they do from the folder natively (#1409).
    #[test]
    fn same_named_clips_get_their_own_names_with_nothing_on_disk() {
        let dir = std::env::temp_dir().join(format!("sc-clip-names-{}", std::process::id()));
        let mut names = super::ClipNames::default();
        for want in ["Take.wav", "Take-2.wav", "Take-3.wav"] {
            let path = super::clip_file(&dir.to_string_lossy(), "Take", &mut names, std::path::Path::exists).unwrap();
            assert_eq!(path, dir.join(want));
        }
        assert!(!dir.exists());
    }

    /// A file already in the folder is still skipped, and a run of same-named clips picks up where
    /// the last one left off instead of looking at every earlier name again for each clip.
    #[test]
    fn same_named_clips_skip_files_on_disk_and_do_not_count_up_from_the_start_each_time() {
        let dir = std::env::temp_dir().join(format!("sc-clip-disk-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("Take-2.wav"), b"").unwrap();
        let looked = std::cell::Cell::new(0usize);
        let on_disk = |p: &std::path::Path| {
            looked.set(looked.get() + 1);
            p.exists()
        };
        let mut names = super::ClipNames::default();
        for want in ["Take.wav", "Take-3.wav", "Take-4.wav"] {
            let path = super::clip_file(&dir.to_string_lossy(), "Take", &mut names, on_disk).unwrap();
            assert_eq!(path, dir.join(want));
        }
        let before = looked.get();
        for _ in 0..1_000 {
            super::clip_file(&dir.to_string_lossy(), "Take", &mut names, on_disk).unwrap();
        }
        assert_eq!(looked.get() - before, 1_000);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
