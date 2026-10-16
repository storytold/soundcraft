//! Bounded step patterns made from integer pulse distribution and user-edited cells.
//! No recorded performances or external pattern tables are used.

use crate::{Note, PPQ, Sequence};
use serde::{Deserialize, Serialize};

pub const VOICES: [(&str, u8); 6] = [("Kick", 36), ("Snare", 38), ("Closed hat", 42), ("Clap", 39), ("Tom", 45), ("Ride", 51)];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Division {
    Eighth,
    #[default]
    Sixteenth,
}

impl Division {
    pub fn ticks(self) -> i64 {
        i64::from(PPQ)
            / match self {
                Self::Eighth => 2,
                Self::Sixteenth => 4,
            }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Lane {
    pub enabled: bool,
    pub pulses: u8,
    pub rotate: u8,
    /// Explicit cells replace the pulse distribution when present. Bit zero is step one.
    pub cells: Option<u32>,
}

impl Lane {
    pub fn hit(&self, step: u8, steps: u8) -> bool {
        if !self.enabled || steps == 0 || step >= steps || steps > 32 {
            return false;
        }
        if let Some(cells) = self.cells {
            return cells & (1u32 << step) != 0;
        }
        let steps = u16::from(steps);
        let index = (u16::from(step) + steps - u16::from(self.rotate) % steps) % steps;
        let pulses = u16::from(self.pulses).min(steps);
        (index * pulses) % steps < pulses
    }

    pub fn mask(&self, steps: u8) -> u32 {
        (0..steps.min(32)).filter(|step| self.hit(*step, steps)).fold(0, |mask, step| mask | (1u32 << step))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub steps: u8,
    pub repeats: u8,
    pub division: Division,
    pub swing: u8,
    pub velocity: u8,
    pub velocity_spread: u8,
    pub timing_spread: u8,
    /// Number of final-cycle cells eligible for an alternating snare/tom ending.
    pub ending: u8,
    pub seed: u64,
    pub lanes: [Lane; 6],
}

impl Default for Config {
    fn default() -> Self {
        Self {
            steps: 16,
            repeats: 4,
            division: Division::Sixteenth,
            swing: 0,
            velocity: 96,
            velocity_spread: 0,
            timing_spread: 0,
            ending: 0,
            seed: 1,
            lanes: [(4, 0), (2, 4), (8, 0), (0, 0), (0, 0), (0, 0)].map(|(pulses, rotate)| Lane { enabled: true, pulses, rotate, cells: None }),
        }
    }
}

impl Config {
    pub fn validate(&self) -> Result<(), &'static str> {
        if !(1..=32).contains(&self.steps) || !(1..=32).contains(&self.repeats) {
            return Err("steps and repeats must be 1..32");
        }
        if !(1..=127).contains(&self.velocity) || self.swing > 75 || self.velocity_spread > 40 || self.timing_spread > 20 {
            return Err("velocity must be 1..127, swing 0..75, velocity spread 0..40, timing spread 0..20");
        }
        if self.ending > self.steps {
            return Err("ending cannot exceed the cycle length");
        }
        for lane in &self.lanes {
            if lane.pulses > self.steps || lane.rotate >= self.steps {
                return Err("lane pulses or rotation exceed the cycle length");
            }
            if self.steps < 32 && lane.cells.is_some_and(|mask| mask >> self.steps != 0) {
                return Err("manual cells exceed the cycle length");
            }
        }
        Ok(())
    }

    pub fn length_ticks(&self) -> Result<i64, &'static str> {
        self.validate()?;
        Ok(i64::from(self.steps) * i64::from(self.repeats) * self.division.ticks())
    }
}

// Counter-based mixing: editing one cell does not reshuffle the other cells' variations.
fn random(seed: u64, step: u64, lane: u64) -> u64 {
    let mut x = seed.wrapping_add(step.wrapping_mul(0x9e3779b97f4a7c15)).wrapping_add(lane.wrapping_mul(0xbf58476d1ce4e5b9));
    x = (x ^ (x >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94d049bb133111eb);
    x ^ (x >> 31)
}

pub fn generate(config: &Config) -> Result<Sequence, &'static str> {
    let length = config.length_ticks()?;
    let tick = config.division.ticks();
    let mut out = Sequence::default();
    for cycle in 0..config.repeats {
        for step in 0..config.steps {
            let index = u64::from(cycle) * u64::from(config.steps) + u64::from(step);
            for (voice, (lane, (_, pitch))) in config.lanes.iter().zip(VOICES).enumerate() {
                let ending = cycle + 1 == config.repeats && step >= config.steps - config.ending && voice == if step % 2 == 0 { 1 } else { 4 };
                if !lane.enabled || !(lane.hit(step, config.steps) || ending) {
                    continue;
                }
                let rnd = random(config.seed, index, voice as u64);
                let spread = i64::from(config.timing_spread) * tick / 100;
                let jitter = (rnd % (2 * spread + 1) as u64) as i64 - spread;
                let delay = if step % 2 == 1 { tick * i64::from(config.swing) / 100 } else { 0 };
                let start = (index as i64 * tick + delay + jitter).clamp(0, length - 1);
                let variation = ((rnd >> 32) % (2 * u64::from(config.velocity_spread) + 1)) as i64 - i64::from(config.velocity_spread);
                out.notes.push(Note {
                    pitch,
                    velocity: (i64::from(config.velocity) + variation).clamp(1, 127) as u8,
                    release_velocity: 64,
                    channel: 9,
                    start,
                    length: (tick / 4).min(length - start).max(1),
                });
            }
        }
    }
    out.sort();
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_pulse_count_is_exact_and_rotation_preserves_it() {
        for steps in 1..=32 {
            for pulses in 0..=steps {
                for rotate in 0..steps {
                    let lane = Lane { enabled: true, pulses, rotate, cells: None };
                    assert_eq!(lane.mask(steps).count_ones(), u32::from(pulses));
                }
            }
        }
    }

    #[test]
    fn explicit_cells_override_distribution_and_muting_also_mutes_endings() {
        let mut c = Config { repeats: 1, ending: 16, ..Config::default() };
        for lane in &mut c.lanes {
            lane.enabled = false;
        }
        assert!(generate(&c).unwrap().notes.is_empty());
        c.ending = 0;
        c.lanes[0].enabled = true;
        c.lanes[0].cells = Some(0b1010);
        assert_eq!(generate(&c).unwrap().notes.iter().map(|n| n.start).collect::<Vec<_>>(), vec![240, 720]);
    }

    #[test]
    fn swing_changes_only_odd_cells_at_the_selected_division() {
        for division in [Division::Eighth, Division::Sixteenth] {
            let mut c = Config { division, repeats: 1, ..Config::default() };
            for lane in &mut c.lanes {
                lane.pulses = 0;
            }
            c.lanes[0].pulses = 16;
            let straight = generate(&c).unwrap();
            c.swing = 50;
            let swung = generate(&c).unwrap();
            for (i, (a, b)) in straight.notes.iter().zip(&swung.notes).enumerate() {
                assert_eq!(b.start - a.start, if i % 2 == 1 { division.ticks() / 2 } else { 0 });
            }
        }
    }

    #[test]
    fn variation_is_reproducible_bounded_and_stable_when_another_lane_changes() {
        let mut c = Config { velocity_spread: 40, timing_spread: 20, swing: 75, ending: 8, ..Config::default() };
        let first = generate(&c).unwrap();
        assert_eq!(first, generate(&c).unwrap());
        assert!(first.notes.iter().all(|n| n.start >= 0 && n.end() <= c.length_ticks().unwrap() && (1..=127).contains(&n.velocity)));
        c.seed += 1;
        assert_ne!(first, generate(&c).unwrap());
        c.seed -= 1;
        c.lanes[2].enabled = false;
        let kicks = |s: Sequence| s.notes.into_iter().filter(|n| n.pitch == 36).collect::<Vec<_>>();
        assert_eq!(kicks(first), kicks(generate(&c).unwrap()));
    }

    #[test]
    fn invalid_input_is_rejected_before_generation() {
        for steps in [0, 33, 255] {
            assert!(generate(&Config { steps, ..Config::default() }).is_err());
        }
        assert!(generate(&Config { repeats: 0, ..Config::default() }).is_err());
        assert!(generate(&Config { velocity: 0, ..Config::default() }).is_err());
        let mut c = Config::default();
        c.lanes[0].cells = Some(1 << 31);
        assert!(generate(&c).is_err());
    }
}
