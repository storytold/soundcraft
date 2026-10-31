//! The scan cache file: what each plugin file held when it was last loaded, keyed by the size and
//! modification time of its binary, so a scan loads only new and changed files (as REAPER does).
//! A file that failed to load is remembered too and not retried until it changes.
//!
//! Files can also be loaded in a child process (see [`load_in_child`]), so that a plugin's
//! start-up never runs in the app: on Windows a library initialising holds the loader lock, which
//! stalls every other thread of the process that loads a library or starts a thread (the app's
//! window stayed black at launch while a plugin took seconds to initialise), and a plugin that
//! crashes while it loads takes only the child down.

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, UNIX_EPOCH};

/// Bumped when the layout changes; a file of another version is ignored (everything is loaded).
const VERSION: u32 = 1;

/// Largest cache file read (a plugin takes a few hundred bytes).
const MAX_BYTES: u64 = 16 * 1024 * 1024;

/// Starts the line a child process answers with (plugins may print to standard output too).
const REPLY: &str = "SOUNDCRAFT-SCAN ";

/// Longest wait for a child process; a plugin that takes longer (or waits on a dialog) is killed.
const CHILD_TIMEOUT: Duration = Duration::from_secs(120);

/// Longest wait, once the child has ended, for an answer it printed just before.
const REPLY_GRACE: Duration = Duration::from_secs(2);

/// One plugin file: the plugins it holds, or why it did not load.
#[derive(Clone, Serialize, Deserialize)]
#[serde(bound(serialize = "D: Serialize", deserialize = "D: DeserializeOwned"))]
pub struct Entry<D> {
    pub path: PathBuf,
    size: u64,
    modified_ns: u64,
    pub plugins: Vec<D>,
    pub error: Option<String>,
}

#[derive(Deserialize)]
#[serde(bound = "D: DeserializeOwned")]
struct FileIn<D> {
    version: u32,
    files: Vec<Entry<D>>,
}

#[derive(Serialize)]
#[serde(bound = "D: Serialize")]
struct FileOut<'a, D> {
    version: u32,
    files: &'a [Entry<D>],
}

/// Size and modification time (ns since 1970) of a file; `None` when unreadable.
fn stamp(file: &Path) -> Option<(u64, u64)> {
    let m = std::fs::metadata(file).ok()?;
    let ns = m.modified().ok()?.duration_since(UNIX_EPOCH).ok()?.as_nanos();
    Some((m.len(), u64::try_from(ns).ok()?))
}

/// The entries of a cache file; none when it is missing, unreadable or of another version.
pub fn read<D: DeserializeOwned>(file: &Path) -> Vec<Entry<D>> {
    use std::io::Read as _;
    let Ok(f) = std::fs::File::open(file) else { return Vec::new() };
    let mut bytes = Vec::new();
    if f.take(MAX_BYTES).read_to_end(&mut bytes).is_err() {
        return Vec::new();
    }
    match serde_json::from_slice::<FileIn<D>>(&bytes) {
        Ok(c) if c.version == VERSION => c.files,
        Ok(_) => Vec::new(),
        Err(e) => {
            log::warn!("{}: {e}", file.display());
            Vec::new()
        }
    }
}

/// Writes a cache file (through a temporary file, so a crash never leaves half of one).
pub fn write<D: Serialize>(file: &Path, files: &[Entry<D>]) {
    let tmp = file.with_extension("json.tmp");
    let written = serde_json::to_vec_pretty(&FileOut { version: VERSION, files })
        .map_err(|e| e.to_string())
        .and_then(|bytes| {
            if let Some(dir) = file.parent() {
                std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
            }
            std::fs::write(&tmp, bytes).map_err(|e| e.to_string())
        })
        .and_then(|()| std::fs::rename(&tmp, file).map_err(|e| e.to_string()));
    if let Err(e) = written {
        log::warn!("{}: {e}", file.display());
    }
}

/// One entry per plugin file in `files`: the `known` entry when the file's `binary` (else the
/// file itself) has the same size and modification time, else what `load` reads now. Before each
/// load, `ahead(index, count, file)` is told what comes; when it returns `false` the file is left
/// out (it is neither listed nor cached, so the next scan loads it), as is a file whose `load`
/// gives `None` (interrupted).
pub fn scan<D: Clone>(
    files: Vec<PathBuf>,
    known: &[Entry<D>],
    binary: impl Fn(&Path) -> Option<PathBuf>,
    load: impl Fn(&Path) -> Option<Result<Vec<D>, String>>,
    ahead: impl Fn(usize, usize, &Path) -> bool,
) -> Vec<Entry<D>> {
    let count = files.len();
    files
        .into_iter()
        .enumerate()
        .filter_map(|(i, path)| {
            let stamp = stamp(&binary(&path).unwrap_or_else(|| path.clone()));
            if let Some((size, modified_ns)) = stamp
                && let Some(e) = known.iter().find(|e| e.path == path && e.size == size && e.modified_ns == modified_ns)
            {
                return Some(e.clone());
            }
            if !ahead(i, count, &path) {
                return None;
            }
            let (plugins, error) = match load(&path)? {
                Ok(p) => (p, None),
                Err(e) => {
                    log::warn!("{e}");
                    (Vec::new(), Some(e))
                }
            };
            let (size, modified_ns) = stamp.unwrap_or_default();
            Some(Entry { path, size, modified_ns, plugins, error })
        })
        .collect()
}

/// What a child process started with [`load_in_child`] prints: one line with the plugins that
/// `loaded` found, or why it failed.
pub fn reply<D: Serialize, E: std::fmt::Display>(loaded: Result<Vec<D>, E>) -> String {
    let body = match loaded {
        Ok(plugins) => serde_json::json!({ "plugins": plugins }),
        Err(e) => serde_json::json!({ "error": e.to_string() }),
    };
    format!("{REPLY}{body}")
}

#[derive(Deserialize)]
#[serde(bound = "D: DeserializeOwned")]
struct Reply<D> {
    plugins: Option<Vec<D>>,
    error: Option<String>,
}

/// Loads `file` in a child process: `program flag file`, which answers with [`reply`]. `None`
/// when `stop` turned true meanwhile: the child was killed.
pub fn load_in_child<D: DeserializeOwned>(program: &Path, flag: &str, file: &Path, stop: impl Fn() -> bool) -> Option<Result<Vec<D>, String>> {
    let mut stopped = false;
    let r = run_child(program, flag, file, || {
        stopped = stop();
        stopped
    });
    (!stopped).then_some(r)
}

fn run_child<D: DeserializeOwned>(program: &Path, flag: &str, file: &Path, mut stop: impl FnMut() -> bool) -> Result<Vec<D>, String> {
    use std::process::{Command, Stdio};
    let fail = |m: String| format!("{}: {m}", file.display());
    let mut command = Command::new(program);
    command.arg(flag).arg(file).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        /// `CREATE_NO_WINDOW`: no console window flashes up for the child.
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    let mut child = command.spawn().map_err(|e| fail(format!("the scan process did not start: {e}")))?;
    // Read while it runs, so that a chatty plugin cannot fill the pipe and stall it. The reader
    // also flags the answer line the moment it arrives: a plugin that answers and then hangs the
    // process on exit (Native Instruments' plug-ins do, in their library unload) must not cost
    // the whole timeout, so the parent stops waiting on the answer, not on the exit.
    // The reader is never joined: a helper process the plugin started (a licence or update agent)
    // inherits the output and can hold it open long after the child is gone.
    let out = child.stdout.take().ok_or_else(|| fail("no output from the scan process".into()))?;
    let (send, answers) = std::sync::mpsc::channel::<String>();
    std::thread::spawn(move || {
        use std::io::BufRead as _;
        let mut reader = std::io::BufReader::new(out);
        let mut line = Vec::new();
        // Bytes, converted lossily: a plugin printing non-UTF-8 cannot hide the answer.
        while let Ok(n) = reader.read_until(b'\n', &mut line) {
            if n == 0 {
                break;
            }
            if let Some(json) = String::from_utf8_lossy(&line).trim_end().strip_prefix(REPLY)
                && send.send(json.to_string()).is_err()
            {
                break;
            }
            line.clear();
        }
    });
    let started = Instant::now();
    let mut answer = None;
    let status = loop {
        answer = answer.or_else(|| answers.try_recv().ok());
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            // Answered: the plugin's start-up is done; its unload is not waited for.
            Ok(None) if answer.is_some() => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
            Ok(None) if started.elapsed() < CHILD_TIMEOUT && !stop() => std::thread::sleep(Duration::from_millis(10)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
        }
    };
    // A child that answered and ended at once can leave its answer still on the way through the
    // reader: wait for it a moment (the reader ends at once when the pipe closes), never longer.
    if answer.is_none() && status.is_some() {
        answer = answers.recv_timeout(REPLY_GRACE).ok();
    }
    match (answer.as_deref(), status) {
        (Some(json), _) => {
            let r: Reply<D> = serde_json::from_str(json).map_err(|e| fail(format!("unreadable scan reply: {e}")))?;
            match (r.plugins, r.error) {
                (Some(plugins), _) => Ok(plugins),
                (None, e) => Err(e.unwrap_or_else(|| fail("the scan process found nothing".into()))),
            }
        }
        (None, None) => Err(fail(format!("still loading after {} s; stopped", CHILD_TIMEOUT.as_secs()))),
        (None, Some(status)) => Err(fail(format!("the plugin ended the scan process ({status})"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    fn dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("sc-scan-cache-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let _ = std::fs::create_dir_all(&d);
        d
    }

    #[test]
    fn unchanged_files_are_not_loaded_again() {
        let d = dir("unchanged");
        let (a, b) = (d.join("a.plug"), d.join("b.plug"));
        std::fs::write(&a, "a").ok();
        std::fs::write(&b, "bb").ok();
        let loads = Cell::new(0);
        let load = |p: &Path| {
            loads.set(loads.get() + 1);
            Some(if p == b { Err("b is broken".to_string()) } else { Ok(vec![p.display().to_string()]) })
        };
        let first = scan(vec![a.clone(), b.clone()], &[], |_| None, load, |_, _, _| true);
        assert_eq!(loads.get(), 2);
        let file = d.join("cache.json");
        write(&file, &first);
        let known: Vec<Entry<String>> = read(&file);
        assert_eq!(known.len(), 2);

        // Both unchanged: nothing is loaded, the failure is remembered.
        let again = scan(vec![a.clone(), b.clone()], &known, |_| None, load, |_, _, _| true);
        assert_eq!(loads.get(), 2);
        assert_eq!(again[0].plugins, vec![a.display().to_string()]);
        assert_eq!(again[1].error.as_deref(), Some("b is broken"));

        // A changed file and a new one are loaded; a removed one is dropped.
        std::fs::write(&b, "fixed, longer").ok();
        let c = d.join("c.plug");
        std::fs::write(&c, "c").ok();
        let next = scan(vec![b.clone(), c.clone()], &known, |_| None, load, |_, _, _| true);
        assert_eq!(loads.get(), 4);
        assert_eq!(next.iter().map(|e| e.path.clone()).collect::<Vec<_>>(), vec![b.clone(), c.clone()]);

        // Skipped while loading: the known file is still listed, the skipped ones are left out.
        let told = std::cell::RefCell::new(Vec::new());
        let skipped = scan(
            vec![a.clone(), b.clone(), c],
            &known,
            |_| None,
            load,
            |i, n, _| {
                told.borrow_mut().push((i, n));
                false
            },
        );
        assert_eq!(loads.get(), 4);
        assert_eq!(told.into_inner(), vec![(1, 3), (2, 3)]);
        assert_eq!(skipped.iter().map(|e| e.path.clone()).collect::<Vec<_>>(), vec![a]);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn an_unreadable_or_foreign_cache_file_is_ignored() {
        let d = dir("foreign");
        let file = d.join("cache.json");
        assert!(read::<String>(&file).is_empty());
        std::fs::write(&file, "not json").ok();
        assert!(read::<String>(&file).is_empty());
        std::fs::write(&file, r#"{"version":999,"files":[]}"#).ok();
        assert!(read::<String>(&file).is_empty());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_reply_reads_back_past_other_output() {
        let ok = reply::<String, String>(Ok(vec!["Reverb".into()]));
        let text = format!(
            "plugin says hello
{ok}
"
        );
        let json = text.lines().rev().find_map(|l| l.strip_prefix(REPLY)).unwrap_or_default();
        let r: Reply<String> = serde_json::from_str(json).unwrap_or(Reply { plugins: None, error: None });
        assert_eq!(r.plugins, Some(vec!["Reverb".to_string()]));
        let err = reply::<String, String>(Err("broken".into()));
        let r: Reply<String> = serde_json::from_str(err.trim_start_matches(REPLY)).unwrap_or(Reply { plugins: None, error: None });
        assert_eq!(r.error.as_deref(), Some("broken"));
    }

    #[test]
    fn a_missing_scan_program_is_a_load_error() {
        let r = load_in_child::<String>(Path::new("no-such-soundcraft-scanner.exe"), "--scan", Path::new("x.plug"), || false);
        assert!(r.is_some_and(|r| r.is_err_and(|e| e.contains("did not start"))));
    }

    #[test]
    fn a_child_that_answers_and_then_hangs_is_not_waited_for() {
        // Like a plugin hanging in its library unload: the answer, then no exit.
        let answer = reply::<String, String>(Ok(vec!["Bite".into()]));
        #[cfg(windows)]
        let (program, flag, script) = ("powershell", "-Command", format!("Write-Output '{answer}'; Start-Sleep 60"));
        #[cfg(not(windows))]
        let (program, flag, script) = ("sh", "-c", format!("echo '{answer}'; exec sleep 60"));
        let started = Instant::now();
        let r = load_in_child::<String>(Path::new(program), flag, Path::new(&script), || false);
        assert_eq!(r, Some(Ok(vec!["Bite".to_string()])));
        assert!(started.elapsed() < Duration::from_secs(30), "waited {:?}", started.elapsed());
    }

    #[test]
    fn a_helper_process_holding_the_output_open_is_not_waited_for() {
        // Like a plugin starting a helper (a licence or update agent) that inherits the child's
        // output and outlives it: the helper starts, the child answers and ends, the pipe stays open.
        let answer = reply::<String, String>(Ok(vec!["Bite".into()]));
        #[cfg(windows)]
        let (program, flag, script) = {
            let d = dir("helper");
            let script = d.join("scan.cmd");
            std::fs::write(&script, format!("@start /b \"\" ping -n 60 127.0.0.1 >nul\r\n@echo {answer}\r\n")).ok();
            ("cmd", "/c", script.display().to_string())
        };
        #[cfg(not(windows))]
        let (program, flag, script) = ("sh", "-c", format!("sleep 60 & echo '{answer}'"));
        let started = Instant::now();
        let r = load_in_child::<String>(Path::new(program), flag, Path::new(&script), || false);
        assert_eq!(r, Some(Ok(vec!["Bite".to_string()])));
        assert!(started.elapsed() < Duration::from_secs(30), "waited {:?}", started.elapsed());
    }
}
