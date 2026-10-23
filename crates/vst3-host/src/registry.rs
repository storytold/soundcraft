//! The process-wide VST3 registry: scan cache, leaked `PluginInfo`s, instantiation.

use crate::cache::{self, Entry, Stamp};
use crate::ffi::{Bundle, Instance, PARAM_BYPASS, PARAM_HIDDEN, PARAM_READ_ONLY, RawParam};
use crate::plugin::Vst3Plugin;
use crate::scan as paths;
use crate::{AUDIO_MODULE_CLASS, ScanProgress, Vst3Descriptor, Vst3Error, parse_id};
use soundcraft_dsp::{ParamInfo, Plugin, PluginInfo, Taper, Unit};
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock, PoisonError};
use std::time::{Duration, Instant};

/// Most labels read for a stepped parameter (more steps are shown as a plain range).
const MAX_CHOICES: i32 = 128;

#[derive(Default)]
struct Registry {
    scanned: Option<Vec<Vst3Descriptor>>,
    extra_dirs: Vec<PathBuf>,
    /// One leaked `PluginInfo` per plugin type that was actually instantiated (bounded by the
    /// number of distinct installed plugins).
    infos: HashMap<String, &'static PluginInfo>,
    /// Ids that failed to instantiate (not retried every UI frame).
    failed: HashSet<String>,
    /// Where the scan cache lives (`None`: scans are not cached on disk).
    cache_file: Option<PathBuf>,
    /// The scan under way, advanced a bundle at a time by [`scan_step`].
    job: Option<ScanJob>,
}

/// A scan in progress: the bundles still to list and what the finished ones held.
struct ScanJob {
    pending: VecDeque<PathBuf>,
    total: usize,
    /// The cache read at the start (empty when rescanning: every bundle is loaded again).
    known: HashMap<PathBuf, Entry>,
    found: Vec<Vst3Descriptor>,
    entries: Vec<Entry>,
}

impl ScanJob {
    fn new(dirs: &[PathBuf], known: HashMap<PathBuf, Entry>) -> ScanJob {
        let pending: VecDeque<PathBuf> = paths::find_bundles(dirs).into();
        ScanJob { total: pending.len(), pending, known, found: Vec::new(), entries: Vec::new() }
    }

    /// Adds `bundles` the scan has not listed or queued yet (a folder added while it runs).
    fn include(&mut self, bundles: Vec<PathBuf>) {
        for b in bundles {
            if !self.pending.contains(&b) && !self.entries.iter().any(|e| e.path == b) {
                self.pending.push_back(b);
                self.total += 1;
            }
        }
    }

    fn progress(&self) -> ScanProgress {
        let next = self.pending.front().and_then(|p| p.file_stem()).map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        ScanProgress::Scanning { done: self.total.saturating_sub(self.pending.len()), total: self.total, next }
    }

    /// Lists bundles until `budget` is spent; a bundle that must be loaded (new, changed or never
    /// cached) is the slow part, so at most one is loaded per call once any work has been done.
    fn step(&mut self, budget: Duration) {
        let start = Instant::now();
        let mut loaded = false;
        while let Some(path) = self.pending.front().cloned() {
            let stamp = Stamp::of(&path);
            let cached = stamp.and_then(|st| self.known.get(&path).filter(|e| e.stamp == st)).cloned();
            if cached.is_none() && loaded {
                break;
            }
            self.pending.pop_front();
            let entry = match (cached, stamp) {
                (Some(e), _) => e,
                (None, stamp) => {
                    loaded = true;
                    let (descriptors, error) = match load_bundle(&path) {
                        Ok(ds) => (ds, None),
                        Err(e) => {
                            log::warn!("{e}");
                            (Vec::new(), Some(e.to_string()))
                        }
                    };
                    let stamp = stamp.unwrap_or(Stamp { size: 0, modified_ns: 0 });
                    Entry { path: path.clone(), stamp, descriptors, error }
                }
            };
            for d in &entry.descriptors {
                if !self.found.iter().any(|o| o.id == d.id) {
                    self.found.push(d.clone());
                }
            }
            self.entries.push(entry);
            if start.elapsed() >= budget {
                break;
            }
        }
    }
}

fn registry() -> &'static Mutex<Registry> {
    static R: OnceLock<Mutex<Registry>> = OnceLock::new();
    R.get_or_init(|| Mutex::new(Registry::default()))
}

/// Descriptors of every audio module class in one `.vst3`.
pub fn load_bundle(path: &Path) -> Result<Vec<Vst3Descriptor>, Vst3Error> {
    let b = Bundle::load(path)?;
    Ok(b.classes()
        .into_iter()
        .filter(|c| c.category == AUDIO_MODULE_CLASS)
        .map(|c| {
            let subs = paths::split_sub_categories(&c.sub_categories);
            let class_id = crate::ffi::hex(&c.cid);
            Vst3Descriptor {
                id: format!("{}{class_id}", crate::ID_PREFIX),
                category: paths::category_from_sub_categories(&subs),
                is_instrument: paths::is_instrument(&subs),
                class_id,
                name: if c.name.is_empty() { "VST3 plugin".to_string() } else { c.name },
                vendor: c.vendor,
                version: c.version,
                sdk_version: c.sdk_version,
                sub_categories: subs,
                path: path.to_path_buf(),
            }
        })
        .collect())
}

/// Scans `dirs` (uncached). Unloadable files are logged and skipped; the first plugin with a
/// given id wins.
pub fn scan_paths(dirs: &[PathBuf]) -> Vec<Vst3Descriptor> {
    let mut out: Vec<Vst3Descriptor> = Vec::new();
    for path in paths::find_bundles(dirs) {
        match load_bundle(&path) {
            Ok(ds) => {
                for d in ds {
                    if !out.iter().any(|o| o.id == d.id) {
                        out.push(d);
                    }
                }
            }
            Err(e) => log::warn!("{e}"),
        }
    }
    out.sort_by_key(|d| d.name.to_lowercase());
    out
}

fn search_dirs(r: &Registry) -> Vec<PathBuf> {
    let mut dirs = paths::default_search_paths();
    dirs.extend(r.extra_dirs.iter().cloned());
    dirs
}

/// Starts a scan (unless one is under way): from the disk cache, or loading every bundle when `fresh`.
fn start_job(r: &mut Registry, fresh: bool) {
    if r.job.is_none() {
        let known = match (&r.cache_file, fresh) {
            (Some(f), false) => cache::read(f),
            _ => HashMap::new(),
        };
        r.job = Some(ScanJob::new(&search_dirs(r), known));
    }
}

/// Advances the scan; when it has finished, publishes the list and saves the cache.
fn advance(r: &mut Registry, budget: Duration) -> ScanProgress {
    let Some(job) = r.job.as_mut() else { return ScanProgress::Done };
    job.step(budget);
    if !job.pending.is_empty() {
        return job.progress();
    }
    let Some(job) = r.job.take() else { return ScanProgress::Done };
    let mut found = job.found;
    found.sort_by_key(|d| d.name.to_lowercase());
    r.scanned = Some(found);
    if let Some(f) = &r.cache_file
        && let Err(e) = cache::write(f, job.entries)
    {
        log::warn!("vst3: cannot save the scan cache {}: {e}", f.display());
    }
    ScanProgress::Done
}

fn ensure_scanned(r: &mut Registry) -> &Vec<Vst3Descriptor> {
    if r.scanned.is_none() {
        start_job(r, false);
        while advance(r, Duration::MAX) != ScanProgress::Done {}
    }
    r.scanned.get_or_insert_with(Vec::new)
}

pub fn scan() -> Vec<Vst3Descriptor> {
    let mut r = registry().lock().unwrap_or_else(PoisonError::into_inner);
    ensure_scanned(&mut r).clone()
}

pub fn cached() -> Option<Vec<Vst3Descriptor>> {
    registry().lock().unwrap_or_else(PoisonError::into_inner).scanned.clone()
}

pub fn rescan() -> Vec<Vst3Descriptor> {
    let mut r = registry().lock().unwrap_or_else(PoisonError::into_inner);
    r.scanned = None;
    r.job = None;
    r.failed.clear();
    start_job(&mut r, true);
    ensure_scanned(&mut r).clone()
}

pub fn scan_step(budget: Duration) -> ScanProgress {
    let mut r = registry().lock().unwrap_or_else(PoisonError::into_inner);
    if r.scanned.is_some() {
        return ScanProgress::Done;
    }
    start_job(&mut r, false);
    advance(&mut r, budget)
}

pub fn scan_in_progress() -> bool {
    registry().lock().unwrap_or_else(PoisonError::into_inner).job.is_some()
}

pub fn set_cache_file(path: &Path) {
    registry().lock().unwrap_or_else(PoisonError::into_inner).cache_file = Some(path.to_path_buf());
}

pub fn add_search_dir(dir: &Path) -> usize {
    let mut r = registry().lock().unwrap_or_else(PoisonError::into_inner);
    if !r.extra_dirs.iter().any(|d| d == dir) {
        r.extra_dirs.push(dir.to_path_buf());
    }
    let found = scan_paths(&[dir.to_path_buf()]);
    let n = found.len();
    // A scan under way lists the folder too (its bundles are loaded now, so that is cheap):
    // a list of only this folder would end it with the standard folders' plugins missing.
    if r.scanned.is_none()
        && let Some(job) = r.job.as_mut()
    {
        job.include(paths::find_bundles(&[dir.to_path_buf()]));
        return n;
    }
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

/// Maps a VST3 unit label to a SoundCraft display unit.
pub(crate) fn unit_from_label(units: &str) -> Unit {
    match units.trim().to_ascii_lowercase().as_str() {
        "db" => Unit::Db,
        "hz" => Unit::Hz,
        "ms" => Unit::Ms,
        "%" => Unit::Percent,
        "s" | "sec" | "secs" | "seconds" => Unit::Seconds,
        "st" | "semi" | "semitones" => Unit::Semitones,
        "ct" | "cent" | "cents" => Unit::Cents,
        _ => Unit::None,
    }
}

fn param_info(inst: &Instance, p: &RawParam) -> ParamInfo {
    let name = if p.title.is_empty() { format!("Param {}", p.id) } else { p.title.clone() };
    let base = ParamInfo {
        id: leak(p.id.to_string()),
        name: leak(name),
        min: 0.0,
        max: 1.0,
        default: 0.0,
        unit: Unit::None,
        taper: Taper::Linear,
        choices: &[],
    };
    if p.step_count == 1 {
        return ParamInfo { unit: Unit::Toggle, default: p.default_normalized.round() as f32, ..base };
    }
    if (2..=MAX_CHOICES).contains(&p.step_count) {
        let steps = f64::from(p.step_count);
        let labels: Vec<&'static str> =
            (0..=p.step_count).map(|i| leak(inst.value_text(p.id, f64::from(i) / steps).unwrap_or_else(|| i.to_string()))).collect();
        return ParamInfo {
            max: p.step_count as f32,
            default: (p.default_normalized * steps).round() as f32,
            unit: Unit::Choice,
            choices: Box::leak(labels.into_boxed_slice()),
            ..base
        };
    }
    // Continuous (or too many steps for a menu): plain range from the controller.
    let unit = unit_from_label(&p.units);
    match (inst.to_plain(p.id, 0.0), inst.to_plain(p.id, 1.0)) {
        (Some(lo), Some(hi)) if hi > lo && (lo as f32).is_finite() && (hi as f32).is_finite() && (hi as f32) > (lo as f32) => {
            let default = inst.to_plain(p.id, p.default_normalized).map_or(lo, |d| d.clamp(lo, hi));
            // A logarithmic plugin mapping (frequency knobs) shows its geometric mean at mid travel.
            let taper = match inst.to_plain(p.id, 0.5) {
                Some(mid)
                    if lo > 0.0
                        && ((mid - (lo * hi).sqrt()).abs() <= 0.01 * (lo * hi).sqrt())
                        && (mid - (lo + hi) / 2.0).abs() > 0.01 * (hi - lo) =>
                {
                    Taper::Log
                }
                _ => Taper::Linear,
            };
            ParamInfo { min: lo as f32, max: hi as f32, default: default as f32, unit, taper, ..base }
        }
        _ => ParamInfo { default: p.default_normalized as f32, unit, ..base },
    }
}

/// Builds and leaks the `PluginInfo` for one plugin type. Called at most once per id (cached).
fn build_info(d: &Vst3Descriptor, inst: &Instance) -> &'static PluginInfo {
    let params: Vec<ParamInfo> =
        inst.params().iter().filter(|p| p.flags & (PARAM_HIDDEN | PARAM_READ_ONLY | PARAM_BYPASS) == 0).map(|p| param_info(inst, p)).collect();
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
fn instantiate_raw(id: &str) -> Result<(Instance, &'static PluginInfo), Vst3Error> {
    let cid = parse_id(id).ok_or_else(|| Vst3Error::NotFound(id.to_string()))?;
    let desc = {
        let mut r = registry().lock().unwrap_or_else(PoisonError::into_inner);
        if r.failed.contains(id) {
            return Err(Vst3Error::Instantiate(id.to_string(), "failed before".into()));
        }
        ensure_scanned(&mut r).iter().find(|d| d.id == id).cloned().ok_or_else(|| Vst3Error::NotFound(id.to_string()))?
    };
    let made = Bundle::load(&desc.path).and_then(|b| b.instantiate(&cid));
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

pub fn instantiate(id: &str) -> Result<Box<dyn Plugin>, Vst3Error> {
    Ok(Box::new(instantiate_plugin(id)?))
}

pub fn instantiate_plugin(id: &str) -> Result<Vst3Plugin, Vst3Error> {
    let (inst, info) = instantiate_raw(id)?;
    let mut p = Vst3Plugin::new(inst, info);
    p.prepare(48_000.0, 1024, 2);
    Ok(p)
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
    // Instantiate once to learn the parameters (VST3 exposes them only on a live controller).
    instantiate_raw(id).ok().map(|(_, info)| info)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unit_labels_map() {
        assert_eq!(unit_from_label("dB"), Unit::Db);
        assert_eq!(unit_from_label(" Hz "), Unit::Hz);
        assert_eq!(unit_from_label("%"), Unit::Percent);
        assert_eq!(unit_from_label("ms"), Unit::Ms);
        assert_eq!(unit_from_label("sec"), Unit::Seconds);
        assert_eq!(unit_from_label("cents"), Unit::Cents);
        assert_eq!(unit_from_label("st"), Unit::Semitones);
        assert_eq!(unit_from_label("furlongs"), Unit::None);
    }

    fn folder_with(name: &str, bundles: &[&str]) -> PathBuf {
        let d = std::env::temp_dir().join(format!("sc-vst3-job-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        for b in bundles {
            // Not a loadable module: the scan records why and moves on.
            std::fs::write(d.join(b), b"not a plugin").unwrap();
        }
        d
    }

    #[test]
    fn a_folder_added_during_a_scan_joins_it_and_unchanged_bundles_come_from_the_cache() {
        let (a, b) = (folder_with("a", &["One.vst3"]), folder_with("b", &["Two.vst3", "Three.vst3"]));
        let mut job = ScanJob::new(std::slice::from_ref(&a), HashMap::new());
        assert_eq!(job.total, 1);
        job.include(paths::find_bundles(std::slice::from_ref(&b)));
        job.include(paths::find_bundles(std::slice::from_ref(&b)));
        assert_eq!(job.total, 3, "an added folder's bundles are queued once");
        // At most one bundle is loaded per step: three steps for three uncached bundles.
        for _ in 0..3 {
            job.step(Duration::MAX);
        }
        assert!(job.pending.is_empty());
        assert_eq!(job.entries.len(), 3);
        assert!(job.entries.iter().all(|e| e.error.is_some() && e.descriptors.is_empty()));
        // A later scan lists every unchanged bundle from the cache in one step, loading none.
        let known: HashMap<PathBuf, Entry> = job.entries.iter().map(|e| (e.path.clone(), e.clone())).collect();
        let mut again = ScanJob::new(&[a.clone(), b.clone()], known);
        again.step(Duration::MAX);
        assert!(again.pending.is_empty(), "nothing had to be loaded");
        assert_eq!(again.entries, job.entries, "the same entries, in the same (path) order");
        let _ = (std::fs::remove_dir_all(&a), std::fs::remove_dir_all(&b));
    }
}
