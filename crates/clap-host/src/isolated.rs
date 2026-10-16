//! Scanning in helper processes, with the results kept on disk: the CLAP side of
//! `soundcraft_vst3_host::isolated` (same design, same reasons, measured there).
//!
//! Scanning loads each `.clap` and runs its entry and factory: the plugin's own code. Done in the
//! app, one plugin that crashes there takes the app down, and the window freezes for the scan.
//! So each file is loaded by a short-lived copy of the app started with [`WORKER_ARG`], which
//! prints what it found on one line and exits; a helper that crashes, aborts or hangs costs that
//! one file, logged by name and remembered. Results are cached by each binary's size and
//! modification time, so a later launch scans only what was added or changed.
//!
//! Off unless the app calls [`enable`] (tests and library users scan in-process, as before).

use crate::ClapDescriptor;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock, PoisonError, mpsc};
use std::time::{Duration, Instant, UNIX_EPOCH};

/// The argument that turns the app into a one-file scanner: `<exe> --plugin-scan-worker clap <path>`.
pub const WORKER_ARG: &str = "--plugin-scan-worker";
/// The format word after [`WORKER_ARG`] that this crate answers.
const FORMAT: &str = "clap";
/// Marks the result line, so whatever a plugin prints to standard output is ignored.
const RESULT_PREFIX: &str = "SOUNDCRAFT-PLUGIN-SCAN ";
/// A helper still running after this is killed and its file skipped. Generous on purpose: some
/// plugins show a licence or activation dialog while they load and wait for it to be answered.
const TIMEOUT: Duration = Duration::from_secs(120);
/// Most helpers at once (each one is a whole process with a plugin loaded in it).
const MAX_WORKERS: usize = 8;
/// The most output read from one helper (a plugin printing without end can't fill memory).
const MAX_OUTPUT: u64 = 64 * 1024 * 1024;
/// Bumped when the cache's shape or meaning changes; an older file is ignored.
const CACHE_VERSION: u32 = 1;

struct Config {
    exe: PathBuf,
    cache: Option<PathBuf>,
}

static CONFIG: OnceLock<Config> = OnceLock::new();

/// Scan in helper processes started from `exe` (the running app, which must call [`worker_main`]
/// first thing in `main`), keeping results in `cache` when given. Only the first call counts.
pub fn enable(exe: PathBuf, cache: Option<PathBuf>) {
    let _ = CONFIG.set(Config { exe, cache });
}

pub(crate) fn enabled() -> bool {
    CONFIG.get().is_some()
}

/// Files handled so far in the current scan, and how many it has (for a progress display).
static DONE: AtomicUsize = AtomicUsize::new(0);
static TOTAL: AtomicUsize = AtomicUsize::new(0);

pub(crate) fn progress() -> (usize, usize) {
    (DONE.load(Ordering::Relaxed), TOTAL.load(Ordering::Relaxed))
}

pub(crate) fn start_progress(total: usize) {
    DONE.store(0, Ordering::Relaxed);
    TOTAL.store(total, Ordering::Relaxed);
}

pub(crate) fn step_progress() {
    DONE.fetch_add(1, Ordering::Relaxed);
}

/// What scanning one file came to.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Outcome {
    /// Loaded; its audio module classes (possibly none).
    Found(Vec<ClapDescriptor>),
    /// Refused cleanly (not a plugin, no factory, missing dependency…).
    Failed(String),
    /// The helper died without answering (the plugin crashed or aborted it).
    Crashed(String),
    /// The helper was still running after [`TIMEOUT`] and was killed.
    TimedOut,
}

/// The size and modification time of a file's binary: a changed plugin is scanned again.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
struct Stamp {
    size: u64,
    modified_ns: u64,
}

fn stamp(bundle: &Path) -> Option<Stamp> {
    let binary = crate::scan::bundle_executable(bundle).unwrap_or_else(|| bundle.to_path_buf());
    let m = std::fs::metadata(binary).ok()?;
    let modified = m.modified().ok()?.duration_since(UNIX_EPOCH).ok()?;
    Some(Stamp { size: m.len(), modified_ns: u64::try_from(modified.as_nanos()).unwrap_or(u64::MAX) })
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CacheEntry {
    path: PathBuf,
    stamp: Stamp,
    outcome: Outcome,
}

#[derive(Debug, Serialize, Deserialize)]
struct CacheFile {
    version: u32,
    entries: Vec<CacheEntry>,
}

fn read_cache(path: &Path) -> HashMap<PathBuf, CacheEntry> {
    let Ok(text) = std::fs::read_to_string(path) else { return HashMap::new() };
    match serde_json::from_str::<CacheFile>(&text) {
        Ok(f) if f.version == CACHE_VERSION => f.entries.into_iter().map(|e| (e.path.clone(), e)).collect(),
        Ok(_) => HashMap::new(),
        Err(e) => {
            log::warn!("ignoring the CLAP scan cache {}: {e}", path.display());
            HashMap::new()
        }
    }
}

/// Written to a temporary file and renamed over the old one, so a crash mid-write can't leave a
/// half file behind.
fn write_cache(path: &Path, entries: Vec<CacheEntry>) {
    let text = match serde_json::to_string(&CacheFile { version: CACHE_VERSION, entries }) {
        Ok(t) => t,
        Err(e) => return log::warn!("cannot save the CLAP scan cache: {e}"),
    };
    let tmp = path.with_extension("json.tmp");
    let written =
        path.parent().map_or(Ok(()), std::fs::create_dir_all).and_then(|()| std::fs::write(&tmp, text)).and_then(|()| std::fs::rename(&tmp, path));
    if let Err(e) = written {
        log::warn!("cannot save the CLAP scan cache {}: {e}", path.display());
    }
}

/// In the helper: if `args` (the app's arguments after the program name) ask for a CLAP scan, load
/// that one file, print the result line and return the exit code; otherwise `None`. Call before
/// anything else in `main` (in particular before the logger opens the user's log file).
pub fn worker_main(args: &[String]) -> Option<i32> {
    let (Some(flag), Some(format), Some(path)) = (args.first(), args.get(1), args.get(2)) else { return None };
    if flag != WORKER_ARG || format != FORMAT {
        return None;
    }
    let outcome = match crate::registry::load_bundle(Path::new(path)) {
        Ok(found) => Outcome::Found(found),
        Err(e) => Outcome::Failed(e.to_string()),
    };
    let line = serde_json::to_string(&outcome).unwrap_or_else(|e| format!("{{\"failed\":\"cannot encode the result: {e}\"}}"));
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{RESULT_PREFIX}{line}");
    let _ = out.flush();
    drop(out);
    Some(0)
}

/// How a helper ended, for the log ("exit code 0xC0000005 (access violation)").
fn describe(status: ExitStatus) -> String {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(sig) = status.signal() {
            return format!("killed by signal {sig}");
        }
    }
    match status.code() {
        Some(code) => {
            let raw = code as u32;
            let what = match raw {
                0xC000_0005 => " (access violation)",
                0xC000_0409 => " (aborted: fail-fast, such as a C++ exception reaching Rust)",
                0xC000_00FD => " (stack overflow)",
                0xC000_001D => " (illegal instruction)",
                _ => "",
            };
            if cfg!(windows) { format!("exit code 0x{raw:08X}{what}") } else { format!("exit code {code}") }
        }
        None => "no exit code".to_string(),
    }
}

/// Runs one helper on `bundle`.
fn scan_in_helper(exe: &Path, bundle: &Path) -> Outcome {
    let mut cmd = Command::new(exe);
    cmd.arg(WORKER_ARG).arg(FORMAT).arg(bundle).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // CREATE_NO_WINDOW: no console window flashing up per file.
        cmd.creation_flags(0x0800_0000);
    }
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => return Outcome::Failed(format!("cannot start the plugin scanner: {e}")),
    };
    // Read on another thread: a large shell plugin's answer can outgrow the pipe, and the helper
    // would block writing it. A channel rather than a join, because a process the plugin started
    // (a licence agent) can inherit the pipe and keep it open after the helper is gone.
    let (tx, rx) = mpsc::channel();
    if let Some(out) = child.stdout.take() {
        let _ = std::thread::Builder::new().name("plugin-scan-read".into()).spawn(move || {
            let mut bytes = Vec::new();
            let _ = out.take(MAX_OUTPUT).read_to_end(&mut bytes);
            let _ = tx.send(bytes);
        });
    }
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break Some(s),
            Ok(None) if started.elapsed() >= TIMEOUT => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
            Err(e) => return Outcome::Crashed(format!("lost the plugin scanner: {e}")),
        }
    };
    let Some(status) = status else { return Outcome::TimedOut };
    let bytes = rx.recv_timeout(Duration::from_secs(5)).unwrap_or_default();
    let text = String::from_utf8_lossy(&bytes);
    let answer = text.lines().find_map(|l| l.strip_prefix(RESULT_PREFIX));
    match answer.map(serde_json::from_str::<Outcome>) {
        Some(Ok(o)) => o,
        Some(Err(e)) => Outcome::Crashed(format!("unreadable answer ({e}), {}", describe(status))),
        None => Outcome::Crashed(describe(status)),
    }
}

/// Scans `bundles` in helpers (or from the cache) and returns every descriptor found, in file
/// order. `force` ignores the cache (a rescan). Needs [`enable`].
pub(crate) fn scan(bundles: &[PathBuf], force: bool) -> Vec<ClapDescriptor> {
    let Some(config) = CONFIG.get() else { return Vec::new() };
    let started = Instant::now();
    start_progress(bundles.len());
    let cache = match (&config.cache, force) {
        (Some(p), false) => read_cache(p),
        _ => HashMap::new(),
    };
    let stamps: Vec<Option<Stamp>> = bundles.iter().map(|b| stamp(b)).collect();
    let mut outcomes: Vec<Option<Outcome>> = vec![None; bundles.len()];
    let mut todo = Vec::new();
    for (i, b) in bundles.iter().enumerate() {
        match (cache.get(b), stamps.get(i).copied().flatten()) {
            (Some(e), Some(s)) if e.stamp == s => {
                if let Some(slot) = outcomes.get_mut(i) {
                    *slot = Some(e.outcome.clone());
                }
                step_progress();
            }
            _ => todo.push(i),
        }
    }
    let cached = bundles.len().saturating_sub(todo.len());
    let workers = std::thread::available_parallelism().map_or(4, |n| n.get()).clamp(1, MAX_WORKERS).min(todo.len().max(1));
    let next = AtomicUsize::new(0);
    let fresh: Mutex<Vec<(usize, Outcome)>> = Mutex::new(Vec::new());
    std::thread::scope(|s| {
        let work = || {
            while let Some(&i) = todo.get(next.fetch_add(1, Ordering::Relaxed)) {
                let Some(bundle) = bundles.get(i) else { break };
                let t = Instant::now();
                let o = scan_in_helper(&config.exe, bundle);
                let took = t.elapsed();
                if took >= crate::registry::SLOW_LOAD {
                    log::warn!("CLAP plugin {} took {:.1} s to scan", bundle.display(), took.as_secs_f64());
                }
                fresh.lock().unwrap_or_else(PoisonError::into_inner).push((i, o));
                step_progress();
            }
        };
        for _ in 0..workers {
            if std::thread::Builder::new().name("plugin-scan".into()).spawn_scoped(s, work).is_err() {
                // No thread to be had: do the rest here.
                work();
            }
        }
    });
    for (i, o) in fresh.into_inner().unwrap_or_else(PoisonError::into_inner) {
        if let Some(slot) = outcomes.get_mut(i) {
            *slot = Some(o);
        }
    }
    let mut found = Vec::new();
    let mut entries = Vec::new();
    let (mut failed, mut crashed) = (0usize, 0usize);
    for ((bundle, outcome), stamp) in bundles.iter().zip(outcomes).zip(stamps) {
        let Some(outcome) = outcome else { continue };
        match &outcome {
            Outcome::Found(ds) => found.extend(ds.iter().cloned()),
            Outcome::Failed(e) => {
                failed = failed.saturating_add(1);
                log::warn!("{e}");
            }
            Outcome::Crashed(how) => {
                crashed = crashed.saturating_add(1);
                log::warn!("CLAP plugin {} crashed the plugin scanner ({how}); skipped until it changes or plugins are rescanned", bundle.display());
            }
            Outcome::TimedOut => {
                crashed = crashed.saturating_add(1);
                log::warn!(
                    "CLAP plugin {} did not finish loading within {} s and was skipped until it changes or plugins are rescanned",
                    bundle.display(),
                    TIMEOUT.as_secs()
                );
            }
        }
        if let Some(stamp) = stamp {
            entries.push(CacheEntry { path: bundle.clone(), stamp, outcome });
        }
    }
    if let Some(p) = &config.cache {
        write_cache(p, entries);
    }
    log::info!(
        "CLAP scan: {} plugins from {} files ({} from the cache, {failed} failed, {crashed} crashed or hung) in {:.1} s",
        found.len(),
        bundles.len(),
        cached,
        started.elapsed().as_secs_f64()
    );
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn worker_main_ignores_other_arguments() {
        assert_eq!(worker_main(&[]), None);
        assert_eq!(worker_main(&["--demo".into()]), None);
        assert_eq!(worker_main(&[WORKER_ARG.into(), "vst3".into(), "x.vst3".into()]), None);
        assert_eq!(worker_main(&[WORKER_ARG.into(), FORMAT.into()]), None);
    }

    #[test]
    fn outcomes_round_trip_through_the_cache_format() {
        let entries = vec![
            CacheEntry { path: "a.clap".into(), stamp: Stamp { size: 1, modified_ns: 2 }, outcome: Outcome::Crashed("exit code 0xC0000005".into()) },
            CacheEntry { path: "b.clap".into(), stamp: Stamp { size: 3, modified_ns: 4 }, outcome: Outcome::TimedOut },
            CacheEntry { path: "c.clap".into(), stamp: Stamp { size: 5, modified_ns: 6 }, outcome: Outcome::Found(Vec::new()) },
        ];
        let d = std::env::temp_dir().join(format!("soundcraft-clap-cache-{}", std::process::id()));
        let file = d.join("clap-scan.json");
        write_cache(&file, entries);
        let back = read_cache(&file);
        assert_eq!(back.len(), 3);
        assert!(matches!(back.get(Path::new("a.clap")).map(|e| &e.outcome), Some(Outcome::Crashed(_))));
        assert!(matches!(back.get(Path::new("b.clap")).map(|e| &e.outcome), Some(Outcome::TimedOut)));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_damaged_or_older_cache_is_ignored() {
        let d = std::env::temp_dir().join(format!("soundcraft-clap-badcache-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let file = d.join("clap-scan.json");
        std::fs::write(&file, "{not json").unwrap();
        assert!(read_cache(&file).is_empty());
        std::fs::write(&file, r#"{"version":0,"entries":[]}"#).unwrap();
        assert!(read_cache(&file).is_empty());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_helper_that_never_answers_is_reported_as_crashed() {
        // Any program that ignores our arguments and prints no result line stands in for a
        // helper killed by its plugin.
        let exe = if cfg!(windows) { PathBuf::from("C:\\Windows\\System32\\whoami.exe") } else { PathBuf::from("/bin/true") };
        if !exe.exists() {
            return;
        }
        assert!(matches!(scan_in_helper(&exe, Path::new("x.clap")), Outcome::Crashed(_)));
    }
}
