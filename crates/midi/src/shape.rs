//! Pencil shapes: what a Pencil stroke from a press point to the pointer draws, as values over ticks.
//!
//! Line, Parabolic and S-Curve run from one end of the stroke to the other. Triangle, Square and
//! Random repeat every `period` ticks (the grid), swinging between the value where the stroke
//! started and the value under the pointer. Free Hand follows the pointer, so its points come from
//! the editor; here it draws like Line.

use crate::XorShift;
use serde::{Deserialize, Serialize};

/// Most points one stroke produces, however long it is or however fine its step.
pub const MAX_POINTS: i64 = 10_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PencilShape {
    #[default]
    FreeHand,
    Line,
    Triangle,
    Square,
    Random,
    Parabolic,
    SCurve,
}

impl PencilShape {
    pub const ALL: [PencilShape; 7] = [
        PencilShape::FreeHand,
        PencilShape::Line,
        PencilShape::Triangle,
        PencilShape::Square,
        PencilShape::Random,
        PencilShape::Parabolic,
        PencilShape::SCurve,
    ];

    pub fn id(self) -> &'static str {
        match self {
            PencilShape::FreeHand => "free_hand",
            PencilShape::Line => "line",
            PencilShape::Triangle => "triangle",
            PencilShape::Square => "square",
            PencilShape::Random => "random",
            PencilShape::Parabolic => "parabolic",
            PencilShape::SCurve => "s_curve",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            PencilShape::FreeHand => "Free Hand",
            PencilShape::Line => "Line",
            PencilShape::Triangle => "Triangle",
            PencilShape::Square => "Square",
            PencilShape::Random => "Random",
            PencilShape::Parabolic => "Parabolic",
            PencilShape::SCurve => "S-Curve",
        }
    }

    pub fn from_id(s: &str) -> Option<PencilShape> {
        let s = s.trim().to_ascii_lowercase().replace(['-', ' '], "_");
        match s.as_str() {
            "freehand" => Some(PencilShape::FreeHand),
            "scurve" => Some(PencilShape::SCurve),
            _ => PencilShape::ALL.into_iter().find(|p| p.id() == s),
        }
    }

    /// Whether the shape repeats at the grid.
    pub fn periodic(self) -> bool {
        matches!(self, PencilShape::Triangle | PencilShape::Square | PencilShape::Random)
    }
}

/// One stroke: from the press point `from` to the pointer `to`, each `(tick, value)`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ShapeStroke {
    pub shape: PencilShape,
    pub from: (i64, f64),
    pub to: (i64, f64),
    /// The repeat length of Triangle, Square and Random, in ticks (the grid).
    pub period: i64,
    /// Seeds Random, so the same stroke draws the same values.
    pub seed: u64,
}

impl ShapeStroke {
    /// The first and last tick the stroke covers.
    pub fn span(&self) -> (i64, i64) {
        (self.from.0.min(self.to.0), self.from.0.max(self.to.0))
    }

    /// The period in use: even (so a Square flips on a tick) and long enough to stay within
    /// [`MAX_POINTS`] corners.
    fn period(&self) -> i64 {
        let (lo, hi) = self.span();
        let floor = hi.saturating_sub(lo) / (MAX_POINTS / 2) + 1;
        (self.period.max(floor).max(2) / 2).saturating_mul(2)
    }

    /// The value the stroke draws at `tick` (clamped to the span).
    pub fn value_at(&self, tick: i64) -> f64 {
        let (lo, hi) = self.span();
        let tick = tick.clamp(lo, hi);
        let (a, b) = (self.from.1, self.to.1);
        let p = self.period();
        let phase = || tick.saturating_sub(lo).rem_euclid(p) as f64 / p as f64;
        match self.shape {
            PencilShape::FreeHand | PencilShape::Line | PencilShape::Parabolic | PencilShape::SCurve => {
                // From the stroke's left end to its right end, whichever way it was drawn.
                let (l, r) = if self.from.0 <= self.to.0 { (a, b) } else { (b, a) };
                let x = if hi > lo { (tick.saturating_sub(lo)) as f64 / (hi.saturating_sub(lo)) as f64 } else { 1.0 };
                let f = match self.shape {
                    PencilShape::Parabolic => x * x,
                    PencilShape::SCurve => x * x * (3.0 - 2.0 * x),
                    _ => x,
                };
                l + (r - l) * f
            }
            PencilShape::Triangle => a + (b - a) * (1.0 - (2.0 * phase() - 1.0).abs()),
            PencilShape::Square => {
                if phase() < 0.5 {
                    a
                } else {
                    b
                }
            }
            PencilShape::Random => {
                let k = tick.saturating_sub(lo) / p;
                let mut rng = XorShift::new(self.seed ^ (k as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15));
                a + (b - a) * (rng.next_signed() + 1.0) / 2.0
            }
        }
    }

    /// The ticks to put events at. Square and Random change only at their steps, so they get one
    /// event per change; the others get one every `step` ticks plus their ends and corners.
    pub fn ticks(&self, step: i64) -> Vec<i64> {
        let (lo, hi) = self.span();
        let len = hi.saturating_sub(lo);
        let p = self.period();
        let every = |by: i64| (0..MAX_POINTS).map(move |k| lo.saturating_add(k.saturating_mul(by))).take_while(move |t| *t <= hi);
        let mut out: Vec<i64> = match self.shape {
            PencilShape::Square => every(p / 2).collect(),
            PencilShape::Random => every(p).collect(),
            _ => {
                let step = step.max(1).max(len / MAX_POINTS + 1);
                let mut v: Vec<i64> = every(step).collect();
                if self.shape == PencilShape::Triangle {
                    v.extend(every(p / 2));
                }
                v.push(hi);
                v
            }
        };
        out.sort_unstable();
        out.dedup();
        out
    }

    /// `(tick, value)` for each of [`ShapeStroke::ticks`].
    pub fn points(&self, step: i64) -> Vec<(i64, f64)> {
        self.ticks(step).into_iter().map(|t| (t, self.value_at(t))).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stroke(shape: PencilShape, from: (i64, f64), to: (i64, f64)) -> ShapeStroke {
        ShapeStroke { shape, from, to, period: 480, seed: 7 }
    }

    #[test]
    fn ids_round_trip() {
        for s in PencilShape::ALL {
            assert_eq!(PencilShape::from_id(s.id()), Some(s));
        }
        assert_eq!(PencilShape::from_id("S-Curve"), Some(PencilShape::SCurve));
        assert_eq!(PencilShape::from_id("freehand"), Some(PencilShape::FreeHand));
        assert_eq!(PencilShape::from_id("spiral"), None);
    }

    #[test]
    fn line_runs_end_to_end_whichever_way_it_was_drawn() {
        let right = stroke(PencilShape::Line, (0, 0.0), (960, 100.0)).points(240);
        assert_eq!(right, vec![(0, 0.0), (240, 25.0), (480, 50.0), (720, 75.0), (960, 100.0)]);
        let left = stroke(PencilShape::Line, (960, 100.0), (0, 0.0)).points(240);
        assert_eq!(left, right);
    }

    #[test]
    fn curves_start_flat_and_end_at_the_pointer() {
        let p = stroke(PencilShape::Parabolic, (0, 0.0), (1000, 100.0));
        assert_eq!(p.value_at(500), 25.0);
        let s = stroke(PencilShape::SCurve, (0, 0.0), (1000, 100.0));
        assert_eq!(s.value_at(500), 50.0);
        assert!(s.value_at(100) < 10.0 && s.value_at(900) > 90.0);
        assert_eq!(s.value_at(1000), 100.0);
    }

    #[test]
    fn triangle_swings_to_the_pointer_value_once_per_period() {
        let t = stroke(PencilShape::Triangle, (0, 10.0), (960, 90.0));
        assert_eq!(t.value_at(0), 10.0);
        assert_eq!(t.value_at(240), 90.0);
        assert_eq!(t.value_at(480), 10.0);
        // The peaks are events even when the step misses them.
        assert!(t.ticks(100).contains(&240) && t.ticks(100).contains(&720));
    }

    #[test]
    fn square_and_random_change_only_at_their_steps() {
        let sq = stroke(PencilShape::Square, (0, 0.0), (960, 127.0));
        assert_eq!(sq.points(10), vec![(0, 0.0), (240, 127.0), (480, 0.0), (720, 127.0), (960, 0.0)]);
        let r = stroke(PencilShape::Random, (0, 20.0), (960, 60.0));
        assert_eq!(r.ticks(10), vec![0, 480, 960]);
        for (_, v) in r.points(10) {
            assert!((20.0..=60.0).contains(&v), "{v}");
        }
        assert_eq!(r.points(10), r.points(10), "the same seed draws the same values");
    }

    #[test]
    fn hostile_spans_stay_bounded() {
        let huge = ShapeStroke { shape: PencilShape::Triangle, from: (i64::MIN, 0.0), to: (i64::MAX, 1.0), period: 1, seed: 0 };
        assert!(huge.points(1).len() as i64 <= 2 * MAX_POINTS + 1);
        let sq = ShapeStroke { shape: PencilShape::Square, period: 0, ..huge };
        assert!(sq.points(0).len() as i64 <= MAX_POINTS);
        assert!(sq.value_at(0).is_finite());
        let point = stroke(PencilShape::Line, (5, 1.0), (5, 9.0));
        assert_eq!(point.points(60), vec![(5, 9.0)]);
    }
}
