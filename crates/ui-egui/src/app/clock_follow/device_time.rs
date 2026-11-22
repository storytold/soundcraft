use super::{SampleRate, Snapshot};
use std::time::Instant;

/// Compare the physical playback clock with monotonic time over one continuous pass.
/// The MIDI reader's tempo and the recorded audio samples remain in their original domains.
#[derive(Default)]
pub(super) struct DeviceTime {
    first: Option<(u64, Instant, f64)>,
    next_us: Option<i64>,
    ratio: Option<f64>,
}

impl DeviceTime {
    pub(super) fn project(
        &mut self,
        mut wire: Snapshot,
        anchor: Option<(i64, i64)>,
        sr: SampleRate,
        observation: impl FnOnce() -> Option<(u64, Instant, f64)>,
    ) -> Snapshot {
        let Some((_, origin_us)) = anchor else {
            *self = Self::default();
            return wire;
        };
        if self.next_us.is_none_or(|at| wire.now_us >= at) {
            self.next_us = Some(wire.now_us.saturating_add(1_000_000));
            if let Some((pass, time, sample)) = observation() {
                if let Some((first_pass, start, base)) = self.first
                    && pass == first_pass
                    && time >= start
                {
                    let elapsed = time.duration_since(start).as_secs_f64();
                    if elapsed >= 20. {
                        let ratio = (sample - base) / (elapsed * sr.as_f64());
                        if (ratio - 1.).abs() <= 0.001 {
                            self.ratio = Some(ratio);
                        } else {
                            // An unusable stamp cannot undo the accumulated correction
                            // in the same roll. Start a fresh observation window.
                            self.first = Some((pass, time, sample));
                        }
                    }
                } else {
                    self.first = Some((pass, time, sample));
                    self.ratio = None;
                }
            }
        }
        let Some(ratio) = self.ratio else { return wire };
        let delta = wire.stamp_us.saturating_sub(origin_us);
        wire.stamp_us = wire.stamp_us.saturating_add((delta as f64 * (ratio - 1.)).round() as i64);
        wire.bpm = wire.bpm.map(|v| v / ratio);
        wire.phase_grid_bpm = wire.phase_grid_bpm.map(|v| v / ratio);
        wire.brake_grid_bpm = wire.brake_grid_bpm.map(|v| v / ratio);
        wire
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::clock_follow::{State, TICKS_PER_PULSE, TempoMap, update_grid};
    use std::time::Duration;

    #[test]
    fn missing_observations_do_not_reset_the_grid_midpass_or_leak_into_a_new_start() {
        let mut device = DeviceTime::default();
        let start = Instant::now();
        let rate = SampleRate::HZ_48000;
        let wire = |s: i64| Snapshot { bpm: Some(120.), stamp_us: s * 1_000_000, now_us: s * 1_000_000, ..Snapshot::default() };
        let cue = Some((0, 0));
        device.project(wire(0), cue, rate, || Some((1, start, 0.)));
        let projected = device.project(wire(30), cue, rate, || Some((1, start + Duration::from_secs(30), 1_440_144.)));
        assert_eq!(projected.stamp_us, 30_003_000);
        let missing = device.project(wire(40), cue, rate, || None);
        assert_eq!(missing.stamp_us, 40_004_000);
        let stopped = device.project(wire(41), None, rate, || panic!("stopped transport cannot supply an observation"));
        assert_eq!(stopped.stamp_us, 41_000_000);
        let next = device.project(wire(42), Some((0, 42_000_000)), rate, || Some((2, start + Duration::from_secs(42), 0.)));
        assert_eq!(next.bpm, Some(120.));
    }

    #[test]
    fn next_external_downbeat_uses_physical_samples_over_a_two_hour_pass() {
        let rate = SampleRate::HZ_48000;
        for ppm in [39., -40., 0.] {
            let mut device = DeviceTime::default();
            let mut grid = TempoMap::new(120., 4, 4).unwrap();
            let start = Instant::now();
            let actual_rate = 48_000. * (1. + ppm / 1e6);
            let mut max_ms = 0_f64;
            for seconds in 0..=7200_i64 {
                let pulse = seconds * 48;
                let stamp = seconds * 1_000_000;
                let wire = Snapshot { state: State::Locked, pulse, now_us: stamp, stamp_us: stamp, bpm: Some(120.), ..Snapshot::default() };
                let actual_sample = actual_rate * seconds as f64;
                let measured =
                    device.project(wire, Some((0, 0)), rate, || Some((1, start + Duration::from_secs(seconds as u64), actual_sample.round())));
                update_grid(&mut grid, Some((0, 0)), 0, measured, rate, actual_sample as i64 + 1440);
                let scheduled = grid.tick_to_samples((pulse + 24) * TICKS_PER_PULSE, rate);
                let error = (scheduled as f64 / actual_rate - seconds as f64 - 0.5) * 1000.;
                max_ms = max_ms.max(error.abs());
            }
            eprintln!("physical oscillator {ppm:+} ppm; maximum musical grid error {max_ms:.3} ms");
            assert!(max_ms < 4., "{ppm} ppm, {max_ms:.3} ms");
        }
    }
}
