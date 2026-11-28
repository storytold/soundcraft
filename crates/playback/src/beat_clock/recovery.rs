//! Spread owed Beat Clock pulses evenly: after a late first callback on Start, or when a
//! compensated output loses timing while its transport keeps rolling.
use super::{Cursor, TICKS_PER_CLOCK};
use crate::Session;
use crate::sync_clock::{CATCH_UP_SPACING, Presentation};
use craft_midi_sync::sync::{CLOCK, CLOCKS_PER_QUARTER};
use std::time::{Duration, Instant};

#[derive(Clone, Copy)]
pub(super) struct Repayment {
    first_owed: i64,
    last_pulse: i64,
    start_sample: i64,
    end_sample: i64,
}

impl Repayment {
    pub(super) fn after_start(first: i64, at: i64, session: &Session, prearmed_zero: bool) -> Option<Self> {
        let rate = session.sample_rate;
        let sample = |pulse: i64| session.tempo.tick_to_samples(pulse.saturating_mul(TICKS_PER_CLOCK), rate);
        let start = sample(first);
        let next = sample(first.saturating_add(1));
        let delay = at.saturating_sub(start);
        if delay <= 0 || next <= start || (!prearmed_zero && at >= next) || delay.saturating_mul(4) < next.saturating_sub(start) {
            return None;
        }
        let last_pulse = first.saturating_add(i64::from(CLOCKS_PER_QUARTER));
        Some(Self { first_owed: first, last_pulse, start_sample: at, end_sample: sample(last_pulse) })
    }

    pub(super) fn new(first_owed: i64, at: i64, session: &Session) -> Option<Self> {
        let tempo = &session.tempo;
        let rate = session.sample_rate;
        let sample = |pulse: i64| tempo.tick_to_samples(pulse.saturating_mul(TICKS_PER_CLOCK), rate);
        let estimate = (tempo.samples_to_ticks_f(at, rate) / TICKS_PER_CLOCK as f64).floor() as i64;
        // Match the same rounded sample boundaries used during ordinary playback.
        let first_live = if sample(estimate.saturating_sub(1)) >= at {
            estimate.saturating_sub(1)
        } else {
            estimate.saturating_add(i64::from(sample(estimate) < at))
        };
        if first_owed >= first_live {
            return None;
        }
        // The first live pulse can be almost a pulse ahead of the recovery point.
        // Rejoin on the 24th live pulse so even an off-grid return finishes within a quarter note.
        let last_pulse = first_live.saturating_add(i64::from(CLOCKS_PER_QUARTER) - 1);
        Some(Self { first_owed, last_pulse, start_sample: sample(first_live), end_sample: sample(last_pulse) })
    }

    /// Whether `new` moves the end of this window, which `old` planned.
    pub(super) fn moved(self, old: &Session, new: &Session) -> bool {
        let end = |s: &Session| s.tempo.tick_to_samples(self.last_pulse.saturating_mul(TICKS_PER_CLOCK), s.sample_rate);
        end(old) != end(new)
    }

    pub(super) fn expired(self, at: i64) -> bool {
        at > self.end_sample
    }

    fn sample(self, pulse: i64) -> f64 {
        let fraction = (pulse - self.first_owed) as f64 / (self.last_pulse - self.first_owed) as f64;
        self.start_sample as f64 + (self.end_sample - self.start_sample) as f64 * fraction
    }
}

impl Cursor {
    pub(super) fn repay(&mut self, window: Repayment, b: Presentation, now: Instant, s: &Session, send: &mut dyn FnMut(Instant, &[u8])) -> Instant {
        let target = window.sample(self.next_pulse);
        if target > b.limit {
            return now + Duration::from_millis(1);
        }
        let due = b.instant_for(target);
        // Preserve wire spacing when the worker wakes late. At fast tempos the
        // planned spacing itself can be shorter than the ordinary catch-up floor.
        let period_samples = (window.end_sample - window.start_sample) as f64 / (window.last_pulse - window.first_owed) as f64;
        let spacing = Duration::try_from_secs_f64(period_samples / b.samples_per_second).unwrap_or(CATCH_UP_SPACING).min(CATCH_UP_SPACING);
        let earliest = self.last_sent.map_or(due, |last| due.max(last + spacing));
        if now < earliest {
            return earliest;
        }
        send(due, &[CLOCK]);
        self.last_sent = Some(now);
        self.last_position = Some(b.sample_index_at(now));
        self.catchups += u64::from(self.next_pulse < window.last_pulse);
        self.next_pulse += 1;
        let next = if self.next_pulse <= window.last_pulse {
            window.sample(self.next_pulse)
        } else {
            self.repayment = None;
            s.tempo.tick_to_samples(self.next_pulse.saturating_mul(TICKS_PER_CLOCK), s.sample_rate) as f64
        };
        if next > b.limit { now + Duration::from_millis(1) } else { b.instant_for(next) }
    }
}
