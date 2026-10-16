//! Plugin scanning in short-lived child processes, cached on disk; off until an app calls [`in_children`].

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use serde::Serialize;
use serde::de::DeserializeOwned;
use std::collections::BTreeMap;
use std::fmt::Display;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::{Duration, Instant};

const CHILD_ARG: &str = "--scan-plugin";
const REPLY_MARK: &str = "@@soundcraft-scan@@ ";
const CACHE_VERSION: u32 = 1;
const MAX_CACHE_BYTES: u64 = 64 << 20;
const MAX_OUTPUT: u64 = 32 << 20;
const MAX_FINGERPRINT_ENTRIES: usize = 4096;

type Outcome<D> = Result<Vec<D>, String>;
type Cache<D> = BTreeMap<String, (u64, Outcome<D>)>;

#[derive(Clone)]
enum Mode {
    InProcess,
    Children(Children),
}

#[derive(Clone)]
struct Children {
    program: PathBuf,
    args: Vec<String>,
    cache_dir: Option<PathBuf>,
    timeout: Duration,
}

struct State {
    mode: Option<Mode>,
    failed: Vec<(String, PathBuf, String)>,
}

fn state() -> MutexGuard<'static, State> {
    static STATE: Mutex<State> = Mutex::new(State { mode: None, failed: Vec::new() });
    STATE.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Reads each plugin file in a child process, `program --scan-plugin <format> <file>`, a few at a
/// time, and caches what they report in `cache_dir`.
pub fn in_children(program: PathBuf, cache_dir: Option<PathBuf>) {
    let children = Children { program, args: vec![CHILD_ARG.into()], cache_dir, timeout: Duration::from_secs(60) };
    state().mode = Some(Mode::Children(children));
}

/// Reads plugin files in this process, uncached: only for plugins built by this repo's tests.
pub fn in_process() {
    state().mode = Some(Mode::InProcess);
}

/// The plugins in `files`, in order. Files that cannot be read are logged and listed by [`failures`].
pub fn probe<D, E>(format: &str, files: &[PathBuf], load: impl Fn(&Path) -> Result<Vec<D>, E>) -> Vec<D>
where
    D: Clone + Serialize + DeserializeOwned + Send + Sync,
    E: Display,
{
    let mode = state().mode.clone();
    let outcomes = match mode {
        None => return Vec::new(),
        Some(Mode::InProcess) => files.iter().map(|f| load(f).map_err(|e| e.to_string())).collect(),
        Some(Mode::Children(c)) => c.probe(format, files),
    };
    let mut found = Vec::new();
    let mut st = state();
    st.failed.retain(|(f, p, _)| f != format || (!files.contains(p) && p.exists()));
    for (path, outcome) in files.iter().zip(outcomes) {
        match outcome {
            Ok(ds) => found.extend(ds),
            Err(why) => {
                log::warn!("{format} plugin {}: {why}", path.display());
                st.failed.push((format.to_string(), path.clone(), why));
            }
        }
    }
    found
}

/// The plugin files that could not be read: `(format, file, why)`.
pub fn failures() -> Vec<(String, PathBuf, String)> {
    state().failed.clone()
}

/// Drops the cached results for `format`, so that the next probe reads every file again.
pub fn forget(format: &str) {
    if let Some(Mode::Children(Children { cache_dir: Some(dir), .. })) = &state().mode {
        let _ = std::fs::remove_file(cache_path(dir, format));
    }
}

/// In a scan child (`<app> --scan-plugin <format> <file>`), the format and file to read.
pub fn child_request() -> Option<(String, PathBuf)> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    match args.as_slice() {
        [flag, format, file] if flag == CHILD_ARG => Some((format.to_str()?.to_string(), file.into())),
        _ => None,
    }
}

/// Prints a scan child's reply for its parent and returns the child's exit code.
pub fn child_reply<D: Serialize, E: Display>(read: Result<Vec<D>, E>) -> i32 {
    let code = i32::from(read.is_err());
    let reply = serde_json::to_string(&read.map_err(|e| e.to_string()))
        .unwrap_or_else(|e| serde_json::json!({ "Err": format!("cannot encode the scan result: {e}") }).to_string());
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "\n{REPLY_MARK}{reply}").and_then(|()| out.flush());
    code
}

impl Children {
    fn probe<D: Clone + Serialize + DeserializeOwned + Send + Sync>(&self, format: &str, files: &[PathBuf]) -> Vec<Outcome<D>> {
        let cache_file = self.cache_dir.as_deref().map(|d| cache_path(d, format));
        let mut cache: Cache<D> = cache_file.as_deref().map(load_cache).unwrap_or_default();
        let read = par_map(files, |f| {
            let print = fingerprint(f);
            match cache.get(f.to_string_lossy().as_ref()) {
                Some((p, hit)) if Some(*p) == print => (hit.clone(), None),
                _ => (self.run(format, f), print),
            }
        });
        let mut changed = false;
        let mut out = Vec::with_capacity(files.len());
        for (f, r) in files.iter().zip(read) {
            let (outcome, print) = r.unwrap_or_else(|| (Err("not scanned".into()), None));
            if let Some(p) = print {
                cache.insert(f.to_string_lossy().into_owned(), (p, outcome.clone()));
                changed = true;
            }
            out.push(outcome);
        }
        if changed && let Some(path) = cache_file {
            save_cache(&path, cache);
        }
        out
    }

    fn run<D: DeserializeOwned>(&self, format: &str, file: &Path) -> Outcome<D> {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let cannot_start = |e: std::io::Error| format!("cannot start the plugin scanner: {e}");
        let name = format!("soundcraft-plugin-scan-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed));
        let path = std::env::temp_dir().join(name);
        let mut output = File::options().read(true).write(true).create_new(true).open(&path).map_err(cannot_start)?;
        let _ = std::fs::remove_file(&path);
        let mut cmd = Command::new(&self.program);
        cmd.args(&self.args).arg(format).arg(file).stdin(Stdio::null()).stderr(Stdio::null());
        cmd.env("QT_MAC_DISABLE_FOREGROUND_APPLICATION_TRANSFORM", "1");
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }
        let mut child = output.try_clone().and_then(|stdout| cmd.stdout(stdout).spawn()).map_err(cannot_start)?;
        let deadline = Instant::now() + self.timeout;
        let status = loop {
            match child.try_wait() {
                Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(10)),
                Ok(Some(status)) => break Some(status),
                _ => {
                    let _ = child.kill();
                    let _ = child.wait();
                    break None;
                }
            }
        };
        let mut text = Vec::new();
        let _ = output.seek(SeekFrom::Start(0)).and_then(|_| output.take(MAX_OUTPUT).read_to_end(&mut text));
        parse_reply(&String::from_utf8_lossy(&text)).unwrap_or_else(|| {
            Err(match status {
                Some(status) => format!("ended without a result ({status})"),
                None => format!("timed out after {:?}", self.timeout),
            })
        })
    }
}

fn parse_reply<D: DeserializeOwned>(output: &str) -> Option<Outcome<D>> {
    let line = output.lines().rev().find_map(|l| l.trim_end().strip_prefix(REPLY_MARK))?;
    Some(serde_json::from_str(line).unwrap_or_else(|e| Err(format!("unreadable plugin scanner reply: {e}"))))
}

fn par_map<T: Sync, R: Send + Sync>(items: &[T], f: impl Fn(&T) -> R + Sync) -> Vec<Option<R>> {
    let slots: Vec<OnceLock<R>> = items.iter().map(|_| OnceLock::new()).collect();
    let next = AtomicUsize::new(0);
    let work = || {
        loop {
            let i = next.fetch_add(1, Ordering::Relaxed);
            let Some((item, slot)) = items.get(i).zip(slots.get(i)) else { break };
            let _ = slot.set(f(item));
        }
    };
    let threads = std::thread::available_parallelism().map_or(2, |n| n.get()).min(8).min(items.len());
    std::thread::scope(|s| {
        for _ in 1..threads {
            if std::thread::Builder::new().name("plugin-scan".into()).spawn_scoped(s, work).is_err() {
                break;
            }
        }
        work();
    });
    slots.into_iter().map(OnceLock::into_inner).collect()
}

fn cache_path(dir: &Path, format: &str) -> PathBuf {
    dir.join(format!("plugin-scan-{format}.json"))
}

fn load_cache<D: DeserializeOwned>(path: &Path) -> Cache<D> {
    let bytes = std::fs::metadata(path).ok().filter(|m| m.len() <= MAX_CACHE_BYTES).and_then(|_| std::fs::read(path).ok());
    match bytes.and_then(|b| serde_json::from_slice(&b).ok()) {
        Some((CACHE_VERSION, cache)) => cache,
        _ => Cache::new(),
    }
}

fn save_cache<D: Serialize>(path: &Path, mut cache: Cache<D>) {
    cache.retain(|file, _| Path::new(file).exists());
    let tmp = path.with_extension(format!("json.{}", std::process::id()));
    let written = path
        .parent()
        .map_or(Ok(()), std::fs::create_dir_all)
        .and_then(|()| serde_json::to_vec(&(CACHE_VERSION, &cache)).map_err(std::io::Error::other))
        .and_then(|bytes| std::fs::write(&tmp, bytes))
        .and_then(|()| std::fs::rename(&tmp, path));
    if let Err(e) = written {
        let _ = std::fs::remove_file(&tmp);
        log::warn!("plugin scan cache {}: {e}", path.display());
    }
}

fn fingerprint(path: &Path) -> Option<u64> {
    let (mut hash, mut budget) = (0xcbf2_9ce4_8422_2325, MAX_FINGERPRINT_ENTRIES);
    stamp(&mut hash, path, 3, &mut budget).then_some(hash)
}

/// FNV-1a over the path, size and modification time of `path` and of what it holds, `depth` levels down.
fn stamp(hash: &mut u64, path: &Path, depth: u32, budget: &mut usize) -> bool {
    let Ok(md) = std::fs::metadata(path) else { return false };
    let modified = md.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).unwrap_or_default();
    for b in format!("{}\0{}\0{}\0", path.display(), md.len(), modified.as_nanos()).bytes() {
        *hash = (*hash ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3);
    }
    if md.is_dir()
        && depth > 0
        && let Ok(dir) = std::fs::read_dir(path)
    {
        let mut entries: Vec<PathBuf> = dir.flatten().take(*budget).map(|e| e.path()).collect();
        entries.sort();
        for e in entries {
            let Some(left) = budget.checked_sub(1) else { break };
            *budget = left;
            stamp(hash, &e, depth - 1, budget);
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
    struct Desc {
        name: String,
    }

    fn temp_dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("soundcraft-plugin-scan-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[cfg(unix)]
    fn shell(script: &str, cache_dir: Option<PathBuf>) -> Children {
        let args = vec!["-c".into(), script.into(), "sh".into()];
        Children { program: "/bin/sh".into(), args, cache_dir, timeout: Duration::from_secs(10) }
    }

    #[cfg(unix)]
    #[test]
    fn children_report_what_they_find_and_results_are_cached() {
        let d = temp_dir("cache");
        let (a, b) = (d.join("A.vst3"), d.join("B.vst3"));
        std::fs::write(&a, b"a").unwrap();
        std::fs::create_dir_all(b.join("Contents/MacOS")).unwrap();
        std::fs::write(b.join("Contents/MacOS/B"), b"b").unwrap();
        let cache = Some(d.join("cache"));
        let reads = shell(r#"echo chatter; printf '@@soundcraft-scan@@ {"Ok":[{"name":"%s %s"}]}\n' "$1" "$(basename "$2")""#, cache.clone());
        let files = [a, b.clone()];
        let found = reads.probe::<Desc>("vst3", &files);
        let desc = |n: &str| Ok(vec![Desc { name: n.into() }]);
        assert_eq!(found, vec![desc("vst3 A.vst3"), desc("vst3 B.vst3")]);
        let fails = shell("exit 3", cache);
        assert_eq!(fails.probe::<Desc>("vst3", &files), found);
        std::fs::write(b.join("Contents/MacOS/B"), b"bb").unwrap();
        let again = fails.probe::<Desc>("vst3", &files);
        assert_eq!(again, vec![found[0].clone(), Err("ended without a result (exit status: 3)".into())]);
        assert_eq!(reads.probe::<Desc>("vst3", &files), again);
        assert_eq!(reads.probe::<Desc>("clap", &files)[1], desc("clap B.vst3"));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[cfg(unix)]
    #[test]
    fn crashes_hangs_and_garbage_cost_only_their_file() {
        let d = temp_dir("fail");
        let f = d.join("X.clap");
        std::fs::write(&f, b"x").unwrap();
        let read = |script: &str| {
            Children { timeout: Duration::from_millis(500), ..shell(script, None) }.probe::<Desc>("clap", std::slice::from_ref(&f)).remove(0)
        };
        let failed = |why: &str| Err(why.to_string());
        // KILL leaves no crash report on the developer's machine (SEGV and ABRT do).
        assert_eq!(read("kill -KILL $$"), failed("ended without a result (signal: 9 (SIGKILL))"));
        assert_eq!(read("exec sleep 5"), failed("timed out after 500ms"));
        assert_eq!(read("echo not a reply"), failed("ended without a result (exit status: 0)"));
        assert!(matches!(read("echo '@@soundcraft-scan@@ {nope'"), Err(e) if e.starts_with("unreadable plugin scanner reply")));
        assert_eq!(read(r#"echo '@@soundcraft-scan@@ {"Err":"not a plugin"}'; exit 1"#), failed("not a plugin"));
        assert_eq!(read(r#"echo '@@soundcraft-scan@@ {"Ok":[]}'; kill -KILL $$"#), Ok(Vec::new()));
        assert_eq!(read(r#"echo '@@soundcraft-scan@@ {"Ok":[]}'; exec sleep 5"#), Ok(Vec::new()));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn nothing_is_read_until_an_app_chooses_how() {
        let found = probe("vst3", &[PathBuf::from("Y.vst3")], |_| Ok::<_, String>(vec![Desc { name: "Y".into() }]));
        assert!(found.is_empty() && failures().is_empty());
    }
}
