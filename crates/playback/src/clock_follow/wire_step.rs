type Proposal = Option<((i64, i64), u8)>;

#[derive(Clone, Copy, Default)]
struct Point {
    stamp: i64,
    prior_period: f64,
    prior_noise: f64,
}

/// Online least-squares fit with centered sums. One update per input pulse.
#[derive(Default)]
struct Slope {
    count: u32,
    mean_x: f64,
    mean_y: f64,
    xx: f64,
    xy: f64,
    yy: f64,
}

impl Slope {
    fn add(&mut self, x: f64, y: f64) {
        self.count = self.count.saturating_add(1);
        let (dx, dy) = (x - self.mean_x, y - self.mean_y);
        self.mean_x += dx / f64::from(self.count);
        self.mean_y += dy / f64::from(self.count);
        self.xx += dx * (x - self.mean_x);
        self.xy += dx * (y - self.mean_y);
        self.yy += dy * (y - self.mean_y);
    }

    fn period_noise(&self) -> Option<(f64, f64)> {
        let period = self.xy / self.xx;
        (2_500.0..=500_000.0).contains(&period).then(|| (period, ((self.yy - self.xy * period) / f64::from(self.count)).max(0.).sqrt()))
    }
}

struct Candidate {
    start: (i64, i64),
    prior: Point,
    fit: Slope,
}

/// A percent threshold is a comparison with the clock *before* the transition.
/// Near 8%, freeze the old fit while accumulating a new, independent slope.
pub(super) struct WireStep {
    recent: [Point; 32],
    seen: u8,
    next_check: i64,
    candidate: Option<Candidate>,
}

impl Default for WireStep {
    fn default() -> Self {
        Self { recent: [Point::default(); 32], seen: 0, next_check: 0, candidate: None }
    }
}

impl WireStep {
    pub(super) fn observe(&mut self, pulse: i64, stamp: i64, old: f64, noise: f64, proposed: Proposal) -> Proposal {
        let slot = |p: i64| p.rem_euclid(32) as usize;
        self.recent[slot(pulse)] = Point { stamp, prior_period: old, prior_noise: noise };
        self.seen = self.seen.saturating_add(1);
        if self.seen < 32 || pulse < self.next_check {
            return proposed;
        }
        // Take the old estimate before the four-interval candidate and before its onset
        // could have contaminated the long fit. Recent observations still drive phase.
        let prior = self.recent[slot(pulse.saturating_sub(12))];
        let recent = stamp.saturating_sub(self.recent[slot(pulse.saturating_sub(4))].stamp) as f64 / 4.;
        // A just-restarted long fit can have only two noisy points. Verify the
        // pre-transition plateau independently before treating it as a reference.
        let before = self.recent[slot(pulse.saturating_sub(8))].stamp.saturating_sub(self.recent[slot(pulse.saturating_sub(28))].stamp) as f64 / 20.;
        let reliable = (prior.prior_period / before - 1.).abs() < 0.025;
        let fraction = (before / recent - 1.).abs();
        // Near 8%, four intervals at this precision cannot resolve a tenth of a percent.
        // Keep the ordinary short path for clear jumps outside that uncertainty band.
        if let Some(((_, start), intervals)) = proposed {
            let period = stamp.saturating_sub(start) as f64 / f64::from(intervals);
            if (before / period - 1.).abs() > 0.14 {
                self.candidate = None;
                self.next_check = pulse.saturating_add(17);
                return proposed;
            }
        }
        if self.candidate.is_none() && reliable && (0.052..=0.14).contains(&fraction) {
            self.candidate = Some(Candidate { start: (pulse, stamp), prior, fit: Default::default() });
        }
        let Some(c) = self.candidate.as_mut() else { return None };
        let n = pulse.saturating_sub(c.start.0);
        c.fit.add(n as f64, stamp.saturating_sub(c.start.1) as f64);
        let Some((new, noise_after)) = c.fit.period_noise() else {
            if n > 4 {
                self.candidate = None;
            }
            return None;
        };
        if n < 4 || (c.prior.prior_noise > 30. && n < 16) {
            return None;
        }
        let delta = (c.prior.prior_period / new - 1.).abs();
        let n_f = n as f64;
        // Standard error of an equally spaced least-squares slope, with independent
        // errors before and after the change. Endpoint rounding adds one microsecond.
        let error = (12.0_f64.sqrt() * 4. * (c.prior.prior_noise / (192. * 192_f64.sqrt()) + noise_after / (n_f * n_f.sqrt())) + 1. / n_f) / new;
        let resolved = if delta - error >= 0.08 {
            Some(true)
        } else if delta + error < 0.08 {
            Some(false)
        } else if error <= 0.00045 {
            // Within rounding/measurement resolution the 8% boundary is inclusive.
            Some(true)
        } else if n >= 159 {
            Some(delta >= 0.08)
        } else {
            None
        };
        if let Some(step) = resolved {
            let start = c.start;
            self.candidate = None;
            self.next_check = pulse.saturating_add(17);
            return step.then_some((start, n.min(159) as u8));
        }
        None
    }
}
