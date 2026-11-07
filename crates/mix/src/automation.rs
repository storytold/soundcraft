use super::{AutoParam, Samples, Session, TICKS_PER_QUARTER, Track, Transport, db_to_gain};
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

/// Calls `f(i, gain)` for every frame `i` of the block at `pos`, with the gain of the sum of
/// `levels` (dB, at most `max_db`) at that frame's own timeline sample. Lanes are linear in dB
/// between breakpoints, so the sum is read at each breakpoint and interpolated in between: a
/// breakpoint takes effect on its own sample, whatever size the blocks are.
pub(crate) fn for_each_gain(levels: &[Level<'_>], pos: Samples, frames: usize, max_db: f32, mut f: impl FnMut(usize, f32)) {
    let db = |at: Samples| levels.iter().map(|l| l.at(at)).sum::<f32>();
    let last = frames.saturating_sub(1);
    let mut i = 0usize;
    while i < frames {
        let at = pos.saturating_add(i as i64);
        // The stretch up to the next breakpoint (or the block's last frame) is linear in dB.
        let j = levels
            .iter()
            .filter_map(|l| l.next_point(at))
            .min()
            .map_or(last, |k| usize::try_from(k.saturating_sub(pos)).map_or(last, |j| j.min(last)));
        let (d0, d1) = (db(at), db(pos.saturating_add(j as i64)));
        if d0 == d1 {
            let g = db_to_gain(d0.min(max_db));
            (i..=j).for_each(|m| f(m, g));
        } else {
            let n = (j - i) as f32;
            for m in i..=j {
                f(m, db_to_gain((d0 + (d1 - d0) * ((m - i) as f32 / n)).min(max_db)));
            }
        }
        i = j + 1;
    }
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
