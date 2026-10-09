//! SoundCraft's VST3 plugin host.
//!
//! Scans the standard VST3 folders (plus `VST3_PATH`), loads `.vst3` bundles in-process and
//! exposes each "Audio Module Class" as a [`soundcraft_dsp::Plugin`], so the mix engine treats
//! third-party plugins exactly like the built-in ones. Plugin ids are `vst3:<class id hex>` (32
//! upper-case hex digits of the component's class id); parameter ids are VST3 `ParamID`s as
//! decimal strings, with titles, units and plain ranges from the edit controller in
//! [`PluginInfo::params`]. Values are plain (display) values; the host converts to and from the
//! VST3 normalized 0..1 values. Stepped parameters become toggles or choices.
//!
//! Like `soundcraft-clap-host`, this is an isolated `unsafe` crate (craftrules never-crash):
//! `unsafe` is denied crate-wide and allowed only in the private `ffi` module. The public API is
//! safe, returns `Result`/`Option` and never panics. Limits of in-process hosting: a plugin that
//! itself crashes (segfault) takes the process down with it; we cannot sandbox native code.
//!
//! `PluginInfo` must be `&'static`: the registry leaks exactly one `PluginInfo` (with its param
//! table) per plugin *type* the first time that type is instantiated, and caches it. The leak is
//! bounded by the number of distinct installed plugins actually used. Loaded binaries likewise
//! stay mapped for the life of the process (module exit functions are never called).
//!
//! On `wasm32` the crate compiles to stubs: scans are empty and nothing can be created.
//!
//! Plugin state: [`Plugin::save_state`] stores the component state and the controller state in
//! one blob (`SCV3` + length-prefixed parts); [`Plugin::load_state`] restores both (a bare
//! component state is accepted too) and re-reads the parameter values.
//!
//! Plugin editors (`IPlugView`): on macOS the view is attached to the content `NSView` of a host
//! `NSWindow` (titled and closable, floating above the app, resized on `IPlugFrame::resizeView`).
//! [`Plugin::open_editor`] opens it directly; [`Plugin::editor`] returns a handle usable on the
//! main thread while the instance processes on the audio thread (VST3's own threading model:
//! the controller and its views live on the UI thread). Parameter edits made in the editor
//! (`IComponentHandler::performEdit`) are reported by the handle's `idle` so the host can write
//! them into the session. Windows (HWND) and Linux (X11) editor windows are not hosted yet:
//! opening reports "unsupported".
//!
//! Not hosted yet: `IMessage`/`IAttributeList` creation through the host (plugins that need it
//! for processor ↔ controller messaging may lose those messages), sidechain (aux buses are kept
//! inactive) and 64-bit processing.

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
pub use plugin::Vst3Plugin;

use soundcraft_dsp::{Category, Plugin, PluginInfo};
use std::path::{Path, PathBuf};

/// Prefix of every hosted-VST3 plugin id.
pub const ID_PREFIX: &str = "vst3:";

/// The factory class category of audio processors (the only classes SoundCraft lists).
pub const AUDIO_MODULE_CLASS: &str = "Audio Module Class";

/// The 16-byte class id inside a `vst3:<32 hex digits>` SoundCraft id (`None` for other ids).
pub fn parse_id(id: &str) -> Option<[u8; 16]> {
    let hex = id.strip_prefix(ID_PREFIX)?;
    if hex.len() != 32 || !hex.is_ascii() {
        return None;
    }
    let mut out = [0u8; 16];
    for (i, o) in out.iter_mut().enumerate() {
        let pair = hex.get(i * 2..i * 2 + 2)?;
        *o = u8::from_str_radix(pair, 16).ok()?;
    }
    Some(out)
}

/// One plugin found by a scan.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Vst3Descriptor {
    /// SoundCraft id: `vst3:<class_id>`.
    pub id: String,
    /// The component's class id as 32 upper-case hex digits.
    pub class_id: String,
    pub name: String,
    pub vendor: String,
    pub version: String,
    /// The VST3 SDK version the plugin was built with (when reported).
    pub sdk_version: String,
    /// The `|`-separated sub-categories, split (`["Fx", "Reverb"]`).
    pub sub_categories: Vec<String>,
    pub category: Category,
    pub is_instrument: bool,
    /// The `.vst3` bundle or file it lives in.
    pub path: PathBuf,
}

/// Why a VST3 operation failed.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum Vst3Error {
    #[error("cannot load VST3 plugin {0}: {1}")]
    Load(String, String),
    #[error("no VST3 plugin `{0}`")]
    NotFound(String),
    #[error("cannot create VST3 plugin `{0}`: {1}")]
    Instantiate(String, String),
    #[error("VST3 processing: {0}")]
    Process(String),
    #[error("VST3 plugins are not supported on this platform")]
    Unsupported,
}

#[cfg(not(target_arch = "wasm32"))]
mod api {
    use super::*;

    pub fn scan() -> Vec<Vst3Descriptor> {
        registry::scan()
    }
    pub fn rescan() -> Vec<Vst3Descriptor> {
        registry::rescan()
    }
    pub fn add_search_dir(dir: &Path) -> usize {
        registry::add_search_dir(dir)
    }
    pub fn scan_paths(dirs: &[PathBuf]) -> Vec<Vst3Descriptor> {
        registry::scan_paths(dirs)
    }
    pub fn load_bundle(path: &Path) -> Result<Vec<Vst3Descriptor>, Vst3Error> {
        registry::load_bundle(path)
    }
    pub fn instantiate(id: &str) -> Result<Box<dyn Plugin>, Vst3Error> {
        registry::instantiate(id)
    }
    pub fn plugin_info(id: &str) -> Option<&'static PluginInfo> {
        registry::plugin_info(id)
    }
    pub fn shutdown() {
        ffi::exit_modules();
    }
}

#[cfg(target_arch = "wasm32")]
mod api {
    use super::*;

    pub fn scan() -> Vec<Vst3Descriptor> {
        Vec::new()
    }
    pub fn rescan() -> Vec<Vst3Descriptor> {
        Vec::new()
    }
    pub fn add_search_dir(_dir: &Path) -> usize {
        0
    }
    pub fn scan_paths(_dirs: &[PathBuf]) -> Vec<Vst3Descriptor> {
        Vec::new()
    }
    pub fn load_bundle(_path: &Path) -> Result<Vec<Vst3Descriptor>, Vst3Error> {
        Err(Vst3Error::Unsupported)
    }
    pub fn instantiate(_id: &str) -> Result<Box<dyn Plugin>, Vst3Error> {
        Err(Vst3Error::Unsupported)
    }
    pub fn plugin_info(_id: &str) -> Option<&'static PluginInfo> {
        None
    }
    pub fn shutdown() {}
}

/// Every plugin in the standard VST3 folders, `VST3_PATH` and folders added with
/// [`add_search_dir`]. Scanned on first call (loads each binary and reads its factory, but
/// creates no plugin instances), then cached in memory.
pub fn scan() -> Vec<Vst3Descriptor> {
    api::scan()
}

/// Forgets the cached scan (and remembered failures) and scans again.
pub fn rescan() -> Vec<Vst3Descriptor> {
    api::rescan()
}

/// Adds a folder to the search path and merges its plugins into the cache. Returns how many
/// plugins the folder holds.
pub fn add_search_dir(dir: &Path) -> usize {
    api::add_search_dir(dir)
}

/// Scans the given folders without touching the cache.
pub fn scan_paths(dirs: &[PathBuf]) -> Vec<Vst3Descriptor> {
    api::scan_paths(dirs)
}

/// Shuts plugin hosting down: runs the module exit of every loaded binary that has no live
/// instance left, each on the thread that loaded it; later loads fail. Call it on the main
/// thread once the plugin instances are gone, before the process exits. Without it the exits
/// run from an exit handler, by which time some plugins have already torn down state they need
/// (HALion Sonic crashes in its module exit after it has been used).
pub fn shutdown() {
    api::shutdown()
}

/// The audio module classes in one `.vst3` bundle or file.
pub fn load_bundle(path: &Path) -> Result<Vec<Vst3Descriptor>, Vst3Error> {
    api::load_bundle(path)
}

/// Creates a plugin by `vst3:<class id>`, prepared for 48 kHz stereo with 1024-frame blocks (like
/// `soundcraft_dsp::create`), with the reason when it fails.
pub fn instantiate(id: &str) -> Result<Box<dyn Plugin>, Vst3Error> {
    api::instantiate(id)
}

/// Like [`instantiate`], but returns the concrete [`Vst3Plugin`] (for its extra methods, such
/// as [`Vst3Plugin::editor_size`]).
#[cfg(not(target_arch = "wasm32"))]
pub fn instantiate_plugin(id: &str) -> Result<Vst3Plugin, Vst3Error> {
    registry::instantiate_plugin(id)
}

/// Creates a plugin by `vst3:<class id>`; `None` if unknown or it fails to load.
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

/// The description of a `vst3:<class id>` plugin. The first call for a type instantiates it once
/// to read its parameters; the result is cached (and leaked, see the crate docs).
pub fn plugin_info(id: &str) -> Option<&'static PluginInfo> {
    api::plugin_info(id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_parse() {
        let id = "vst3:0123456789ABCDEFfedcba9876543210";
        assert_eq!(parse_id(id), Some([0x01, 0x23, 0x45, 0x67, 0x89, 0xAB, 0xCD, 0xEF, 0xFE, 0xDC, 0xBA, 0x98, 0x76, 0x54, 0x32, 0x10]));
        assert_eq!(parse_id("vst3:"), None);
        assert_eq!(parse_id("vst3:0123"), None);
        assert_eq!(parse_id("vst3:0123456789ABCDEF0123456789ABCDEG"), None);
        assert_eq!(parse_id("vst3:ééééééééééééééééé"), None);
        assert_eq!(parse_id("VST3:0123456789ABCDEF0123456789ABCDEF"), None);
        assert_eq!(parse_id("clap:x"), None);
        assert_eq!(parse_id("eq_7band"), None);
    }

    #[test]
    fn non_vst3_ids_are_rejected_without_scanning() {
        assert!(create("eq_7band").is_none());
        assert!(create("clap:com.x").is_none());
        assert!(plugin_info("reverb").is_none());
    }

    #[test]
    fn garbage_files_are_errors_not_crashes() {
        let d = std::env::temp_dir().join(format!("soundcraft-vst3-garbage-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let arch = if cfg!(target_os = "macos") {
            "MacOS"
        } else if cfg!(windows) {
            "x86_64-win"
        } else {
            "x86_64-linux"
        };
        let exe = if cfg!(target_os = "macos") {
            "Bundle"
        } else if cfg!(windows) {
            "Bundle.vst3"
        } else {
            "Bundle.so"
        };
        std::fs::create_dir_all(d.join(format!("Bundle.vst3/Contents/{arch}"))).unwrap();
        std::fs::write(d.join("garbage.vst3"), b"\x7fELF this is not a plugin \x00\x01\x02").unwrap();
        std::fs::write(d.join("empty.vst3"), b"").unwrap();
        std::fs::write(d.join(format!("Bundle.vst3/Contents/{arch}/{exe}")), [0xcf, 0xfa, 0xed, 0xfe, 1, 2, 3]).unwrap();
        std::fs::create_dir_all(d.join("NoBinary.vst3/Contents")).unwrap();
        for f in ["garbage.vst3", "empty.vst3", "Bundle.vst3", "NoBinary.vst3", "missing.vst3"] {
            assert!(load_bundle(&d.join(f)).is_err(), "{f}");
        }
        assert!(scan_paths(std::slice::from_ref(&d)).is_empty());
        let missing = "vst3:00000000000000000000000000000000";
        assert!(create(missing).is_none());
        assert!(plugin_info(missing).is_none());
        assert!(instantiate(missing).is_err());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_real_library_without_the_vst3_export_is_rejected() {
        // A system library loads fine but has no `GetPluginFactory`.
        // (macOS system libraries live only in the dyld shared cache, not as files.)
        let lib = if cfg!(target_os = "macos") || cfg!(windows) {
            None
        } else {
            ["/lib/x86_64-linux-gnu/libz.so.1", "/usr/lib/libz.so.1", "/lib64/libz.so.1"].iter().map(PathBuf::from).find(|p| p.exists())
        };
        let Some(lib) = lib else { return };
        let d = std::env::temp_dir().join(format!("soundcraft-vst3-nolib-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        // Point a single-file ".vst3" at it via a symlink (no copying of binaries).
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&lib, d.join("z.vst3")).unwrap();
            let e = load_bundle(&d.join("z.vst3")).unwrap_err();
            assert!(e.to_string().contains("not a VST3 plugin"), "{e}");
        }
        let _ = std::fs::remove_dir_all(&d);
    }
}
