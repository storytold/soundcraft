//! The process-wide CLAP registry: scan cache, leaked `PluginInfo`s, instantiation.

use crate::ffi::{Bundle, Instance, RawParam};
use crate::plugin::ClapPlugin;
use crate::scan as paths;
use crate::{ClapDescriptor, ClapError, parse_id};
use clap_sys::ext::params::{CLAP_PARAM_IS_ENUM, CLAP_PARAM_IS_HIDDEN, CLAP_PARAM_IS_READONLY, CLAP_PARAM_IS_STEPPED};
use soundcraft_dsp::{ParamInfo, Plugin, PluginInfo, Taper, Unit};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Condvar, Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::{Duration, Instant};

/// Most labels read for an enum parameter.
const MAX_CHOICES: f64 = 128.0;

/// A file that takes longer than this to load is named in the log (some plugins wait on a licence
/// check or a network drive at load, and a scan of hundreds of files is slow without saying why).
pub(crate) const SLOW_LOAD: Duration = Duration::from_secs(5);

#[derive(Default)]
struct Registry {
    scanned: Option<Vec<ClapDescriptor>>,
    extra_dirs: Vec<PathBuf>,
    /// One leaked `PluginInfo` per plugin type that was actually instantiated (bounded by the
    /// number of distinct installed plugins).
    infos: HashMap<String, &'static PluginInfo>,
    /// Ids that failed to instantiate (not retried every UI frame).
    failed: HashSet<String>,
    /// A scan is running (with the lock released); see [`ensure_scanned`].
    scanning: bool,
    /// A background scan was started and has not stored its result yet.
    background: bool,
    /// The next scan ignores the helper processes' cache (a rescan).
    force_next: bool,
}

fn registry() -> &'static Mutex<Registry> {
    static R: OnceLock<Mutex<Registry>> = OnceLock::new();
    R.get_or_init(|| Mutex::new(Registry::default()))
}

/// Descriptors of every plugin in one `.clap` file or bundle.
pub fn load_bundle(path: &Path) -> Result<Vec<ClapDescriptor>, ClapError> {
    let b = Bundle::load(path)?;
    Ok(b.descriptors()
        .into_iter()
        .map(|d| ClapDescriptor {
            id: format!("{}{}", crate::ID_PREFIX, d.id),
            category: paths::category_from_features(&d.features),
            is_instrument: paths::is_instrument(&d.features),
            plugin_id: d.id,
            name: d.name,
            vendor: d.vendor,
            version: d.version,
            description: d.description,
            features: d.features,
            path: path.to_path_buf(),
        })
        .collect())
}

/// Scans `dirs` (uncached). Unloadable files are logged and skipped; the first plugin with a
/// given id wins.
pub fn scan_paths(dirs: &[PathBuf]) -> Vec<ClapDescriptor> {
    scan_dirs(dirs, false)
}

/// [`scan_paths`], in helper processes when the app enabled them ([`crate::isolated`]); `force`
/// ignores their cache.
fn scan_dirs(dirs: &[PathBuf], force: bool) -> Vec<ClapDescriptor> {
    let bundles = paths::find_bundles(dirs);
    let found = if crate::isolated::enabled() { crate::isolated::scan(&bundles, force) } else { scan_in_process(&bundles) };
    let mut out: Vec<ClapDescriptor> = Vec::new();
    for d in found {
        if !out.iter().any(|o| o.id == d.id) {
            out.push(d);
        }
    }
    out.sort_by_key(|d| d.name.to_lowercase());
    out
}

/// Loads every file into this process.
///
/// Each file is logged *before* it is loaded. Loading runs the plugin's own code in this process,
/// and a plugin that crashes there (an access violation, or a C++ exception unwinding into Rust,
/// which aborts) takes the app down before any error can be logged. The log file is written
/// unbuffered, so the last `scanning` line then names the plugin that did it.
fn scan_in_process(bundles: &[PathBuf]) -> Vec<ClapDescriptor> {
    let started = Instant::now();
    crate::isolated::start_progress(bundles.len());
    let mut out = Vec::new();
    let mut failed = 0usize;
    for path in bundles {
        log::info!("scanning CLAP plugin {}", path.display());
        let one = Instant::now();
        match load_bundle(path) {
            Ok(ds) => out.extend(ds),
            Err(e) => {
                failed = failed.saturating_add(1);
                log::warn!("{e}");
            }
        }
        let took = one.elapsed();
        if took >= SLOW_LOAD {
            log::warn!("CLAP plugin {} took {:.1} s to scan", path.display(), took.as_secs_f64());
        }
        crate::isolated::step_progress();
    }
    log::info!("CLAP scan: {} plugins from {} files ({failed} failed) in {:.1} s", out.len(), bundles.len(), started.elapsed().as_secs_f64());
    out
}

/// Signalled when a scan stores its result, for callers waiting on one that is running.
fn scan_finished() -> &'static Condvar {
    static C: Condvar = Condvar::new();
    &C
}

fn lock() -> MutexGuard<'static, Registry> {
    registry().lock().unwrap_or_else(PoisonError::into_inner)
}

/// The scanned list, scanning first if needed. The lock is released while the scan runs (the UI
/// can keep asking how far it got); a caller that finds a scan already running waits for it
/// rather than starting a second one.
fn ensure_scanned(mut r: MutexGuard<'static, Registry>) -> MutexGuard<'static, Registry> {
    while r.scanned.is_none() && r.scanning {
        r = scan_finished().wait(r).unwrap_or_else(PoisonError::into_inner);
    }
    if r.scanned.is_some() {
        return r;
    }
    r.scanning = true;
    let force = std::mem::take(&mut r.force_next);
    let mut dirs = paths::default_search_paths();
    dirs.extend(r.extra_dirs.iter().cloned());
    drop(r);
    let list = scan_dirs(&dirs, force);
    let mut r = lock();
    r.scanned = Some(list);
    r.scanning = false;
    scan_finished().notify_all();
    r
}

pub fn scan() -> Vec<ClapDescriptor> {
    ensure_scanned(lock()).scanned.clone().unwrap_or_default()
}

pub fn cached() -> Option<Vec<ClapDescriptor>> {
    registry().lock().unwrap_or_else(PoisonError::into_inner).scanned.clone()
}

pub fn rescan() -> Vec<ClapDescriptor> {
    let mut r = lock();
    while r.scanning {
        r = scan_finished().wait(r).unwrap_or_else(PoisonError::into_inner);
    }
    r.scanned = None;
    r.failed.clear();
    r.force_next = true;
    ensure_scanned(r).scanned.clone().unwrap_or_default()
}

/// The list if the scan is done; otherwise starts it on a background thread (once) and says how
/// far it got. Only with helper processes: an in-process scan stays where it was asked from,
/// because a module must be entered on the thread that will run its exit.
pub fn scan_in_background() -> crate::ScanState {
    let mut r = lock();
    if let Some(list) = &r.scanned {
        return crate::ScanState::Ready(list.clone());
    }
    if !crate::isolated::enabled() {
        return crate::ScanState::Ready(ensure_scanned(r).scanned.clone().unwrap_or_default());
    }
    if !r.scanning && !r.background {
        r.background = true;
        crate::isolated::start_progress(0);
        let spawned = std::thread::Builder::new().name("clap-scan".into()).spawn(|| {
            let mut r = ensure_scanned(lock());
            r.background = false;
        });
        if spawned.is_err() {
            r.background = false;
            return crate::ScanState::Ready(ensure_scanned(r).scanned.clone().unwrap_or_default());
        }
    }
    let (done, total) = crate::isolated::progress();
    crate::ScanState::Scanning { done, total }
}

pub fn add_search_dir(dir: &Path) -> usize {
    let mut r = lock();
    if !r.extra_dirs.iter().any(|d| d == dir) {
        r.extra_dirs.push(dir.to_path_buf());
    }
    let found = scan_paths(&[dir.to_path_buf()]);
    let n = found.len();
    let list = r.scanned.get_or_insert_with(Vec::new);
    for d in found {
        if !list.iter().any(|o| o.id == d.id) {
            list.push(d);
        }
    }
    n
}

fn leak(s: String) -> &'static str {
    Box::leak(s.into_boxed_str())
}

fn param_info(inst: &Instance, p: &RawParam) -> ParamInfo {
    let stepped = p.flags & CLAP_PARAM_IS_STEPPED != 0;
    let is_enum = p.flags & CLAP_PARAM_IS_ENUM != 0;
    let (mut unit, mut choices): (Unit, &'static [&'static str]) = (Unit::None, &[]);
    if stepped && p.min == 0.0 && p.max == 1.0 && !is_enum {
        unit = Unit::Toggle;
    } else if stepped && p.min == 0.0 && p.max >= 1.0 && p.max <= MAX_CHOICES {
        let labels: Vec<&'static str> =
            (0..=p.max as u32).map(|i| leak(inst.value_text(p.id, f64::from(i)).unwrap_or_else(|| i.to_string()))).collect();
        unit = Unit::Choice;
        choices = Box::leak(labels.into_boxed_slice());
    }
    let name = match (p.module.is_empty(), p.name.is_empty()) {
        (_, true) => format!("Param {}", p.id),
        (true, false) => p.name.clone(),
        (false, false) => format!("{}/{}", p.module, p.name),
    };
    ParamInfo {
        id: leak(p.id.to_string()),
        name: leak(name),
        min: p.min as f32,
        max: p.max as f32,
        default: p.default as f32,
        unit,
        taper: Taper::Linear,
        choices,
    }
}

/// Builds and leaks the `PluginInfo` for one plugin type. Called at most once per id (cached).
fn build_info(d: &ClapDescriptor, inst: &Instance) -> &'static PluginInfo {
    let params: Vec<ParamInfo> =
        inst.params().iter().filter(|p| p.flags & (CLAP_PARAM_IS_HIDDEN | CLAP_PARAM_IS_READONLY) == 0).map(|p| param_info(inst, p)).collect();
    let short: String = d.name.chars().take(8).collect();
    Box::leak(Box::new(PluginInfo {
        id: leak(d.id.clone()),
        name: leak(d.name.clone()),
        short_name: leak(short),
        category: d.category,
        params: Box::leak(params.into_boxed_slice()),
        is_instrument: d.is_instrument,
        audiosuite: false,
    }))
}

/// Finds the descriptor, loads its binary and creates an instance plus its (cached) info.
fn instantiate_raw(id: &str) -> Result<(Instance, &'static PluginInfo), ClapError> {
    let plugin_id = parse_id(id).ok_or_else(|| ClapError::NotFound(id.to_string()))?;
    let desc = {
        let r = lock();
        if r.failed.contains(id) {
            return Err(ClapError::Instantiate(plugin_id.to_string(), "failed before".into()));
        }
        let r = ensure_scanned(r);
        r.scanned.iter().flatten().find(|d| d.id == id).cloned().ok_or_else(|| ClapError::NotFound(id.to_string()))?
    };
    let made = Bundle::load(&desc.path).and_then(|b| b.instantiate(plugin_id));
    let mut r = registry().lock().unwrap_or_else(PoisonError::into_inner);
    match made {
        Ok(inst) => {
            let info = match r.infos.get(id) {
                Some(i) => *i,
                None => {
                    let i = build_info(&desc, &inst);
                    r.infos.insert(id.to_string(), i);
                    i
                }
            };
            Ok((inst, info))
        }
        Err(e) => {
            r.failed.insert(id.to_string());
            Err(e)
        }
    }
}

pub fn instantiate(id: &str) -> Result<Box<dyn Plugin>, ClapError> {
    let (inst, info) = instantiate_raw(id)?;
    let mut p = ClapPlugin::new(inst, info);
    p.prepare(48_000.0, 1024, 2);
    Ok(Box::new(p))
}

pub fn plugin_info(id: &str) -> Option<&'static PluginInfo> {
    parse_id(id)?;
    {
        let r = registry().lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(i) = r.infos.get(id) {
            return Some(*i);
        }
        if r.failed.contains(id) {
            return None;
        }
    }
    // Instantiate once to learn the parameters (CLAP exposes them only on an instance).
    instantiate_raw(id).ok().map(|(_, info)| info)
}
