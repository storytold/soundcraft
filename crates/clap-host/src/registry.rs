//! The process-wide CLAP registry: scan cache, leaked `PluginInfo`s, instantiation.

use crate::ffi::{Bundle, Instance, RawParam};
use crate::plugin::ClapPlugin;
use crate::scan as paths;
use crate::{ClapDescriptor, ClapError, parse_id};
use clap_sys::ext::params::{CLAP_PARAM_IS_ENUM, CLAP_PARAM_IS_HIDDEN, CLAP_PARAM_IS_READONLY, CLAP_PARAM_IS_STEPPED};
use soundcraft_dsp::{ParamInfo, Plugin, PluginInfo, Taper, Unit};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock, PoisonError};

/// Most labels read for an enum parameter.
const MAX_CHOICES: f64 = 128.0;

#[derive(Default)]
struct Registry {
    scanned: Option<Vec<ClapDescriptor>>,
    extra_dirs: Vec<PathBuf>,
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
    let mut out: Vec<ClapDescriptor> = Vec::new();
    for d in soundcraft_plugin_scan::probe("clap", &paths::find_bundles(dirs), load_bundle) {
        if !out.iter().any(|o| o.id == d.id) {
            out.push(d);
        }
    }
    out.sort_by_key(|d| d.name.to_lowercase());
    out
}

fn ensure_scanned(r: &mut Registry) -> &Vec<ClapDescriptor> {
    if r.scanned.is_none() {
        let mut dirs = paths::default_search_paths();
        dirs.extend(r.extra_dirs.iter().cloned());
        r.scanned = Some(scan_paths(&dirs));
    }
    r.scanned.get_or_insert_with(Vec::new)
}

pub fn scan() -> Vec<ClapDescriptor> {
    let mut r = registry().lock().unwrap_or_else(PoisonError::into_inner);
    ensure_scanned(&mut r).clone()
}

pub fn rescan() -> Vec<ClapDescriptor> {
    let mut r = registry().lock().unwrap_or_else(PoisonError::into_inner);
    r.scanned = None;
    r.failed.clear();
    soundcraft_plugin_scan::forget("clap");
    ensure_scanned(&mut r).clone()
}

pub fn add_search_dir(dir: &Path) -> usize {
    let mut r = registry().lock().unwrap_or_else(PoisonError::into_inner);
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
        let mut r = registry().lock().unwrap_or_else(PoisonError::into_inner);
        if r.failed.contains(id) {
            return Err(ClapError::Instantiate(plugin_id.to_string(), "failed before".into()));
        }
        ensure_scanned(&mut r).iter().find(|d| d.id == id).cloned().ok_or_else(|| ClapError::NotFound(id.to_string()))?
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
