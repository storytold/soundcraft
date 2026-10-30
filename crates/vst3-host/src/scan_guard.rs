//! Crash guard for plugin scans.
//!
//! A scan loads every plugin in the search folders into this process, and a plugin that crashes
//! while loading takes the whole app down. Before each load the guard writes the plugin's path to
//! a marker file and removes it once the load returned. When the next scan finds a marker left
//! over, the app died inside that plugin: the plugin goes on a blocklist and is skipped from then
//! on (the log names the blocklist file, one path per line, to edit by hand).
//!
//! The guard sits on the automatic scans only. Two SoundCraft processes scanning at the same time
//! share the marker and may blame the wrong plugin; that is rare and only skips a plugin.

use std::path::{Path, PathBuf};

fn state_dir() -> Option<PathBuf> {
    let non_empty = |k: &str| std::env::var_os(k).filter(|v| !v.is_empty()).map(PathBuf::from);
    let base =
        non_empty("XDG_STATE_HOME").or_else(|| non_empty("LOCALAPPDATA")).or_else(|| non_empty("HOME").map(|h| h.join(".local").join("state")))?;
    Some(base.join("soundcraft"))
}

fn marker_file(dir: &Path, kind: &str) -> PathBuf {
    dir.join(format!("{kind}-scan-current.txt"))
}

fn blocklist_file(dir: &Path, kind: &str) -> PathBuf {
    dir.join(format!("{kind}-scan-blocked.txt"))
}

fn read_paths(file: &Path) -> Vec<PathBuf> {
    std::fs::read_to_string(file).map(|t| t.lines().map(str::trim).filter(|l| !l.is_empty()).map(PathBuf::from).collect()).unwrap_or_default()
}

pub(crate) struct ScanGuard {
    dir: Option<PathBuf>,
    kind: &'static str,
    blocked: Vec<PathBuf>,
}

impl ScanGuard {
    /// Reads the blocklist and settles a marker left by a crashed scan. Without a usable state
    /// folder the guard does nothing.
    pub fn open(kind: &'static str) -> ScanGuard {
        ScanGuard::open_in(state_dir(), kind)
    }

    fn open_in(dir: Option<PathBuf>, kind: &'static str) -> ScanGuard {
        let mut blocked = Vec::new();
        if let Some(d) = &dir {
            blocked = read_paths(&blocklist_file(d, kind));
            let marker = marker_file(d, kind);
            for crashed in read_paths(&marker) {
                log::warn!(
                    "{kind}: {} crashed the app during an earlier scan and is skipped from now on (blocklist: {})",
                    crashed.display(),
                    blocklist_file(d, kind).display()
                );
                if !blocked.contains(&crashed) {
                    blocked.push(crashed);
                }
            }
            let _ = std::fs::remove_file(&marker);
            if !blocked.is_empty() {
                let text: String = blocked.iter().map(|p| format!("{}\n", p.display())).collect();
                let _ = std::fs::write(blocklist_file(d, kind), text);
            }
        }
        ScanGuard { dir, kind, blocked }
    }

    pub fn is_blocked(&self, plugin: &Path) -> bool {
        self.blocked.iter().any(|b| b == plugin)
    }

    /// Marks `plugin` as being loaded. Call [`ScanGuard::end`] once the load returned.
    pub fn begin(&self, plugin: &Path) {
        if let Some(d) = &self.dir {
            let _ = std::fs::create_dir_all(d);
            let _ = std::fs::write(marker_file(d, self.kind), plugin.to_string_lossy().as_bytes());
        }
    }

    pub fn end(&self) {
        if let Some(d) = &self.dir {
            let _ = std::fs::remove_file(marker_file(d, self.kind));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("soundcraft-scan-guard-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn a_plugin_that_never_finished_loading_is_blocked_next_time() {
        let dir = temp_dir("crash");
        let bad = Path::new("/plugins/Bad.vst3");
        let first = ScanGuard::open_in(Some(dir.clone()), "test");
        assert!(!first.is_blocked(bad));
        first.begin(bad);
        // The process "dies" here: `end` is never called.
        let second = ScanGuard::open_in(Some(dir.clone()), "test");
        assert!(second.is_blocked(bad));
        assert!(!second.is_blocked(Path::new("/plugins/Good.vst3")));
        // The decision is remembered, and the marker is gone.
        let third = ScanGuard::open_in(Some(dir.clone()), "test");
        assert!(third.is_blocked(bad));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_plugin_that_loaded_fine_is_not_blocked() {
        let dir = temp_dir("fine");
        let ok = Path::new("/plugins/Fine.vst3");
        let guard = ScanGuard::open_in(Some(dir.clone()), "test");
        guard.begin(ok);
        guard.end();
        assert!(!ScanGuard::open_in(Some(dir.clone()), "test").is_blocked(ok));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn without_a_state_folder_nothing_is_blocked() {
        let guard = ScanGuard::open_in(None, "test");
        guard.begin(Path::new("/x.vst3"));
        guard.end();
        assert!(!guard.is_blocked(Path::new("/x.vst3")));
    }
}
