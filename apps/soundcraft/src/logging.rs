//! The desktop app's logger: `log::` records go to standard error and to a log file.
//!
//! Without a logger every `log::warn!`/`log::error!` in the workspace (the engine's panic report,
//! audio devices that failed to open or broke, plugins that refused their stored state or failed
//! to activate, CLAP plugins' own log messages, decode errors on import) vanished. A launch from a
//! desktop menu has no terminal, so the file is what a bug report can attach:
//! `<settings dir>/logs/soundcraft.log`, beside `ui.json`, `Autosave/` and `Presets/`. Each start
//! moves the previous log to `soundcraft.1.log` (and that one to `.2`), so the log of a run that
//! crashed survives the next launch. Runs without preferences (`SOUNDCRAFT_NO_PREFS`, agents' test
//! runs) log to standard error only, so they don't rotate away the user's own logs.
//!
//! Levels: `info` for SoundCraft's own crates, `warn` for everything else (wgpu and naga are
//! chatty). `RUST_LOG` replaces that with env_logger-style directives: `debug`,
//! `warn,soundcraft_mix=trace`, `wgpu_core=info`. A directive ending in `*` matches every target
//! that starts with it (`soundcraft*=debug`).
//!
//! Records logged before the settings directory is known are kept (up to [`MAX_PENDING`]) and
//! written once the file is attached. Writing never panics: a file that can't be created or
//! written leaves standard error as the only sink.
//!
//! Realtime audio: a record takes a lock and writes to standard error and a file, which the audio
//! callback must never do. Code that runs on the audio thread logs anyway (the cpal error callback,
//! the synth's full event queue, a hosted plugin's failed `process`, a CLAP plugin calling the
//! host's `log` from its audio thread, the panic hook). So the cpal callbacks mark their thread
//! (`soundcraft_playback::mark_audio_thread`; the mix engine carries the mark into the worker
//! threads that render strips for it), and a record from a marked thread is not written
//! there: the first one is kept in a fixed buffer (`try_lock`, no allocation, never waits) and the
//! rest are only counted (an atomic). Another thread writes them out: the UI thread every frame
//! ([`AppLogger::report_audio_thread`]) and any other thread before its own next record.

use std::fmt::Write as _;
use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock, PoisonError};
use std::time::SystemTime;

use log::LevelFilter;

/// The current log file's name inside the log directory.
pub const LOG_FILE: &str = "soundcraft.log";
/// The log directory inside the settings directory.
pub const LOG_DIR: &str = "logs";
/// How many previous logs are kept (`soundcraft.1.log` … `soundcraft.<KEEP>.log`).
pub const KEEP: usize = 2;
/// The log file stops growing past this size (a runaway warning can't fill the disk).
pub const MAX_FILE_BYTES: u64 = 16 * 1024 * 1024;
/// Records kept in memory until the log file is attached.
pub const MAX_PENDING: usize = 512;
/// The built-in filter when `RUST_LOG` is unset or empty.
pub const DEFAULT_FILTER: &str = "warn,soundcraft*=info";
/// The longest message kept from the audio thread (longer ones are cut, marked with `…`).
pub const REALTIME_MESSAGE_BYTES: usize = 480;
/// The longest target kept from the audio thread.
const REALTIME_TARGET_BYTES: usize = 96;
/// The thread name shown for records from the audio thread.
const AUDIO_THREAD: &str = "audio";

/// Per-target level filter parsed from env_logger-style directives.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Filter {
    default: LevelFilter,
    directives: Vec<(String, LevelFilter)>,
}

impl Filter {
    /// Parse `spec`; unknown levels and empty parts are skipped. Without a bare level, targets no
    /// directive names log errors only.
    pub fn parse(spec: &str) -> Filter {
        let mut f = Filter { default: LevelFilter::Error, directives: Vec::new() };
        for part in spec.split(',').map(str::trim).filter(|p| !p.is_empty()) {
            match part.split_once('=') {
                Some((name, level)) => {
                    let name = name.trim();
                    if let (false, Some(level)) = (name.is_empty(), parse_level(level)) {
                        f.directives.push((name.to_owned(), level));
                    }
                }
                None => match parse_level(part) {
                    Some(level) => f.default = level,
                    // A bare target name: everything from it (env_logger does the same).
                    None => f.directives.push((part.to_owned(), LevelFilter::Trace)),
                },
            }
        }
        f
    }

    /// The level that applies to `target` (the most specific matching directive wins).
    pub fn level_for(&self, target: &str) -> LevelFilter {
        self.directives.iter().filter(|(name, _)| matches(name, target)).max_by_key(|(name, _)| name.len()).map_or(self.default, |(_, level)| *level)
    }

    /// The most verbose level any target can reach (for `log::set_max_level`).
    pub fn max(&self) -> LevelFilter {
        self.directives.iter().map(|(_, level)| *level).fold(self.default, Ord::max)
    }
}

fn parse_level(s: &str) -> Option<LevelFilter> {
    s.trim().parse().ok()
}

/// `name` matches `target` itself and its submodules (`a` matches `a` and `a::b`, not `ab`);
/// `name*` matches every target starting with `name`.
fn matches(name: &str, target: &str) -> bool {
    match name.strip_suffix('*') {
        Some(prefix) => target.starts_with(prefix),
        None => target.strip_prefix(name).is_some_and(|rest| rest.is_empty() || rest.starts_with("::")),
    }
}

/// `2026-10-08T07:59:17.728Z` (UTC, milliseconds). A clock before 1970 reads as the epoch.
/// A clock far past chrono's year range still converts (the calendar repeats every 400 years), so
/// logging never panics and never silently resets to the epoch for a convertible instant.
pub fn timestamp(t: SystemTime) -> String {
    let since = t.duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
    // Pre-1970 is clamped to the epoch by `unwrap_or_default` above. `as_secs` is `u64`; a value
    // past `i64::MAX` seconds saturates rather than falling back to the epoch.
    let secs = i64::try_from(since.as_secs()).unwrap_or(i64::MAX);
    let (y, t) = utc_parts(secs, since.subsec_nanos());
    // `{:04}` rather than `%Y`, which writes a `+` before years past 9999.
    format!("{y:04}-{}", t.format("%m-%dT%H:%M:%S%.3fZ"))
}

/// Unix seconds (+ nanos) → (proleptic Gregorian year, the same wall-clock instant moved into
/// chrono's range by whole 400-year cycles). The calendar repeats every 400 years (146_097 days),
/// so month, day and time of day come from chrono and only the year is carried, which keeps every
/// `i64` exact. Replaces the old hand-rolled `civil_from_days`; chrono is the only calendar logic.
fn utc_parts(secs: i64, nanos: u32) -> (i64, chrono::NaiveDateTime) {
    use chrono::Datelike;
    let (days, sod) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    let (cycles, rest) = (days.div_euclid(146_097), days.rem_euclid(146_097));
    // `rest + 719_163` (1970-01-01 is CE day 719_163) is always a valid CE day, and `sod` < 86_400.
    let date = i32::try_from(rest + 719_163).ok().and_then(chrono::NaiveDate::from_num_days_from_ce_opt);
    let time = u32::try_from(sod).ok().and_then(|s| chrono::NaiveTime::from_num_seconds_from_midnight_opt(s, nanos.min(999_999_999)));
    match (date, time) {
        (Some(d), Some(t)) => {
            let y = i128::from(d.year()) + i128::from(cycles) * 400;
            (i64::try_from(y).unwrap_or(if y < 0 { i64::MIN } else { i64::MAX }), d.and_time(t))
        }
        _ => (1970, chrono::NaiveDateTime::default()), // unreachable
    }
}

fn numbered(dir: &Path, n: usize) -> PathBuf {
    if n == 0 { dir.join(LOG_FILE) } else { dir.join(format!("soundcraft.{n}.log")) }
}

/// Shift the previous logs up one (`.log` → `.1.log` → … → `.<KEEP>.log`, the oldest dropped)
/// and create a fresh, empty log file in `dir` (created if missing).
pub fn rotate(dir: &Path) -> std::io::Result<(PathBuf, File)> {
    std::fs::create_dir_all(dir)?;
    for n in (1..=KEEP).rev() {
        match std::fs::rename(numbered(dir, n - 1), numbered(dir, n)) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e),
            _ => {}
        }
    }
    let path = numbered(dir, 0);
    let file = File::create(&path)?;
    Ok((path, file))
}

/// One formatted log line: `<timestamp> <LEVEL> [<thread>] <target>: <message>\n`.
pub fn format_line(ts: &str, level: log::Level, thread: &str, target: &str, message: &str) -> String {
    let message = message.strip_suffix('\n').unwrap_or(message);
    format!("{ts} {level:<5} [{thread}] {target}: {message}\n")
}

/// Where formatted lines go besides standard error: kept in memory until a file is attached,
/// then appended to it until it reaches its size cap.
pub struct Sink {
    file: Option<File>,
    pending: Vec<String>,
    dropped: usize,
    written: u64,
    max_bytes: u64,
    capped: bool,
}

impl Sink {
    pub fn new(max_bytes: u64) -> Sink {
        Sink { file: None, pending: Vec::new(), dropped: 0, written: 0, max_bytes, capped: false }
    }

    /// Append `line` to the file, or keep it until [`Sink::attach`].
    pub fn write(&mut self, line: &str) {
        if self.file.is_none() {
            if self.capped {
                return;
            }
            if self.pending.len() < MAX_PENDING {
                self.pending.push(line.to_owned());
            } else {
                self.dropped = self.dropped.saturating_add(1);
            }
            return;
        }
        self.append(line);
    }

    /// Start writing to `file`, first the lines kept so far.
    pub fn attach(&mut self, file: File) {
        self.file = Some(file);
        for line in std::mem::take(&mut self.pending) {
            self.append(&line);
        }
        if self.dropped > 0 {
            let note = format!("{} earlier log lines were dropped before the log file was opened\n", self.dropped);
            self.dropped = 0;
            self.append(&note);
        }
    }

    /// There will be no log file: forget the lines kept for it and keep no more.
    pub fn no_file(&mut self) {
        self.file = None;
        self.pending = Vec::new();
        self.dropped = 0;
        self.capped = true;
    }

    fn append(&mut self, line: &str) {
        if self.capped {
            return;
        }
        let Some(file) = self.file.as_mut() else { return };
        let len = u64::try_from(line.len()).unwrap_or(u64::MAX);
        let text = if self.written.saturating_add(len) > self.max_bytes {
            self.capped = true;
            format!("log file reached {} bytes; later records go to standard error only\n", self.max_bytes)
        } else {
            self.written = self.written.saturating_add(len);
            line.to_owned()
        };
        // A full disk or a vanished file must not take the app down: drop the file sink (and
        // stop buffering, since no file will come).
        if file.write_all(text.as_bytes()).is_err() {
            self.file = None;
            self.capped = true;
        }
    }
}

/// Text formatted into a fixed buffer: no allocation, cut at a character boundary when full.
struct Bounded<const N: usize> {
    buf: [u8; N],
    len: usize,
    cut: bool,
}

impl<const N: usize> Bounded<N> {
    fn new() -> Self {
        Bounded { buf: [0; N], len: 0, cut: false }
    }

    fn as_str(&self) -> &str {
        // Only whole characters are ever copied in, so this never fails.
        self.buf.get(..self.len).and_then(|b| std::str::from_utf8(b).ok()).unwrap_or("")
    }
}

impl<const N: usize> std::fmt::Write for Bounded<N> {
    fn write_str(&mut self, s: &str) -> std::fmt::Result {
        let room = N.saturating_sub(self.len);
        let mut n = s.len().min(room);
        while !s.is_char_boundary(n) {
            n = n.saturating_sub(1);
        }
        if let (Some(dst), Some(src)) = (self.buf.get_mut(self.len..self.len.saturating_add(n)), s.as_bytes().get(..n)) {
            dst.copy_from_slice(src);
            self.len = self.len.saturating_add(n);
        }
        if n < s.len() {
            self.cut = true;
        }
        Ok(())
    }
}

/// The first record the audio thread logged since the last report.
struct Held {
    at: SystemTime,
    level: log::Level,
    target: Bounded<REALTIME_TARGET_BYTES>,
    message: Bounded<REALTIME_MESSAGE_BYTES>,
}

/// Records from the realtime audio thread, waiting for another thread to write them.
#[derive(Default)]
struct Realtime {
    /// Records logged on the audio thread since the last report (the held one included).
    records: AtomicUsize,
    /// The first of them; the audio thread only `try_lock`s it.
    held: Mutex<Option<Held>>,
}

impl Realtime {
    /// Audio thread: count `record` and keep it if it is the first. Never waits, never allocates.
    fn keep(&self, record: &log::Record<'_>) {
        self.records.fetch_add(1, Ordering::Relaxed);
        if let Ok(mut held) = self.held.try_lock()
            && held.is_none()
        {
            let mut target = Bounded::new();
            let _ = target.write_str(record.target());
            let mut message = Bounded::new();
            let _ = write!(message, "{}", record.args());
            *held = Some(Held { at: SystemTime::now(), level: record.level(), target, message });
        }
    }

    /// Another thread: the lines to write for what the audio thread logged (empty when nothing).
    fn take_lines(&self) -> String {
        if self.records.load(Ordering::Relaxed) == 0 {
            return String::new();
        }
        let held = self.held.lock().unwrap_or_else(PoisonError::into_inner).take();
        let records = self.records.swap(0, Ordering::Relaxed);
        let mut lines = String::new();
        if let Some(h) = &held {
            let mut message = h.message.as_str().to_owned();
            if h.message.cut {
                message.push('…');
            }
            lines.push_str(&format_line(&timestamp(h.at), h.level, AUDIO_THREAD, h.target.as_str(), &message));
        }
        let more = records.saturating_sub(usize::from(held.is_some()));
        if more > 0 {
            let thread = std::thread::current();
            let note = format!("{more} more log records from the realtime audio thread were not written (only the first is kept)");
            lines.push_str(&format_line(&timestamp(SystemTime::now()), log::Level::Warn, thread.name().unwrap_or("?"), module_path!(), &note));
        }
        lines
    }
}

/// The installed logger: a [`Filter`], standard error and a [`Sink`].
pub struct AppLogger {
    filter: Filter,
    stderr: bool,
    sink: Mutex<Sink>,
    realtime: Realtime,
}

impl AppLogger {
    pub fn new(filter: Filter, stderr: bool) -> AppLogger {
        AppLogger { filter, stderr, sink: Mutex::new(Sink::new(MAX_FILE_BYTES)), realtime: Realtime::default() }
    }

    /// Rotate the logs in `dir` and send records to the fresh file; returns its path.
    /// An unusable directory leaves standard error as the only sink ([`AppLogger::no_file`]).
    pub fn attach_dir(&self, dir: &Path) -> Result<PathBuf, String> {
        match rotate(dir) {
            Ok((path, file)) => {
                self.sink.lock().unwrap_or_else(PoisonError::into_inner).attach(file);
                Ok(path)
            }
            Err(e) => {
                self.no_file();
                Err(format!("{}: {e}", dir.display()))
            }
        }
    }

    /// No log file this run (no settings directory, `SOUNDCRAFT_NO_PREFS`, or the directory can't
    /// be used): stop keeping records for one.
    pub fn no_file(&self) {
        self.sink.lock().unwrap_or_else(PoisonError::into_inner).no_file();
    }

    /// Write what the audio thread logged since the last report (never call it on the audio thread).
    pub fn report_audio_thread(&self) {
        let lines = self.realtime.take_lines();
        if !lines.is_empty() {
            self.emit(&lines);
        }
    }

    /// Standard error, then the sink; the lock covers one write of already formatted text.
    fn emit(&self, text: &str) {
        if self.stderr {
            // No terminal (a Windows GUI build, a closed pipe) is not an error worth reporting.
            let _ = std::io::stderr().write_all(text.as_bytes());
        }
        self.sink.lock().unwrap_or_else(PoisonError::into_inner).write(text);
    }
}

impl log::Log for AppLogger {
    fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
        metadata.level() <= self.filter.level_for(metadata.target())
    }

    fn log(&self, record: &log::Record<'_>) {
        if !self.enabled(record.metadata()) {
            return;
        }
        if soundcraft_playback::on_audio_thread() {
            self.realtime.keep(record);
            return;
        }
        self.report_audio_thread();
        // Formatted before the sink is locked, so a message that logs while it is formatted can't deadlock.
        let thread = std::thread::current();
        let line =
            format_line(&timestamp(SystemTime::now()), record.level(), thread.name().unwrap_or("?"), record.target(), &record.args().to_string());
        self.emit(&line);
    }

    fn flush(&self) {}
}

/// Install the logger (filter from `rust_log`, the `RUST_LOG` value, else [`DEFAULT_FILTER`]); `None` when another
/// logger was installed first. Call [`AppLogger::attach_dir`] or [`AppLogger::no_file`] once the
/// settings directory is known.
pub fn install(rust_log: Option<&str>) -> Option<&'static AppLogger> {
    static LOGGER: OnceLock<AppLogger> = OnceLock::new();
    let filter = Filter::parse(rust_log.unwrap_or(DEFAULT_FILTER));
    let max = filter.max();
    let logger = LOGGER.get_or_init(|| AppLogger::new(filter, true));
    log::set_logger(logger).ok()?;
    log::set_max_level(max);
    Some(logger)
}

#[cfg(test)]
mod tests;
