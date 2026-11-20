//! MIDI driver timestamps have a per-connection epoch. Map them onto the
//! feed's monotonic (audio host) clock before comparing different callbacks.
#[cfg(any(test, target_os = "windows", target_os = "macos", target_os = "linux"))]
#[derive(Default)]
pub(crate) struct DriverTime {
    last: Option<(u64, i64)>,
    offset: i64,
}

#[cfg(any(test, target_os = "windows", target_os = "macos", target_os = "linux"))]
impl DriverTime {
    pub(crate) fn map(&mut self, stamp: u64, arrival: i64) -> i64 {
        let t = i64::try_from(stamp).unwrap_or(i64::MAX);
        let offset = arrival.saturating_sub(t);
        if self.last.is_none_or(|(last, _)| stamp < last) {
            self.offset = offset;
        } else {
            // The least delayed delivery is the best observed epoch estimate.
            self.offset = self.offset.min(offset);
        }
        let when = t.saturating_add(self.offset).clamp(self.last.map_or(0, |(_, t)| t).min(arrival), arrival);
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
}

impl<const N: usize> Default for PulseFit<N> {
    fn default() -> Self {
        Self { points: [(0, 0); N], cursor: 0, len: 0, last: None, slope: 0., intercept: 0., jitter: 0. }
    }
}

impl<const N: usize> PulseFit<N> {
    pub fn clear(&mut self) {
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
            assert!(max_error < 2000, "driver {ppm} ppm: max timestamp error {max_error} us");
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
