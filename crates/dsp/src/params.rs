//! Parameter storage and const constructors for static parameter tables.

use crate::{ParamInfo, Taper, Unit};

/// A continuous parameter.
pub(crate) const fn p(id: &'static str, name: &'static str, min: f32, max: f32, default: f32, unit: Unit, taper: Taper) -> ParamInfo {
    ParamInfo { id, name, min, max, default, unit, taper, choices: &[], automatable: true }
}

/// A linear-taper parameter.
pub(crate) const fn lin(id: &'static str, name: &'static str, min: f32, max: f32, default: f32, unit: Unit) -> ParamInfo {
    p(id, name, min, max, default, unit, Taper::Linear)
}

/// A log-taper parameter (frequencies, times, ratios).
pub(crate) const fn log(id: &'static str, name: &'static str, min: f32, max: f32, default: f32, unit: Unit) -> ParamInfo {
    p(id, name, min, max, default, unit, Taper::Log)
}

/// An on/off switch.
pub(crate) const fn toggle(id: &'static str, name: &'static str, default_on: bool) -> ParamInfo {
    ParamInfo {
        id,
        name,
        min: 0.0,
        max: 1.0,
        default: if default_on { 1.0 } else { 0.0 },
        unit: Unit::Toggle,
        taper: Taper::Linear,
        choices: &[],
        automatable: true,
    }
}

/// A list choice; the value is the index.
pub(crate) const fn choice(id: &'static str, name: &'static str, choices: &'static [&'static str], default: usize) -> ParamInfo {
    let max = if choices.is_empty() { 0.0 } else { (choices.len() - 1) as f32 };
    ParamInfo { id, name, min: 0.0, max, default: default as f32, unit: Unit::Choice, taper: Taper::Linear, choices, automatable: true }
}

/// Current values for a plugin's parameter table (allocated once at construction).
#[derive(Debug, Clone)]
pub(crate) struct Params {
    info: &'static [ParamInfo],
    values: Vec<f32>,
}

impl Params {
    pub(crate) fn new(info: &'static [ParamInfo]) -> Self {
        Self { info, values: info.iter().map(|p| p.default).collect() }
    }

    /// Clamps and stores; returns the parameter index, or `None` for an unknown id.
    pub(crate) fn set(&mut self, id: &str, value: f32) -> Option<usize> {
        let idx = self.info.iter().position(|p| p.id == id)?;
        let info = self.info.get(idx)?;
        let slot = self.values.get_mut(idx)?;
        *slot = info.clamp(value);
        Some(idx)
    }

    pub(crate) fn get(&self, id: &str) -> Option<f32> {
        let idx = self.info.iter().position(|p| p.id == id)?;
        self.values.get(idx).copied()
    }

    /// Value by index (0.0 if out of range, which only a table/index mismatch could cause).
    #[inline]
    pub(crate) fn v(&self, idx: usize) -> f32 {
        self.values.get(idx).copied().unwrap_or(0.0)
    }

    #[inline]
    pub(crate) fn on(&self, idx: usize) -> bool {
        self.v(idx) >= 0.5
    }

    #[inline]
    pub(crate) fn choice(&self, idx: usize) -> usize {
        let v = self.v(idx);
        if v.is_finite() && v > 0.0 { v.round() as usize } else { 0 }
    }
}

/// Implements `info`, `set_param` and `param` for a plugin struct with a `p: Params` field and an
/// `update(&mut self)` method that pushes new values to smoothers/coefficients.
macro_rules! param_plumbing {
    ($info:expr) => {
        fn info(&self) -> &'static $crate::PluginInfo {
            &$info
        }
        fn set_param(&mut self, id: &str, value: f32) -> bool {
            if self.p.set(id, value).is_some() {
                self.update();
                true
            } else {
                false
            }
        }
        fn param(&self, id: &str) -> Option<f32> {
            self.p.get(id)
        }
    };
}
pub(crate) use param_plumbing;
