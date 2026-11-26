//! A second-order delay-locked loop over incoming F8 times.
//!
//! The loop keeps a predicted time for the next pulse and a period estimate, and
//! corrects both from each arrival's error. One bandwidth sets the trade-off between
//! rejecting timestamp jitter and following a tempo change. After a sustained
//! excursion beyond the measured noise, a wider bandwidth tracks the change, then the
//! narrow bandwidth returns.
//!
//! Method: F. Adriaensen, "Using a DLL to filter time" (2005), sections 2–3, with the
//! critically damped second-order loop gains given there. Background: F. M. Gardner,
//! Phaselock Techniques (1966); R. E. Best, Phase-Locked Loops (1984).

/// Narrow loop bandwidth in Hz: rejects wire jitter during steady tempo.
const NARROW_HZ: f64 = 1.0;
/// Wide loop bandwidth in Hz while a tempo change is being tracked.
const WIDE_HZ: f64 = 3.0;
/// Pulses spent at the wide bandwidth after an excursion is detected.
const WIDE_PULSES: u16 = 24;
/// Pulses after (re)initialisation before predictions are offered.
const READY_PULSES: u32 = 24;
/// Consecutive arrivals beyond the noise band that count as an excursion.
const EXCURSION_RUN: u8 = 2;

#[derive(Default, Clone, Copy, Debug)]
pub(crate) struct PulseDll {
    last: Option<i64>,
    /// Filtered time of the latest pulse (µs).
    current: f64,
    /// Predicted time of the next pulse (µs).
    next: f64,
    period: f64,
    /// Running mean square of the loop error at the narrow bandwidth (µs²).
    noise: f64,
    pulses: u32,
    wide: u16,
    outside: u8,
}

fn gains(hz: f64, period_us: f64) -> (f64, f64) {
    let w = 2. * std::f64::consts::PI * hz * period_us / 1e6;
    (std::f64::consts::SQRT_2 * w, w * w)
}

impl PulseDll {
    pub(crate) fn clear(&mut self) {
        *self = Self::default();
    }

    /// Feed one pulse. `seed_period` (µs) starts the loop after a gap or a clear.
    pub(crate) fn observe(&mut self, ordinal: i64, at: i64, seed_period: Option<f64>) {
        let at = at as f64;
        if self.last == Some(ordinal) {
            // Already taken in by a restart at this pulse.
            return;
        }
        let continuing = self.last.is_some_and(|prev| ordinal == prev + 1) && self.period > 0.;
        if !continuing {
            let Some(period) = seed_period.filter(|p| (2500. ..=500_000.).contains(p)) else {
                self.clear();
                return;
            };
            *self = Self { last: Some(ordinal), current: at, next: at + period, period, noise: 0., pulses: 1, wide: WIDE_PULSES, outside: 0 };
            return;
        }
        self.update(ordinal, at, seed_period);
    }

    fn restart(&mut self, ordinal: i64, at: f64, period: f64) {
        // Keep the learned noise: the wire did not change, the tempo did.
        let noise = self.noise;
        *self = Self { last: Some(ordinal), current: at, next: at + period, period, noise, pulses: 1, wide: WIDE_PULSES, outside: 0 };
    }

    fn update(&mut self, ordinal: i64, at: f64, seed_period: Option<f64>) {
        let e = at - self.next;
        let band = 16. * self.noise.max(150. * 150.);
        self.outside = if self.pulses >= READY_PULSES && e * e > band { self.outside.saturating_add(1) } else { 0 };
        if self.outside >= EXCURSION_RUN {
            // A large rate change restarts the loop from the recent pulses; tracking it
            // through the loop gain alone would overshoot. A small one widens the loop.
            if let Some(period) = seed_period.filter(|p| (2500. ..=500_000.).contains(p) && (p - self.period).abs() > 0.02 * self.period) {
                self.restart(ordinal, at, period);
                return;
            }
            self.wide = WIDE_PULSES;
        }
        let (b, c) = gains(if self.wide > 0 { WIDE_HZ } else { NARROW_HZ }, self.period);
        if self.outside == 0 {
            // Learn the wire's noise at either bandwidth; an excursion is not noise.
            self.noise += 0.05 * (e * e - self.noise);
        }
        self.current = self.next + b * e;
        self.period = (self.period + c * e).clamp(2500., 500_000.);
        self.next = self.current + self.period;
        self.wide = self.wide.saturating_sub(1);
        self.pulses = self.pulses.saturating_add(1);
        self.last = Some(ordinal);
    }

    /// Restart from the recent pulses after a confirmed tempo step.
    pub(crate) fn step(&mut self, ordinal: i64, at: i64, period: Option<f64>) {
        match period.filter(|p| (2500. ..=500_000.).contains(p)) {
            Some(period) if self.last.is_some() => self.restart(ordinal, at as f64, period),
            _ => self.wide = WIDE_PULSES,
        }
    }

    /// Filtered latest pulse time and period, once settled at the narrow bandwidth. A pulse
    /// outside the noise band may be the first of a slowdown: hold until the next one shows
    /// whether it was jitter, since an attack already handed to the device cannot be recalled.
    pub(crate) fn prediction(&self) -> Option<(i64, f64)> {
        (self.last.is_some() && self.pulses >= READY_PULSES && self.wide == 0 && self.outside == 0)
            .then(|| (self.current.round() as i64, self.period))
    }
}
