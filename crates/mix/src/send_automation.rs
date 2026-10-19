//! Local send playback: sample-exact mono-to-stereo pan and stepped ordinary-bus mute.
//! No document mutation, allocation, locks or persistent cursor survive a callback/seek.

use super::{MAX_CHANNELS, Matrix};
use soundcraft_dsp::pan::{PanLaw, gains};
use soundcraft_model::{AutoParam, AutomationPoint, Track};
use soundcraft_time::Samples;

/// A forward cursor rebuilt at each callback's absolute position. Like the existing mixer
/// readers, this deliberately ignores lane.enabled; global suspension/migration is separate.
/// Nonfinite or decreasing-time lanes fall back to the static control for the whole callback.
/// Finite values clamp to the control range; duplicate times deterministically use the last knot.
pub(super) struct Reader<'a> {
    points: &'a [AutomationPoint],
    next: usize,
    mute: bool,
}

impl<'a> Reader<'a> {
    pub(super) fn for_send(t: &'a Track, param: fn(u8) -> AutoParam, slot: usize, pos: Samples) -> Option<Self> {
        if !t.mixer.automation_mode.reads() {
            return None;
        }
        let param = param(u8::try_from(slot).ok()?);
        let lane = t.lane(&param)?;
        let points = &lane.points;
        if points.is_empty() || points.iter().any(|p| !p.value.is_finite()) || points.windows(2).any(|p| p[0].at > p[1].at) {
            return None;
        }
        Some(Self { points, next: points.partition_point(|p| p.at <= pos), mute: matches!(param, AutoParam::SendMute(_)) })
    }

    pub(super) fn value(&mut self, pos: Samples) -> f32 {
        while self.points.get(self.next).is_some_and(|p| p.at <= pos) {
            self.next = self.next.saturating_add(1);
        }
        let Some(a) = self.points.get(self.next.saturating_sub(1)) else { return 0.0 };
        let lo = if self.mute { 0.0 } else { -1.0 };
        let av = a.value.clamp(lo, 1.0);
        if self.next == 0 || self.mute {
            return av;
        }
        let Some(b) = self.points.get(self.next) else { return av };
        // Valid timestamps may span i64::MIN..MAX. Widen before subtraction, and interpolate
        // bounded values in f64 so neither timestamp nor value arithmetic can overflow.
        let width = i128::from(b.at) - i128::from(a.at);
        if width <= 0 {
            return av;
        }
        let fraction = ((i128::from(pos) - i128::from(a.at)) as f64 / width as f64).clamp(0.0, 1.0);
        (f64::from(av) + (f64::from(b.value.clamp(lo, 1.0)) - f64::from(av)) * fraction) as f32
    }
}

/// Timeline position represented by a delayed source tap. Widen the entire expression before
/// clamping, so advancing a frame near MIN/MAX does not lose an earlier delay subtraction.
#[derive(Clone, Copy)]
pub(super) struct Clock {
    position: Samples,
    age: u64,
}

impl Clock {
    pub(super) fn new(position: Samples, age: u64) -> Self {
        Self { position, age }
    }

    pub(super) fn at(self, frame: usize) -> Samples {
        let frame = i128::try_from(frame).unwrap_or(i128::MAX);
        let at = i128::from(self.position).saturating_add(frame).saturating_sub(i128::from(self.age));
        at.clamp(i128::from(Samples::MIN), i128::from(Samples::MAX)) as Samples
    }
}

impl From<Samples> for Clock {
    fn from(position: Samples) -> Self {
        Self::new(position, 0)
    }
}

#[cfg(test)]
fn frame_position(pos: Samples, frame: usize) -> Samples {
    Clock::from(pos).at(frame)
}

#[allow(clippy::too_many_arguments)]
pub(super) fn apply_mono_stereo(
    src: &[Vec<f32>],
    frames: usize,
    dst: &mut [Vec<f32>],
    gain: f32,
    law: PanLaw,
    pos: impl Into<Clock>,
    mut pan: Reader<'_>,
    mut mute: Option<Reader<'_>>,
) {
    let clock = pos.into();
    let Some(src) = src.first() else { return };
    for (i, sample) in src.iter().enumerate().take(frames) {
        let at = clock.at(i);
        if mute.as_mut().is_some_and(|m| m.value(at) >= 0.5) {
            continue;
        }
        let (left, right) = gains(pan.value(at), law);
        for (channel, factor) in [(0, left), (1, right)] {
            if let Some(out) = dst.get_mut(channel).and_then(|c| c.get_mut(i)) {
                *out += sample * gain * factor;
            }
        }
    }
}

/// Keep the existing static routing matrix (including mono folds and surround positions),
/// applying only the exact mute gate. Sparse taps are prepared on the stack once per block.
pub(super) fn apply_gated(
    src: &[Vec<f32>],
    frames: usize,
    dst: &mut [Vec<f32>],
    matrix: &Matrix,
    gain: f32,
    pos: impl Into<Clock>,
    mut mute: Reader<'_>,
) {
    let clock = pos.into();
    let mut taps = [[(0usize, 0.0f32); MAX_CHANNELS]; MAX_CHANNELS];
    let mut counts = [0usize; MAX_CHANNELS];
    for (d, (taps, count)) in taps.iter_mut().zip(counts.iter_mut()).enumerate().take(dst.len()) {
        for (s, row) in matrix.iter().enumerate().take(src.len().min(MAX_CHANNELS)) {
            let g = row.get(d).copied().unwrap_or(0.0);
            if g != 0.0
                && let Some(tap) = taps.get_mut(*count)
            {
                *tap = (s, g);
                *count += 1;
            }
        }
    }
    for i in 0..frames {
        if mute.value(clock.at(i)) >= 0.5 {
            continue;
        }
        for (d, channel) in dst.iter_mut().enumerate().take(MAX_CHANNELS) {
            let Some(out) = channel.get_mut(i) else { continue };
            let mut acc = 0.0;
            for &(s, g) in taps.get(d).and_then(|t| counts.get(d).and_then(|n| t.get(..*n))).unwrap_or(&[]) {
                acc += src.get(s).and_then(|c| c.get(i)).copied().unwrap_or(0.0) * gain * g;
            }
            *out += acc;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use soundcraft_model::{AutomationLane, ChannelFormat, Session, TrackKind};

    fn track(points: &[(i64, f32)], mute: bool) -> Track {
        let mut s = Session::default();
        let id = s.add_track(TrackKind::Audio, ChannelFormat::Mono, None);
        let t = s.track_mut(id).unwrap();
        let mut lane = AutomationLane::new(if mute { AutoParam::SendMute(0) } else { AutoParam::SendPan(0) });
        lane.points = points.iter().map(|&(at, value)| AutomationPoint { at, value }).collect();
        t.automation.push(lane);
        t.clone()
    }
    #[test]
    fn wide_interpolation_does_not_overflow() {
        let t = track(&[(i64::MIN, -1.0), (i64::MAX, 1.0)], false);
        let mut r = Reader::for_send(&t, AutoParam::SendPan, 0, i64::MIN).unwrap();
        assert_eq!(r.value(i64::MIN), -1.0);
        assert!(r.value(0).abs() < 1e-7);
        assert_eq!(r.value(i64::MAX), 1.0);
        assert_eq!(frame_position(i64::MAX - 1, 3), i64::MAX);
    }
    #[test]
    fn endpoints_duplicates_and_finite_extremes_are_deterministic() {
        let t = track(&[(3, -f32::MAX), (3, 0.25), (7, f32::MAX)], false);
        let mut r = Reader::for_send(&t, AutoParam::SendPan, 0, 0).unwrap();
        assert_eq!(r.value(0), -1.0);
        assert_eq!(r.value(3), 0.25);
        assert_eq!(r.value(5), 0.625);
        assert_eq!(r.value(9), 1.0);
        let t = track(&[(i64::MIN, -f32::MAX), (i64::MAX, f32::MAX)], true);
        let mut r = Reader::for_send(&t, AutoParam::SendMute, 0, i64::MIN).unwrap();
        assert_eq!(r.value(0), 0.0);
        assert_eq!(r.value(i64::MAX), 1.0);
    }
    #[test]
    fn short_buffers_and_saturating_positions_do_not_panic() {
        let t = track(&[(i64::MIN, 0.0), (i64::MAX, 1.0)], true);
        let r = Reader::for_send(&t, AutoParam::SendMute, 0, i64::MAX - 1).unwrap();
        let mut dst = vec![vec![0.0; 1]];
        let mut matrix = [[0.0; MAX_CHANNELS]; MAX_CHANNELS];
        matrix[0][0] = 1.0;
        apply_gated(&[vec![0.25; 2]], 4, &mut dst, &matrix, 1.0, i64::MAX - 1, r);
        assert_eq!(dst, vec![vec![0.25]]);
        let t = track(&[(i64::MIN, -1.0), (i64::MAX, 1.0)], false);
        let r = Reader::for_send(&t, AutoParam::SendPan, 0, i64::MAX - 1).unwrap();
        apply_mono_stereo(&[vec![0.25; 2]], 4, &mut [], 1.0, PanLaw::Minus3, i64::MAX - 1, r, None);
    }

    #[test]
    fn source_clock_clamps_after_the_composite_delay_expression() {
        assert_eq!(Clock::new(i64::MIN + 5, 10).at(8), i64::MIN + 3);
        assert_eq!(Clock::new(i64::MAX, 10).at(3), i64::MAX - 7);
        assert_eq!(Clock::new(i64::MAX, 10).at(20), i64::MAX);
        assert_eq!(Clock::new(-5, 10).at(3), -12);
        assert_eq!(Clock::new(i64::MAX, u64::MAX).at(0), i64::MIN);
        assert_eq!(Clock::new(i64::MIN, u64::MAX).at(0), i64::MIN);
    }

    #[test]
    fn delayed_reader_seed_and_gate_advancement_use_the_same_clock() {
        let t = track(&[(0, 0.0), (30, 1.0), (40, 0.0)], true);
        let clock = Clock::new(45, 25);
        let reader = Reader::for_send(&t, AutoParam::SendMute, 0, clock.at(0)).unwrap();
        let mut matrix = [[0.0; MAX_CHANNELS]; MAX_CHANNELS];
        matrix[0][0] = 1.0;
        let mut destination = vec![vec![0.0; 25]];
        apply_gated(&[vec![0.25; 25]], 25, &mut destination, &matrix, 1.0, clock, reader);
        assert_eq!(destination[0], (0..25).map(|frame| if (10..20).contains(&frame) { 0.0 } else { 0.25 }).collect::<Vec<_>>());
    }
}
