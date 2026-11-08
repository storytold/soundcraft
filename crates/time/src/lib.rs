//! SoundCraft timebases.
//!
//! Every position in a session is an absolute sample index (`i64`) at the session sample rate.
//! This crate converts those positions to and from the five Pro Tools-style timebases
//! (Bars|Beats, Min:Secs, Timecode, Feet+Frames, Samples) through a [`TempoMap`], and computes
//! grid lines and snapping for the Grid / Nudge values.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

mod format;
mod grid;
mod tempo;
mod timecode;

pub use format::{TimeFormat, format_length, format_position, parse_position};
pub use grid::{FeetFramesValue, GridScale, GridValue, NoteValue, TimecodeValue};
pub use tempo::{BarBeat, MeterEvent, TICKS_PER_QUARTER, TempoEvent, TempoMap};
pub use timecode::{FrameRate, Timecode};

/// A position or length in samples.
pub type Samples = i64;

/// Largest position accepted from parameters and files: 2^40 samples (about 265 days at 48 kHz).
/// Clamping there keeps every later addition and subtraction on positions far away from overflow.
pub const MAX_POSITION: Samples = 1 << 40;

/// Errors from parsing or converting time values.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum TimeError {
    #[error("cannot parse `{0}` as a {1} value")]
    Parse(String, &'static str),
    #[error("invalid sample rate {0}")]
    SampleRate(u32),
    #[error("invalid tempo {0}")]
    Tempo(f64),
    #[error("invalid meter {0}/{1}")]
    Meter(u32, u32),
}

/// Sample rates supported by sessions (Pro Tools offers 44.1 k – 192 k; we accept 8 k – 768 k).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct SampleRate(u32);

impl SampleRate {
    pub const HZ_44100: SampleRate = SampleRate(44_100);
    pub const HZ_48000: SampleRate = SampleRate(48_000);
    pub const COMMON: [u32; 8] = [44_100, 48_000, 88_200, 96_000, 176_400, 192_000, 352_800, 384_000];

    pub fn new(hz: u32) -> Result<Self, TimeError> {
        if (8_000..=768_000).contains(&hz) { Ok(SampleRate(hz)) } else { Err(TimeError::SampleRate(hz)) }
    }
    pub fn hz(self) -> u32 {
        self.0
    }
    pub fn as_f64(self) -> f64 {
        f64::from(self.0)
    }
    /// Samples → seconds.
    pub fn seconds(self, s: Samples) -> f64 {
        s as f64 / self.as_f64()
    }
    /// Seconds → samples, rounded to nearest and saturated.
    pub fn samples(self, seconds: f64) -> Samples {
        to_samples(seconds * self.as_f64())
    }
}

impl Default for SampleRate {
    fn default() -> Self {
        SampleRate::HZ_48000
    }
}

/// Round a float sample count to `i64`, mapping NaN to 0 and saturating infinities.
pub fn to_samples(v: f64) -> Samples {
    if v.is_nan() {
        0
    } else if v >= i64::MAX as f64 {
        i64::MAX
    } else if v <= i64::MIN as f64 {
        i64::MIN
    } else {
        v.round() as i64
    }
}

/// A half-open sample range `[start, end)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, serde::Serialize, serde::Deserialize)]
pub struct Range {
    pub start: Samples,
    pub end: Samples,
}

impl Range {
    pub fn new(a: Samples, b: Samples) -> Self {
        if a <= b { Range { start: a, end: b } } else { Range { start: b, end: a } }
    }
    pub fn point(at: Samples) -> Self {
        Range { start: at, end: at }
    }
    pub fn len(&self) -> Samples {
        self.end.saturating_sub(self.start)
    }
    pub fn is_empty(&self) -> bool {
        self.end <= self.start
    }
    pub fn contains(&self, s: Samples) -> bool {
        s >= self.start && s < self.end
    }
    pub fn overlaps(&self, o: &Range) -> bool {
        self.start < o.end && o.start < self.end
    }
    pub fn intersect(&self, o: &Range) -> Option<Range> {
        let s = self.start.max(o.start);
        let e = self.end.min(o.end);
        (s < e).then_some(Range { start: s, end: e })
    }
    pub fn shifted(&self, by: Samples) -> Range {
        Range { start: self.start.saturating_add(by), end: self.end.saturating_add(by) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sample_rate_bounds() {
        assert!(SampleRate::new(48_000).is_ok());
        assert!(SampleRate::new(0).is_err());
        assert!(SampleRate::new(10_000_000).is_err());
    }

    #[test]
    fn seconds_round_trip() {
        let sr = SampleRate::HZ_44100;
        assert_eq!(sr.samples(1.0), 44_100);
        assert!((sr.seconds(22_050) - 0.5).abs() < 1e-12);
    }

    #[test]
    fn to_samples_is_total() {
        assert_eq!(to_samples(f64::NAN), 0);
        assert_eq!(to_samples(f64::INFINITY), i64::MAX);
        assert_eq!(to_samples(f64::NEG_INFINITY), i64::MIN);
    }

    #[test]
    fn range_ops() {
        let a = Range::new(10, 0);
        assert_eq!(a, Range { start: 0, end: 10 });
        assert_eq!(a.intersect(&Range::new(5, 20)), Some(Range::new(5, 10)));
        assert_eq!(a.intersect(&Range::new(10, 20)), None);
        assert!(a.contains(9) && !a.contains(10));
    }
}
