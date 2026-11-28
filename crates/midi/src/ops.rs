//! MIDI editing operations (Event Operations): quantize, transpose, velocity, duration, split.
//!
//! All operations work in place on a slice of notes and never change the slice length, so they
//! can be applied to a selection. Randomised operations use a deterministic [`XorShift`] seeded
//! with [`DEFAULT_SEED`] (or an explicit seed via the `*_with_seed` variants).

use crate::{Note, XorShift};

/// Seed used by [`quantize`] and [`change_velocity`] for their random components.
pub const DEFAULT_SEED: u64 = 0x5EED_50DC_4AF7_0001;

fn unit(x: f32) -> f64 {
    if x.is_finite() { f64::from(x).clamp(0.0, 1.0) } else { 0.0 }
}

fn round_i64(x: f64) -> i64 {
    if x.is_finite() { x.round().clamp(i64::MIN as f64, i64::MAX as f64) as i64 } else { 0 }
}

/// Options for [`quantize`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct QuantizeOptions {
    /// Grid spacing in ticks (e.g. 240 = 1/16 note at 960 PPQ). Values <= 0 make quantize a no-op.
    pub grid_ticks: i64,
    /// How far notes move towards the grid line, 0..=1 (1 = all the way).
    pub strength: f32,
    /// Swing, 0..=1: every second grid line is delayed by `swing * grid / 3`, so 1.0 gives a
    /// triplet feel.
    pub swing: f32,
    /// Only notes within this fraction of half a grid step of their target are moved
    /// (1 = all notes).
    pub include_within: f32,
    /// Notes closer than this fraction of half a grid step to their target are left alone
    /// (0 = none excluded).
    pub exclude_within: f32,
    /// Also quantize note ends to the grid.
    pub quantize_ends: bool,
    /// Keep each note's length when its start moves (otherwise the end stays put). Ignored when
    /// `quantize_ends` is set.
    pub preserve_duration: bool,
    /// Random offset after quantizing, 0..=1 fraction of half a grid step.
    pub random: f32,
}

impl Default for QuantizeOptions {
    fn default() -> Self {
        Self {
            grid_ticks: 240,
            strength: 1.0,
            swing: 0.0,
            include_within: 1.0,
            exclude_within: 0.0,
            quantize_ends: false,
            preserve_duration: true,
            random: 0.0,
        }
    }
}

/// The swung grid line nearest to `t`.
fn nearest_line(t: i64, grid: i64, swing: f64) -> i64 {
    let swing_off = round_i64(swing * grid as f64 / 3.0);
    let k = t.saturating_add(grid / 2).div_euclid(grid);
    let mut best = i64::MAX;
    let mut best_d = u64::MAX;
    for kk in [k.saturating_sub(1), k, k.saturating_add(1)] {
        let line = kk.saturating_mul(grid).saturating_add(if kk.rem_euclid(2) == 1 { swing_off } else { 0 });
        let d = line.abs_diff(t);
        if d < best_d {
            best_d = d;
            best = line;
        }
    }
    best
}

/// Quantizes note starts (and optionally ends) to a grid. See [`QuantizeOptions`].
pub fn quantize(notes: &mut [Note], opts: &QuantizeOptions) {
    quantize_with_seed(notes, opts, DEFAULT_SEED);
}

/// [`quantize`] with an explicit seed for the `random` component.
pub fn quantize_with_seed(notes: &mut [Note], opts: &QuantizeOptions, seed: u64) {
    let grid = opts.grid_ticks;
    if grid <= 0 {
        return;
    }
    let strength = unit(opts.strength);
    let swing = unit(opts.swing);
    let include = unit(opts.include_within);
    let exclude = unit(opts.exclude_within);
    let random = unit(opts.random);
    let half = grid as f64 / 2.0;
    let mut rng = XorShift::new(seed);

    for n in notes.iter_mut() {
        let old_start = n.start;
        let old_end = n.end();
        let target = nearest_line(old_start, grid, swing);
        let d = target.saturating_sub(old_start);
        let dist = d.unsigned_abs() as f64;
        if include < 1.0 && dist > include * half {
            continue;
        }
        if dist < exclude * half {
            continue;
        }
        let mut new_start = old_start.saturating_add(round_i64(d as f64 * strength));
        if random > 0.0 {
            new_start = new_start.saturating_add(round_i64(rng.next_signed() * random * half));
        }
        n.start = new_start;
        n.length = if opts.quantize_ends {
            let end_target = nearest_line(old_end, grid, swing);
            let new_end = old_end.saturating_add(round_i64(end_target.saturating_sub(old_end) as f64 * strength));
            new_end.saturating_sub(new_start)
        } else if opts.preserve_duration {
            n.length
        } else {
            old_end.saturating_sub(new_start)
        }
        .max(1);
    }
}

/// Transposes notes by `semitones`. Pitches are **clamped** to 0..=127 (notes are never
/// dropped, so the slice length is unchanged); notes pushed past either end pile up on 0 or 127.
pub fn transpose(notes: &mut [Note], semitones: i32) {
    for n in notes.iter_mut() {
        n.pitch = i32::from(n.pitch).saturating_add(semitones).clamp(0, 127) as u8;
    }
}

/// Velocity change for [`change_velocity`]. Results are clamped to 1..=127.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum VelocityOp {
    Set(u8),
    Add(i32),
    /// Multiply (non-finite factors leave velocities unchanged).
    Scale(f32),
    /// Clamp into `min..=max` (bounds are swapped if reversed).
    Limit {
        min: u8,
        max: u8,
    },
    /// Linear ramp over the notes' time span: the earliest note gets `from`, the latest `to`.
    Ramp {
        from: u8,
        to: u8,
    },
    /// Random offset of up to ± the amount.
    Humanize(u8),
}

fn clamp_vel(v: i64) -> u8 {
    v.clamp(1, 127) as u8
}

/// Changes note-on velocities. See [`VelocityOp`].
pub fn change_velocity(notes: &mut [Note], op: VelocityOp) {
    change_velocity_with_seed(notes, op, DEFAULT_SEED);
}

/// [`change_velocity`] with an explicit seed for [`VelocityOp::Humanize`].
pub fn change_velocity_with_seed(notes: &mut [Note], op: VelocityOp, seed: u64) {
    match op {
        VelocityOp::Set(v) => notes.iter_mut().for_each(|n| n.velocity = clamp_vel(i64::from(v))),
        VelocityOp::Add(d) => notes.iter_mut().for_each(|n| n.velocity = clamp_vel(i64::from(n.velocity) + i64::from(d))),
        VelocityOp::Scale(f) => {
            if f.is_finite() {
                notes.iter_mut().for_each(|n| n.velocity = clamp_vel(round_i64(f64::from(n.velocity) * f64::from(f))));
            }
        }
        VelocityOp::Limit { min, max } => {
            let (lo, hi) = if min <= max { (min, max) } else { (max, min) };
            notes.iter_mut().for_each(|n| n.velocity = clamp_vel(i64::from(n.velocity.clamp(lo, hi))));
        }
        VelocityOp::Ramp { from, to } => {
            let first = notes.iter().map(|n| n.start).min().unwrap_or(0);
            let last = notes.iter().map(|n| n.start).max().unwrap_or(0);
            let span = (i128::from(last) - i128::from(first)) as f64;
            for n in notes.iter_mut() {
                let offset = (i128::from(n.start) - i128::from(first)) as f64;
                let t = if span > 0.0 { offset / span } else { 0.0 };
                let v = f64::from(from) + (f64::from(to) - f64::from(from)) * t;
                n.velocity = clamp_vel(round_i64(v));
            }
        }
        VelocityOp::Humanize(amount) => {
            let mut rng = XorShift::new(seed);
            for n in notes.iter_mut() {
                let d = round_i64(rng.next_signed() * f64::from(amount));
                n.velocity = clamp_vel(i64::from(n.velocity) + d);
            }
        }
    }
}

/// Duration change for [`change_duration`]. Lengths are always kept >= 1 tick.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DurationOp {
    Set(i64),
    Add(i64),
    /// Multiply (non-finite factors leave lengths unchanged).
    Scale(f32),
    /// Each note extends to `gap` ticks before the next later note start (any pitch). The last
    /// note(s) keep their length.
    Legato {
        gap: i64,
    },
    /// Shortens notes so they end no later than the next note of the same pitch and channel.
    RemoveOverlap,
}

/// Changes note lengths. See [`DurationOp`].
pub fn change_duration(notes: &mut [Note], op: DurationOp) {
    match op {
        DurationOp::Set(l) => notes.iter_mut().for_each(|n| n.length = l.max(1)),
        DurationOp::Add(d) => notes.iter_mut().for_each(|n| n.length = n.length.saturating_add(d).max(1)),
        DurationOp::Scale(f) => {
            if f.is_finite() {
                notes.iter_mut().for_each(|n| n.length = round_i64(n.length as f64 * f64::from(f)).max(1));
            }
        }
        DurationOp::Legato { gap } => {
            let mut starts: Vec<i64> = notes.iter().map(|n| n.start).collect();
            starts.sort_unstable();
            starts.dedup();
            for n in notes.iter_mut() {
                let idx = starts.partition_point(|&s| s <= n.start);
                if let Some(&next) = starts.get(idx) {
                    n.length = next.saturating_sub(gap).saturating_sub(n.start).max(1);
                }
            }
        }
        DurationOp::RemoveOverlap => {
            let mut order: Vec<usize> = (0..notes.len()).collect();
            order.sort_by_key(|&i| notes.get(i).map(|n| (n.channel, n.pitch, n.start)));
            for w in order.windows(2) {
                let (Some(&a), Some(&b)) = (w.first(), w.get(1)) else { continue };
                let Some(&nb) = notes.get(b) else { continue };
                if let Some(na) = notes.get_mut(a)
                    && na.channel == nb.channel
                    && na.pitch == nb.pitch
                    && na.end() > nb.start
                {
                    na.length = nb.start.saturating_sub(na.start).max(1);
                }
            }
        }
    }
}

/// Select/Split Notes by pitch: returns `(notes with pitch in pitch_from..=pitch_to, the rest)`,
/// both in input order. The bounds are swapped if reversed.
pub fn split_notes(notes: &[Note], pitch_from: u8, pitch_to: u8) -> (Vec<Note>, Vec<Note>) {
    let (lo, hi) = if pitch_from <= pitch_to { (pitch_from, pitch_to) } else { (pitch_to, pitch_from) };
    notes.iter().partition(|n| (lo..=hi).contains(&n.pitch))
}
