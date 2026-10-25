//! SMPTE timecode and Feet+Frames.

use crate::{SampleRate, Samples, TimeError};

/// Session timecode rates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, serde::Serialize, serde::Deserialize)]
pub enum FrameRate {
    Fps23_976,
    #[default]
    Fps24,
    Fps25,
    Fps29_97,
    Fps29_97Drop,
    Fps30,
    Fps47_952,
    Fps48,
    Fps50,
    Fps59_94,
    Fps59_94Drop,
    Fps60,
    Fps100,
    Fps119_88,
    Fps120,
}

impl FrameRate {
    pub const ALL: [FrameRate; 15] = [
        FrameRate::Fps23_976,
        FrameRate::Fps24,
        FrameRate::Fps25,
        FrameRate::Fps29_97,
        FrameRate::Fps29_97Drop,
        FrameRate::Fps30,
        FrameRate::Fps47_952,
        FrameRate::Fps48,
        FrameRate::Fps50,
        FrameRate::Fps59_94,
        FrameRate::Fps59_94Drop,
        FrameRate::Fps60,
        FrameRate::Fps100,
        FrameRate::Fps119_88,
        FrameRate::Fps120,
    ];

    /// Real frames per second as a rational (num, den).
    pub fn rational(self) -> (i64, i64) {
        match self {
            FrameRate::Fps23_976 => (24_000, 1001),
            FrameRate::Fps24 => (24, 1),
            FrameRate::Fps25 => (25, 1),
            FrameRate::Fps29_97 | FrameRate::Fps29_97Drop => (30_000, 1001),
            FrameRate::Fps30 => (30, 1),
            FrameRate::Fps47_952 => (48_000, 1001),
            FrameRate::Fps48 => (48, 1),
            FrameRate::Fps50 => (50, 1),
            FrameRate::Fps59_94 | FrameRate::Fps59_94Drop => (60_000, 1001),
            FrameRate::Fps60 => (60, 1),
            FrameRate::Fps100 => (100, 1),
            FrameRate::Fps119_88 => (120_000, 1001),
            FrameRate::Fps120 => (120, 1),
        }
    }

    /// Frames per labelled second (the counting base).
    pub fn nominal(self) -> i64 {
        let (n, d) = self.rational();
        (n + d / 2) / d
    }

    pub fn is_drop(self) -> bool {
        matches!(self, FrameRate::Fps29_97Drop | FrameRate::Fps59_94Drop)
    }

    pub fn label(self) -> &'static str {
        match self {
            FrameRate::Fps23_976 => "23.976",
            FrameRate::Fps24 => "24",
            FrameRate::Fps25 => "25",
            FrameRate::Fps29_97 => "29.97",
            FrameRate::Fps29_97Drop => "29.97 Drop",
            FrameRate::Fps30 => "30",
            FrameRate::Fps47_952 => "47.952",
            FrameRate::Fps48 => "48",
            FrameRate::Fps50 => "50",
            FrameRate::Fps59_94 => "59.94",
            FrameRate::Fps59_94Drop => "59.94 Drop",
            FrameRate::Fps60 => "60",
            FrameRate::Fps100 => "100",
            FrameRate::Fps119_88 => "119.88",
            FrameRate::Fps120 => "120",
        }
    }

    /// Whole frame index at a sample position (floored).
    pub fn frames_at(self, s: Samples, sr: SampleRate) -> i64 {
        let (n, d) = self.rational();
        let num = i128::from(s) * i128::from(n);
        let den = i128::from(sr.hz()) * i128::from(d);
        if den == 0 {
            return 0;
        }
        i64::try_from(num.div_euclid(den)).unwrap_or(0)
    }

    /// Sample position of the start of frame `f`.
    pub fn samples_at_frame(self, f: i64, sr: SampleRate) -> Samples {
        let (n, d) = self.rational();
        if n == 0 {
            return 0;
        }
        // Round half away from zero so negative frames mirror positive ones.
        let mag = (i128::from(f.unsigned_abs()) * i128::from(sr.hz()) * i128::from(d) + i128::from(n) / 2) / i128::from(n);
        let v = if f < 0 { -mag } else { mag };
        i64::try_from(v).unwrap_or(if v < 0 { i64::MIN } else { i64::MAX })
    }

    /// Samples per frame (fractional).
    pub fn samples_per_frame(self, sr: SampleRate) -> f64 {
        let (n, d) = self.rational();
        sr.as_f64() * d as f64 / n as f64
    }
}

/// A timecode label.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Timecode {
    pub negative: bool,
    pub hours: i64,
    pub minutes: i64,
    pub seconds: i64,
    pub frames: i64,
}

impl Timecode {
    /// Label for an absolute frame count.
    pub fn from_frames(frames: i64, rate: FrameRate) -> Timecode {
        let negative = frames < 0;
        // Drop-frame labels can exceed i64::MAX even when the frame count fits. Widen before
        // taking the magnitude so i64::MIN is represented exactly too.
        let mut f = i128::from(frames).abs();
        let base = i128::from(rate.nominal().max(1));
        if rate.is_drop() {
            // Drop 2 (or 4) labels each minute except every tenth.
            let drop = if base >= 60 { 4 } else { 2 };
            let per_10min = base * 600 - drop * 9;
            let per_min = base * 60 - drop;
            let tens = f / per_10min;
            let rem = f % per_10min;
            let extra = if rem > drop { drop * 9 * tens + drop * ((rem - drop) / per_min) } else { drop * 9 * tens };
            f += extra;
        }
        let frames_l = f % base;
        let total_secs = f / base;
        Timecode {
            negative,
            hours: i64::try_from(total_secs / 3600).unwrap_or(i64::MAX),
            minutes: i64::try_from((total_secs / 60) % 60).unwrap_or(0),
            seconds: i64::try_from(total_secs % 60).unwrap_or(0),
            frames: i64::try_from(frames_l).unwrap_or(0),
        }
    }

    /// Absolute frame count of this label.
    pub fn to_frames(&self, rate: FrameRate) -> i64 {
        let base = rate.nominal().max(1);
        let total_min = i128::from(self.hours) * 60 + i128::from(self.minutes);
        let mut f = (total_min * 60 + i128::from(self.seconds)) * i128::from(base) + i128::from(self.frames);
        if rate.is_drop() {
            let drop = if base >= 60 { 4 } else { 2 };
            f -= drop * (total_min - total_min / 10);
        }
        let f = if self.negative { -f } else { f };
        i64::try_from(f).unwrap_or(if f < 0 { i64::MIN } else { i64::MAX })
    }

    pub fn from_samples(s: Samples, rate: FrameRate, sr: SampleRate) -> Timecode {
        Timecode::from_frames(rate.frames_at(s, sr), rate)
    }

    pub fn to_samples(&self, rate: FrameRate, sr: SampleRate) -> Samples {
        rate.samples_at_frame(self.to_frames(rate), sr)
    }

    pub fn format(&self, rate: FrameRate) -> String {
        let sep = if rate.is_drop() { ';' } else { ':' };
        let sign = if self.negative { "-" } else { "" };
        format!("{sign}{:02}:{:02}:{:02}{sep}{:02}", self.hours, self.minutes, self.seconds, self.frames)
    }

    /// Parse `HH:MM:SS:FF` (also `;` / `.` separators and fewer fields, right-aligned).
    pub fn parse(text: &str) -> Result<Timecode, TimeError> {
        let t = text.trim();
        let (negative, t) = match t.strip_prefix('-') {
            Some(r) => (true, r),
            None => (false, t),
        };
        let parts: Vec<&str> = t.split([':', ';', '.']).collect();
        if parts.is_empty() || parts.len() > 4 {
            return Err(TimeError::Parse(text.to_string(), "timecode"));
        }
        let mut nums = [0i64; 4];
        let offset = 4 - parts.len();
        for (i, p) in parts.iter().enumerate() {
            let v: i64 = p.trim().parse().map_err(|_| TimeError::Parse(text.to_string(), "timecode"))?;
            if v < 0 {
                return Err(TimeError::Parse(text.to_string(), "timecode"));
            }
            if let Some(slot) = nums.get_mut(offset + i) {
                *slot = v;
            }
        }
        Ok(Timecode { negative, hours: nums[0], minutes: nums[1], seconds: nums[2], frames: nums[3] })
    }
}

/// Feet+Frames for 35 mm film (16 frames per foot), `start` frames in at sample 0.
pub fn feet_frames(s: Samples, rate: FrameRate, sr: SampleRate, start: i64) -> (i64, i64) {
    let f = rate.frames_at(s, sr).saturating_add(start);
    (f.div_euclid(16), f.rem_euclid(16))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn non_drop_round_trip() {
        let sr = SampleRate::HZ_48000;
        let tc = Timecode::from_samples(48_000 * 3661, FrameRate::Fps25, sr);
        assert_eq!(tc.format(FrameRate::Fps25), "01:01:01:00");
        assert_eq!(tc.to_samples(FrameRate::Fps25, sr), 48_000 * 3661);
    }

    #[test]
    fn negative_timecode_rounds_symmetrically() {
        let sr = SampleRate::HZ_48000;
        let tc = Timecode::parse("-00:00:01:00").unwrap();
        assert_eq!(tc.to_samples(FrameRate::Fps25, sr), -48_000);
        for r in [FrameRate::Fps25, FrameRate::Fps29_97, FrameRate::Fps29_97Drop, FrameRate::Fps23_976] {
            for f in [1i64, 7, 25, 1799, 107_892] {
                assert_eq!(r.samples_at_frame(-f, sr), -r.samples_at_frame(f, sr), "{r:?} frame {f}");
            }
        }
    }

    #[test]
    fn drop_frame_labels() {
        let r = FrameRate::Fps29_97Drop;
        // Frame 1800 is the first frame of minute 1 → labelled 00:01:00;02.
        assert_eq!(Timecode::from_frames(1800, r).format(r), "00:01:00;02");
        assert_eq!(Timecode::from_frames(1799, r).format(r), "00:00:59;29");
        // Minute 10 keeps its labels.
        assert_eq!(Timecode::from_frames(17_982, r).format(r), "00:10:00;00");
        for f in [0, 1, 1799, 1800, 1801, 17_981, 17_982, 107_892, 500_000] {
            assert_eq!(Timecode::from_frames(f, r).to_frames(r), f, "frame {f}");
        }
    }

    #[test]
    fn huge_timecode_saturates_instead_of_overflowing() {
        let sr = SampleRate::HZ_48000;
        for r in [FrameRate::Fps30, FrameRate::Fps29_97Drop] {
            let tc = Timecode::parse("9223372036854775807:00:00:00").unwrap();
            assert_eq!(tc.to_samples(r, sr), i64::MAX);
            let tc = Timecode::parse("-9223372036854775807:00:00:00").unwrap();
            assert_eq!(tc.to_samples(r, sr), i64::MIN);
        }
        let tc = Timecode::parse("01:02:03:04").unwrap();
        assert_eq!(tc.to_samples(FrameRate::Fps30, sr), 178_710_400);
    }

    #[test]
    fn extreme_frame_counts_round_trip_at_every_rate() {
        for rate in FrameRate::ALL {
            for frames in [i64::MIN, i64::MIN + 1, -1, 0, 1, i64::MAX - 1, i64::MAX] {
                let tc = Timecode::from_frames(frames, rate);
                assert_eq!(tc.to_frames(rate), frames, "{rate:?}: {frames}");
                assert_eq!(tc.negative, frames < 0);
                assert!((0..60).contains(&tc.minutes));
                assert!((0..60).contains(&tc.seconds));
                assert!((0..rate.nominal()).contains(&tc.frames));
                assert_eq!(Timecode::parse(&tc.format(rate)).unwrap(), tc);
            }
        }
    }

    #[test]
    fn extreme_drop_frame_labels_do_not_overflow() {
        for rate in [FrameRate::Fps29_97Drop, FrameRate::Fps59_94Drop] {
            assert_eq!(Timecode::from_frames(i64::MAX, rate).to_frames(rate), i64::MAX);
        }
    }

    #[test]
    fn parse_variants() {
        let tc = Timecode::parse("01:02:03:04").unwrap();
        assert_eq!((tc.hours, tc.minutes, tc.seconds, tc.frames), (1, 2, 3, 4));
        let tc = Timecode::parse("3:04").unwrap();
        assert_eq!((tc.seconds, tc.frames), (3, 4));
        assert!(Timecode::parse("a:b").is_err());
        assert!(Timecode::parse("1:2:3:4:5").is_err());
    }

    #[test]
    fn feet_and_frames() {
        let sr = SampleRate::HZ_48000;
        assert_eq!(feet_frames(48_000, FrameRate::Fps24, sr, 0), (1, 8));
    }
}
