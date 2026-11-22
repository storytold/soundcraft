//! MIDI driver timestamps have a per-connection epoch. Map them onto the
//! feed's monotonic (audio host) clock before comparing different callbacks.
#[cfg(any(test, target_os = "windows", target_os = "macos", target_os = "linux"))]
#[derive(Default)]
pub(crate) struct DriverTime {
    last: Option<(u64, i64)>,
    bucket: i64,
    minimum: i64,
    previous_min: Option<i64>,
}

#[cfg(any(test, target_os = "windows", target_os = "macos", target_os = "linux"))]
impl DriverTime {
    pub(crate) fn map(&mut self, stamp: u64, arrival: i64) -> i64 {
        let t = i64::try_from(stamp).unwrap_or(i64::MAX);
        let offset = arrival.saturating_sub(t);
        // The lower envelope rejects late deliveries, but cannot retain an epoch
        // forever: the driver's oscillator may run slower than the host's.
        let bucket = arrival.div_euclid(2_000_000);
        if self.last.is_none_or(|(last, _)| stamp < last) || bucket != self.bucket {
            self.previous_min = self.last.filter(|(last, _)| *last <= stamp).map(|(last, _)| {
                if bucket == self.bucket + 1 {
                    return self.minimum;
                }
                // Project only previously observed upward drift across a gap, capped at 200 ppm.
                // With a stable epoch the first delayed batch retains its learned mapping.
                let drift = self.previous_min.map_or(0, |previous| self.minimum.saturating_sub(previous).clamp(0, 400));
                let gap = stamp.saturating_sub(last);
                self.minimum.saturating_add((u128::from(gap) * drift as u128 / 2_000_000).min(i64::MAX as u128) as i64)
            });
            self.bucket = bucket;
            self.minimum = offset;
        } else {
            self.minimum = self.minimum.min(offset);
        }
        let estimate = self.previous_min.map_or(self.minimum, |last| last.min(self.minimum));
        let when = t.saturating_add(estimate).clamp(self.last.map_or(0, |(_, t)| t).min(arrival), arrival);
        self.last = Some((stamp, when));
        when
    }
}

/// Least-squares fit of host time in microseconds against an increasing MIDI pulse index.
/// Fixed capacity; restarting a transport or jumping backwards starts a new fit.
pub struct PulseFit<const N: usize> {
    points: [(i64, i64); N],
    cursor: usize,
    len: usize,
    last: Option<(i64, i64)>,
    slope: f64,
    intercept: f64,
    jitter: f64,
    tempo_ramp: std::sync::OnceLock<bool>,
}

impl<const N: usize> Default for PulseFit<N> {
    fn default() -> Self {
        Self { points: [(0, 0); N], cursor: 0, len: 0, last: None, slope: 0., intercept: 0., jitter: 0., tempo_ramp: Default::default() }
    }
}

impl<const N: usize> PulseFit<N> {
    pub fn clear(&mut self) {
        self.tempo_ramp.take();
        self.len = 0;
        self.cursor = 0;
        self.last = None;
        self.slope = 0.;
        self.jitter = 0.;
    }
    pub fn push(&mut self, pulse: i64, at_us: i64) {
        if N == 0 {
            return;
        }
        if self.last.is_some_and(|(i, t)| pulse <= i || at_us < t) {
            self.clear();
        }
        if N >= 24 {
            self.tempo_ramp.take();
        }
        self.points[self.cursor] = (pulse, at_us);
        self.cursor = (self.cursor + 1) % N;
        self.len = (self.len + 1).min(N);
        self.last = Some((pulse, at_us));
        if self.len < 2 {
            return;
        }
        // Centre relative to the newest stamp; a long host uptime cannot
        // lose short timestamp differences in large sum-of-squares terms.
        let n = self.len as f64;
        let mut x = 0.;
        let mut y = 0.;
        let mut xx = 0.;
        let mut xy = 0.;
        let mut yy = 0.;
        for &(i, t) in &self.points[..self.len] {
            let dx = i.saturating_sub(pulse) as f64;
            let dy = t.saturating_sub(at_us) as f64;
            x += dx;
            y += dy;
            xx += dx * dx;
            xy += dx * dy;
            yy += dy * dy;
        }
        let variance = xx - x * x / n;
        self.slope = if variance > 0. { (xy - x * y / n) / variance } else { 0. };
        self.intercept = (y - self.slope * x) / n;
        self.jitter = ((yy - y * y / n - self.slope * (xy - x * y / n)) / n).max(0.).sqrt();
    }
    /// Oldest point in a train of consistent intervals that depart from a previous
    /// period. A delayed packet and the shorter interval that follows it do not qualify.
    pub fn confirmed_step_start(&self, previous: f64, noise_us: f64) -> Option<(i64, i64)> {
        self.confirmed_step(previous, noise_us).map(|(start, _)| start)
    }

    pub(crate) fn confirmed_step(&self, previous: f64, noise_us: f64) -> Option<((i64, i64), u8)> {
        // Two nearly identical intervals can distinguish a large jump from a
        // delayed packet followed by a short one. Keep the four-interval noise
        // test for ordinary changes and for devices with appreciable jitter.
        self.step_intervals(previous, 2, 0.30, noise_us * 8., 0.03)
            .map(|head| (head, 2))
            .or_else(|| self.step_intervals(previous, 4, 0.08, noise_us * 2., 0.4).map(|head| (head, 4)))
    }

    fn step_intervals(&self, previous: f64, count: usize, bpm_fraction: f64, noise_floor: f64, tolerance: f64) -> Option<(i64, i64)> {
        if self.len <= count {
            return None;
        }
        let first = (self.cursor + N - count - 1) % N;
        let start = self.points[first];
        let (index, end) = self.last?;
        let pulses = index.saturating_sub(start.0) as f64;
        let period = end.saturating_sub(start.1) as f64 / pulses;
        let change = period - previous;
        // BPM is reciprocal period. Allow for the microsecond rounding of the endpoints.
        if !(2500. ..=500_000.).contains(&period) || change.abs() + 1. / pulses < period * bpm_fraction || change.abs() < noise_floor {
            return None;
        }
        // Timestamp jitter moves neighbouring intervals in opposite directions. Its
        // observed amplitude can justify a wider agreement band, while every interval
        // must still depart from the old period in the direction of this change.
        let jitter_margin = if count == 2 { (0.40 * noise_floor).min(change.abs() * 0.25) } else { (1.7 * noise_floor).min(change.abs() * 0.95) };
        let band = (change.abs() * tolerance).max(jitter_margin);
        for i in 0..count {
            let (a, start) = self.points[(first + i) % N];
            let (b, end) = self.points[(first + i + 1) % N];
            let interval = end.saturating_sub(start) as f64 / b.saturating_sub(a) as f64;
            if (interval - period).abs() > band {
                return None;
            }
        }
        Some(start)
    }

    /// Prediction requires several independently observed intervals at the fitted rate.
    pub(crate) fn regular_intervals(&self, period: f64) -> bool {
        const COUNT: usize = 4;
        if self.len <= COUNT {
            return false;
        }
        let start = (self.cursor + N - COUNT - 1) % N;
        (0..COUNT).all(|i| {
            let elapsed = self.points[(start + i + 1) % N].1.saturating_sub(self.points[(start + i) % N].1);
            (elapsed as f64 - period).abs() <= (0.02 * period).max(2.)
        })
    }

    /// Detect sustained tempo motion without changing the stable linear period/phase estimates.
    /// With evenly spaced pulse indices, the centred quadratic is orthogonal to both a constant
    /// and a linear slope. A constant-speed clock therefore contributes no curvature.
    pub(crate) fn is_tempo_ramp(&self) -> bool {
        if self.len < 24 || self.slope <= 0. {
            return false;
        }
        // UI snapshots and audio render lookahead can read the same fit several
        // times between wire pulses. Its curvature changes only on push or clear.
        *self.tempo_ramp.get_or_init(|| self.compute_tempo_ramp())
    }

    fn compute_tempo_ramp(&self) -> bool {
        let Some((last, stamp)) = self.last else { return false };
        let first = (self.cursor + N - self.len) % N;
        if last.saturating_sub(self.points[first].0) != self.len as i64 - 1 {
            return false;
        }
        let n = self.len as f64;
        let center = (n - 1.) / 2.;
        let variance = (n * n - 1.) / 12.;
        let norm = n * (n * n - 1.) * (n * n - 4.) / 180.;
        let projection = self.points[..self.len]
            .iter()
            .map(|&(i, t)| {
                let x = i.saturating_sub(last) as f64 + center;
                (x * x - variance) * t.saturating_sub(stamp) as f64
            })
            .sum::<f64>()
            .abs();
        // Reject stationary timestamp noise (three standard errors) and slow clock flutter.
        // Twice the quadratic coefficient is the change in pulse period per pulse.
        projection > 3. * self.jitter * norm.sqrt() && 2. * projection / norm > 0.00045 * self.slope * self.slope / 1_000_000.
    }

    pub fn period_us(&self) -> Option<f64> {
        (self.len >= 2 && self.slope > 0.).then_some(self.slope)
    }
    pub fn predict_us(&self, pulse: i64) -> Option<i64> {
        let (i, t) = self.last?;
        Some(t.saturating_add((self.intercept + self.period_us()? * (pulse.saturating_sub(i) as f64)).round() as i64))
    }
    pub fn jitter_us(&self) -> f64 {
        self.jitter
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn repeated_ramp_queries_follow_each_new_pulse_and_transport_reset() {
        let mut fit = PulseFit::<192>::default();
        let mut t = 0;
        let mut saw_ramp = false;
        for pulse in 0..1800 {
            t += if (300..800).contains(&pulse) { 15000 + (pulse - 300) * 10 } else { 20000 };
            fit.push(pulse, t + pulse % 3);
            let value = fit.is_tempo_ramp();
            saw_ramp |= value;
            for _ in 0..8 {
                assert_eq!(value, fit.is_tempo_ramp());
            }
            if pulse >= 23 {
                assert_eq!(value, fit.compute_tempo_ramp());
            }
            if pulse > 1100 {
                assert!(!value);
            }
        }
        assert!(saw_ramp);
        fit.push(0, 1);
        assert!(!fit.is_tempo_ramp());
        fit.clear();
        assert!(!fit.is_tempo_ramp());
    }

    #[test]
    fn eight_percent_bpm_steps_in_either_direction_qualify() {
        for (bpm, qualifies) in [(108., true), (92., true), (107., false), (93., false)] {
            let mut fit = PulseFit::<12>::default();
            for pulse in 0..=4 {
                fit.push(pulse, (pulse as f64 * 2_500_000. / bpm).round() as i64);
            }
            assert_eq!(fit.confirmed_step_start(25_000., 0.).is_some(), qualifies, "{bpm}");
        }
    }

    #[test]
    fn driver_time_retains_spacing_in_a_late_batch_and_recovers_after_reset() {
        let mut clock = DriverTime::default();
        assert_eq!(clock.map(20_000, 3_000_000), 3_000_000);
        assert_eq!(clock.map(22_000, 3_010_000), 3_002_000);
        assert_eq!(clock.map(24_000, 3_010_000), 3_004_000);
        assert_eq!(clock.map(0, 3_020_000), 3_020_000);
    }
    #[test]
    fn timestamp_mapping_tracks_both_directions_of_clock_drift_for_an_hour() {
        for ppm in [-100, 100] {
            let mut clock = DriverTime::default();
            let mut max_error = 0;
            for second in 0..=3600_i64 {
                let actual = second * 1_000_000;
                let arrival = actual + if second % 3 == 0 { 0 } else { 1200 };
                let stamp = (actual + second * ppm) as u64;
                let mapped = clock.map(stamp, arrival);
                max_error = max_error.max(mapped.abs_diff(actual));
            }

            assert!(max_error < 500, "driver {ppm} ppm: max timestamp error {max_error} us");
        }
    }
    #[test]
    fn driver_time_keeps_its_epoch_across_a_transport_pause() {
        let mut clock = DriverTime::default();
        for n in 0..1000 {
            clock.map(n * 10_000, (n * 10_000) as i64);
        }
        for n in 1500..1520 {
            let driver = n * 10_000;
            let mapped = clock.map(driver, driver as i64 + 3_000);
            assert!(mapped.abs_diff(driver as i64) < 500, "mapped pause error {} us", mapped - driver as i64);
        }
    }

    #[test]
    fn windowed_fit_handles_host_uptime_jitter_and_discontinuities() {
        let mut fit = PulseFit::<24>::default();
        let start = 9_000_000_000;
        for i in 0..120 {
            fit.push(i, start + i * 20_000 + if i % 2 == 0 { 300 } else { -300 });
        }
        assert!((fit.period_us().unwrap() - 20_000.).abs() < 5.);
        assert!(fit.predict_us(120).unwrap().abs_diff(start + 120 * 20_000) < 75);
        assert!(fit.jitter_us() < 301.);
        fit.push(0, 0);
        assert!(fit.period_us().is_none());
    }
}
