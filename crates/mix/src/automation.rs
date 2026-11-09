use super::{AutoParam, Samples, Session, TICKS_PER_QUARTER, Track, Transport, db_to_gain};
use soundcraft_dsp::MIN_DB;
use soundcraft_model::AutomationLane;

pub(crate) fn transport_at(s: &Session, pos: Samples, playing: bool, recording: bool) -> Transport {
    let sr = s.sample_rate;
    let ticks = Some(s.tempo.samples_to_ticks_f(pos, sr)).filter(|t| t.is_finite()).unwrap_or(0.0);
    let tick = ticks.floor() as i64;
    let meter = s.tempo.meter_at_tick(tick);
    let bar = s.tempo.bar_beat_at_tick(tick).bar;
    let q = TICKS_PER_QUARTER as f64;
    Transport {
        playing,
        recording,
        tempo: s.tempo.tempo_at_tick(tick),
        numerator: meter.numerator,
        denominator: meter.denominator,
        samples: pos,
        seconds: pos as f64 / sr.as_f64().max(1.0),
        quarters: ticks / q,
        bar_start: s.tempo.bar_start_tick(bar) as f64 / q,
        bar: bar - 1,
    }
}

/// One term of a fader's level in dB: a fixed value, or an automation lane (with its default).
#[derive(Clone, Copy)]
pub(crate) enum Level<'a> {
    Fixed(f32),
    Lane(&'a AutomationLane, f32),
}

impl Level<'_> {
    fn at(&self, pos: Samples) -> f32 {
        match self {
            Level::Fixed(v) => *v,
            Level::Lane(l, default) => l.value_at(pos, *default),
        }
    }

    /// The lane's first breakpoint after `pos`. Up to it, the level is linear in time.
    fn next_point(&self, pos: Samples) -> Option<Samples> {
        match self {
            Level::Fixed(_) => None,
            Level::Lane(l, _) => l.points.get(l.points.partition_point(|p| p.at <= pos)).map(|p| p.at),
        }
    }
}

/// A track's volume: its automation lane when the mode reads it, else the fader.
pub(crate) fn volume_level(t: &Track) -> Level<'_> {
    if t.mixer.automation_mode.reads()
        && t.mixer.automation_mode != soundcraft_model::AutomationMode::Write
        && let Some(l) = t.lane(&AutoParam::Volume)
        && !l.points.is_empty()
    {
        return Level::Lane(l, t.mixer.volume_db);
    }
    Level::Fixed(t.mixer.volume_db)
}

/// Calls `f(i, j, d0, d1)` for each stretch `i..=j` of the block at `pos` over which the sum of
/// `levels` is linear in dB, with its values at `i` and `j`. Lanes are linear between breakpoints,
/// so a stretch ends at the next breakpoint or at the block's last frame.
fn for_each_segment(levels: &[Level<'_>], pos: Samples, frames: usize, mut f: impl FnMut(usize, usize, f32, f32)) {
    let db = |at: Samples| levels.iter().map(|l| l.at(at)).sum::<f32>();
    let last = frames.saturating_sub(1);
    let mut i = 0usize;
    while i < frames {
        let at = pos.saturating_add(i as i64);
        let j = levels
            .iter()
            .filter_map(|l| l.next_point(at))
            .min()
            .map_or(last, |k| usize::try_from(k.saturating_sub(pos)).map_or(last, |j| j.min(last)));
        f(i, j, db(at), db(pos.saturating_add(j as i64)));
        i = j + 1;
    }
}

/// Multiplies `x` by a gain that goes from `d0` dB at its first sample to `d1` dB at its last,
/// linearly in dB, capped at `max_db` and silent at or below [`MIN_DB`] (or NaN), as `db_to_gain`
/// would give it sample by sample. Linear in dB is geometric in gain, so the ramp steps by one ratio
/// instead of converting every sample; the ends are converted exactly.
pub(crate) fn scale(x: &mut [f32], d0: f32, d1: f32, max_db: f32) {
    let Some(n) = x.len().checked_sub(1) else { return };
    if d0.is_nan() || d1.is_nan() {
        x.fill(0.0);
        return;
    }
    if d0 == d1 || n == 0 {
        let g = gain(d0, max_db);
        x.iter_mut().for_each(|v| *v *= g);
        return;
    }
    let step = (f64::from(d1) - f64::from(d0)) / n as f64;
    let ratio = 10f64.powf(step / 20.0);
    let (floor, cap) = (f64::from(MIN_DB), f64::from(max_db));
    let gcap = f64::from(db_to_gain(max_db));
    let (mut d, mut g) = (f64::from(d0), 10f64.powf(f64::from(d0) / 20.0));
    if let Some((first, rest)) = x.split_first_mut() {
        *first *= gain(d0, max_db);
        if let Some((last, mid)) = rest.split_last_mut() {
            for v in mid {
                d += step;
                g *= ratio;
                let gd = if d.is_nan() || d <= floor {
                    0.0
                } else if d >= cap {
                    gcap
                } else {
                    g
                };
                *v *= gd as f32;
            }
            *last *= gain(d1, max_db);
        }
    }
}

/// The gain of `db`, at most `max_db`.
fn gain(db: f32, max_db: f32) -> f32 {
    db_to_gain(db.min(max_db))
}

/// Fader gain over the block at `pos`, read at every frame: multiplies `buf`'s channels by the
/// gain of the sum of `levels` (dB, at most `max_db`).
pub(crate) fn apply_gain(levels: &[Level<'_>], pos: Samples, frames: usize, max_db: f32, buf: &mut [Vec<f32>]) {
    for_each_segment(levels, pos, frames, |i, j, d0, d1| {
        for c in buf.iter_mut() {
            if let Some(x) = c.get_mut(i..=j) {
                scale(x, d0, d1, max_db);
            }
        }
    });
}

/// [`apply_gain`] from `src` into `dst`.
pub(crate) fn copy_with_gain(levels: &[Level<'_>], pos: Samples, frames: usize, max_db: f32, src: &[Vec<f32>], dst: &mut [Vec<f32>]) {
    for_each_segment(levels, pos, frames, |i, j, d0, d1| {
        for (d, s) in dst.iter_mut().zip(src) {
            if let (Some(d), Some(s)) = (d.get_mut(i..=j), s.get(i..=j)) {
                d.copy_from_slice(s);
                scale(d, d0, d1, max_db);
            }
        }
    });
}

pub(crate) fn pan_at(t: &Track, idx: usize, pos: Samples) -> f32 {
    let base = t.mixer.pan.get(idx).copied().unwrap_or(if idx == 0 { 0.0 } else { 1.0 });
    if t.mixer.automation_mode.reads()
        && let Some(l) = t.lane(&AutoParam::Pan(u8::try_from(idx).unwrap_or(0)))
        && !l.points.is_empty()
    {
        return l.value_at(pos, base);
    }
    base
}

/// Pan automation is read on a grid of [`PAN_STEP`] timeline samples rather than once per block,
/// so playback (callback-sized blocks) and bounces (1024-frame blocks) pan identically.
const PAN_STEP: Samples = 64;

/// The position to read the pan at for the frames from `start` of the block at `pos`, and the end
/// of the frames that share it. Without pan automation the whole block shares one pan.
pub(crate) fn pan_segment(t: &Track, pos: Samples, start: usize, frames: usize) -> (Samples, usize) {
    let automated = t.mixer.automation_mode.reads() && t.automation.iter().any(|l| matches!(l.param, AutoParam::Pan(_)) && !l.points.is_empty());
    if !automated {
        return (pos, frames);
    }
    let here = pos.saturating_add(start as i64);
    let at = here.div_euclid(PAN_STEP).saturating_mul(PAN_STEP);
    let left = usize::try_from(at.saturating_add(PAN_STEP).saturating_sub(here)).unwrap_or(1).max(1);
    (at, start.saturating_add(left).min(frames))
}

pub(crate) fn send_level(t: &Track, i: usize, base: f32, pos: Samples) -> f32 {
    if t.mixer.automation_mode.reads()
        && let Some(l) = t.lane(&AutoParam::SendLevel(u8::try_from(i).unwrap_or(0)))
        && !l.points.is_empty()
    {
        return l.value_at(pos, base).clamp(soundcraft_dsp::MIN_DB, crate::SEND_LEVEL_MAX_DB);
    }
    base
}
