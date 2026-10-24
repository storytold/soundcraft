//! SoundCraft's CLAP plugin host.
//!
//! Scans the standard CLAP folders (plus `CLAP_PATH`), loads `.clap` binaries in-process and
//! exposes each plugin as a [`soundcraft_dsp::Plugin`], so the mix engine treats third-party
//! plugins exactly like the built-in ones. Plugin ids are `clap:<CLAP plugin id>`; parameter ids
//! are the CLAP param ids as decimal strings, with their human names in [`PluginInfo::params`].
//!
//! This is SoundCraft's single isolated `unsafe` crate (craftrules never-crash): `unsafe` is
//! denied crate-wide and allowed only in the private `ffi` module. The public API is safe, returns
//! `Result`/`Option` and never panics. Limits of in-process hosting: a plugin that itself crashes
//! (segfault) takes the process down with it; we cannot sandbox native code.
//!
//! `PluginInfo` must be `&'static`: the registry leaks exactly one `PluginInfo` (with its param
//! table) per plugin *type* the first time that type is instantiated, and caches it. The leak is
//! bounded by the number of distinct installed plugins actually used. Loaded binaries likewise
//! stay mapped for the life of the process.
//!
//! On `wasm32` the crate compiles to stubs: scans are empty and nothing can be created.
//!
//! Plugin state (`clap.state`) round-trips through [`Plugin::save_state`] /
//! [`Plugin::load_state`] as an opaque blob. Plugin editors (`clap.gui`) open as the plugin's own
//! floating window ([`Plugin::open_editor`], or [`Plugin::editor`] for a handle usable on the main
//! thread while the instance processes on the audio thread). Plugins that offer only an embedded
//! editor get a host window of SoundCraft's on Windows (resized on `request_resize`, closed by
//! its title bar); elsewhere they are not hosted yet, and SoundCraft's generic parameter editor
//! still works for them.
//! Parameter changes made in the plugin's GUI arrive as output events while the plugin processes
//! and are reported by the editor's `idle`.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod scan;

#[cfg(not(target_arch = "wasm32"))]
#[allow(unsafe_code)]
mod ffi;
#[cfg(not(target_arch = "wasm32"))]
mod plugin;
#[cfg(not(target_arch = "wasm32"))]
mod registry;

#[cfg(not(target_arch = "wasm32"))]
pub use plugin::ClapPlugin;

use soundcraft_dsp::{Category, Plugin, PluginInfo};
use std::path::{Path, PathBuf};

/// Prefix of every hosted-CLAP plugin id.
pub const ID_PREFIX: &str = "clap:";

/// The CLAP plugin id inside a `clap:<id>` SoundCraft id (`None` for other ids or an empty id).
pub fn parse_id(id: &str) -> Option<&str> {
    id.strip_prefix(ID_PREFIX).filter(|s| !s.is_empty() && !s.contains('\0'))
}

/// One plugin found by a scan.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ClapDescriptor {
    /// SoundCraft id: `clap:<plugin_id>`.
    pub id: String,
    /// The CLAP plugin id (reverse-DNS, e.g. `com.vendor.reverb`).
    pub plugin_id: String,
    pub name: String,
    pub vendor: String,
    pub version: String,
    pub description: String,
    pub features: Vec<String>,
    pub category: Category,
    pub is_instrument: bool,
    /// The `.clap` file or bundle it lives in.
    pub path: PathBuf,
}

/// Why a CLAP operation failed.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum ClapError {
    #[error("cannot load CLAP plugin {0}: {1}")]
    Load(String, String),
    #[error("no CLAP plugin `{0}`")]
    NotFound(String),
    #[error("cannot create CLAP plugin `{0}`: {1}")]
    Instantiate(String, String),
    #[error("CLAP processing: {0}")]
    Process(String),
    #[error("CLAP plugins are not supported on this platform")]
    Unsupported,
}

#[cfg(not(target_arch = "wasm32"))]
mod api {
    use super::{ClapDescriptor, ClapError, Path, PathBuf, Plugin, PluginInfo, registry};

    pub fn cached() -> Option<Vec<ClapDescriptor>> {
        registry::cached()
    }
    pub fn scan() -> Vec<ClapDescriptor> {
        registry::scan()
    }
    pub fn rescan() -> Vec<ClapDescriptor> {
        registry::rescan()
    }
    pub fn set_cache_file(file: Option<PathBuf>) {
        registry::set_cache_file(file)
    }
    pub fn scan_in_background() {
        registry::scan_in_background()
    }
    pub fn rescan_in_background(clear_cache: bool) {
        registry::rescan_in_background(clear_cache)
    }
    pub fn scanning() -> bool {
        registry::scanning()
    }
    pub fn search_dirs() -> (Vec<PathBuf>, Vec<PathBuf>) {
        registry::search_dirs()
    }
    pub fn set_extra_dirs(dirs: Vec<PathBuf>) {
        registry::set_extra_dirs(dirs)
    }
    pub fn problems() -> Vec<(PathBuf, String)> {
        registry::problems()
    }
    pub fn progress() -> Option<(usize, usize, PathBuf)> {
        registry::progress()
    }
    pub fn set_scanner(program: Option<PathBuf>) {
        registry::set_scanner(program)
    }
    pub fn scan_reply(file: &Path) -> String {
        registry::scan_reply(file)
    }
    pub fn skip_scan() {
        registry::skip_scan()
    }
    pub fn add_search_dir(dir: &Path) -> usize {
        registry::add_search_dir(dir)
    }
    pub fn scan_paths(dirs: &[PathBuf]) -> Vec<ClapDescriptor> {
        registry::scan_paths(dirs)
    }
    pub fn load_bundle(path: &Path) -> Result<Vec<ClapDescriptor>, ClapError> {
        registry::load_bundle(path)
    }
    pub fn instantiate(id: &str) -> Result<Box<dyn Plugin>, ClapError> {
        registry::instantiate(id)
    }
    pub fn plugin_info(id: &str) -> Option<&'static PluginInfo> {
        registry::plugin_info(id)
    }
}

#[cfg(target_arch = "wasm32")]
mod api {
    use super::{ClapDescriptor, ClapError, Path, PathBuf, Plugin, PluginInfo};

    pub fn cached() -> Option<Vec<ClapDescriptor>> {
        Some(Vec::new())
    }
    pub fn scan() -> Vec<ClapDescriptor> {
        Vec::new()
    }
    pub fn rescan() -> Vec<ClapDescriptor> {
        Vec::new()
    }
    pub fn set_cache_file(_file: Option<PathBuf>) {}
    pub fn scan_in_background() {}
    pub fn rescan_in_background(_clear_cache: bool) {}
    pub fn scanning() -> bool {
        false
    }
    pub fn search_dirs() -> (Vec<PathBuf>, Vec<PathBuf>) {
        (Vec::new(), Vec::new())
    }
    pub fn set_extra_dirs(_dirs: Vec<PathBuf>) {}
    pub fn problems() -> Vec<(PathBuf, String)> {
        Vec::new()
    }
    pub fn progress() -> Option<(usize, usize, PathBuf)> {
        None
    }
    pub fn set_scanner(_program: Option<PathBuf>) {}
    pub fn scan_reply(_file: &Path) -> String {
        String::new()
    }
    pub fn skip_scan() {}
    pub fn add_search_dir(_dir: &Path) -> usize {
        0
    }
    pub fn scan_paths(_dirs: &[PathBuf]) -> Vec<ClapDescriptor> {
        Vec::new()
    }
    pub fn load_bundle(_path: &Path) -> Result<Vec<ClapDescriptor>, ClapError> {
        Err(ClapError::Unsupported)
    }
    pub fn instantiate(_id: &str) -> Result<Box<dyn Plugin>, ClapError> {
        Err(ClapError::Unsupported)
    }
    pub fn plugin_info(_id: &str) -> Option<&'static PluginInfo> {
        None
    }
}

/// Every plugin in the standard CLAP folders, `CLAP_PATH` and folders added with
/// [`add_search_dir`]. Scanned on first call (loads each binary's entry, but creates no plugin
/// instances), then cached in memory; with a [`set_cache_file`], only files that are new or
/// changed since an earlier run are loaded. Waits for a scan that is running.
pub fn scan() -> Vec<ClapDescriptor> {
    api::scan()
}

/// Forgets the cached scan (and remembered failures) and scans again, loading every file (the
/// cache file is rewritten, not read).
pub fn rescan() -> Vec<ClapDescriptor> {
    api::rescan()
}

/// Remembers scans in `file` across runs (a JSON file of what each plugin file held, keyed by
/// its size and modification time), so that later scans load only new and changed files. `None`
/// (the default) keeps scans in memory only. Set it before the first scan.
pub fn set_cache_file(file: Option<PathBuf>) {
    api::set_cache_file(file)
}

/// Starts [`scan`] on a background thread, unless the plugins are scanned or a scan thread runs
/// already; [`cached`] has the result when it ends. Loading plugin files can take seconds (some
/// load models or check licences), which a UI thread must not wait for.
pub fn scan_in_background() {
    api::scan_in_background()
}

/// Starts a fresh scan on a background thread (unless one runs): with `clear_cache`, every file
/// is loaded again, as [`rescan`] does; without, only new and changed ones. [`cached`] is `None`
/// until it ends.
pub fn rescan_in_background(clear_cache: bool) {
    api::rescan_in_background(clear_cache)
}

/// Whether a scan thread started by [`scan_in_background`] or [`rescan_in_background`] runs.
pub fn scanning() -> bool {
    api::scanning()
}

/// The folders scans look in: the standard ones for this system (with `CLAP_PATH`), then the
/// ones added with [`set_extra_dirs`] or [`add_search_dir`].
pub fn search_dirs() -> (Vec<PathBuf>, Vec<PathBuf>) {
    api::search_dirs()
}

/// Replaces the folders scanned besides the standard ones. Takes effect at the next scan.
pub fn set_extra_dirs(dirs: Vec<PathBuf>) {
    api::set_extra_dirs(dirs)
}

/// The plugin files the last scan could not load, with the reason. They are not loaded again
/// until they change or the cache is cleared ([`rescan`]).
pub fn problems() -> Vec<(PathBuf, String)> {
    api::problems()
}

/// The argument that makes the scan program set with [`set_scanner`] load one file:
/// `<program> --scan-clap <file>`; the program then prints [`scan_reply`] and exits.
pub const SCAN_FLAG: &str = "--scan-clap";

/// Makes scans load each plugin file in a child process, `program` --scan-clap `<file>`, rather than in
/// this one. A plugin's start-up then never runs here: on Windows a library initialising holds the
/// loader lock, which stalls every thread here that loads a library or starts one (a window stays
/// black while it lasts), and a plugin that crashes while loading takes only the child down. The
/// scan's plugins are then loaded here only when first used. `None` (the default) loads in-process.
pub fn set_scanner(program: Option<PathBuf>) {
    api::set_scanner(program)
}

/// The answer of a scan child process (see [`set_scanner`]) for one CLAP file: a line with its
/// plugins or the reason it failed, to print to standard output.
pub fn scan_reply(file: &Path) -> String {
    api::scan_reply(file)
}

/// The plugin file a scan is loading now: its index among the files found, their number and its
/// path. `None` when no scan runs or it only reuses the cache file.
pub fn progress() -> Option<(usize, usize, PathBuf)> {
    api::progress()
}

/// Makes the running scan load no more files. It ends after the file it is loading (a plugin's
/// own start-up cannot be interrupted); the plugins found so far are listed, and the files
/// skipped are left out of the cache file, so the next scan loads them.
pub fn skip_scan() {
    api::skip_scan()
}

/// Adds a folder to the search path and merges its plugins into the cache. Returns how many
/// plugins the folder holds.
pub fn add_search_dir(dir: &Path) -> usize {
    api::add_search_dir(dir)
}

/// Scans the given folders without touching the cache.
pub fn scan_paths(dirs: &[PathBuf]) -> Vec<ClapDescriptor> {
    api::scan_paths(dirs)
}

/// The plugins of the last scan, without scanning: `None` until something has scanned.
pub fn cached() -> Option<Vec<ClapDescriptor>> {
    api::cached()
}

/// The plugins in one `.clap` file or bundle.
pub fn load_bundle(path: &Path) -> Result<Vec<ClapDescriptor>, ClapError> {
    api::load_bundle(path)
}

/// Creates a plugin by `clap:<id>`, prepared for 48 kHz stereo with 1024-frame blocks (like
/// `soundcraft_dsp::create`), with the reason when it fails.
pub fn instantiate(id: &str) -> Result<Box<dyn Plugin>, ClapError> {
    api::instantiate(id)
}

/// Creates a plugin by `clap:<id>`; `None` if unknown or it fails to load.
pub fn create(id: &str) -> Option<Box<dyn Plugin>> {
    parse_id(id)?;
    match api::instantiate(id) {
        Ok(p) => Some(p),
        Err(e) => {
            log::warn!("{e}");
            None
        }
    }
}

/// The description of a `clap:<id>` plugin. The first call for a type instantiates it once to
/// read its parameters; the result is cached (and leaked, see the crate docs).
pub fn plugin_info(id: &str) -> Option<&'static PluginInfo> {
    api::plugin_info(id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_parse() {
        assert_eq!(parse_id("clap:com.u-he.diva"), Some("com.u-he.diva"));
        assert_eq!(parse_id("clap:"), None);
        assert_eq!(parse_id("eq_7band"), None);
        assert_eq!(parse_id("CLAP:x"), None);
        assert_eq!(parse_id("clap:a\0b"), None);
    }

    #[test]
    fn non_clap_ids_are_rejected_without_scanning() {
        assert!(create("eq_7band").is_none());
        assert!(plugin_info("reverb").is_none());
    }

    #[test]
    fn garbage_files_are_errors_not_crashes() {
        let d = std::env::temp_dir().join(format!("soundcraft-clap-garbage-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("Bundle.clap/Contents/MacOS")).unwrap();
        std::fs::write(d.join("garbage.clap"), b"\x7fELF this is not a plugin \x00\x01\x02").unwrap();
        std::fs::write(d.join("empty.clap"), b"").unwrap();
        std::fs::write(d.join("Bundle.clap/Contents/MacOS/Bundle"), [0xcf, 0xfa, 0xed, 0xfe, 1, 2, 3]).unwrap();
        for f in ["garbage.clap", "empty.clap", "Bundle.clap", "missing.clap"] {
            assert!(load_bundle(&d.join(f)).is_err(), "{f}");
        }
        assert!(scan_paths(std::slice::from_ref(&d)).is_empty());
        assert!(create("clap:does.not.exist").is_none());
        assert!(plugin_info("clap:does.not.exist").is_none());
        assert!(instantiate("clap:does.not.exist").is_err());
        let _ = std::fs::remove_dir_all(&d);
    }
}
