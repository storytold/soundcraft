//! Grid and Nudge values: musical or absolute step sizes, grid lines and snapping.

use crate::{FrameRate, SampleRate, Samples, TICKS_PER_QUARTER, TempoMap, Timecode, to_samples};

/// Note values for Bars|Beats grids.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum NoteValue {
    Bar,
    Whole,
    Half,
    Quarter,
    Eighth,
    Sixteenth,
    ThirtySecond,
    SixtyFourth,
}

impl NoteValue {
    /// Every musical increment in menu order, from largest to smallest.
    pub const ALL: [NoteValue; 8] = [
        NoteValue::Bar,
        NoteValue::Whole,
        NoteValue::Half,
        NoteValue::Quarter,
        NoteValue::Eighth,
        NoteValue::Sixteenth,
        NoteValue::ThirtySecond,
        NoteValue::SixtyFourth,
    ];
    /// Returns the nominal tick length; bars are adjusted for the active meter when used.
    pub fn ticks(self) -> i64 {
        match self {
            NoteValue::Bar => TICKS_PER_QUARTER * 4,
            NoteValue::Whole => TICKS_PER_QUARTER * 4,
            NoteValue::Half => TICKS_PER_QUARTER * 2,
            NoteValue::Quarter => TICKS_PER_QUARTER,
            NoteValue::Eighth => TICKS_PER_QUARTER / 2,
            NoteValue::Sixteenth => TICKS_PER_QUARTER / 4,
            NoteValue::ThirtySecond => TICKS_PER_QUARTER / 8,
            NoteValue::SixtyFourth => TICKS_PER_QUARTER / 16,
        }
    }
    /// Returns the menu label for this musical increment.
    pub fn label(self) -> &'static str {
        match self {
            NoteValue::Bar => "1 bar",
            NoteValue::Whole => "Whole note",
            NoteValue::Half => "1/2 note",
            NoteValue::Quarter => "1/4 note",
            NoteValue::Eighth => "1/8 note",
            NoteValue::Sixteenth => "1/16 note",
            NoteValue::ThirtySecond => "1/32 note",
            NoteValue::SixtyFourth => "1/64 note",
        }
    }
}

/// The time scale used by a Grid or Nudge value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum GridScale {
    BarsBeats,
    MinSecs,
    Timecode,
    FeetFrames,
    Samples,
    ClipsMarkers,
}

impl GridScale {
    /// Numeric time scales shared by Grid and Nudge.
    pub const TIME: [GridScale; 5] = [GridScale::BarsBeats, GridScale::MinSecs, GridScale::Timecode, GridScale::FeetFrames, GridScale::Samples];

    /// Returns the user-facing scale label.
    pub fn label(self) -> &'static str {
        match self {
            GridScale::BarsBeats => "Bars|Beats",
            GridScale::MinSecs => "Min:Secs",
            GridScale::Timecode => "Timecode",
            GridScale::FeetFrames => "Feet+Frames",
            GridScale::Samples => "Samples",
            GridScale::ClipsMarkers => "Clips/Markers",
        }
    }

    /// Returns the stable command and session identifier.
    pub fn id(self) -> &'static str {
        match self {
            GridScale::BarsBeats => "bars_beats",
            GridScale::MinSecs => "min_secs",
            GridScale::Timecode => "timecode",
            GridScale::FeetFrames => "feet_frames",
            GridScale::Samples => "samples",
            GridScale::ClipsMarkers => "clips_markers",
        }
    }

    /// Parses a stable identifier or user-facing label.
    pub fn from_id(id: &str) -> Option<GridScale> {
        Self::TIME.into_iter().chain([GridScale::ClipsMarkers]).find(|scale| scale.id() == id || scale.label().eq_ignore_ascii_case(id))
    }
}

/// Timecode Grid/Nudge increments, ordered independently of the session frame rate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum TimecodeValue {
    Second,
    FiveFrames,
    Frame,
    HalfFrame,
    QuarterFrame,
    SubFrame,
}

impl TimecodeValue {
    /// Every Timecode increment in menu order, from largest to smallest.
    pub const ALL: [TimecodeValue; 6] = [
        TimecodeValue::Second,
        TimecodeValue::FiveFrames,
        TimecodeValue::Frame,
        TimecodeValue::HalfFrame,
        TimecodeValue::QuarterFrame,
        TimecodeValue::SubFrame,
    ];

    /// Returns the user-facing increment label.
    pub fn label(self) -> &'static str {
        match self {
            TimecodeValue::Second => "1 second",
            TimecodeValue::FiveFrames => "5 frames",
            TimecodeValue::Frame => "1 frame",
            TimecodeValue::HalfFrame => "1/2 frame",
            TimecodeValue::QuarterFrame => "1/4 frame",
            TimecodeValue::SubFrame => "1 subframe",
        }
    }

    /// Returns the stable command and session identifier.
    pub fn id(self) -> &'static str {
        match self {
            TimecodeValue::Second => "second",
            TimecodeValue::FiveFrames => "5_frames",
            TimecodeValue::Frame => "frame",
            TimecodeValue::HalfFrame => "half_frame",
            TimecodeValue::QuarterFrame => "quarter_frame",
            TimecodeValue::SubFrame => "subframe",
        }
    }

    /// Parses a stable identifier or user-facing label.
    pub fn from_id(id: &str) -> Option<TimecodeValue> {
        Self::ALL.into_iter().find(|value| value.id() == id || value.label().eq_ignore_ascii_case(id))
    }

    /// Returns the increment length in frames at the session rate.
    fn frames(self, rate: FrameRate) -> f64 {
        match self {
            TimecodeValue::Second => rate.nominal() as f64,
            TimecodeValue::FiveFrames => 5.0,
            TimecodeValue::Frame => 1.0,
            TimecodeValue::HalfFrame => 0.5,
            TimecodeValue::QuarterFrame => 0.25,
            TimecodeValue::SubFrame => 0.01,
        }
    }
}

/// Feet+Frames Grid/Nudge increments.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum FeetFramesValue {
    Foot,
    Frame,
    QuarterFrame,
    SubFrame,
}

impl FeetFramesValue {
    /// Every Feet+Frames increment in menu order, from largest to smallest.
    pub const ALL: [FeetFramesValue; 4] = [FeetFramesValue::Foot, FeetFramesValue::Frame, FeetFramesValue::QuarterFrame, FeetFramesValue::SubFrame];

    /// Returns the user-facing increment label.
    pub fn label(self) -> &'static str {
        match self {
            FeetFramesValue::Foot => "1 foot",
            FeetFramesValue::Frame => "1 frame",
            FeetFramesValue::QuarterFrame => "1/4 frame",
            FeetFramesValue::SubFrame => "1 subframe",
        }
    }

    /// Returns the stable command and session identifier.
    pub fn id(self) -> &'static str {
        match self {
            FeetFramesValue::Foot => "foot",
            FeetFramesValue::Frame => "frame",
            FeetFramesValue::QuarterFrame => "quarter_frame",
            FeetFramesValue::SubFrame => "subframe",
        }
    }

    /// Parses a stable identifier or user-facing label.
    pub fn from_id(id: &str) -> Option<FeetFramesValue> {
        Self::ALL.into_iter().find(|value| value.id() == id || value.label().eq_ignore_ascii_case(id))
    }

    /// Returns the increment length in frames.
    fn frames(self) -> f64 {
        match self {
            FeetFramesValue::Foot => 16.0,
            FeetFramesValue::Frame => 1.0,
            FeetFramesValue::QuarterFrame => 0.25,
            FeetFramesValue::SubFrame => 0.01,
        }
    }
}

/// A grid or nudge step.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum GridValue {
    /// Musical: note value, optionally dotted or triplet.
    Note { value: NoteValue, dotted: bool, triplet: bool },
    /// Absolute seconds (Min:Secs grids: 0.001 … 1 s).
    Seconds(f64),
    /// Frames at the session timecode rate.
    /// Kept for sessions and commands written before named timecode increments were added.
    Frames(i64),
    /// A named Timecode increment, including fractional frames and one second.
    Timecode(TimecodeValue),
    /// A Feet+Frames increment (one foot is 16 frames).
    FeetFrames(FeetFramesValue),
    /// Raw samples.
    Samples(i64),
}

impl Default for GridValue {
    /// Starts on a straight sixteenth-note Bars|Beats grid.
    fn default() -> Self {
        GridValue::Note { value: NoteValue::Sixteenth, dotted: false, triplet: false }
    }
}

impl GridValue {
    /// Returns the user-facing label for this increment.
    pub fn label(&self) -> String {
        match self {
            GridValue::Note { value, dotted, triplet } => {
                let mut s = value.label().to_string();
                if *dotted {
                    s.push_str(" dotted");
                }
                if *triplet {
                    s.push_str(" triplet");
                }
                s
            }
            GridValue::Seconds(s) => {
                if *s >= 1.0 {
                    format!("{s} second{}", if *s == 1.0 { "" } else { "s" })
                } else {
                    format!("{} ms", (s * 1000.0).round() as i64)
                }
            }
            GridValue::Frames(f) => format!("{f} frame{}", if *f == 1 { "" } else { "s" }),
            GridValue::Timecode(value) => value.label().to_string(),
            GridValue::FeetFrames(value) => value.label().to_string(),
            GridValue::Samples(n) => format!("{n} sample{}", if *n == 1 { "" } else { "s" }),
        }
    }

    /// Returns the time scale to which this increment belongs.
    pub fn scale(self) -> GridScale {
        match self {
            GridValue::Note { .. } => GridScale::BarsBeats,
            GridValue::Seconds(_) => GridScale::MinSecs,
            GridValue::Frames(_) | GridValue::Timecode(_) => GridScale::Timecode,
            GridValue::FeetFrames(_) => GridScale::FeetFrames,
            GridValue::Samples(_) => GridScale::Samples,
        }
    }

    /// Returns the default increment for a newly selected time scale.
    pub fn for_scale(scale: GridScale) -> GridValue {
        match scale {
            GridScale::BarsBeats => GridValue::default(),
            GridScale::MinSecs => GridValue::Seconds(1.0),
            GridScale::Timecode => GridValue::Timecode(TimecodeValue::Frame),
            GridScale::FeetFrames => GridValue::FeetFrames(FeetFramesValue::Frame),
            GridScale::Samples => GridValue::Samples(100),
            GridScale::ClipsMarkers => GridValue::default(),
        }
    }

    /// The next larger or smaller menu value of the same kind, or `self` at either end. Note values keep
    /// their dot or triplet modifier.
    pub fn stepped(self, larger: bool) -> GridValue {
        /// Finds the next value in a numeric increment ladder without wrapping.
        fn next<T: PartialOrd + Copy>(ladder: &[T], v: T, larger: bool) -> Option<T> {
            if larger { ladder.iter().copied().find(|x| *x > v) } else { ladder.iter().rev().copied().find(|x| *x < v) }
        }
        match self {
            GridValue::Note { value, dotted, triplet } => {
                let i = NoteValue::ALL.iter().position(|v| *v == value).unwrap_or(0);
                let j = if larger { i.checked_sub(1) } else { i.checked_add(1) };
                j.and_then(|j| NoteValue::ALL.get(j)).map_or(self, |&value| GridValue::Note { value, dotted, triplet })
            }
            GridValue::Seconds(s) => next(&[0.001, 0.01, 0.1, 0.5, 1.0], s, larger).map_or(self, GridValue::Seconds),
            GridValue::Frames(f) => next(&[1, 10, 100], f, larger).map_or(self, GridValue::Frames),
            GridValue::Timecode(value) => step_enum(&TimecodeValue::ALL, value, larger).map_or(self, GridValue::Timecode),
            GridValue::FeetFrames(value) => step_enum(&FeetFramesValue::ALL, value, larger).map_or(self, GridValue::FeetFrames),
            GridValue::Samples(n) => next(&[1, 2, 10, 100, 1_000, 10_000], n, larger).map_or(self, GridValue::Samples),
        }
    }

    fn note_ticks(value: NoteValue, dotted: bool, triplet: bool, map: &TempoMap, at_tick: i64) -> i64 {
        let base = if value == NoteValue::Bar { map.meter_at_tick(at_tick).ticks_per_bar() } else { value.ticks() };
        let mut t = base;
        if dotted {
            t = t * 3 / 2;
        }
        if triplet {
            t = t * 2 / 3;
        }
        t.max(1)
    }

    /// The step in samples at position `at` (musical steps depend on tempo).
    pub fn step_samples(&self, at: Samples, sr: SampleRate, map: &TempoMap, rate: FrameRate) -> Samples {
        match *self {
            GridValue::Note { value, dotted, triplet } => {
                let t0 = map.samples_to_ticks(at, sr);
                let dt = Self::note_ticks(value, dotted, triplet, map, t0);
                (map.tick_to_samples(t0.saturating_add(dt), sr).saturating_sub(map.tick_to_samples(t0, sr))).max(1)
            }
            GridValue::Seconds(s) => sr.samples(s.clamp(0.000_01, 3600.0)).max(1),
            GridValue::Frames(f) => to_samples(rate.samples_per_frame(sr) * f.clamp(1, 1_000_000) as f64).max(1),
            GridValue::Timecode(value) => to_samples(rate.samples_per_frame(sr) * value.frames(rate)).max(1),
            GridValue::FeetFrames(value) => to_samples(rate.samples_per_frame(sr) * value.frames()).max(1),
            GridValue::Samples(n) => n.max(1),
        }
    }

    /// Nearest grid line to `pos`.
    pub fn snap(&self, pos: Samples, sr: SampleRate, map: &TempoMap, rate: FrameRate) -> Samples {
        let lo = self.line_at_or_before(pos, sr, map, rate);
        let hi = self.next_line(lo, sr, map, rate);
        if (pos - lo) <= (hi - pos) { lo } else { hi }
    }

    /// The grid line at or before `pos`.
    pub fn line_at_or_before(&self, pos: Samples, sr: SampleRate, map: &TempoMap, rate: FrameRate) -> Samples {
        match *self {
            GridValue::Note { value, dotted, triplet } => {
                // Float ticks can land a hair either side of a line's own tick.
                let mut t = map.samples_to_ticks_f(pos, sr).floor() as i64;
                if map.tick_to_samples(t, sr) > pos {
                    t = t.saturating_sub(1);
                } else if map.tick_to_samples(t.saturating_add(1), sr) <= pos {
                    t = t.saturating_add(1);
                }
                // Grid lines restart at each bar line so odd meters stay aligned.
                let bb = map.bar_beat_at_tick(t);
                let bar_tick = map.bar_start_tick(bb.bar);
                let step = Self::note_ticks(value, dotted, triplet, map, bar_tick);
                let k = (t - bar_tick).div_euclid(step);
                map.tick_to_samples(bar_tick + k * step, sr)
            }
            _ => self.abs_line(self.abs_index_at_or_before(pos, sr, rate), sr, rate),
        }
    }

    /// Position of absolute grid line `k` (Seconds, Frames, Samples), at an exact multiple of the step
    /// rounded once, so rounding never accumulates from line to line.
    fn abs_line(&self, k: i64, sr: SampleRate, rate: FrameRate) -> Samples {
        if matches!(self, GridValue::Timecode(TimecodeValue::Second)) && rate.is_drop() {
            return rate.samples_at_frame(Self::drop_second_frame(k, rate), sr);
        }
        if let Some(frames) = self.frame_step(rate) {
            let whole = frames.round();
            if frames == whole {
                return rate.samples_at_frame(k.saturating_mul(whole as i64), sr);
            }
            let step = (rate.samples_per_frame(sr) * frames).max(1.0);
            return to_samples(k as f64 * step);
        }
        match *self {
            GridValue::Seconds(s) => to_samples(k as f64 * Self::seconds_step(s, sr)),
            GridValue::Samples(n) => k.saturating_mul(n.max(1)),
            GridValue::Note { .. } => 0,
            GridValue::Frames(_) | GridValue::Timecode(_) | GridValue::FeetFrames(_) => 0,
        }
    }

    /// Index of the absolute grid line at or before `pos`.
    fn abs_index_at_or_before(&self, pos: Samples, sr: SampleRate, rate: FrameRate) -> i64 {
        if matches!(self, GridValue::Timecode(TimecodeValue::Second)) && rate.is_drop() {
            let tc = Timecode::from_frames(rate.frames_at(pos, sr), rate);
            let magnitude = tc.hours.saturating_mul(3600).saturating_add(tc.minutes.saturating_mul(60)).saturating_add(tc.seconds);
            let mut k = if tc.negative { magnitude.saturating_neg() } else { magnitude };
            while self.abs_line(k, sr, rate) > pos {
                let previous = k.saturating_sub(1);
                if previous == k {
                    break;
                }
                k = previous;
            }
            while self.abs_line(k.saturating_add(1), sr, rate) <= pos {
                let next = k.saturating_add(1);
                if next == k {
                    break;
                }
                k = next;
            }
            return k;
        }
        let mut k = if let Some(frames) = self.frame_step(rate) {
            let step = (rate.samples_per_frame(sr) * frames).max(1.0);
            (pos as f64 / step).floor() as i64
        } else {
            match *self {
                GridValue::Seconds(s) => (pos as f64 / Self::seconds_step(s, sr)).floor() as i64,
                GridValue::Samples(n) => return pos.div_euclid(n.max(1)),
                GridValue::Note { .. } => return 0,
                GridValue::Frames(_) | GridValue::Timecode(_) | GridValue::FeetFrames(_) => return 0,
            }
        };
        // Lines are rounded to whole samples, so the floored index can be one off either way.
        if self.abs_line(k, sr, rate) > pos {
            k = k.saturating_sub(1);
        } else if self.abs_line(k.saturating_add(1), sr, rate) <= pos {
            k = k.saturating_add(1);
        }
        k
    }

    /// The exact (fractional) Seconds step in samples, at least one sample.
    fn seconds_step(s: f64, sr: SampleRate) -> f64 {
        (s.clamp(0.000_01, 3600.0) * sr.as_f64()).max(1.0)
    }

    /// Maps a signed whole-second timecode label to its physical frame at a drop-frame rate.
    fn drop_second_frame(index: i64, rate: FrameRate) -> i64 {
        let seconds = index.unsigned_abs();
        let total_minutes = seconds / 60;
        let frames = if seconds.is_multiple_of(60) && !total_minutes.is_multiple_of(10) { if rate.nominal() >= 60 { 4 } else { 2 } } else { 0 };
        Timecode {
            negative: index < 0,
            hours: i64::try_from(seconds / 3600).unwrap_or(i64::MAX),
            minutes: i64::try_from((seconds / 60) % 60).unwrap_or(0),
            seconds: i64::try_from(seconds % 60).unwrap_or(0),
            frames,
        }
        .to_frames(rate)
    }

    /// A frame-based increment as a possibly fractional number of frames.
    fn frame_step(&self, rate: FrameRate) -> Option<f64> {
        match *self {
            GridValue::Frames(frames) => Some(frames.clamp(1, 1_000_000) as f64),
            GridValue::Timecode(value) => Some(value.frames(rate)),
            GridValue::FeetFrames(value) => Some(value.frames()),
            _ => None,
        }
    }

    /// The first grid line strictly after `line`.
    pub fn next_line(&self, line: Samples, sr: SampleRate, map: &TempoMap, rate: FrameRate) -> Samples {
        if !matches!(self, GridValue::Note { .. }) {
            let next = self.abs_line(self.abs_index_at_or_before(line, sr, rate).saturating_add(1), sr, rate);
            return if next > line { next } else { line.saturating_add(1) };
        }
        let step = self.step_samples(line, sr, map, rate);
        let candidate = self.line_at_or_before(line.saturating_add(step), sr, map, rate);
        if candidate > line { candidate } else { line.saturating_add(step) }
    }

    /// All grid lines within `[start, end)`, capped at `max` lines.
    pub fn lines(&self, start: Samples, end: Samples, sr: SampleRate, map: &TempoMap, rate: FrameRate, max: usize) -> Vec<Samples> {
        let mut out = Vec::new();
        let mut p = self.line_at_or_before(start, sr, map, rate);
        while p < end && out.len() < max {
            if p >= start {
                out.push(p);
            }
            let n = self.next_line(p, sr, map, rate);
            if n <= p {
                break;
            }
            p = n;
        }
        out
    }
}

/// Moves one position through an ordered increment menu without wrapping.
fn step_enum<T: Copy + PartialEq>(values: &[T], value: T, larger: bool) -> Option<T> {
    let index = values.iter().position(|candidate| *candidate == value)?;
    let next = if larger { index.checked_sub(1) } else { index.checked_add(1) }?;
    values.get(next).copied()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grid_lines_are_their_own_line_at_odd_tempos() {
        let (g, sr, rate) = (GridValue::default(), SampleRate::HZ_48000, FrameRate::Fps30);
        for bpm in [90.0, 95.0, 110.0, 130.0, 140.0] {
            let map = TempoMap::new(bpm, 4, 4).unwrap();
            let mut line = 0;
            for _ in 0..64 {
                assert_eq!(g.line_at_or_before(line, sr, &map, rate), line, "{bpm} bpm");
                line = g.next_line(line, sr, &map, rate);
            }
        }
    }

    #[test]
    fn sixteenth_grid_at_120() {
        let g = GridValue::default();
        let (sr, map, rate) = (SampleRate::HZ_48000, TempoMap::default(), FrameRate::Fps30);
        assert_eq!(g.step_samples(0, sr, &map, rate), 6000);
        assert_eq!(g.snap(2900, sr, &map, rate), 0);
        assert_eq!(g.snap(3100, sr, &map, rate), 6000);
        assert_eq!(g.lines(0, 24_000, sr, &map, rate, 100), vec![0, 6000, 12000, 18000]);
    }

    /// Absolute increments report their expected sample lengths at a fixed rate.
    #[test]
    fn absolute_grids() {
        let (sr, map, rate) = (SampleRate::HZ_48000, TempoMap::default(), FrameRate::Fps25);
        assert_eq!(GridValue::Seconds(1.0).snap(70_000, sr, &map, rate), 48_000);
        assert_eq!(GridValue::Frames(1).step_samples(0, sr, &map, rate), 1920);
        assert_eq!(GridValue::Timecode(TimecodeValue::Second).step_samples(0, sr, &map, rate), 48_000);
        assert_eq!(GridValue::Timecode(TimecodeValue::FiveFrames).step_samples(0, sr, &map, rate), 9_600);
        assert_eq!(GridValue::Timecode(TimecodeValue::HalfFrame).step_samples(0, sr, &map, rate), 960);
        assert_eq!(GridValue::Timecode(TimecodeValue::QuarterFrame).step_samples(0, sr, &map, rate), 480);
        assert_eq!(GridValue::Timecode(TimecodeValue::SubFrame).step_samples(0, sr, &map, rate), 19);
        assert_eq!(GridValue::FeetFrames(FeetFramesValue::Foot).step_samples(0, sr, &map, rate), 30_720);
        assert_eq!(GridValue::Samples(0).step_samples(0, sr, &map, rate), 1);
    }

    /// Bar length follows the meter while a whole note always spans four quarter notes.
    #[test]
    fn a_bar_and_a_whole_note_differ_in_three_four() {
        let (sr, map, rate) = (SampleRate::HZ_48000, TempoMap::new(120.0, 3, 4).unwrap(), FrameRate::Fps25);
        let note = |value| GridValue::Note { value, dotted: false, triplet: false };
        assert_eq!(note(NoteValue::Bar).step_samples(0, sr, &map, rate), 72_000);
        assert_eq!(note(NoteValue::Whole).step_samples(0, sr, &map, rate), 96_000);
    }

    #[test]
    fn seconds_grid_lines_land_on_whole_seconds() {
        let (sr, map, rate) = (SampleRate::HZ_44100, TempoMap::default(), FrameRate::Fps30);
        for g in [GridValue::Seconds(0.001), GridValue::Seconds(0.01)] {
            for n in 0..20 {
                let s = n * 44_100;
                assert_eq!(g.snap(s, sr, &map, rate), s, "{g:?} at {n} s");
                assert_eq!(g.line_at_or_before(s, sr, &map, rate), s, "{g:?} at {n} s");
            }
        }
    }

    /// Legacy frame grids stay aligned to exact frame starts at fractional rates.
    #[test]
    fn frame_grid_lines_land_on_frame_starts() {
        let sr = SampleRate::HZ_48000;
        let map = TempoMap::default();
        for rate in [FrameRate::Fps29_97, FrameRate::Fps23_976] {
            for f in [1, 10] {
                let g = GridValue::Frames(f);
                for k in (0..5000).step_by(37) {
                    let s = rate.samples_at_frame(k, sr);
                    let line = rate.samples_at_frame(k.div_euclid(f) * f, sr);
                    assert_eq!(g.line_at_or_before(s, sr, &map, rate), line, "{rate:?} {f} frames at frame {k}");
                    if k % f == 0 {
                        assert_eq!(g.snap(s, sr, &map, rate), s, "{rate:?} {f} frames at frame {k}");
                    }
                }
                let start = rate.samples_at_frame(4000, sr);
                let end = rate.samples_at_frame(4000 + 20 * f, sr);
                let want: Vec<_> = (0..20).map(|i| rate.samples_at_frame(4000 + i * f, sr)).collect();
                assert_eq!(g.lines(start, end, sr, &map, rate, 100), want, "{rate:?} {f} frames");
            }
        }
    }

    /// Named whole- and sub-frame grids retain the exact-multiple calculation used by legacy frame grids.
    #[test]
    fn named_frame_grids_land_on_exact_subdivisions() {
        let sr = SampleRate::HZ_48000;
        let map = TempoMap::default();
        let grids = TimecodeValue::ALL.into_iter().map(GridValue::Timecode).chain(FeetFramesValue::ALL.into_iter().map(GridValue::FeetFrames));
        for rate in [FrameRate::Fps29_97, FrameRate::Fps23_976] {
            for grid in grids.clone() {
                for index in 0..200 {
                    let line = grid.abs_line(index, sr, rate);
                    assert_eq!(grid.line_at_or_before(line, sr, &map, rate), line, "{grid:?} {rate:?} line {index}");
                    assert_eq!(grid.next_line(line, sr, &map, rate), grid.abs_line(index + 1, sr, rate), "{grid:?} {rate:?} line {index}");
                }
            }
        }
    }

    /// Drop-frame second grids follow labelled second boundaries in both timeline directions.
    #[test]
    fn drop_frame_second_grids_follow_timecode_labels() {
        let sr = SampleRate::HZ_48000;
        let map = TempoMap::default();
        let grid = GridValue::Timecode(TimecodeValue::Second);
        for rate in [FrameRate::Fps29_97Drop, FrameRate::Fps59_94Drop] {
            for seconds in [0, 59, 60, 61, 600, 601, 3600] {
                let frame = GridValue::drop_second_frame(seconds, rate);
                let line = rate.samples_at_frame(frame, sr);
                assert_eq!(grid.line_at_or_before(line, sr, &map, rate), line, "{rate:?} second {seconds}");
                assert_eq!(grid.line_at_or_before(-line, sr, &map, rate), -line, "{rate:?} second -{seconds}");
                for offset in -2..=rate.nominal() + 2 {
                    let position = rate.samples_at_frame(frame.saturating_add(offset), sr);
                    let before = grid.line_at_or_before(position, sr, &map, rate);
                    assert!(before <= position, "{rate:?} second {seconds}, frame offset {offset}");
                    assert!(grid.next_line(before, sr, &map, rate) > position, "{rate:?} second {seconds}, frame offset {offset}");
                }
            }
            let labelled = Timecode { negative: false, hours: 0, minutes: 1, seconds: 1, frames: 0 }.to_samples(rate, sr);
            assert_eq!(grid.snap(labelled, sr, &map, rate), labelled);
        }

        for (rate, minute_frame) in [(FrameRate::Fps29_97Drop, 1800), (FrameRate::Fps59_94Drop, 3600)] {
            let boundary = rate.samples_at_frame(minute_frame, sr);
            assert_eq!(grid.line_at_or_before(boundary, sr, &map, rate), boundary, "{rate:?} positive minute boundary");
            assert_eq!(grid.snap(boundary, sr, &map, rate), boundary, "{rate:?} positive minute snap");
            assert_eq!(grid.line_at_or_before(-boundary, sr, &map, rate), -boundary, "{rate:?} negative minute boundary");
            assert_eq!(grid.snap(-boundary, sr, &map, rate), -boundary, "{rate:?} negative minute snap");
        }
    }

    #[test]
    fn lines_are_capped() {
        let (sr, map, rate) = (SampleRate::HZ_48000, TempoMap::default(), FrameRate::Fps25);
        assert_eq!(GridValue::Samples(1).lines(0, 1_000_000, sr, &map, rate, 50).len(), 50);
    }

    #[test]
    fn triplets_and_dots() {
        let (sr, map, rate) = (SampleRate::HZ_48000, TempoMap::default(), FrameRate::Fps25);
        let t = GridValue::Note { value: NoteValue::Quarter, dotted: false, triplet: true };
        assert_eq!(t.step_samples(0, sr, &map, rate), 16_000);
        let d = GridValue::Note { value: NoteValue::Quarter, dotted: true, triplet: false };
        assert_eq!(d.step_samples(0, sr, &map, rate), 36_000);
    }

    /// Stepping follows each menu ladder and stops at its endpoints.
    #[test]
    fn values_step_to_the_next_larger_or_smaller() {
        let note = |value, triplet| GridValue::Note { value, dotted: false, triplet };
        assert_eq!(note(NoteValue::Sixteenth, false).stepped(true), note(NoteValue::Eighth, false));
        assert_eq!(note(NoteValue::Eighth, true).stepped(false), note(NoteValue::Sixteenth, true));
        assert_eq!(note(NoteValue::Bar, false).stepped(true), note(NoteValue::Bar, false));
        assert_eq!(note(NoteValue::SixtyFourth, false).stepped(false), note(NoteValue::SixtyFourth, false));
        assert_eq!(GridValue::Seconds(0.01).stepped(true), GridValue::Seconds(0.1));
        assert_eq!(GridValue::Seconds(0.5).stepped(false), GridValue::Seconds(0.1));
        assert_eq!(GridValue::Seconds(1.0).stepped(true), GridValue::Seconds(1.0));
        assert_eq!(GridValue::Frames(1).stepped(true), GridValue::Frames(10));
        assert_eq!(GridValue::Timecode(TimecodeValue::Frame).stepped(false), GridValue::Timecode(TimecodeValue::HalfFrame));
        assert_eq!(GridValue::FeetFrames(FeetFramesValue::Frame).stepped(true), GridValue::FeetFrames(FeetFramesValue::Foot));
        assert_eq!(GridValue::Samples(100).stepped(false), GridValue::Samples(10));
        assert_eq!(GridValue::Samples(1).stepped(true), GridValue::Samples(2));
        assert_eq!(GridValue::Samples(1).stepped(false), GridValue::Samples(1));
    }

    #[test]
    fn snap_at_extreme_position_does_not_overflow() {
        let sr = SampleRate::new(8000).unwrap();
        let map = TempoMap::new(1000.0, 4, 4).unwrap();
        let _ = GridValue::default().snap(i64::MAX, sr, &map, FrameRate::Fps30);
    }
}
