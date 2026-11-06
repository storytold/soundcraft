//! The process-wide CLAP registry: scan cache, leaked `PluginInfo`s, instantiation.

use crate::ffi::{Bundle, Instance, RawParam};
use crate::plugin::ClapPlugin;
use crate::scan as paths;
use crate::{ClapDescriptor, ClapError, parse_id};
use clap_sys::ext::params::{CLAP_PARAM_IS_ENUM, CLAP_PARAM_IS_HIDDEN, CLAP_PARAM_IS_READONLY, CLAP_PARAM_IS_STEPPED};
use soundcraft_dsp::{ParamInfo, Plugin, PluginInfo, Taper, Unit};
use soundcraft_plugin_scan as scan_cache;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, MutexGuard, OnceLock, PoisonError};

/// Most labels read for an enum parameter.
const MAX_CHOICES: f64 = 128.0;

#[derive(Default)]
struct Registry {
    scanned: Option<Vec<ClapDescriptor>>,
    /// While a scan runs: the plugins of the cache file or of the scan before, so that a plugin
    /// can be created without waiting for the scan.
    remembered: Vec<ClapDescriptor>,
    extra_dirs: Vec<PathBuf>,
    /// Where scans are remembered across runs (see [`scan_cache`]); `None` keeps them in memory only.
    cache_file: Option<PathBuf>,
    /// The program that loads files for scans in a child process (see [`set_scanner`]).
    scanner: Option<PathBuf>,
    /// The files the last scan could not load, with the reason.
    problems: Vec<(PathBuf, String)>,
    /// One leaked `PluginInfo` per plugin type that was actually instantiated (bounded by the
    /// number of distinct installed plugins).
    infos: HashMap<String, &'static PluginInfo>,
    /// Ids that failed to instantiate (not retried every UI frame).
    failed: HashSet<String>,
}

fn registry() -> &'static Mutex<Registry> {
    static R: OnceLock<Mutex<Registry>> = OnceLock::new();
    R.get_or_init(|| Mutex::new(Registry::default()))
}

fn lock() -> MutexGuard<'static, Registry> {
    registry().lock().unwrap_or_else(PoisonError::into_inner)
}

/// Held for a whole scan, so that scans run one at a time. The registry itself is not locked
/// while plugin files load, so that the UI can ask for [`cached`] and plugin infos meanwhile.
fn scan_lock() -> MutexGuard<'static, ()> {
    static S: Mutex<()> = Mutex::new(());
    S.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Set while [`scan_in_background`]'s thread runs.
static BACKGROUND: AtomicBool = AtomicBool::new(false);

/// Set by [`skip_scan`]: the running scan loads no more files.
static SKIP: AtomicBool = AtomicBool::new(false);

/// The file a scan is loading: its index, the number of files, its path.
fn loading() -> MutexGuard<'static, Option<(usize, usize, PathBuf)>> {
    static L: Mutex<Option<(usize, usize, PathBuf)>> = Mutex::new(None);
    L.lock().unwrap_or_else(PoisonError::into_inner)
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
    merge(scan_cache::scan(
        paths::find_bundles(dirs),
        &[],
        paths::bundle_executable,
        |p| Some(load_bundle(p).map_err(|e| e.to_string())),
        |_, _, _| true,
    ))
}

/// The plugins of every file, the first plugin with a given id winning, sorted by name.
fn merge(files: Vec<scan_cache::Entry<ClapDescriptor>>) -> Vec<ClapDescriptor> {
    let mut out: Vec<ClapDescriptor> = Vec::new();
    for d in files.into_iter().flat_map(|f| f.plugins) {
        if !out.iter().any(|o| o.id == d.id) {
            out.push(d);
        }
    }
    out.sort_by_key(|d| d.name.to_lowercase());
    out
}

/// Scans the search folders and keeps the result. With `reuse`, files the cache file knows are
/// not loaded again unless they changed; either way the cache file is rewritten.
fn scan_now(reuse: bool) -> Vec<ClapDescriptor> {
    let (dirs, cache_file, scanner) = {
        let r = lock();
        let mut dirs = paths::default_search_paths();
        dirs.extend(r.extra_dirs.iter().cloned());
        (dirs, r.cache_file.clone(), r.scanner.clone())
    };
    let load = |p: &Path| match &scanner {
        // Skip kills a child that is loading; a file loading here has to finish.
        Some(program) => scan_cache::load_in_child(program, SCAN_FLAG, p, || SKIP.load(Ordering::Acquire)),
        None => Some(load_bundle(p).map_err(|e| e.to_string())),
    };
    let known = match &cache_file {
        Some(f) if reuse => scan_cache::read(f),
        _ => Vec::new(),
    };
    if !known.is_empty() {
        lock().remembered = merge(known.clone());
    }
    SKIP.store(false, Ordering::Release);
    let ahead = |i: usize, n: usize, p: &Path| {
        *loading() = Some((i, n, p.to_path_buf()));
        !SKIP.load(Ordering::Acquire)
    };
    let files = scan_cache::scan(paths::find_bundles(&dirs), &known, paths::bundle_executable, load, ahead);
    *loading() = None;
    if let Some(f) = &cache_file {
        scan_cache::write(f, &files);
    }
    let problems = files.iter().filter_map(|f| Some((f.path.clone(), f.error.clone()?))).collect();
    let list = merge(files);
    let mut r = lock();
    r.scanned = Some(list.clone());
    r.problems = problems;
    list
}

pub fn scan() -> Vec<ClapDescriptor> {
    let _one_at_a_time = scan_lock();
    cached().unwrap_or_else(|| scan_now(true))
}

pub fn cached() -> Option<Vec<ClapDescriptor>> {
    lock().scanned.clone()
}

pub fn rescan() -> Vec<ClapDescriptor> {
    rescan_with(false)
}

/// Forgets the scan and the failed instances and scans again, reusing the cache file with `reuse`.
fn rescan_with(reuse: bool) -> Vec<ClapDescriptor> {
    let _one_at_a_time = scan_lock();
    {
        let mut r = lock();
        if let Some(before) = r.scanned.take() {
            r.remembered = before;
        }
        r.failed.clear();
    }
    scan_now(reuse)
}

pub fn set_cache_file(file: Option<PathBuf>) {
    lock().cache_file = file;
}

pub fn scan_in_background() {
    if cached().is_none() {
        in_background(|| {
            scan();
        });
    }
}

pub fn rescan_in_background(clear_cache: bool) {
    in_background(if clear_cache { || drop(rescan_with(false)) } else { || drop(rescan_with(true)) });
}

/// Runs `job` on a scan thread, unless one is running already.
fn in_background(job: fn()) {
    if BACKGROUND.swap(true, Ordering::AcqRel) {
        return;
    }
    /// Clears [`BACKGROUND`] however the job ends.
    struct Done;
    impl Drop for Done {
        fn drop(&mut self) {
            BACKGROUND.store(false, Ordering::Release);
        }
    }
    let done = Done;
    if let Err(e) = std::thread::Builder::new().name("clap-scan".into()).spawn(move || {
        let _done = done;
        job();
    }) {
        log::warn!("clap: no scan thread: {e}");
    }
}

pub fn search_dirs() -> (Vec<PathBuf>, Vec<PathBuf>) {
    (paths::default_search_paths(), lock().extra_dirs.clone())
}

pub fn set_extra_dirs(dirs: Vec<PathBuf>) {
    lock().extra_dirs = dirs;
}

/// The argument that makes the scan program load one file: `<program> --scan-clap <file>`.
pub const SCAN_FLAG: &str = "--scan-clap";

pub fn set_scanner(program: Option<PathBuf>) {
    lock().scanner = program;
}

pub fn scan_reply(file: &Path) -> String {
    scan_cache::reply(load_bundle(file))
}

pub fn progress() -> Option<(usize, usize, PathBuf)> {
    loading().clone()
}

pub fn skip_scan() {
    SKIP.store(true, Ordering::Release);
}

pub fn problems() -> Vec<(PathBuf, String)> {
    lock().problems.clone()
}

pub fn scanning() -> bool {
    BACKGROUND.load(Ordering::Acquire)
}

pub fn add_search_dir(dir: &Path) -> usize {
    {
        let mut r = lock();
        if !r.extra_dirs.iter().any(|d| d == dir) {
            r.extra_dirs.push(dir.to_path_buf());
        }
    }
    let found = scan_paths(&[dir.to_path_buf()]);
    let n = found.len();
    // Not scanned yet: the next scan covers the folder along with the others.
    if let Some(list) = lock().scanned.as_mut() {
        for d in found {
            if !list.iter().any(|o| o.id == d.id) {
                list.push(d);
            }
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
    if lock().failed.contains(id) {
        return Err(ClapError::Instantiate(plugin_id.to_string(), "failed before".into()));
    }
    let known = {
        let r = lock();
        r.scanned.iter().flatten().chain(&r.remembered).find(|d| d.id == id).cloned()
    };
    let desc = match known {
        Some(d) => d,
        None => scan().into_iter().find(|d| d.id == id).ok_or_else(|| ClapError::NotFound(id.to_string()))?,
    };
    let made = Bundle::load(&desc.path).and_then(|b| b.instantiate(plugin_id));
    let mut r = lock();
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
        let r = lock();
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
