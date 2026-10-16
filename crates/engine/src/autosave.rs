//! Crash-recovery copies of open sessions, one folder each, written in the background, and the
//! recovery of the ones left behind with unsaved work.

use crate::io::{Recording, open_session, sanitize_name};
use crate::{Engine, EngineError, Result};
use soundcraft_audio_io::{BitDepth, EncodeOptions, FileFormat};
use soundcraft_model::{Session, SourceAudio, SourceId};
use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::sync::{Arc, Mutex, PoisonError, Weak};
use std::time::{Duration, Instant, SystemTime};

const SESSION: &str = "session.scraft";
const ORIGIN: &str = "origin";
const LOCK: &str = "lock";
/// The plan of a recording still running when the folder was written.
const RECORDING: &str = "recording.json";
/// While changes keep coming, the newest is written at most this often.
const INTERVAL: Duration = Duration::from_secs(1);
const STOPPED: &str = "the autosave thread stopped";

/// Writes autosaves on a thread of its own, so the UI never waits on the disk.
pub struct Autosaver {
    jobs: Option<Sender<Job>>,
    thread: Option<std::thread::JoinHandle<()>>,
    failure: Arc<Mutex<Option<String>>>,
}

enum Job {
    Write(Arc<Session>, Option<String>, Option<Recording>),
    /// Done with the session's folder: keep it for the next launch, or delete it.
    Close(bool),
    Flush(Sender<std::result::Result<(), String>>),
}

impl Autosaver {
    /// A writer for autosave folders under `root`.
    pub fn new(root: PathBuf) -> Autosaver {
        let (tx, rx) = channel();
        let failure = Arc::new(Mutex::new(None));
        let writer = Writer { root, failure: Arc::clone(&failure), folder: None, pending: None, last: None, written: Ok(()) };
        let thread = std::thread::Builder::new().name("soundcraft-autosave".into()).spawn(move || writer.run(&rx));
        if let Err(e) = &thread {
            report(&failure, format!("{STOPPED}: {e}"));
        }
        Autosaver { jobs: Some(tx), thread: thread.ok(), failure }
    }

    /// Autosave `session`, opened from the session file `origin`, soon, with the plan of the
    /// recording running in it.
    pub fn write(&self, session: Arc<Session>, origin: Option<String>, recording: Option<Recording>) {
        self.send(Job::Write(session, origin, recording));
    }

    /// Done with the open session's folder: keep it for the next launch, or delete it.
    pub fn close(&self, keep: bool) {
        self.send(Job::Close(keep));
    }

    /// Wait until everything handed over so far is done. A failure returned here isn't reported
    /// again by `failure`.
    pub fn flush(&self) -> Result<()> {
        let (tx, rx) = channel();
        self.send(Job::Flush(tx));
        let done = rx.recv().unwrap_or_else(|_| Err(STOPPED.into()));
        if done.is_err() {
            self.failure();
        }
        done.map_err(EngineError::Io)
    }

    /// The newest failure since the last call.
    pub fn failure(&self) -> Option<String> {
        self.failure.lock().unwrap_or_else(PoisonError::into_inner).take()
    }

    fn send(&self, job: Job) {
        if self.jobs.as_ref().is_none_or(|tx| tx.send(job).is_err()) {
            report(&self.failure, STOPPED.into());
        }
    }
}

impl Drop for Autosaver {
    /// What was handed over is written, and an open folder stays for the next launch.
    fn drop(&mut self) {
        drop(self.jobs.take());
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// The writer thread.
struct Writer {
    root: PathBuf,
    failure: Arc<Mutex<Option<String>>>,
    /// The open session's folder.
    folder: Option<Autosave>,
    /// The newest session not written yet.
    pending: Option<(Arc<Session>, Option<String>, Option<Recording>)>,
    last: Option<Instant>,
    /// How the newest write into the open folder went, for `Flush`.
    written: std::result::Result<(), String>,
}

impl Writer {
    fn run(mut self, jobs: &Receiver<Job>) {
        loop {
            let wait = match (&self.pending, self.last) {
                (None, _) => Duration::MAX,
                (Some(_), last) => last.map_or(Duration::ZERO, |t| INTERVAL.saturating_sub(t.elapsed())),
            };
            match jobs.recv_timeout(wait) {
                Ok(Job::Write(session, origin, recording)) => self.pending = Some((session, origin, recording)),
                Ok(Job::Flush(reply)) => {
                    self.write_pending();
                    let _ = reply.send(self.written.clone());
                }
                Ok(Job::Close(keep)) => {
                    if keep {
                        self.write_pending();
                    }
                    self.pending = None;
                    self.written = Ok(());
                    if let Some(a) = self.folder.take()
                        && !keep
                    {
                        a.discard();
                    }
                }
                Err(RecvTimeoutError::Timeout) => self.write_pending(),
                Err(RecvTimeoutError::Disconnected) => {
                    self.write_pending();
                    return;
                }
            }
        }
    }

    fn write_pending(&mut self) {
        let Some((session, origin, recording)) = self.pending.take() else { return };
        self.last = Some(Instant::now());
        let written = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> Result<()> {
            if self.folder.is_none() {
                self.folder = Some(Autosave::create(&self.root, &session.name)?);
            }
            self.folder.as_mut().map_or(Ok(()), |a| a.write(&session, origin.as_deref(), recording.as_ref()))
        }));
        self.written = match written {
            Ok(written) => written.map_err(|e| e.to_string()),
            Err(_) => Err("internal error (see the log)".into()),
        };
        if let Err(e) = &self.written {
            report(&self.failure, e.clone());
        }
    }
}

fn report(failure: &Mutex<Option<String>>, e: String) {
    log::warn!("autosave failed: {e}");
    *failure.lock().unwrap_or_else(PoisonError::into_inner) = Some(e);
}

/// An open session's autosave folder, locked while the session is open so a later launch can tell
/// it from one a crash left behind.
struct Autosave {
    dir: PathBuf,
    lock: Option<std::fs::File>,
    copies: BTreeMap<SourceId, (Weak<SourceAudio>, String)>,
    next: u64,
    /// A recording's plan is in the folder.
    recording: bool,
}

impl Autosave {
    /// A new folder under `root` for the session called `name`.
    fn create(root: &Path, name: &str) -> Result<Autosave> {
        let stamp = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap_or_default().as_millis();
        Self::create_with_stamp(root, name, stamp)
    }

    fn create_with_stamp(root: &Path, name: &str, stamp: u128) -> Result<Autosave> {
        std::fs::create_dir_all(root).map_err(|err| io_error(root, err))?;
        let prefix = format!("{} {stamp}-{}", sanitize_name(name), std::process::id());
        // Several sessions can close in one clock tick. Claim a fresh directory atomically;
        // reopening an existing one could overwrite work another session left for recovery.
        for suffix in 0..1024 {
            let dir = root.join(format!("{prefix}-{suffix}"));
            match std::fs::create_dir(&dir) {
                Ok(()) => {
                    let lock = std::fs::File::create(dir.join(LOCK)).map_err(|err| io_error(&dir, err))?;
                    lock.try_lock().map_err(|err| EngineError::Io(format!("{}: {err}", dir.display())))?;
                    return Ok(Autosave { dir, lock: Some(lock), copies: BTreeMap::new(), next: 1, recording: false });
                }
                Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(err) => return Err(io_error(&dir, err)),
            }
        }
        Err(EngineError::Io(format!("{}: could not allocate a new autosave folder", root.display())))
    }

    /// Write `session`, opened from the session file `origin`, and the plan of the `recording`
    /// running in it. Audio saved with it is referenced where it is; audio that exists only in
    /// memory is copied, once per recording or render.
    fn write(&mut self, session: &Session, origin: Option<&str>, recording: Option<&Recording>) -> Result<()> {
        let base = origin.and_then(|p| Path::new(p).parent()).and_then(|p| std::path::absolute(p).ok());
        let audio_dir = self.dir.join("Audio Files");
        let mut s = session.clone();
        let mut copies = BTreeMap::new();
        for src in &mut s.sources {
            let file = if Path::new(&src.path).is_absolute() { Some(PathBuf::from(&src.path)) } else { base.as_ref().map(|b| b.join(&src.path)) };
            let audio = s.pool.get(src.id);
            if let Some(f) = &file
                && (!src.unsaved && f.is_file() || audio.is_none())
            {
                src.path = f.to_string_lossy().into_owned();
                continue;
            }
            let Some(audio) = audio else { continue };
            let name = match self.copies.remove(&src.id) {
                Some((copy, name)) if copy.upgrade().is_some_and(|a| Arc::ptr_eq(&a, audio)) => name,
                _ => {
                    let name = format!("{} {}.wav", sanitize_name(&src.name), self.next);
                    self.next += 1;
                    let opts = EncodeOptions { format: FileFormat::Wav, bit_depth: BitDepth::Float32, dither: false, bwf: None };
                    let bytes = soundcraft_audio_io::encode(&audio.buffer, &opts).map_err(|err| EngineError::Io(err.to_string()))?;
                    std::fs::create_dir_all(&audio_dir)
                        .and_then(|()| write_synced(&audio_dir.join(&name), &bytes))
                        .map_err(|err| io_error(&audio_dir, err))?;
                    name
                }
            };
            src.path = audio_dir.join(&name).to_string_lossy().into_owned();
            src.unsaved = true;
            copies.insert(src.id, (Arc::downgrade(audio), name));
        }
        let text = s.to_json().map_err(|err| EngineError::Io(err.to_string()))?;
        write_atomically(&self.dir.join(ORIGIN), origin.unwrap_or_default().as_bytes())?;
        write_atomically(&self.dir.join(SESSION), text.as_bytes())?;
        for (_, name) in std::mem::replace(&mut self.copies, copies).into_values() {
            let _ = std::fs::remove_file(audio_dir.join(name));
        }
        if let Some(r) = recording {
            let plan = serde_json::to_string(r).map_err(|err| EngineError::Io(err.to_string()))?;
            write_atomically(&self.dir.join(RECORDING), plan.as_bytes())?;
        } else if self.recording {
            let _ = std::fs::remove_file(self.dir.join(RECORDING));
        }
        self.recording = recording.is_some();
        Ok(())
    }

    /// Delete the folder: the session closed with nothing left to recover.
    fn discard(mut self) {
        drop(self.lock.take());
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// Unsaved work left behind in an autosave folder.
#[derive(Debug, Clone, PartialEq)]
pub struct Leftover {
    pub dir: PathBuf,
    /// The session's name.
    pub name: String,
    /// The session file it was autosaved from, if it had one.
    pub origin: Option<String>,
    pub saved: SystemTime,
    /// A recording was still running in it.
    pub recording: bool,
}

/// Leftover autosaves under `root`, newest first: folders no open session holds, written after the
/// session file they came from or holding a recording. Equal timestamps are ambiguous, so those
/// copies are retained too: a filesystem clock tick can cover both writes. Empty and provably
/// outdated ones are deleted.
pub fn leftovers(root: &Path) -> Vec<Leftover> {
    let mut out = Vec::new();
    for dir in std::fs::read_dir(root).into_iter().flatten().flatten().map(|d| d.path()) {
        let Ok(lock) = std::fs::OpenOptions::new().write(true).open(dir.join(LOCK)) else { continue };
        if lock.try_lock().is_err() {
            continue;
        }
        let origin = std::fs::read_to_string(dir.join(ORIGIN)).ok().filter(|o| !o.is_empty());
        let origin_saved = origin.as_ref().and_then(|o| std::fs::metadata(o).and_then(|m| m.modified()).ok());
        let saved = std::fs::metadata(dir.join(SESSION)).and_then(|m| m.modified());
        let recording = dir.join(RECORDING).is_file();
        drop(lock);
        match saved {
            Ok(saved) if recording || origin_saved.is_none_or(|o| saved >= o) => {
                let text = std::fs::read_to_string(dir.join(SESSION)).unwrap_or_default();
                let named = serde_json::from_str::<serde_json::Value>(&text).ok().and_then(|v| v.get("name")?.as_str().map(str::to_string));
                let name = named.unwrap_or_else(|| dir.file_name().unwrap_or_default().to_string_lossy().into_owned());
                out.push(Leftover { dir, name, origin, saved, recording });
            }
            _ => {
                let _ = std::fs::remove_dir_all(&dir);
            }
        }
    }
    out.sort_by_key(|l| std::cmp::Reverse(l.saved));
    out
}

/// Open `left` as an unsaved version of the session it came from, so Save writes it back there,
/// with the takes of a recording that never finished placed as Stop would have. Returns the
/// media that couldn't be loaded.
pub fn recover(e: &mut Engine, left: &Leftover) -> Result<Vec<String>> {
    let mut r = Engine::default();
    let mut missing = open_session(&mut r, &left.dir.join(SESSION).to_string_lossy())?;
    let mut s = r.session().clone();
    let unfinished = unfinished_takes(left, &s, &mut missing);
    if let Some(home) = left.origin.as_deref().and_then(|o| Path::new(o).parent()) {
        for src in s.sources.iter_mut().filter(|src| !src.unsaved) {
            if let Ok(rel) = Path::new(&src.path).strip_prefix(home) {
                src.path = rel.components().map(|c| c.as_os_str().to_string_lossy()).collect::<Vec<_>>().join("/");
            }
        }
    }
    e.replace_session(Session::default());
    *e.session_mut() = s;
    e.path = left.origin.clone();
    if let Some((plan, takes)) = unfinished {
        crate::io::place_recording(e, &plan, takes);
    }
    Ok(missing)
}

/// The takes of the recording `left` holds the plan of, repaired, minus those already in `s`.
/// A take the plan names that isn't a take file SoundCraft recorded, in a folder it records into,
/// is left untouched and added to `skipped`.
fn unfinished_takes(left: &Leftover, s: &Session, skipped: &mut Vec<String>) -> Option<(Recording, Vec<(soundcraft_model::TrackId, PathBuf)>)> {
    let plan: Recording = serde_json::from_str(&std::fs::read_to_string(left.dir.join(RECORDING)).ok()?).ok()?;
    let known: Vec<PathBuf> = s.sources.iter().filter_map(|src| std::fs::canonicalize(&src.path).ok()).collect();
    let folders = take_folders(left);
    let mut takes = Vec::new();
    for t in &plan.takes {
        if std::fs::canonicalize(&t.path).is_ok_and(|p| known.contains(&p)) {
            continue;
        }
        match repair_take(&t.path, &folders, t.channels, plan.rate) {
            Ok(frames) if frames > 0 => takes.push((t.track, t.path.clone())),
            Ok(_) => {}
            Err(err) => {
                log::warn!("recovery skipped the take {}: {err}", t.path.display());
                skipped.push(format!("{}: {err}", t.path.display()));
            }
        }
    }
    Some((plan, takes))
}

/// The folders a recording in `left`'s session writes its takes into: the `Audio Files` next to
/// the session file, or, never saved, the one in its folder under the autosave root.
fn take_folders(left: &Leftover) -> Vec<PathBuf> {
    let saved = left.origin.as_deref().and_then(|o| Path::new(o).parent()).map(|d| d.join("Audio Files"));
    let unsaved = left.dir.parent().map(|root| root.join(sanitize_name(&left.name)).join("Audio Files"));
    saved.into_iter().chain(unsaved).filter_map(|d| std::fs::canonicalize(d).ok()).collect()
}

/// Make a take file's header cover all the audio in it; returns its frames. The recorder
/// rewrites the header only every second, so a crash leaves up to a second uncovered. Only a
/// regular file in one of `folders` that starts with the header the recorder writes is touched.
fn repair_take(path: &Path, folders: &[PathBuf], channels: usize, rate: u32) -> std::io::Result<u64> {
    let refuse = |why: &str| std::io::Error::new(std::io::ErrorKind::InvalidData, why.to_string());
    if !std::fs::symlink_metadata(path)?.file_type().is_file() {
        return Err(refuse("not a regular file"));
    }
    let folder = path.parent().and_then(|d| std::fs::canonicalize(d).ok());
    if !folder.is_some_and(|d| folders.contains(&d)) {
        return Err(refuse("not in a folder SoundCraft records into"));
    }
    let header = |frames| soundcraft_audio_io::wav_float_header(channels, rate, frames).map_err(std::io::Error::other);
    let expected = header(0)?;
    let mut f = std::fs::OpenOptions::new().read(true).write(true).open(path)?;
    let mut head = vec![0u8; expected.len()];
    if std::io::Read::read_exact(&mut f, &mut head).is_err() || !is_take_header(&head, &expected) {
        return Err(refuse("not a take SoundCraft recorded"));
    }
    let audio = f.metadata()?.len().saturating_sub(expected.len() as u64);
    let frames = audio / (channels.max(1) as u64 * 4);
    std::io::Seek::rewind(&mut f)?;
    f.write_all(&header(frames)?)?;
    f.sync_all()?;
    Ok(frames)
}

/// `head` is a take header like `expected` (the recorder's header for no frames yet): RIFF or
/// RF64 WAVE, the same format chunk at the same place, and the data chunk right after it all.
/// Only the sizes and frame counts may differ.
fn is_take_header(head: &[u8], expected: &[u8]) -> bool {
    let magic = matches!(head.get(..4), Some(b"RIFF" | b"RF64")) && head.get(8..12) == Some(b"WAVE");
    let fmt = expected.windows(4).position(|w| w == b"fmt ");
    let fmt_len = fmt.and_then(|at| expected.get(at + 4..at + 8)).and_then(|b| b.try_into().ok()).map(u32::from_le_bytes);
    let same_fmt = match (fmt, fmt_len) {
        (Some(at), Some(n)) => {
            let end = at.saturating_add(8).saturating_add(n as usize);
            expected.get(at..end).is_some_and(|chunk| head.get(at..end) == Some(chunk))
        }
        _ => false,
    };
    let data = expected.len().checked_sub(8).and_then(|at| head.get(at..at + 4)) == Some(b"data");
    magic && same_fmt && data
}

/// Delete a leftover autosave.
pub fn discard(left: &Leftover) {
    let _ = std::fs::remove_dir_all(&left.dir);
}

fn write_synced(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut f = std::fs::File::create(path)?;
    f.write_all(bytes)?;
    f.sync_all()
}

fn write_atomically(path: &Path, bytes: &[u8]) -> Result<()> {
    let tmp = path.with_extension("tmp");
    write_synced(&tmp, bytes).and_then(|()| std::fs::rename(&tmp, path)).map_err(|err| {
        let _ = std::fs::remove_file(&tmp);
        io_error(path, err)
    })
}

fn io_error(path: &Path, err: std::io::Error) -> EngineError {
    EngineError::Io(format!("{}: {err}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use soundcraft_audio_io::AudioBuffer;
    use soundcraft_model::{ChannelFormat, Clip, TrackKind};

    fn folder(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("soundcraft-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    fn set_modified(path: &Path, time: SystemTime) {
        std::fs::OpenOptions::new().write(true).open(path).unwrap().set_modified(time).unwrap();
        assert_eq!(std::fs::metadata(path).unwrap().modified().unwrap(), time);
    }

    fn record(e: &mut Engine, level: f32) {
        let before = e.session_arc();
        let s = e.session_mut();
        let t = match s.tracks.first() {
            Some(t) => t.id,
            None => s.add_track(TrackKind::Audio, ChannelFormat::Mono, None),
        };
        let src =
            crate::io::add_source(s, "Audio 1_01", AudioBuffer { sample_rate: 48_000, channels: vec![vec![level; 4_800]] }, None, FileFormat::Wav);
        let id = s.new_clip_id();
        crate::edit::place_clip(s, t, Clip::audio(id, "Audio 1_01", src, 0, 0, 4_800));
        e.push_undo("Record", before);
    }

    fn levels(e: &Engine) -> Vec<f32> {
        let s = e.session();
        s.sources.iter().map(|src| s.pool.get(src.id).map_or(f32::NAN, |a| a.buffer.channels[0][0])).collect()
    }

    #[test]
    fn saved_audio_is_referenced_and_takes_are_copied_once() {
        let dir = folder("autosave-copies");
        let path = dir.join("Song/Song.scraft");
        let mut e = Engine::default();
        record(&mut e, 0.5);
        crate::io::save_session(&mut e, &path.to_string_lossy()).unwrap();
        record(&mut e, -0.25);
        let mut a = Autosave::create(&dir.join("Autosave"), "Song").unwrap();
        a.write(e.session(), e.path.as_deref(), None).unwrap();
        a.write(e.session(), e.path.as_deref(), None).unwrap();
        assert_eq!(std::fs::read_dir(a.dir.join("Audio Files")).unwrap().count(), 1);
        drop(a);
        let left = leftovers(&dir.join("Autosave"));
        let mut r = Engine::default();
        assert_eq!(recover(&mut r, &left[0]).unwrap(), Vec::<String>::new());
        let paths: Vec<_> = r.session().sources.iter().map(|s| (s.path.starts_with("Audio Files/"), s.unsaved)).collect();
        assert_eq!(paths, [(true, false), (false, true)]);
        assert_eq!(levels(&r), [0.5, -0.25]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn equal_timestamps_keep_unsaved_audio_for_recovery() {
        let dir = folder("autosave-equal-timestamps");
        let path = dir.join("Song/Song.scraft");
        let root = dir.join("Autosave");
        let mut e = Engine::default();
        record(&mut e, 0.5);
        crate::io::save_session(&mut e, &path.to_string_lossy()).unwrap();
        record(&mut e, -0.25);
        let mut a = Autosave::create(&root, "Song").unwrap();
        a.write(e.session(), e.path.as_deref(), None).unwrap();
        let saved = a.dir.join(SESSION);
        let same_time = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
        set_modified(&path, same_time);
        set_modified(&saved, same_time);
        assert!(leftovers(&root).is_empty(), "a live session still owns its recovery copy");
        drop(a);
        let left = leftovers(&root);
        assert_eq!(left.len(), 1, "equal timestamps cannot prove unsaved work obsolete");
        assert!(saved.is_file());
        let mut recovered = Engine::default();
        assert!(recover(&mut recovered, &left[0]).unwrap().is_empty());
        assert_eq!(levels(&recovered), [0.5, -0.25]);
        recovered.execute("session.save", &json!({})).unwrap();
        set_modified(&path, same_time + Duration::from_secs(1));
        assert!(leftovers(&root).is_empty());
        assert!(!left[0].dir.exists());
        let mut reopened = Engine::default();
        open_session(&mut reopened, &path.to_string_lossy()).unwrap();
        assert_eq!(levels(&reopened), [0.5, -0.25]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn same_tick_autosaves_preserve_each_closed_session() {
        let root = folder("autosave-same-tick");
        let mut first = Engine::default();
        record(&mut first, 0.5);
        let mut a = Autosave::create_with_stamp(&root, "Untitled", 1_000_000).unwrap();
        a.write(first.session(), None, None).unwrap();
        let first_dir = a.dir.clone();
        let first_session = std::fs::read(first_dir.join(SESSION)).unwrap();
        let first_audio = std::fs::read_dir(first_dir.join("Audio Files")).unwrap().next().unwrap().unwrap().path();
        let first_samples = std::fs::read(&first_audio).unwrap();
        drop(a);
        let mut second = Engine::default();
        record(&mut second, -0.25);
        let mut b = Autosave::create_with_stamp(&root, "Untitled", 1_000_000).unwrap();
        b.write(second.session(), None, None).unwrap();
        let second_dir = b.dir.clone();
        drop(b);
        assert_ne!(first_dir, second_dir, "a new autosave must not reopen an earlier session's folder");
        assert_eq!(std::fs::read(first_dir.join(SESSION)).unwrap(), first_session);
        assert_eq!(std::fs::read(first_audio).unwrap(), first_samples);
        let left = leftovers(&root);
        assert_eq!(left.len(), 2);
        for (dir, expected) in [(first_dir, 0.5), (second_dir, -0.25)] {
            let copy = left.iter().find(|copy| copy.dir == dir).unwrap();
            let mut restored = Engine::default();
            assert!(recover(&mut restored, copy).unwrap().is_empty());
            assert_eq!(levels(&restored), [expected]);
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn same_tick_autosaves_do_not_reuse_a_live_folder() {
        let root = folder("autosave-live-same-tick");
        let first = Autosave::create_with_stamp(&root, "Untitled", 1_000_000).unwrap();
        let second = Autosave::create_with_stamp(&root, "Untitled", 1_000_000).unwrap();
        assert_ne!(first.dir, second.dir);
        assert!(first.lock.is_some() && second.lock.is_some());
        drop((first, second));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_take_recorded_after_undo_gets_its_own_copy() {
        let dir = folder("autosave-undo");
        let mut e = Engine::default();
        record(&mut e, 0.5);
        let mut a = Autosave::create(&dir, "Untitled").unwrap();
        a.write(e.session(), None, None).unwrap();
        e.execute("edit.undo", &json!({})).unwrap();
        record(&mut e, -0.25);
        a.write(e.session(), None, None).unwrap();
        drop(a);
        let mut r = Engine::default();
        recover(&mut r, &leftovers(&dir)[0]).unwrap();
        assert_eq!(levels(&r), [-0.25]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_writer_saves_the_newest_change_and_drops_the_folder_when_done() {
        let root = folder("autosave-writer");
        let w = Autosaver::new(root.clone());
        let mut e = Engine::default();
        for _ in 0..5 {
            record(&mut e, 0.5);
            w.write(e.session_arc(), None, None);
        }
        w.flush().unwrap();
        let file = std::fs::read_dir(&root).unwrap().flatten().map(|d| d.path().join(SESSION)).collect::<Vec<_>>();
        let mut r = Engine::default();
        open_session(&mut r, &file[0].to_string_lossy()).unwrap();
        assert_eq!((file.len(), r.session().sources.len()), (1, 5));
        w.close(false);
        w.flush().unwrap();
        assert_eq!(std::fs::read_dir(&root).unwrap().count(), 0);
        record(&mut e, 0.5);
        w.write(e.session_arc(), None, None);
        drop(w);
        let mut r = Engine::default();
        recover(&mut r, &leftovers(&root)[0]).unwrap();
        assert_eq!(r.session().sources.len(), 6);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_take_still_recording_at_a_crash_is_placed_on_recovery() {
        use crate::io::Take;
        let dir = folder("autosave-take");
        let root = dir.join("Autosave");
        let mut e = Engine::default();
        let track =
            soundcraft_model::TrackId(e.execute("track.new", &json!({"name": "Vox", "format": "mono"})).unwrap()["tracks"][0].as_u64().unwrap());
        let take = root.join(sanitize_name(&e.session().name)).join("Audio Files/Vox_01.wav");
        std::fs::create_dir_all(take.parent().unwrap()).unwrap();
        let audio: Vec<u8> = (0..480).flat_map(|i| (i as f32 / 1000.0).to_le_bytes()).collect();
        std::fs::write(&take, [soundcraft_audio_io::wav_float_header(1, 48_000, 100).unwrap(), audio].concat()).unwrap();
        let plan = Recording { start: 48_000, pass: None, punch: None, rate: 48_000, takes: vec![Take { track, path: take, channels: 1 }] };
        let clips = |e: &Engine| e.session().track(track).unwrap().clips().iter().map(|c| (c.start, c.length)).collect::<Vec<_>>();
        let w = Autosaver::new(root.clone());
        w.write(e.session_arc(), None, Some(plan.clone()));
        drop(w);
        let left = leftovers(&root);
        assert!(left[0].recording);
        let mut r = Engine::default();
        recover(&mut r, &left[0]).unwrap();
        assert_eq!(clips(&r), [(48_000, 480)], "all of the take, its last second included");
        discard(&left[0]);
        let w = Autosaver::new(root.clone());
        w.write(r.session_arc(), None, Some(plan));
        drop(w);
        let mut again = Engine::default();
        recover(&mut again, &leftovers(&root)[0]).unwrap();
        assert_eq!((clips(&again), again.session().sources.len()), (vec![(48_000, 480)], 1), "placed once");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn recovery_never_writes_a_file_that_isnt_a_take_in_a_take_folder() {
        use crate::io::Take;
        let dir = folder("autosave-hostile-take");
        let root = dir.join("Autosave");
        let mut e = Engine::default();
        let track =
            soundcraft_model::TrackId(e.execute("track.new", &json!({"name": "Vox", "format": "mono"})).unwrap()["tracks"][0].as_u64().unwrap());
        let takes = root.join(sanitize_name(&e.session().name)).join("Audio Files");
        std::fs::create_dir_all(&takes).unwrap();
        let wav = [soundcraft_audio_io::wav_float_header(1, 48_000, 0).unwrap(), vec![0; 4_000]].concat();
        let text = b"not audio at all, but long enough to be mistaken for a header if nobody looked at it ......".repeat(4);
        let files =
            [(takes.join("notes.wav"), text.clone()), (dir.join("Elsewhere/Vox_01.wav"), wav.clone()), (takes.join("../Vox_02.wav"), wav.clone())];
        for (path, bytes) in &files {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, bytes).unwrap();
        }
        let paths: Vec<PathBuf> = files.iter().map(|(p, _)| p.clone()).collect();
        #[cfg(unix)]
        let paths = {
            let mut paths = paths;
            let link = takes.join("Vox_03.wav");
            std::os::unix::fs::symlink(&files[1].0, &link).unwrap();
            paths.push(link);
            paths
        };
        let plan = Recording {
            start: 0,
            pass: None,
            punch: None,
            rate: 48_000,
            takes: paths.iter().map(|p| Take { track, path: p.clone(), channels: 1 }).collect(),
        };
        let w = Autosaver::new(root.clone());
        w.write(e.session_arc(), None, Some(plan));
        drop(w);
        let mut r = Engine::default();
        let skipped = recover(&mut r, &leftovers(&root)[0]).unwrap();
        assert_eq!(skipped.len(), paths.len(), "{skipped:?}");
        assert!(r.session().sources.is_empty());
        for (path, bytes) in &files {
            assert_eq!(&std::fs::read(path).unwrap(), bytes, "{} untouched", path.display());
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn unsaved_work_is_offered_once_its_session_closes() {
        let dir = folder("autosave-leftovers");
        let path = dir.join("Song.scraft").to_string_lossy().into_owned();
        let root = dir.join("Autosave");
        std::fs::create_dir_all(&dir).unwrap();
        let mut e = Engine::default();
        crate::io::save_session(&mut e, &path).unwrap();
        record(&mut e, 0.5);
        let mut a = Autosave::create(&root, "Song").unwrap();
        a.write(e.session(), e.path.as_deref(), None).unwrap();
        assert!(leftovers(&root).is_empty());
        drop(a);
        let left = leftovers(&root);
        assert_eq!((left.len(), left[0].name.as_str(), left[0].origin.as_deref()), (1, "Song", Some(path.as_str())));
        let mut r = Engine::default();
        recover(&mut r, &left[0]).unwrap();
        assert!(r.is_dirty());
        r.execute("session.save", &json!({})).unwrap();
        let autosaved = std::fs::metadata(left[0].dir.join(SESSION)).unwrap().modified().unwrap();
        set_modified(Path::new(&path), autosaved + Duration::from_secs(1));
        assert!(leftovers(&root).is_empty());
        assert!(!left[0].dir.exists());
        let mut reopened = Engine::default();
        open_session(&mut reopened, &path).unwrap();
        assert_eq!(levels(&reopened), [0.5]);
        Autosave::create(&root, "Song").unwrap().discard();
        assert_eq!(std::fs::read_dir(&root).unwrap().count(), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
