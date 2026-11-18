//! The engine's command plug-ins, and their part of the app's preferences.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, PoisonError};

use craft_plugin::fetch::Fetch;
use craft_plugin::{Manifest, Source};
use serde::Serialize;
use serde_json::{Map, Value, json};

use super::install::Pending;
use super::{MAX_ALL_SETTINGS_BYTES, MAX_CATALOGS, MAX_SETTING_KEY, MAX_SETTINGS_BYTES, MAX_SETTINGS_PLUGINS};

/// An installed command plug-in.
#[derive(Clone, Debug)]
pub struct InstalledPlugin {
    pub manifest: Manifest,
    /// The SHA-256 of its bundle (lower-case hex).
    pub sha256: String,
    /// Where it came from.
    pub source: Source,
    /// Kept in the user's `Plug-ins` folder; otherwise for this session only (the CLI, MCP, the web).
    pub stored: bool,
    pub(crate) wasm: Arc<Vec<u8>>,
}

impl InstalledPlugin {
    /// What `plugin.list` says about it.
    pub fn describe(&self) -> Value {
        let mut v = serde_json::to_value(&self.manifest).unwrap_or_else(|_| json!({"id": self.manifest.id}));
        v["kind"] = json!(self.manifest.kind);
        v["sha256"] = json!(self.sha256);
        v["source"] = serde_json::to_value(&self.source).unwrap_or(Value::Null);
        v["stored"] = json!(self.stored);
        v
    }
}

/// The command plug-ins' part of the preferences (`command_plugins` in the desktop app's
/// `ui.json`): each plug-in's own settings and the catalogs added. Read leniently: what is
/// malformed or over the limits is dropped, never the rest of the preferences.
#[derive(Clone, Debug, Default, PartialEq, Serialize, serde::Deserialize)]
#[serde(from = "Value")]
pub struct PluginPrefs {
    /// Each plug-in's settings, by plug-in id ([`MAX_SETTINGS_BYTES`] each).
    pub settings: BTreeMap<String, Map<String, Value>>,
    /// Plug-in catalogs added with `plugin.catalog.add` (`index.json` URLs).
    pub catalogs: Vec<String>,
}

impl From<Value> for PluginPrefs {
    fn from(v: Value) -> Self {
        let mut out = Self::default();
        let mut total = 0usize;
        if let Some(all) = v.get("settings").and_then(Value::as_object) {
            for (id, mine) in all.iter().take(MAX_SETTINGS_PLUGINS) {
                let Some(mine) = mine.as_object() else { continue };
                let mut kept = Map::new();
                let mut size = 0usize;
                for (k, v) in mine {
                    let len = k.len().saturating_add(v.to_string().len());
                    if valid_setting_key(k) && size.saturating_add(len) <= MAX_SETTINGS_BYTES && total.saturating_add(len) <= MAX_ALL_SETTINGS_BYTES {
                        size = size.saturating_add(len);
                        total = total.saturating_add(len);
                        kept.insert(k.clone(), v.clone());
                    }
                }
                if !id.is_empty() && !kept.is_empty() {
                    out.settings.insert(id.clone(), kept);
                }
            }
        }
        for url in v.get("catalogs").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str) {
            if out.catalogs.len() < MAX_CATALOGS && !out.catalogs.iter().any(|u| u == url) && super::install::check_catalog_url(url).is_ok() {
                out.catalogs.push(url.to_string());
            }
        }
        out
    }
}

/// Whether `key` may name a plug-in setting: 1–[`MAX_SETTING_KEY`] characters, none a control
/// character.
pub(super) fn valid_setting_key(key: &str) -> bool {
    !key.is_empty() && key.chars().count() <= MAX_SETTING_KEY && !key.chars().any(char::is_control)
}

/// What settings take up: their keys and JSON values, in bytes.
pub(super) fn settings_size<'a>(entries: impl Iterator<Item = (&'a String, &'a Value)>) -> usize {
    entries.map(|(k, v)| k.len().saturating_add(v.to_string().len())).fold(0, usize::saturating_add)
}

/// The HTTPS client installs download with, shared with the workers the UI runs downloads on.
/// Without one, only files and `file://` catalogs install. Local files go through the automation
/// read roots (`file_access`) whatever the client.
#[derive(Clone, Default)]
pub struct Fetcher(Arc<Mutex<Option<Box<dyn Fetch + Send>>>>);

impl Fetcher {
    /// Run `f` with the client (or [`NoNetwork`]), its local reads confined.
    pub fn with<T>(&self, f: impl FnOnce(&mut dyn Fetch) -> T) -> T {
        let mut client = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        match client.as_deref_mut() {
            Some(http) => f(&mut Confined(http)),
            None => f(&mut Confined(&mut NoNetwork)),
        }
    }

    /// Whether there is a client (else only local files install).
    pub fn has_network(&self) -> bool {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).is_some()
    }
}

/// Longest part of a URL an error repeats.
const SHOWN_URL_CHARS: usize = 120;

/// The fetcher of sessions without an HTTPS client: `file://` catalogs still read.
struct NoNetwork;

impl Fetch for NoNetwork {
    fn get(&mut self, url: &str, _max: usize) -> std::result::Result<Option<Vec<u8>>, String> {
        let shown: String = url.chars().take(SHOWN_URL_CHARS).collect();
        Err(format!("there is no network access here to read {shown}: install the command plug-in from a file, or in the SoundCraft app"))
    }
}

/// A client whose local reads (a `file://` catalog and the files it names) go through the engine's
/// checked reads: the automation read roots (`soundcraft-cli mcp --automation-read-root`), and a
/// regular file only.
struct Confined<'a>(&'a mut dyn Fetch);

impl Fetch for Confined<'_> {
    fn get(&mut self, url: &str, max: usize) -> std::result::Result<Option<Vec<u8>>, String> {
        self.0.get(url, max)
    }

    fn read_local(&mut self, path: &std::path::Path, max: usize) -> std::result::Result<Vec<u8>, String> {
        // the engine's checked gateway: the read roots, a regular file only, at most `max` bytes
        crate::io::read_file_bounded(path, max as u64).map_err(|e| e.to_string())
    }
}

/// The engine's command plug-ins: the installed ones, where they are kept, their settings and
/// catalogs, and the HTTPS client installs download with. The desktop app opens the store
/// ([`Runtime::open_store`]), sets the client ([`Runtime::set_fetch`]) and keeps
/// [`Runtime::prefs`] with its preferences; the CLI, MCP and the web keep plug-ins for the session.
#[derive(Default)]
pub struct Runtime {
    /// The user's `Plug-ins` folder.
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) store: Option<craft_plugin::store::Store>,
    plugins: Vec<InstalledPlugin>,
    /// Plug-in folders that couldn't be read, and plug-ins this version can't run.
    problems: Vec<String>,
    fetch: Fetcher,
    pub(crate) pending: Option<Pending>,
    /// Each plug-in's settings and the catalogs added.
    pub prefs: PluginPrefs,
    /// [`Runtime::prefs`] changed since [`Runtime::take_prefs_changed`] last looked.
    prefs_changed: bool,
    /// The last run (plug-in id, command id, params).
    pub last: Option<(String, String, Value)>,
}

impl Runtime {
    /// Keep plug-ins in `<settings_dir>/Plug-ins` and read the ones installed there.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn open_store(&mut self, settings_dir: &std::path::Path) {
        let store = craft_plugin::store::Store::new(craft_plugin::store::plugins_dir(settings_dir));
        let listing = store.list_for(&super::APP);
        self.plugins.retain(|p| !p.stored);
        self.problems = listing.problems.into_iter().map(|p| format!("{}: {}", p.dir.display(), p.error)).collect();
        // The store keeps whatever was installed; only plug-ins for this SoundCraft are offered
        self.problems.extend(listing.not_runnable.into_iter().map(|(i, why)| format!("{}: {why}", i.manifest.name)));
        for (i, _) in listing.runnable {
            let plugin = InstalledPlugin {
                manifest: i.manifest,
                sha256: i.record.sha256,
                source: i.record.source,
                stored: true,
                wasm: Arc::new(i.bundle.wasm),
            };
            self.keep(plugin);
        }
        self.store = Some(store);
    }

    /// Download catalogs and releases with `client`.
    pub fn set_fetch(&mut self, client: Option<Box<dyn Fetch + Send>>) {
        *self.fetch.0.lock().unwrap_or_else(PoisonError::into_inner) = client;
    }

    /// The HTTPS client, for a worker thread that downloads ([`super::download`]).
    pub fn fetcher(&self) -> Fetcher {
        self.fetch.clone()
    }

    /// The installed plug-ins.
    pub fn list(&self) -> &[InstalledPlugin] {
        &self.plugins
    }

    /// Why plug-in folders couldn't be used.
    pub fn problems(&self) -> &[String] {
        &self.problems
    }

    /// Plug-in `id` (ignoring case, as the store does).
    pub fn get(&self, id: &str) -> Option<&InstalledPlugin> {
        self.plugins.iter().find(|p| p.manifest.id.eq_ignore_ascii_case(id))
    }

    /// Whether plug-ins are kept in the user's folder (else for this session only).
    pub fn has_store(&self) -> bool {
        #[cfg(not(target_arch = "wasm32"))]
        return self.store.is_some();
        #[cfg(target_arch = "wasm32")]
        false
    }

    /// Store (`Some`) or forget (`None`) plug-in `id`'s setting `key` ([`super::set_setting`]).
    pub fn set_setting(&mut self, id: &str, key: &str, value: Option<&str>) -> std::result::Result<(), String> {
        let before = self.prefs.settings.get(id).and_then(|m| m.get(key)).cloned();
        super::set_setting(&mut self.prefs.settings, id, key, value)?;
        if self.prefs.settings.get(id).and_then(|m| m.get(key)) != before.as_ref() {
            self.prefs_changed = true;
        }
        Ok(())
    }

    /// Replace the preferences (the desktop app, from its saved settings), without marking them
    /// changed.
    pub fn set_prefs(&mut self, prefs: PluginPrefs) {
        self.prefs = prefs;
        self.prefs_changed = false;
    }

    /// Whether [`Runtime::prefs`] changed since the last call: the desktop app saves them then.
    pub fn take_prefs_changed(&mut self) -> bool {
        std::mem::take(&mut self.prefs_changed)
    }

    /// Note that [`Runtime::prefs`] changed.
    pub(crate) fn prefs_touched(&mut self) {
        self.prefs_changed = true;
    }

    /// Put `plugin` in the list, in place of one with the same id.
    pub(crate) fn keep(&mut self, plugin: InstalledPlugin) -> InstalledPlugin {
        match self.plugins.iter_mut().find(|p| p.manifest.id.eq_ignore_ascii_case(&plugin.manifest.id)) {
            Some(slot) => *slot = plugin.clone(),
            None => self.plugins.push(plugin.clone()),
        }
        plugin
    }

    /// Drop plug-in `id` from the list, with its settings.
    pub(crate) fn forget(&mut self, id: &str) {
        self.plugins.retain(|p| !p.manifest.id.eq_ignore_ascii_case(id));
        if self.last.as_ref().is_some_and(|(last, ..)| last.eq_ignore_ascii_case(id)) {
            self.last = None;
        }
        let before = self.prefs.settings.len();
        self.prefs.settings.retain(|k, _| !k.eq_ignore_ascii_case(id));
        if self.prefs.settings.len() != before {
            self.prefs_changed = true;
        }
    }
}
