//! The process-wide Audio Unit registry: scan cache, leaked `PluginInfo`s, instantiation.

use crate::ffi::{self, Instance, PARAM_FLAG_DISPLAY_LOG, PARAM_FLAG_METER_READ_ONLY, PARAM_FLAG_WRITABLE, RawParam};
use crate::plugin::AuPlugin;
use crate::{AuDescriptor, AuError, TYPE_EFFECT, TYPE_INSTRUMENT, TYPE_MUSIC_EFFECT, parse_id};
use soundcraft_dsp::{ParamInfo, Plugin, PluginInfo, Taper, Unit};
use std::collections::{HashMap, HashSet};
use std::sync::{Mutex, OnceLock, PoisonError};

/// Most labels for an indexed parameter shown as a choice (more become a plain range).
const MAX_CHOICES: f32 = 128.0;

// AudioUnitParameterUnit values we map.
const U_BOOLEAN: u32 = 2;
const U_PERCENT: u32 = 3;
const U_SECONDS: u32 = 4;
const U_HERTZ: u32 = 8;
const U_CENTS: u32 = 9;
const U_SEMITONES: u32 = 10;
const U_DECIBELS: u32 = 13;
const U_METERS: u32 = 19;
const U_ABSOLUTE_CENTS: u32 = 20;
const U_MILLISECONDS: u32 = 24;
const U_RATIO: u32 = 25;

#[derive(Default)]
struct Registry {
    scanned: Option<Vec<AuDescriptor>>,
    /// One leaked `PluginInfo` per plugin type that was actually instantiated.
    infos: HashMap<String, &'static PluginInfo>,
    /// Ids that failed to instantiate (not retried every UI frame).
    failed: HashSet<String>,
}

fn registry() -> &'static Mutex<Registry> {
    static R: OnceLock<Mutex<Registry>> = OnceLock::new();
    R.get_or_init(|| Mutex::new(Registry::default()))
}

fn version_string(v: u32) -> String {
    format!("{}.{}.{}", v >> 16, (v >> 8) & 0xff, v & 0xff)
}

fn list() -> Vec<AuDescriptor> {
    let mut out: Vec<AuDescriptor> = Vec::new();
    let override_value = std::env::var_os(crate::discovery::MANUFACTURERS_ENV);
    let policy = crate::discovery::DiscoveryPolicy::from_override(override_value.as_deref());
    policy.for_each_query(|ty, manufacturer| {
        for c in ffi::components(ty, manufacturer) {
            let id = crate::format_id(c.ty, c.subtype, c.manufacturer);
            if out.iter().any(|o| o.id == id) {
                continue;
            }
            let (vendor, name) = crate::split_component_name(&c.name);
            let name = if name.is_empty() { "Audio Unit".to_string() } else { name };
            out.push(AuDescriptor {
                category: crate::category_for(c.ty, c.subtype, &name),
                is_instrument: c.ty == TYPE_INSTRUMENT,
                component_type: crate::fourcc_string(c.ty),
                subtype: crate::fourcc_string(c.subtype),
                manufacturer: crate::fourcc_string(c.manufacturer),
                version: version_string(c.version),
                id,
                name,
                vendor,
            });
        }
    });
    out.sort_by_key(|d| d.name.to_lowercase());
    out
}

fn ensure_scanned(r: &mut Registry) -> &Vec<AuDescriptor> {
    if r.scanned.is_none() {
        r.scanned = Some(list());
    }
    r.scanned.get_or_insert_with(Vec::new)
}

pub fn scan() -> Vec<AuDescriptor> {
    let mut r = registry().lock().unwrap_or_else(PoisonError::into_inner);
    ensure_scanned(&mut r).clone()
}

pub fn rescan() -> Vec<AuDescriptor> {
    let mut r = registry().lock().unwrap_or_else(PoisonError::into_inner);
    r.scanned = None;
    r.failed.clear();
    ensure_scanned(&mut r).clone()
}

fn leak(s: String) -> &'static str {
    Box::leak(s.into_boxed_str())
}

/// Maps an Audio Unit parameter unit (and a custom unit label) to a SoundCraft display unit.
pub(crate) fn unit_for(unit: u32, custom: &str) -> Unit {
    match unit {
        U_DECIBELS => Unit::Db,
        U_HERTZ => Unit::Hz,
        U_MILLISECONDS => Unit::Ms,
        U_PERCENT => Unit::Percent,
        U_SECONDS => Unit::Seconds,
        U_SEMITONES => Unit::Semitones,
        U_CENTS | U_ABSOLUTE_CENTS => Unit::Cents,
        U_RATIO => Unit::Ratio,
        ffi::UNIT_CUSTOM => match custom.trim().to_ascii_lowercase().as_str() {
            "db" => Unit::Db,
            "hz" => Unit::Hz,
            "ms" => Unit::Ms,
            "%" => Unit::Percent,
            "s" | "sec" | "secs" => Unit::Seconds,
            _ => Unit::None,
        },
        _ => Unit::None,
    }
}

/// The SoundCraft description of one parameter (`None` for read-only/meter/broken ones).
pub(crate) fn param_info(p: &RawParam) -> Option<ParamInfo> {
    if p.flags & PARAM_FLAG_WRITABLE == 0 || p.flags & PARAM_FLAG_METER_READ_ONLY != 0 || p.unit == U_METERS {
        return None;
    }
    if !(p.min.is_finite() && p.max.is_finite() && p.max > p.min) {
        return None;
    }
    let name = if p.name.trim().is_empty() { format!("Param {}", p.id) } else { p.name.trim().to_string() };
    let default = if p.default.is_finite() { p.default.clamp(p.min, p.max) } else { p.min };
    let base = ParamInfo {
        id: leak(p.id.to_string()),
        name: leak(name),
        min: p.min,
        max: p.max,
        default,
        unit: Unit::None,
        taper: Taper::Linear,
        choices: &[],
    };
    if p.unit == U_BOOLEAN && p.min == 0.0 && p.max == 1.0 {
        return Some(ParamInfo { unit: Unit::Toggle, default: default.round(), ..base });
    }
    if p.unit == ffi::UNIT_INDEXED && p.min == 0.0 && p.max.fract() == 0.0 && p.max <= MAX_CHOICES {
        let n = p.max as usize;
        let labels: Vec<&'static str> =
            (0..=n).map(|i| leak(p.value_strings.get(i).filter(|s| !s.is_empty()).cloned().unwrap_or_else(|| i.to_string()))).collect();
        return Some(ParamInfo { unit: Unit::Choice, default: default.round(), choices: Box::leak(labels.into_boxed_slice()), ..base });
    }
    let unit = unit_for(p.unit, &p.custom_unit);
    let log = p.min > 0.0 && (p.flags & PARAM_FLAG_DISPLAY_LOG != 0 || (unit == Unit::Hz && p.max / p.min >= 10.0));
    Some(ParamInfo { unit, taper: if log { Taper::Log } else { Taper::Linear }, ..base })
}

/// Builds and leaks the `PluginInfo` for one plugin type. Called at most once per id (cached).
fn build_info(d: &AuDescriptor, inst: &Instance) -> &'static PluginInfo {
    let mut seen = HashSet::new();
    let params: Vec<ParamInfo> = inst.params().iter().filter(|p| seen.insert(p.id)).filter_map(param_info).collect();
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

/// Finds the descriptor and creates an instance plus its (cached) info.
fn instantiate_raw(id: &str) -> Result<(Instance, &'static PluginInfo, u32), AuError> {
    let (ty, sub, manu) = parse_id(id).ok_or_else(|| AuError::NotFound(id.to_string()))?;
    let desc = {
        let mut r = registry().lock().unwrap_or_else(PoisonError::into_inner);
        if r.failed.contains(id) {
            return Err(AuError::Instantiate(id.to_string(), "failed before".into()));
        }
        ensure_scanned(&mut r).iter().find(|d| d.id == id).cloned().ok_or_else(|| AuError::NotFound(id.to_string()))?
    };
    let has_input = ty == TYPE_EFFECT || ty == TYPE_MUSIC_EFFECT;
    let made = Instance::new(ty, sub, manu, has_input);
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
            Ok((inst, info, ty))
        }
        Err(e) => {
            r.failed.insert(id.to_string());
            Err(AuError::Instantiate(id.to_string(), e))
        }
    }
}

pub fn instantiate(id: &str) -> Result<Box<dyn Plugin>, AuError> {
    Ok(Box::new(instantiate_plugin(id)?))
}

pub fn instantiate_plugin(id: &str) -> Result<AuPlugin, AuError> {
    let (inst, info, ty) = instantiate_raw(id)?;
    let has_input = ty == TYPE_EFFECT || ty == TYPE_MUSIC_EFFECT;
    let takes_midi = ty == TYPE_INSTRUMENT || ty == TYPE_MUSIC_EFFECT;
    let mut p = AuPlugin::new(inst, info, has_input, takes_midi);
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
    // Instantiate once to learn the parameters.
    instantiate_raw(id).ok().map(|(_, info, _)| info)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw(unit: u32, min: f32, max: f32, default: f32, flags: u32) -> RawParam {
        RawParam {
            id: 7,
            name: "Cutoff".into(),
            unit,
            custom_unit: String::new(),
            min,
            max,
            default,
            flags: flags | PARAM_FLAG_WRITABLE | ffi::PARAM_FLAG_READABLE,
            value_strings: Vec::new(),
        }
    }

    #[test]
    fn units_map() {
        assert_eq!(unit_for(U_DECIBELS, ""), Unit::Db);
        assert_eq!(unit_for(U_HERTZ, ""), Unit::Hz);
        assert_eq!(unit_for(U_MILLISECONDS, ""), Unit::Ms);
        assert_eq!(unit_for(U_CENTS, ""), Unit::Cents);
        assert_eq!(unit_for(ffi::UNIT_CUSTOM, " dB "), Unit::Db);
        assert_eq!(unit_for(ffi::UNIT_CUSTOM, "furlongs"), Unit::None);
        assert_eq!(unit_for(999, ""), Unit::None);
    }

    #[test]
    fn params_map_and_hostile_ones_are_dropped() {
        let p = param_info(&raw(U_HERTZ, 10.0, 22_050.0, 6900.0, 0)).unwrap();
        assert_eq!((p.id, p.name, p.unit, p.taper, p.default), ("7", "Cutoff", Unit::Hz, Taper::Log, 6900.0));
        let t = param_info(&raw(U_BOOLEAN, 0.0, 1.0, 1.0, 0)).unwrap();
        assert_eq!(t.unit, Unit::Toggle);
        let mut ix = raw(ffi::UNIT_INDEXED, 0.0, 2.0, 1.0, 0);
        ix.value_strings = vec!["Low".into(), String::new()];
        let c = param_info(&ix).unwrap();
        assert_eq!((c.unit, c.choices), (Unit::Choice, &["Low", "1", "2"][..]));
        // Indexed not starting at 0 stays a plain range.
        assert_eq!(param_info(&raw(ffi::UNIT_INDEXED, 1.0, 4.0, 1.0, 0)).unwrap().unit, Unit::None);
        // Hostile defaults clamp; broken ranges, meters and read-only params are skipped.
        assert_eq!(param_info(&raw(U_DECIBELS, -20.0, 20.0, f32::NAN, 0)).unwrap().default, -20.0);
        assert_eq!(param_info(&raw(U_DECIBELS, -20.0, 20.0, 99.0, 0)).unwrap().default, 20.0);
        assert!(param_info(&raw(U_DECIBELS, 1.0, 1.0, 1.0, 0)).is_none());
        assert!(param_info(&raw(U_DECIBELS, f32::NEG_INFINITY, 0.0, 0.0, 0)).is_none());
        assert!(param_info(&raw(U_DECIBELS, 0.0, f32::NAN, 0.0, 0)).is_none());
        assert!(param_info(&raw(U_METERS, 0.0, 1.0, 0.0, 0)).is_none());
        assert!(param_info(&raw(U_DECIBELS, 0.0, 1.0, 0.0, PARAM_FLAG_METER_READ_ONLY)).is_none());
        let mut ro = raw(U_DECIBELS, 0.0, 1.0, 0.0, 0);
        ro.flags &= !PARAM_FLAG_WRITABLE;
        assert!(param_info(&ro).is_none());
        assert_eq!(version_string(0x0001_0203), "1.2.3");
    }
}
