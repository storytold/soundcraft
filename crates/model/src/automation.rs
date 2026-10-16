//! Automation lanes and modes.

use soundcraft_time::Samples;

/// Automation modes (per track).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, serde::Serialize, serde::Deserialize)]
pub enum AutomationMode {
    Off,
    #[default]
    Read,
    Touch,
    Latch,
    TouchLatch,
    Write,
    Trim,
}

impl AutomationMode {
    pub const ALL: [AutomationMode; 7] = [
        AutomationMode::Off,
        AutomationMode::Read,
        AutomationMode::Touch,
        AutomationMode::Latch,
        AutomationMode::TouchLatch,
        AutomationMode::Write,
        AutomationMode::Trim,
    ];
    pub fn label(self) -> &'static str {
        match self {
            AutomationMode::Off => "off",
            AutomationMode::Read => "read",
            AutomationMode::Touch => "touch",
            AutomationMode::Latch => "latch",
            AutomationMode::TouchLatch => "touch/latch",
            AutomationMode::Write => "write",
            AutomationMode::Trim => "trim",
        }
    }
    pub fn from_id(s: &str) -> Option<AutomationMode> {
        let s = s.to_ascii_lowercase();
        AutomationMode::ALL.into_iter().find(|m| m.label() == s || m.label().replace('/', "_") == s || format!("{m:?}").to_ascii_lowercase() == s)
    }
    pub fn reads(self) -> bool {
        !matches!(self, AutomationMode::Off)
    }
}

/// Which parameter an automation lane controls.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
pub enum AutoParam {
    /// Fader level, dB.
    Volume,
    /// Pan, -1..1 (index = which panner of a stereo track: 0 left, 1 right).
    Pan(u8),
    /// Mute (0/1).
    Mute,
    /// Send level, dB.
    SendLevel(u8),
    SendPan(u8),
    SendMute(u8),
    /// Insert slot + parameter id.
    Plugin {
        slot: u8,
        param: String,
    },
}

impl AutoParam {
    pub fn label(&self) -> String {
        match self {
            AutoParam::Volume => "volume".into(),
            AutoParam::Pan(0) => "pan".into(),
            AutoParam::Pan(i) => format!("pan {}", i + 1),
            AutoParam::Mute => "mute".into(),
            AutoParam::SendLevel(i) => format!("send {} level", slot_letter(*i)),
            AutoParam::SendPan(i) => format!("send {} pan", slot_letter(*i)),
            AutoParam::SendMute(i) => format!("send {} mute", slot_letter(*i)),
            AutoParam::Plugin { slot, param } => format!("insert {} {param}", slot_letter(*slot)),
        }
    }
    /// Parse ids like `volume`, `pan`, `pan2`, `mute`, `send_a_level`, `plugin:a:threshold`.
    pub fn parse(s: &str) -> Option<AutoParam> {
        let s = s.trim().to_ascii_lowercase();
        match s.as_str() {
            "volume" | "vol" => return Some(AutoParam::Volume),
            "pan" | "pan1" => return Some(AutoParam::Pan(0)),
            "pan2" => return Some(AutoParam::Pan(1)),
            "mute" => return Some(AutoParam::Mute),
            _ => {}
        }
        if let Some(rest) = s.strip_prefix("send_") {
            let mut it = rest.splitn(2, '_');
            let slot = letter_slot(it.next()?)?;
            return match it.next()? {
                "level" => Some(AutoParam::SendLevel(slot)),
                "pan" => Some(AutoParam::SendPan(slot)),
                "mute" => Some(AutoParam::SendMute(slot)),
                _ => None,
            };
        }
        if let Some(rest) = s.strip_prefix("plugin:") {
            let (slot, param) = rest.split_once(':')?;
            return Some(AutoParam::Plugin { slot: letter_slot(slot)?, param: param.to_string() });
        }
        None
    }
    /// Value range and default.
    pub fn range(&self) -> (f32, f32, f32) {
        match self {
            AutoParam::Volume | AutoParam::SendLevel(_) => (-144.0, 12.0, 0.0),
            AutoParam::Pan(_) | AutoParam::SendPan(_) => (-1.0, 1.0, 0.0),
            AutoParam::Mute | AutoParam::SendMute(_) => (0.0, 1.0, 0.0),
            AutoParam::Plugin { .. } => (f32::MIN, f32::MAX, 0.0),
        }
    }
}

pub fn slot_letter(i: u8) -> char {
    char::from(b'A'.saturating_add(i.min(25)))
}

fn letter_slot(s: &str) -> Option<u8> {
    let c = s.chars().next()?.to_ascii_uppercase();
    if s.len() == 1 && c.is_ascii_uppercase() { Some(c as u8 - b'A') } else { s.parse().ok() }
}

#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct AutomationPoint {
    pub at: Samples,
    pub value: f32,
}

/// A breakpoint automation lane. Points are kept sorted by time.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct AutomationLane {
    pub param: AutoParam,
    pub points: Vec<AutomationPoint>,
    #[serde(default)]
    pub enabled: bool,
}

impl AutomationLane {
    pub fn new(param: AutoParam) -> Self {
        AutomationLane { param, points: Vec::new(), enabled: true }
    }

    /// Value at `at` (linear interpolation; `default` when empty).
    pub fn value_at(&self, at: Samples, default: f32) -> f32 {
        let pts = &self.points;
        let Some(first) = pts.first() else { return default };
        if at <= first.at {
            return first.value;
        }
        let idx = pts.partition_point(|p| p.at <= at);
        match (pts.get(idx.wrapping_sub(1)), pts.get(idx)) {
            (Some(a), Some(b)) if b.at > a.at => {
                // Mute-like lanes step instead of ramping.
                if matches!(self.param, AutoParam::Mute | AutoParam::SendMute(_)) {
                    return a.value;
                }
                a.value + (b.value - a.value) * ((at - a.at) as f32 / (b.at - a.at) as f32)
            }
            (Some(a), _) => a.value,
            _ => default,
        }
    }

    /// Insert or replace a breakpoint.
    pub fn set_point(&mut self, at: Samples, value: f32) {
        let (lo, hi, _) = self.param.range();
        let value = if value.is_nan() { 0.0 } else { value.clamp(lo, hi) };
        match self.points.binary_search_by(|p| p.at.cmp(&at)) {
            Ok(i) => {
                if let Some(p) = self.points.get_mut(i) {
                    p.value = value;
                }
            }
            Err(i) => self.points.insert(i, AutomationPoint { at, value }),
        }
    }

    /// Remove points in `[start, end)`.
    pub fn clear_range(&mut self, start: Samples, end: Samples) -> usize {
        let before = self.points.len();
        self.points.retain(|p| p.at < start || p.at >= end);
        before - self.points.len()
    }

    /// Write a constant value over a range, keeping the surrounding curve intact with anchor points.
    pub fn write_range(&mut self, start: Samples, end: Samples, value: f32, default: f32) {
        if end <= start {
            return;
        }
        let before = self.value_at(start.saturating_sub(1), default);
        let after = self.value_at(end, default);
        self.clear_range(start, end.saturating_add(1));
        if start > 0 {
            self.set_point(start - 1, before);
        }
        self.set_point(start, value);
        self.set_point(end, value);
        self.set_point(end.saturating_add(1), after);
    }

    /// Thin: remove points whose removal changes the curve by less than `tolerance`.
    pub fn thin(&mut self, tolerance: f32) -> usize {
        // Linear thinning would move the transitions of step lanes.
        if matches!(self.param, AutoParam::Mute | AutoParam::SendMute(_)) || self.points.len() < 3 || tolerance.is_nan() {
            return 0;
        }
        let tolerance = f64::from(tolerance);
        let mut keep = vec![true; self.points.len()];
        let mut last = 0usize;
        // Slopes from the last kept point that keep every point removed since then within `tolerance`.
        let (mut lo, mut hi) = (f64::NEG_INFINITY, f64::INFINITY);
        for i in 1..self.points.len() - 1 {
            let (Some(a), Some(b), Some(c)) = (self.points.get(last), self.points.get(i), self.points.get(i + 1)) else { continue };
            let dt = b.at.saturating_sub(a.at) as f64;
            let rise = f64::from(b.value) - f64::from(a.value);
            lo = lo.max((rise - tolerance) / dt);
            hi = hi.min((rise + tolerance) / dt);
            let slope = (f64::from(c.value) - f64::from(a.value)) / c.at.saturating_sub(a.at) as f64;
            if (lo..=hi).contains(&slope) {
                if let Some(k) = keep.get_mut(i) {
                    *k = false;
                }
            } else {
                last = i;
                (lo, hi) = (f64::NEG_INFINITY, f64::INFINITY);
            }
        }
        let before = self.points.len();
        let mut it = keep.iter();
        self.points.retain(|_| it.next().copied().unwrap_or(true));
        before - self.points.len()
    }

    /// Shift points at or after `from` by `delta` (Shuffle edits, Insert Time).
    pub fn shift_from(&mut self, from: Samples, delta: Samples) {
        for p in &mut self.points {
            if p.at >= from {
                p.at = p.at.saturating_add(delta).max(0);
            }
        }
        self.points.sort_by_key(|p| p.at);
        self.points.dedup_by_key(|p| p.at);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interpolation_and_steps() {
        let mut l = AutomationLane::new(AutoParam::Volume);
        assert_eq!(l.value_at(10, -3.0), -3.0);
        l.set_point(0, 0.0);
        l.set_point(100, -10.0);
        assert!((l.value_at(50, 0.0) + 5.0).abs() < 1e-6);
        assert_eq!(l.value_at(1000, 0.0), -10.0);
        let mut m = AutomationLane::new(AutoParam::Mute);
        m.set_point(0, 0.0);
        m.set_point(100, 1.0);
        assert_eq!(m.value_at(99, 0.0), 0.0);
        assert_eq!(m.value_at(100, 0.0), 1.0);
    }

    #[test]
    fn write_range_preserves_outside() {
        let mut l = AutomationLane::new(AutoParam::Volume);
        l.set_point(0, 0.0);
        l.set_point(1000, 0.0);
        l.write_range(200, 400, -20.0, 0.0);
        assert_eq!(l.value_at(100, 0.0), 0.0);
        assert_eq!(l.value_at(300, 0.0), -20.0);
        assert_eq!(l.value_at(600, 0.0), 0.0);
    }

    #[test]
    fn thin_removes_collinear() {
        let mut l = AutomationLane::new(AutoParam::Volume);
        for i in 0..=10 {
            l.set_point(i * 10, -(i as f32));
        }
        assert_eq!(l.thin(0.01), 9);
        assert_eq!(l.points.len(), 2);
    }

    #[test]
    fn thin_keeps_removed_points_within_tolerance() {
        for direction in [1.0, -1.0] {
            let mut lane = AutomationLane::new(AutoParam::Volume);
            for (at, value) in [(0, 0.0), (100, 0.0), (200, 0.0), (300, 0.12), (400, 0.26)] {
                lane.set_point(at, value * direction);
            }
            let original = lane.points.clone();
            let tolerance = 0.1;
            assert!(lane.thin(tolerance) > 0);
            for point in original {
                let error = (lane.value_at(point.at, 0.0) - point.value).abs();
                assert!(error <= tolerance, "at {}: error {error} exceeds {tolerance}", point.at);
            }
        }
    }

    #[test]
    fn thin_keeps_mute_changes_in_place() {
        for param in [AutoParam::Mute, AutoParam::SendMute(0)] {
            let mut lane = AutomationLane::new(param);
            for (at, value) in [(0, 0.0), (9000, 1.0), (10000, 1.0)] {
                lane.set_point(at, value);
            }
            lane.thin(0.1);
            assert_eq!(lane.value_at(8999, 0.0), 0.0);
            assert_eq!(lane.value_at(9000, 0.0), 1.0);
        }
    }

    #[test]
    fn param_ids() {
        assert_eq!(AutoParam::parse("send_b_level"), Some(AutoParam::SendLevel(1)));
        assert_eq!(AutoParam::parse("plugin:a:threshold"), Some(AutoParam::Plugin { slot: 0, param: "threshold".into() }));
        assert_eq!(AutoParam::parse("nope"), None);
        assert_eq!(AutomationMode::from_id("touch/latch"), Some(AutomationMode::TouchLatch));
    }

    #[test]
    fn set_point_clamps_nan() {
        let mut l = AutomationLane::new(AutoParam::Pan(0));
        l.set_point(0, f32::NAN);
        l.set_point(1, 5.0);
        assert_eq!(l.points[0].value, 0.0);
        assert_eq!(l.points[1].value, 1.0);
    }
}
