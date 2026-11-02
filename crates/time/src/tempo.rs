//! Tempo and meter maps: the conversion between musical time (ticks) and samples.

use crate::{MAX_POSITION, SampleRate, Samples, TimeError, to_samples};

/// Ticks per quarter note (Pro Tools' resolution).
pub const TICKS_PER_QUARTER: i64 = 960;

/// A tempo change at a tick position. `bpm` is quarter notes per minute.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct TempoEvent {
    pub tick: i64,
    pub bpm: f64,
}

/// A meter (time signature) change at a tick position, which must fall on a bar line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct MeterEvent {
    pub tick: i64,
    pub numerator: u32,
    pub denominator: u32,
}

impl MeterEvent {
    pub fn ticks_per_beat(&self) -> i64 {
        let d = i64::from(self.denominator.max(1));
        (TICKS_PER_QUARTER * 4 / d).max(1)
    }
    pub fn ticks_per_bar(&self) -> i64 {
        self.ticks_per_beat().saturating_mul(i64::from(self.numerator.max(1)))
    }
}

/// A musical position: 1-based bar and beat plus ticks within the beat.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct BarBeat {
    pub bar: i64,
    pub beat: i64,
    pub tick: i64,
}

/// Tempo and meter changes for a session. Always has at least one of each, at tick 0.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct TempoMap {
    tempos: Vec<TempoEvent>,
    meters: Vec<MeterEvent>,
}

impl Default for TempoMap {
    fn default() -> Self {
        TempoMap { tempos: vec![TempoEvent { tick: 0, bpm: 120.0 }], meters: vec![MeterEvent { tick: 0, numerator: 4, denominator: 4 }] }
    }
}

fn valid_bpm(bpm: f64) -> Result<f64, TimeError> {
    if bpm.is_finite() && (5.0..=1000.0).contains(&bpm) { Ok(bpm) } else { Err(TimeError::Tempo(bpm)) }
}

fn valid_meter(numerator: u32, denominator: u32) -> bool {
    (1..=64).contains(&numerator) && [1, 2, 4, 8, 16, 32, 64].contains(&denominator)
}

impl TempoMap {
    pub fn new(bpm: f64, numerator: u32, denominator: u32) -> Result<Self, TimeError> {
        let mut m = TempoMap::default();
        m.set_tempo(0, bpm)?;
        m.set_meter(0, numerator, denominator)?;
        Ok(m)
    }

    pub fn tempos(&self) -> &[TempoEvent] {
        &self.tempos
    }
    pub fn meters(&self) -> &[MeterEvent] {
        &self.meters
    }

    /// Repair a map read from a file: only valid tempos and meters within `0..=MAX_POSITION` ticks, in
    /// tick order, with one of each at tick 0.
    pub fn sanitize(&mut self) {
        self.tempos.retain(|e| (0..=MAX_POSITION).contains(&e.tick) && valid_bpm(e.bpm).is_ok());
        self.tempos.sort_by_key(|e| e.tick);
        self.tempos.dedup_by_key(|e| e.tick);
        if self.tempos.first().is_none_or(|e| e.tick > 0) {
            let bpm = self.tempos.first().map_or(120.0, |e| e.bpm);
            self.tempos.insert(0, TempoEvent { tick: 0, bpm });
        }
        self.meters.retain(|m| (0..=MAX_POSITION).contains(&m.tick) && valid_meter(m.numerator, m.denominator));
        self.meters.sort_by_key(|m| m.tick);
        self.meters.dedup_by_key(|m| m.tick);
        if self.meters.first().is_none_or(|m| m.tick > 0) {
            let (numerator, denominator) = self.meters.first().map_or((4, 4), |m| (m.numerator, m.denominator));
            self.meters.insert(0, MeterEvent { tick: 0, numerator, denominator });
        }
    }

    /// Insert or replace the tempo at `tick`.
    pub fn set_tempo(&mut self, tick: i64, bpm: f64) -> Result<(), TimeError> {
        let bpm = valid_bpm(bpm)?;
        let tick = tick.max(0);
        match self.tempos.binary_search_by(|e| e.tick.cmp(&tick)) {
            Ok(i) => {
                if let Some(e) = self.tempos.get_mut(i) {
                    e.bpm = bpm;
                }
            }
            Err(i) => self.tempos.insert(i, TempoEvent { tick, bpm }),
        }
        Ok(())
    }

    /// Remove a tempo event (the one at tick 0 is kept).
    pub fn remove_tempo(&mut self, tick: i64) -> bool {
        if tick == 0 {
            return false;
        }
        let before = self.tempos.len();
        self.tempos.retain(|e| e.tick != tick);
        before != self.tempos.len()
    }

    /// Insert or replace the meter at `tick` (snapped to the bar line at or before it).
    /// Later meter changes keep their bar numbers, so they stay on bar lines.
    pub fn set_meter(&mut self, tick: i64, numerator: u32, denominator: u32) -> Result<(), TimeError> {
        if !valid_meter(numerator, denominator) {
            return Err(TimeError::Meter(numerator, denominator));
        }
        let tick = if tick <= 0 { 0 } else { self.bar_start_tick(self.bar_beat_at_tick(tick).bar) };
        let later = self.take_meters_after(tick);
        self.put_meter(MeterEvent { tick, numerator, denominator });
        self.restore_meters_at_bars(later);
        Ok(())
    }

    /// Remove a meter event (the one at tick 0 is kept). Later meter changes keep their bar numbers.
    pub fn remove_meter(&mut self, tick: i64) -> bool {
        if tick == 0 || !self.meters.iter().any(|e| e.tick == tick) {
            return false;
        }
        let later = self.take_meters_after(tick);
        self.meters.retain(|e| e.tick != tick);
        self.restore_meters_at_bars(later);
        true
    }

    fn put_meter(&mut self, ev: MeterEvent) {
        match self.meters.binary_search_by(|e| e.tick.cmp(&ev.tick)) {
            Ok(i) => {
                if let Some(e) = self.meters.get_mut(i) {
                    *e = ev;
                }
            }
            Err(i) => self.meters.insert(i, ev),
        }
    }

    /// Remove the meters after `tick`, returning each with the bar it starts.
    fn take_meters_after(&mut self, tick: i64) -> Vec<(i64, MeterEvent)> {
        let later: Vec<_> = self.meters.iter().filter(|e| e.tick > tick).map(|e| (self.bar_beat_at_tick(e.tick).bar, *e)).collect();
        self.meters.retain(|e| e.tick <= tick);
        later
    }

    /// Put meters back at the start of their bars under the current earlier meters, in bar order.
    fn restore_meters_at_bars(&mut self, later: Vec<(i64, MeterEvent)>) {
        for (bar, ev) in later {
            let tick = self.bar_start_tick(bar);
            self.put_meter(MeterEvent { tick, ..ev });
        }
    }

    /// Tempo in effect at `tick`.
    pub fn tempo_at_tick(&self, tick: i64) -> f64 {
        let mut bpm = self.tempos.first().map_or(120.0, |e| e.bpm);
        for e in &self.tempos {
            if e.tick <= tick {
                bpm = e.bpm;
            } else {
                break;
            }
        }
        bpm
    }

    /// Meter in effect at `tick`.
    pub fn meter_at_tick(&self, tick: i64) -> MeterEvent {
        let mut m = self.meters.first().copied().unwrap_or(MeterEvent { tick: 0, numerator: 4, denominator: 4 });
        for e in &self.meters {
            if e.tick <= tick {
                m = *e;
            } else {
                break;
            }
        }
        m
    }

    fn samples_per_tick(bpm: f64, sr: SampleRate) -> f64 {
        sr.as_f64() * 60.0 / (bpm * TICKS_PER_QUARTER as f64)
    }

    /// Ticks → samples (exact for constant-tempo segments).
    pub fn tick_to_samples(&self, tick: i64, sr: SampleRate) -> Samples {
        to_samples(self.tick_to_samples_f(tick, sr))
    }

    fn tick_to_samples_f(&self, tick: i64, sr: SampleRate) -> f64 {
        let mut acc = 0.0;
        let mut seg_tick = 0i64;
        let mut bpm = self.tempos.first().map_or(120.0, |e| e.bpm);
        for e in self.tempos.iter().skip(1) {
            if e.tick >= tick {
                break;
            }
            acc += (e.tick - seg_tick) as f64 * Self::samples_per_tick(bpm, sr);
            seg_tick = e.tick;
            bpm = e.bpm;
        }
        acc + (tick - seg_tick) as f64 * Self::samples_per_tick(bpm, sr)
    }

    /// Samples → ticks (fractional).
    pub fn samples_to_ticks_f(&self, s: Samples, sr: SampleRate) -> f64 {
        let s = s as f64;
        let mut acc = 0.0;
        let mut seg_tick = 0i64;
        let mut bpm = self.tempos.first().map_or(120.0, |e| e.bpm);
        for e in self.tempos.iter().skip(1) {
            let seg = (e.tick - seg_tick) as f64 * Self::samples_per_tick(bpm, sr);
            if acc + seg > s {
                break;
            }
            acc += seg;
            seg_tick = e.tick;
            bpm = e.bpm;
        }
        seg_tick as f64 + (s - acc) / Self::samples_per_tick(bpm, sr)
    }

    pub fn samples_to_ticks(&self, s: Samples, sr: SampleRate) -> i64 {
        to_samples(self.samples_to_ticks_f(s, sr))
    }

    /// Tick of the first beat of `bar` (1-based; bars before 1 extend the first meter backwards).
    pub fn bar_start_tick(&self, bar: i64) -> i64 {
        let mut cur_bar = 1i64;
        let mut cur_tick = 0i64;
        let first = self.meter_at_tick(0);
        if bar < 1 {
            return bar.saturating_sub(1).saturating_mul(first.ticks_per_bar());
        }
        for (i, m) in self.meters.iter().enumerate() {
            let next_tick = self.meters.get(i + 1).map(|n| n.tick);
            let tpb = m.ticks_per_bar();
            let bars_in_seg = next_tick.map(|nt| ceil_div(nt.saturating_sub(cur_tick), tpb));
            match bars_in_seg {
                Some(n) if bar >= cur_bar.saturating_add(n) => {
                    cur_bar = cur_bar.saturating_add(n);
                    cur_tick = next_tick.unwrap_or(cur_tick);
                }
                _ => return cur_tick.saturating_add((bar - cur_bar).saturating_mul(tpb)),
            }
        }
        cur_tick
    }

    /// Bar|beat|tick at `tick`.
    pub fn bar_beat_at_tick(&self, tick: i64) -> BarBeat {
        let first = self.meter_at_tick(0);
        if tick < 0 {
            let tpb = first.ticks_per_bar();
            let abs = tick.unsigned_abs();
            let tpb_u = tpb.unsigned_abs();
            let bars_back_u = abs / tpb_u + u64::from(!abs.is_multiple_of(tpb_u));
            let bars_back = i64::try_from(bars_back_u).unwrap_or(i64::MAX);
            // Offset of `tick` inside its bar, without negating `i64::MIN`.
            let within = ((tpb_u - abs % tpb_u) % tpb_u) as i64;
            let beat_t = first.ticks_per_beat();
            return BarBeat { bar: 1 - bars_back, beat: within / beat_t + 1, tick: within % beat_t };
        }
        let mut bar = 1i64;
        let mut seg_tick = 0i64;
        for (i, m) in self.meters.iter().enumerate() {
            let tpb = m.ticks_per_bar();
            if let Some(n) = self.meters.get(i + 1)
                && tick >= n.tick
            {
                bar = bar.saturating_add(ceil_div(n.tick.saturating_sub(seg_tick), tpb));
                seg_tick = n.tick;
                continue;
            }
            let rel = tick - seg_tick;
            let bars = rel / tpb;
            let within = rel % tpb;
            let beat_t = m.ticks_per_beat();
            return BarBeat { bar: bar + bars, beat: within / beat_t + 1, tick: within % beat_t };
        }
        BarBeat { bar, beat: 1, tick: 0 }
    }

    /// Bar|beat|tick → tick.
    pub fn tick_at_bar_beat(&self, bb: BarBeat) -> i64 {
        let start = self.bar_start_tick(bb.bar);
        let m = self.meter_at_tick(start);
        start.saturating_add(bb.beat.saturating_sub(1).max(0).saturating_mul(m.ticks_per_beat())).saturating_add(bb.tick)
    }

    pub fn bar_beat_at(&self, s: Samples, sr: SampleRate) -> BarBeat {
        self.bar_beat_at_tick(self.samples_to_ticks(s, sr))
    }

    pub fn samples_at_bar_beat(&self, bb: BarBeat, sr: SampleRate) -> Samples {
        self.tick_to_samples(self.tick_at_bar_beat(bb), sr)
    }

    /// Scale every tempo by `factor` (Tempo Operations › Scale).
    pub fn scale_tempos(&mut self, factor: f64) -> Result<(), TimeError> {
        let mut out = self.tempos.clone();
        for e in &mut out {
            e.bpm = valid_bpm(e.bpm * factor)?;
        }
        self.tempos = out;
        Ok(())
    }
}

/// Ceiling division for a non-negative `num` and positive `den`, without overflow.
fn ceil_div(num: i64, den: i64) -> i64 {
    num / den + i64::from(num % den != 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_map_is_120_4_4() {
        let m = TempoMap::default();
        let sr = SampleRate::HZ_48000;
        // One bar of 4/4 at 120 bpm = 2 seconds.
        assert_eq!(m.samples_at_bar_beat(BarBeat { bar: 2, beat: 1, tick: 0 }, sr), 96_000);
        assert_eq!(m.bar_beat_at(96_000, sr), BarBeat { bar: 2, beat: 1, tick: 0 });
        assert_eq!(m.bar_beat_at(24_000, sr), BarBeat { bar: 1, beat: 2, tick: 0 });
    }

    #[test]
    fn meter_near_max_tick_does_not_overflow() {
        let mut m = TempoMap::default();
        m.set_meter(i64::MAX, 7, 8).unwrap();
        let bb = m.bar_beat_at_tick(i64::MAX);
        assert!(bb.bar > 1);
        assert!(m.bar_start_tick(bb.bar) > 0);
        assert!(m.bar_start_tick(i64::MAX) > 0);
    }

    #[test]
    fn minimum_bar_saturates_instead_of_overflowing() {
        let m = TempoMap::default();
        assert_eq!(m.bar_start_tick(i64::MIN), i64::MIN);
        assert_eq!(m.bar_start_tick(0), -TICKS_PER_QUARTER * 4);
    }

    #[test]
    fn minimum_beat_is_clamped_without_overflow() {
        let m = TempoMap::default();
        assert_eq!(m.tick_at_bar_beat(BarBeat { bar: 1, beat: i64::MIN, tick: 0 }), 0);
    }

    #[test]
    fn bar_beat_at_min_tick_does_not_overflow() {
        let m = TempoMap::default();
        let bb = m.bar_beat_at_tick(i64::MIN);
        assert!(bb.bar < 1);
        assert!(bb.beat >= 1 && bb.tick >= 0);
        assert_eq!(m.bar_beat_at_tick(-1).bar, 0);
    }

    #[test]
    fn tempo_change_is_integrated() {
        let mut m = TempoMap::default();
        let sr = SampleRate::HZ_48000;
        m.set_tempo(TICKS_PER_QUARTER * 4, 60.0).unwrap();
        // Bar 1 at 120 = 96000 samples; bar 2 at 60 = 192000 samples.
        assert_eq!(m.tick_to_samples(TICKS_PER_QUARTER * 8, sr), 96_000 + 192_000);
        assert_eq!(m.samples_to_ticks(96_000 + 192_000, sr), TICKS_PER_QUARTER * 8);
    }

    #[test]
    fn meter_change_counts_bars() {
        let mut m = TempoMap::default();
        // 2 bars of 4/4 then 3/4.
        m.set_meter(TICKS_PER_QUARTER * 8, 3, 4).unwrap();
        assert_eq!(m.bar_beat_at_tick(TICKS_PER_QUARTER * 8), BarBeat { bar: 3, beat: 1, tick: 0 });
        assert_eq!(m.bar_beat_at_tick(TICKS_PER_QUARTER * 11), BarBeat { bar: 4, beat: 1, tick: 0 });
        assert_eq!(m.bar_start_tick(4), TICKS_PER_QUARTER * 11);
        let bb = BarBeat { bar: 4, beat: 2, tick: 10 };
        assert_eq!(m.bar_beat_at_tick(m.tick_at_bar_beat(bb)), bb);
    }

    #[test]
    fn eighth_note_meters() {
        let m = TempoMap::new(120.0, 6, 8).unwrap();
        assert_eq!(m.bar_beat_at_tick(TICKS_PER_QUARTER * 3), BarBeat { bar: 2, beat: 1, tick: 0 });
        assert_eq!(m.bar_beat_at_tick(TICKS_PER_QUARTER / 2), BarBeat { bar: 1, beat: 2, tick: 0 });
    }

    #[test]
    fn sanitize_keeps_what_the_setters_accept() {
        let tempo = |tick, bpm| TempoEvent { tick, bpm };
        let meter = |tick, numerator, denominator| MeterEvent { tick, numerator, denominator };
        let mut m = TempoMap {
            tempos: vec![tempo(960, 4e9), tempo(3840, 90.0), tempo(-5, 100.0), tempo(1920, 60.0), tempo(1920, 70.0)],
            meters: vec![meter(0, 0, 4), meter(3840, 3, 4)],
        };
        m.sanitize();
        assert_eq!(m.tempos(), [tempo(0, 60.0), tempo(1920, 60.0), tempo(3840, 90.0)]);
        assert_eq!(m.meters(), [meter(0, 3, 4), meter(3840, 3, 4)]);
        let mut empty = TempoMap { tempos: Vec::new(), meters: Vec::new() };
        empty.sanitize();
        assert_eq!(empty, TempoMap::default());
    }

    #[test]
    fn sanitize_drops_events_beyond_the_position_range() {
        let mut m = TempoMap {
            tempos: vec![TempoEvent { tick: 0, bpm: 120.0 }, TempoEvent { tick: i64::MAX, bpm: 90.0 }],
            meters: vec![MeterEvent { tick: 0, numerator: 4, denominator: 4 }, MeterEvent { tick: i64::MAX, numerator: 3, denominator: 4 }],
        };
        m.sanitize();
        assert_eq!(m, TempoMap::default());
        assert_eq!(m.bar_start_tick(2), TICKS_PER_QUARTER * 4);
    }

    #[test]
    fn rejects_bad_values() {
        let mut m = TempoMap::default();
        assert!(m.set_tempo(0, f64::NAN).is_err());
        assert!(m.set_tempo(0, 0.0).is_err());
        assert!(m.set_meter(0, 0, 4).is_err());
        assert!(m.set_meter(0, 4, 3).is_err());
        assert!(!m.remove_tempo(0));
    }

    #[test]
    fn negative_positions() {
        let m = TempoMap::default();
        assert_eq!(m.bar_beat_at_tick(-TICKS_PER_QUARTER * 4), BarBeat { bar: 0, beat: 1, tick: 0 });
        assert_eq!(m.bar_beat_at_tick(-TICKS_PER_QUARTER), BarBeat { bar: 0, beat: 4, tick: 0 });
    }
}
