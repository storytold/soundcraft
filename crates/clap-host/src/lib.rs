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
//! thread while the instance processes on the audio thread); plugins that offer only embedded
//! editors are not hosted yet, and SoundCraft's generic parameter editor still works for them.
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
    use super::*;

    pub fn scan() -> Vec<ClapDescriptor> {
        registry::scan()
    }
    pub fn rescan() -> Vec<ClapDescriptor> {
        registry::rescan()
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
    use super::*;

    pub fn scan() -> Vec<ClapDescriptor> {
        Vec::new()
    }
    pub fn rescan() -> Vec<ClapDescriptor> {
        Vec::new()
    }
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
/// [`add_search_dir`]. Scanned on first call, in child processes, then cached in memory.
pub fn scan() -> Vec<ClapDescriptor> {
    api::scan()
}

/// Forgets the cached scan (and remembered failures) and scans again.
pub fn rescan() -> Vec<ClapDescriptor> {
    api::rescan()
}

/// Adds a folder to the search path and merges its plugins into the cache. Returns how many
/// plugins the folder holds.
pub fn add_search_dir(dir: &Path) -> usize {
    api::add_search_dir(dir)
}

/// Scans the given folders without keeping the result in memory.
pub fn scan_paths(dirs: &[PathBuf]) -> Vec<ClapDescriptor> {
    api::scan_paths(dirs)
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
        assert!(create("clap:does.not.exist").is_none());
        assert!(plugin_info("clap:does.not.exist").is_none());
        assert!(instantiate("clap:does.not.exist").is_err());
        let _ = std::fs::remove_dir_all(&d);
    }
}
