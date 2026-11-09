//! AudioSuite-style whole-buffer processing on planar `f32` audio.
//!
//! These functions may allocate (they are not for the audio thread) but never panic: lengths,
//! rates and ratios are sanitized and output sizes are capped at [`MAX_OFFLINE_SAMPLES`] per
//! channel.

use std::f64::consts::PI;

use rustfft::{FftPlanner, num_complex::Complex};

use crate::osc::Osc;
pub use crate::osc::Waveform;
use crate::util::sane_sr;
use crate::{Plugin, db_to_gain, gain_to_db};

/// Largest number of samples per channel any offline function will produce.
pub const MAX_OFFLINE_SAMPLES: usize = 1 << 28;

/// Fade curves. [`FadeShape::gain`] is the fade-in curve; a fade-out uses `gain(1 - t)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, serde::Serialize, serde::Deserialize)]
pub enum FadeShape {
    Linear,
    /// Constant power (sin/cos), for crossfades between uncorrelated material.
    #[default]
    EqualPower,
    /// Raised cosine.
    SCurve,
    /// Slow start, fast finish.
    Exponential,
    /// Fast start, slow finish.
    Logarithmic,
}

impl FadeShape {
    pub const ALL: [FadeShape; 5] = [FadeShape::Linear, FadeShape::EqualPower, FadeShape::SCurve, FadeShape::Exponential, FadeShape::Logarithmic];

    /// Fade-in gain at position `t` in 0..1 (0 → silent, 1 → unity). NaN is treated as 0.
    pub fn gain(self, t: f32) -> f32 {
        let t = if t.is_nan() { 0.0 } else { t.clamp(0.0, 1.0) };
        const K: f32 = 4.0;
        match self {
            FadeShape::Linear => t,
            FadeShape::EqualPower => (t * std::f32::consts::FRAC_PI_2).sin(),
            FadeShape::SCurve => 0.5 - 0.5 * (t * std::f32::consts::PI).cos(),
            FadeShape::Exponential => ((K * t).exp() - 1.0) / (K.exp() - 1.0),
            FadeShape::Logarithmic => (1.0 + (K.exp() - 1.0) * t).ln() / K,
        }
    }
}

fn max_len(ch: &[Vec<f32>]) -> usize {
    ch.iter().map(Vec::len).max().unwrap_or(0)
}

/// The largest finite sample magnitude in any channel.
pub fn peak(ch: &[Vec<f32>]) -> f32 {
    ch.iter().map(|c| channel_peak(c)).fold(0.0f32, f32::max)
}

/// Largest finite absolute sample of one channel (0 for none).
pub fn channel_peak(x: &[f32]) -> f32 {
    let mut lanes = [0.0f32; 8];
    let (chunks, rest) = x.as_chunks::<8>();
    for c in chunks {
        for (m, v) in lanes.iter_mut().zip(c) {
            let a = v.abs();
            *m = if a < f32::INFINITY && a > *m { a } else { *m };
        }
    }
    for (m, v) in lanes.iter_mut().zip(rest) {
        let a = v.abs();
        *m = if a < f32::INFINITY && a > *m { a } else { *m };
    }
    lanes.into_iter().fold(0.0f32, f32::max)
}

/// Peak-normalizes to `target_db` dBFS, either all channels together or each on its own.
/// Silent channels are left alone.
pub fn normalize(ch: &mut [Vec<f32>], target_db: f32, per_channel: bool) {
    let target = db_to_gain(if target_db.is_nan() { 0.0 } else { target_db.clamp(-144.0, 24.0) });
    if per_channel {
        for c in ch.iter_mut() {
            let p = peak(std::slice::from_ref(c));
            if p > 0.0 {
                scale_to(c.iter_mut(), target, p);
            }
        }
    } else {
        let p = peak(ch);
        if p > 0.0 {
            scale_to(ch.iter_mut().flatten(), target, p);
        }
    }
}

/// Scales samples whose peak is `p` to `target`. A subnormal `p` makes the f32 gain overflow to
/// infinity, so that case scales in f64.
fn scale_to<'a>(samples: impl Iterator<Item = &'a mut f32>, target: f32, p: f32) {
    let g = target / p;
    if g.is_finite() {
        samples.for_each(|s| *s *= g);
    } else {
        let g = f64::from(target) / f64::from(p);
        samples.for_each(|s| *s = (f64::from(*s) * g) as f32);
    }
}

/// Applies a gain change in dB.
pub fn gain(ch: &mut [Vec<f32>], db: f32) {
    let g = db_to_gain(if db.is_nan() { 0.0 } else { db.min(96.0) });
    ch.iter_mut().flatten().for_each(|s| *s *= g);
}

/// Flips polarity.
pub fn invert(ch: &mut [Vec<f32>]) {
    ch.iter_mut().flatten().for_each(|s| *s = -*s);
}

/// Reverses each channel in time.
pub fn reverse(ch: &mut [Vec<f32>]) {
    ch.iter_mut().for_each(|c| c.reverse());
}

/// Removes each channel's DC offset (subtracts its mean).
pub fn remove_dc(ch: &mut [Vec<f32>]) {
    for c in ch.iter_mut() {
        if c.is_empty() {
            continue;
        }
        let (sum, n) = c.iter().filter(|v| v.is_finite()).fold((0.0f64, 0usize), |(s, n), &v| (s + f64::from(v), n + 1));
        if n == 0 {
            continue;
        }
        let m = (sum / n as f64) as f32;
        c.iter_mut().for_each(|s| *s -= m);
    }
}

/// Fades in over the first `fade_in` samples and out over the last `fade_out` samples.
pub fn fade(ch: &mut [Vec<f32>], fade_in: usize, fade_out: usize, shape: FadeShape) {
    for c in ch.iter_mut() {
        let len = c.len();
        let fi = fade_in.min(len);
        for (i, s) in c.iter_mut().take(fi).enumerate() {
            *s *= shape.gain(i as f32 / fi as f32);
        }
        let fo = fade_out.min(len);
        for (j, s) in c.iter_mut().skip(len - fo).enumerate() {
            *s *= shape.gain((fo - 1 - j) as f32 / fo as f32);
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Resampling (Kaiser-windowed sinc)

fn bessel_i0(x: f64) -> f64 {
    let mut sum = 1.0;
    let mut term = 1.0;
    let y = x * x / 4.0;
    for k in 1..64 {
        term *= y / (k as f64 * k as f64);
        sum += term;
        if term < sum * 1e-12 {
            break;
        }
    }
    sum
}

const SINC_ZC: f64 = 32.0;
const SINC_RES: usize = 512;

/// Precomputed one-sided windowed-sinc kernel for a given cutoff (fraction of input Nyquist).
struct SincTable {
    table: Vec<f32>,
    half_width: f64,
}

impl SincTable {
    fn new(cutoff: f64) -> Self {
        let cutoff = cutoff.clamp(0.01, 1.0);
        let half_width = SINC_ZC / cutoff;
        let n = (half_width * SINC_RES as f64).ceil() as usize + 2;
        let beta = 9.0;
        let i0b = bessel_i0(beta);
        let table = (0..n)
            .map(|i| {
                let t = i as f64 / SINC_RES as f64;
                let x = t / half_width;
                if x >= 1.0 {
                    return 0.0;
                }
                let arg = PI * cutoff * t;
                let sinc = if arg.abs() < 1e-12 { 1.0 } else { arg.sin() / arg };
                let w = bessel_i0(beta * (1.0 - x * x).max(0.0).sqrt()) / i0b;
                (cutoff * sinc * w) as f32
            })
            .collect();
        Self { table, half_width }
    }

    #[inline]
    fn at(&self, t: f64) -> f32 {
        let pos = t.abs() * SINC_RES as f64;
        let i = pos as usize;
        let f = (pos - i as f64) as f32;
        match (self.table.get(i), self.table.get(i + 1)) {
            (Some(a), Some(b)) => a + (b - a) * f,
            (Some(a), None) => *a,
            _ => 0.0,
        }
    }
}

/// Resamples one channel to exactly `out_len` samples, where output sample `n` is read at input
/// time `n / ratio`.
fn resample_channel(x: &[f32], ratio: f64, out_len: usize, table: &SincTable) -> Vec<f32> {
    let len = x.len() as i64;
    let hw = table.half_width;
    (0..out_len)
        .map(|n| {
            let t = n as f64 / ratio;
            let lo = (t - hw).ceil().max(0.0) as i64;
            let hi = ((t + hw).floor() as i64).min(len - 1);
            let mut acc = 0.0f32;
            let mut j = lo;
            while j <= hi {
                if let Some(&v) = x.get(j as usize) {
                    acc += v * table.at(t - j as f64);
                }
                j += 1;
            }
            acc
        })
        .collect()
}

fn resample_ratio(ch: &[Vec<f32>], ratio: f64, out_len: usize) -> Vec<Vec<f32>> {
    if !ratio.is_finite() || ratio <= 0.0 {
        return ch.iter().map(|_| vec![0.0; out_len]).collect();
    }
    // Anti-alias below the lower of the two Nyquists, with a little transition room.
    let cutoff = if ratio < 1.0 { ratio * 0.94 } else { 0.97 };
    let table = SincTable::new(cutoff);
    ch.iter().map(|c| resample_channel(c, ratio, out_len, &table)).collect()
}

/// Most kernel weights a [`Polyphase`] bank may hold (16 MiB of f32).
const MAX_POLYPHASE_WEIGHTS: usize = 1 << 22;

/// The windowed-sinc kernel of [`resample_channel`] precomputed for every phase of a rational
/// rate change `from` → `to` (reduced to `step` / `phases`). Output sample `n` reads input time
/// `n * step / phases`: whole part `i`, phase `p`. Its taps are input samples `i + first ..`, one
/// weight each, so every output is one dot product over contiguous memory instead of a table
/// lookup and interpolation per tap. The weights are the ones `resample_channel` computes, taps
/// beyond the kernel's half width weigh 0, and taps are summed in the same order.
struct Polyphase {
    phases: u64,
    step: u64,
    /// Offset of the first tap from `i`.
    first: i64,
    taps: usize,
    /// `phases * taps` weights, phase-major.
    weights: Vec<f32>,
}

impl Polyphase {
    /// `None` when the bank would be too large (rates with no small common ratio); the caller
    /// then falls back to [`resample_channel`].
    fn new(from: u32, to: u32, table: &SincTable) -> Option<Polyphase> {
        let g = gcd(from, to);
        let (step, phases) = (u64::from(from / g), u64::from(to / g));
        let hw = table.half_width;
        let first = (-hw).floor() as i64;
        let last = hw.ceil() as i64 + 1;
        let taps = usize::try_from(last - first + 1).ok()?;
        if usize::try_from(phases).ok()?.checked_mul(taps)? > MAX_POLYPHASE_WEIGHTS {
            return None;
        }
        let mut weights = Vec::with_capacity(taps * phases as usize);
        for p in 0..phases {
            let frac = p as f64 / phases as f64;
            for k in first..first + taps as i64 {
                let d = frac - k as f64;
                weights.push(if d.abs() <= hw { table.at(d) } else { 0.0 });
            }
        }
        Some(Polyphase { phases, step, first, taps, weights })
    }

    fn run(&self, x: &[f32], out_len: usize) -> Vec<f32> {
        let len = x.len() as i64;
        let mut out = Vec::with_capacity(out_len);
        for n in 0..out_len as u64 {
            let Some(pos) = n.checked_mul(self.step) else { break };
            let (i, p) = ((pos / self.phases) as i64, (pos % self.phases) as usize);
            let w = self.weights.get(p * self.taps..(p + 1) * self.taps).unwrap_or_default();
            let start = i + self.first;
            let acc = match usize::try_from(start).ok().and_then(|s| x.get(s..s + self.taps)) {
                // Inside the signal: a plain dot product.
                Some(xs) => xs.iter().zip(w).fold(0.0f32, |a, (v, k)| a + v * k),
                // At the edges, taps outside the signal are skipped, as `resample_channel` does.
                None => (0..self.taps as i64)
                    .filter(|k| (0..len).contains(&(start + k)))
                    .fold(0.0f32, |a, k| a + x.get((start + k) as usize).copied().unwrap_or(0.0) * w.get(k as usize).copied().unwrap_or(0.0)),
            };
            out.push(acc);
        }
        out.resize(out_len, 0.0);
        out
    }
}

fn gcd(mut a: u32, mut b: u32) -> u32 {
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a.max(1)
}

/// High-quality windowed-sinc sample-rate conversion from `from` Hz to `to` Hz.
pub fn resample(ch: &[Vec<f32>], from: u32, to: u32) -> Vec<Vec<f32>> {
    if from == 0 || to == 0 || from == to {
        return ch.to_vec();
    }
    let ratio = f64::from(to) / f64::from(from);
    let cutoff = if ratio < 1.0 { ratio * 0.94 } else { 0.97 };
    let table = SincTable::new(cutoff);
    let bank = Polyphase::new(from, to, &table);
    ch.iter()
        .map(|c| {
            let out_len = ((c.len() as f64 * ratio).round() as usize).min(MAX_OFFLINE_SAMPLES);
            match &bank {
                Some(b) => b.run(c, out_len),
                None => resample_channel(c, ratio, out_len, &table),
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------------------------
// Time stretch (WSOLA) and pitch shift

/// Start position in the guide signal whose `hop` samples best match the natural continuation at
/// `natural`, searched within `tol` of `nominal` (coarse stride 4, then refined +-3).
fn best_start(g: &impl Fn(i64) -> f32, nominal: i64, natural: i64, tol: i64, hop: usize) -> i64 {
    // Similarity of candidate start `p` with the natural continuation (stride 2).
    let score = |p: i64| -> f32 {
        let (mut xy, mut yy) = (0.0f32, 1e-9f32);
        let mut i = 0i64;
        while i < hop as i64 {
            let a = g(natural + i);
            let b = g(p + i);
            xy += a * b;
            yy += b * b;
            i += 2;
        }
        xy / yy.sqrt()
    };
    let lo = (nominal - tol).max(0);
    let hi = nominal + tol;
    let mut best = nominal.max(0);
    let mut best_s = f32::NEG_INFINITY;
    let mut p = lo;
    while p <= hi {
        let s = score(p);
        if s > best_s {
            best_s = s;
            best = p;
        }
        p += 4;
    }
    let (rlo, rhi) = ((best - 3).max(lo), (best + 3).min(hi));
    for p in rlo..=rhi {
        let s = score(p);
        if s > best_s {
            best_s = s;
            best = p;
        }
    }
    best
}

/// Add one windowed frame of every channel, read from `at.0`, into `out` at `at.1`, and the window
/// into `wsum`.
fn overlap_add(ch: &[Vec<f32>], out: &mut [Vec<f32>], wsum: &mut [f32], window: &[f32], at: (usize, usize)) {
    let (in_pos, out_pos) = at;
    for (c, o) in ch.iter().zip(out.iter_mut()) {
        for (i, w) in window.iter().enumerate() {
            let v = c.get(in_pos + i).copied().unwrap_or(0.0);
            if let Some(d) = o.get_mut(out_pos + i) {
                *d += v * w;
            }
        }
    }
    for (i, w) in window.iter().enumerate() {
        if let Some(d) = wsum.get_mut(out_pos + i) {
            *d += w;
        }
    }
}

/// Divide out the accumulated window weights and cut each channel to `out_len`.
fn normalise(out: &mut [Vec<f32>], wsum: &[f32], out_len: usize) {
    for o in out.iter_mut() {
        for (s, w) in o.iter_mut().zip(wsum) {
            if *w > 1e-3 {
                *s /= w;
            }
        }
        o.truncate(out_len);
    }
}

/// Pitch-preserving time stretch by WSOLA (waveform-similarity overlap-add).
/// `ratio` = output length / input length, clamped to 0.1 ..= 10. All channels share the same
/// segment choices so the stereo image stays coherent.
pub fn time_stretch(ch: &[Vec<f32>], ratio: f64, sample_rate: f32) -> Vec<Vec<f32>> {
    let len = max_len(ch);
    let ratio = if ratio.is_finite() { ratio.clamp(0.1, 10.0) } else { 1.0 };
    let out_len = ((len as f64 * ratio).round() as usize).min(MAX_OFFLINE_SAMPLES);
    if len == 0 || ch.is_empty() {
        return ch.iter().map(|_| vec![0.0; out_len]).collect();
    }
    if (ratio - 1.0).abs() < 1e-9 {
        return ch.iter().map(|c| c.iter().copied().chain(std::iter::repeat(0.0)).take(out_len).collect()).collect();
    }
    let sr = f64::from(sane_sr(sample_rate));
    let frame = (((sr * 0.04) as usize) & !1).clamp(64, 8192);
    let hop = frame / 2;
    let tol = ((sr * 0.010) as usize).clamp(8, 2048) as i64;
    let window: Vec<f32> = (0..frame).map(|i| (0.5 - 0.5 * (2.0 * PI * i as f64 / frame as f64).cos()) as f32).collect();
    let nch = ch.len() as f32;
    let guide: Vec<f32> = (0..len).map(|i| ch.iter().map(|c| c.get(i).copied().unwrap_or(0.0)).sum::<f32>() / nch).collect();
    let g = |i: i64| -> f32 { if i < 0 { 0.0 } else { guide.get(i as usize).copied().unwrap_or(0.0) } };

    let buf_len = out_len + frame;
    let mut out: Vec<Vec<f32>> = ch.iter().map(|_| vec![0.0; buf_len]).collect();
    let mut wsum = vec![0.0f32; buf_len];
    let mut prev_in: i64 = 0;
    let mut k = 0usize;
    loop {
        let out_pos = k * hop;
        if out_pos >= out_len {
            break;
        }
        let in_pos = if k == 0 { 0 } else { best_start(&g, (out_pos as f64 / ratio).round() as i64, prev_in + hop as i64, tol, hop) };
        overlap_add(ch, &mut out, &mut wsum, &window, (in_pos as usize, out_pos));
        prev_in = in_pos;
        k += 1;
    }
    normalise(&mut out, &wsum, out_len);
    out
}

/// Length-preserving pitch shift by `semitones` (clamped to ±48): WSOLA stretch followed by
/// windowed-sinc resampling back to the original length.
pub fn pitch_shift(ch: &[Vec<f32>], semitones: f32, sample_rate: f32) -> Vec<Vec<f32>> {
    let st = if semitones.is_finite() { semitones.clamp(-48.0, 48.0) } else { 0.0 };
    if st.abs() < 1e-4 {
        return ch.to_vec();
    }
    let factor = 2f64.powf(f64::from(st) / 12.0);
    // time_stretch clamps to 0.1..=10, short of the 16x (or 1/16) that ±48 needs: stretch twice by
    // the square root instead.
    let stretched = if (0.1..=10.0).contains(&factor) {
        time_stretch(ch, factor, sample_rate)
    } else {
        let half = factor.sqrt();
        time_stretch(&time_stretch(ch, half, sample_rate), half, sample_rate)
    };
    let len = max_len(ch);
    let slen = max_len(&stretched);
    if slen == 0 || len == 0 {
        return ch.iter().map(|c| vec![0.0; c.len()]).collect();
    }
    let ratio = len as f64 / slen as f64;
    let mut out = resample_ratio(&stretched, ratio, len);
    for (o, c) in out.iter_mut().zip(ch) {
        o.truncate(c.len());
    }
    out
}

/// Which way [`speed_ramp`] ramps the playback speed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpeedRampChange {
    /// From normal speed down to a complete stop.
    SlowDown,
    /// From a complete stop up to normal speed.
    SpeedUp,
}

/// How much of the input [`speed_ramp`] plays, and how long the result is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpeedRampSelection {
    /// The result is as long as the input. Two thirds of the input are played: the first two
    /// thirds for Slow Down, the last two thirds for Speed Up, so each joins its neighbour
    /// without a jump.
    FitTo,
    /// All of the input is played, and the result is 150 % of its length.
    Extend,
}

/// Plays `ch` with the speed ramping between normal speed and a complete stop, so the pitch follows
/// the speed. The speed is `1 - u²` for Slow Down and `1 - (1 - u)²` for Speed Up, where `u`
/// runs from 0 to 1 over the result; both curves average 2/3 of normal speed. With `fades`, the
/// gain follows the same curve: a fade-out for Slow Down and a fade-in for Speed Up. Every channel
/// is read at the same positions, and a channel shorter than the others is silent past its end.
pub fn speed_ramp(ch: &[Vec<f32>], change: SpeedRampChange, selection: SpeedRampSelection, fades: bool) -> Vec<Vec<f32>> {
    let len = max_len(ch) as f64;
    let (total, first, played) = match selection {
        SpeedRampSelection::Extend => ((len * 1.5).round(), 0.0, len),
        SpeedRampSelection::FitTo => {
            let played = len * 2.0 / 3.0;
            (len, if change == SpeedRampChange::SlowDown { 0.0 } else { len - played }, played)
        }
    };
    let out_len = (total as usize).min(MAX_OFFLINE_SAMPLES);
    let table = SincTable::new(0.97);
    let hw = table.half_width;
    // The fraction of the played input reached at `u`, and the speed there (0 to 1).
    let at = |u: f64| -> (f64, f64) {
        match change {
            SpeedRampChange::SlowDown => ((3.0 * u - u * u * u) / 2.0, 1.0 - u * u),
            SpeedRampChange::SpeedUp => {
                let r = 1.0 - u;
                (1.0 - (3.0 * r - r * r * r) / 2.0, 1.0 - r * r)
            }
        }
    };
    // The interpolation reads the first or last sample of the input for taps before or after it.
    let last = (len as i64 - 1).max(0);
    ch.iter()
        .map(|x| {
            (0..out_len)
                .map(|n| {
                    let (pos, speed) = at(n as f64 / total);
                    let t = first + played * pos;
                    let lo = (t - hw).ceil() as i64;
                    let hi = (t + hw).floor() as i64;
                    let mut acc = 0.0f32;
                    let mut j = lo;
                    while j <= hi {
                        if let Some(&v) = x.get(j.clamp(0, last) as usize) {
                            acc += v * table.at(t - j as f64);
                        }
                        j += 1;
                    }
                    if fades { acc * speed as f32 } else { acc }
                })
                .collect()
        })
        .collect()
}

// ---------------------------------------------------------------------------------------------
// Generators and analysis

/// Generates a test signal. Periodic waveforms peak at `level_db`; noise is scaled so its peak
/// stays at or below `level_db`.
pub fn signal_generator(kind: Waveform, freq: f32, level_db: f32, sample_rate: f32, frames: usize, channels: usize) -> Vec<Vec<f32>> {
    let sr = sane_sr(sample_rate);
    let frames = frames.min(MAX_OFFLINE_SAMPLES);
    let channels = channels.min(crate::util::MAX_CHANNELS);
    let f = if freq.is_finite() { freq.clamp(0.0, sr * 0.49) } else { 1000.0 };
    let g = db_to_gain(if level_db.is_nan() { -20.0 } else { level_db.min(0.0) });
    let mut osc = Osc::new(0x1234_5678);
    let mono: Vec<f32> = (0..frames).map(|_| osc.next(kind, f / sr) * g).collect();
    (0..channels).map(|_| mono.clone()).collect()
}

/// Onset detection: how much a log-magnitude spectrum on 24 bands per octave rises against the frame
/// before, each band against the loudest of its neighbours there so vibrato doesn't count
/// (vibrato-suppressed spectral flux). A peak must stand out from the flux within 0.1 s by a fixed
/// amount, not relative to the loudest hit, so quiet hits count and the result doesn't depend on where
/// the analysed audio starts. Each is placed where its attack starts. `sensitivity` 0..1 (higher finds
/// more). Returns sample positions, ascending.
pub fn detect_transients(ch: &[Vec<f32>], sample_rate: f32, sensitivity: f32) -> Vec<usize> {
    let len = max_len(ch);
    let sr = sane_sr(sample_rate);
    let sens = if sensitivity.is_finite() { sensitivity.clamp(0.0, 1.0) } else { 0.5 };
    let n = ((sr * 0.012) as usize).clamp(64, 8192).next_power_of_two();
    let hop = n / 4;
    if len < n || ch.is_empty() {
        return Vec::new();
    }
    let ch: Vec<Vec<f32>> =
        ch.iter().take(crate::util::MAX_CHANNELS).map(|c| c.iter().map(|v| if v.is_finite() { *v } else { 0.0 }).collect()).collect();
    let nch = ch.len() as f32;
    let bands = log_bands(n, sr);
    let fft = FftPlanner::<f32>::new().plan_fft_forward(n);
    let window: Vec<f32> = (0..n).map(|i| (0.5 - 0.5 * (2.0 * PI * i as f64 / n as f64).cos()) as f32).collect();
    // Magnitudes relative to a full-scale sine; below about -80 dB a band's changes barely count.
    let gain = 1.0e4 / (n as f32 / 4.0);
    let frames = (len - n) / hop + 1;
    let mut buf = vec![Complex::new(0.0f32, 0.0); n];
    let mut power = vec![0.0f32; n / 2 + 1];
    let mut prev: Vec<f32> = Vec::new();
    let mut flux = Vec::with_capacity(frames);
    let mut energy: Vec<f32> = Vec::with_capacity(frames);
    for m in 0..frames {
        power.iter_mut().for_each(|p| *p = 0.0);
        for c in &ch {
            for (i, b) in buf.iter_mut().enumerate() {
                *b = Complex::new(c.get(m * hop + i).copied().unwrap_or(0.0) * window.get(i).copied().unwrap_or(0.0), 0.0);
            }
            fft.process(&mut buf);
            for (p, b) in power.iter_mut().zip(&buf) {
                *p += b.norm_sqr() / nch;
            }
        }
        energy.push(power.iter().sum());
        let cur: Vec<f32> = bands
            .iter()
            .map(|band| (1.0 + gain * band.iter().map(|&(k, w)| power.get(k).map_or(0.0, |p| p.sqrt()) * w).sum::<f32>()).log10())
            .collect();
        let rise = |b: usize, v: f32| {
            (v - prev.get(b.saturating_sub(1)..(b + 2).min(prev.len())).map_or(0.0, |s| s.iter().copied().fold(0.0, f32::max))).max(0.0)
        };
        flux.push(if prev.is_empty() { 0.0 } else { cur.iter().enumerate().map(|(b, v)| rise(b, *v)).sum() });
        prev = cur;
    }
    let frames_of = |secs: f32| ((secs * sr) as usize / hop).max(1);
    let (near, before, after, gap) = (frames_of(0.03), frames_of(0.1), frames_of(0.07), frames_of(0.03));
    let delta = 2.5 * 2f32.powf((0.5 - sens) * 4.0);
    let mut out: Vec<usize> = Vec::new();
    let mut last: Option<usize> = None;
    for (m, &cur) in flux.iter().enumerate() {
        let span = |a: usize, b: usize| flux.get(m.saturating_sub(a)..(m + b + 1).min(frames)).unwrap_or(&[]);
        let peak = span(near, near).iter().copied().fold(0.0, f32::max);
        let around = span(before, after);
        let mean = around.iter().sum::<f32>() / around.len().max(1) as f32;
        if cur <= 0.0 || cur < peak || cur < mean + delta || last.is_some_and(|l| m - l < gap) {
            continue;
        }
        let (at, contrast) = attack_start(&ch, m * hop, n, sr);
        // A fast release smears energy between a note's partials, which looks like a rise. Its
        // attack edge is weak and the energy doesn't grow, unlike a real onset's.
        let was = energy.get(m.saturating_sub(1)).copied().unwrap_or(0.0);
        let grows = energy.get(m..(m + 2).min(frames)).is_some_and(|e| e.iter().any(|v| *v > was));
        if contrast < std::f64::consts::LN_2 && !grows {
            continue;
        }
        last = Some(m);
        if out.last().is_none_or(|&p| at > p) {
            out.push(at);
        }
    }
    out
}

/// Triangular filters 24 to the octave from 30 Hz to 20 kHz over the bins of an `n`-point FFT, without
/// duplicates where bins are coarser than that; each filter's weights sum to 1.
fn log_bands(n: usize, sr: f32) -> Vec<Vec<(usize, f32)>> {
    let bin_hz = sr / n as f32;
    let top = 20_000.0f32.min(sr * 0.45);
    let mut centres: Vec<usize> = Vec::new();
    let mut f = 30.0f32;
    while f <= top {
        let k = (f / bin_hz).round() as usize;
        if centres.last().is_none_or(|&c| k > c) {
            centres.push(k);
        }
        f *= 2f32.powf(1.0 / 24.0);
    }
    centres
        .windows(3)
        .filter_map(|w| {
            let (&lo, &mid, &hi) = (w.first()?, w.get(1)?, w.get(2)?);
            let tri: Vec<(usize, f32)> = (lo..=hi)
                .map(|k| (k, if k <= mid { (k - lo) as f32 / (mid - lo).max(1) as f32 } else { (hi - k) as f32 / (hi - mid).max(1) as f32 }))
                .filter(|(_, v)| *v > 0.0)
                .collect();
            let sum: f32 = tri.iter().map(|(_, v)| v).sum();
            (sum > 0.0).then(|| tri.into_iter().map(|(k, v)| (k, v / sum)).collect())
        })
        .collect()
}

/// Where the attack found in the frame starting at `frame` begins. The log spectrum jumps as soon as the
/// window reaches an attack, so it can sit up to half a window past the frame. Find the step edge of the
/// high-passed energy, the sample where the next 3 ms gain most over the 6 ms before, then go back to
/// where the energy first crosses the midpoint (in dB) between the two.
fn attack_start(ch: &[Vec<f32>], frame: usize, n: usize, sr: f32) -> (usize, f64) {
    let (before, after, short) = (((sr * 0.006) as usize).max(1), ((sr * 0.003) as usize).max(1), ((sr * 0.0005) as usize).max(1));
    let (from, to) = (frame.saturating_sub(before), frame + n + n / 2 + after);
    let mut cum = vec![0.0f64; to - from + 1];
    for (j, i) in (from..to).enumerate() {
        let e: f32 = ch
            .iter()
            .map(|x| {
                let d = x.get(i).copied().unwrap_or(0.0) - i.checked_sub(1).and_then(|p| x.get(p)).copied().unwrap_or(0.0);
                d * d
            })
            .sum();
        let sum = cum.get(j).copied().unwrap_or(0.0) + f64::from(e);
        if let Some(c) = cum.get_mut(j + 1) {
            *c = sum;
        }
    }
    let at = |i: usize| cum.get(i.saturating_sub(from)).copied().unwrap_or(0.0);
    let mean = |a: usize, b: usize| (at(b) - at(a)) / b.saturating_sub(a).max(1) as f64;
    let contrast = |k: usize| ((mean(k, k + after) + 1e-12) / (mean(k.saturating_sub(before), k) + 1e-12)).ln();
    let k = (frame..frame + n + n / 2).max_by(|&a, &b| contrast(a).total_cmp(&contrast(b))).unwrap_or(frame);
    let level = (mean(k.saturating_sub(before), k) * mean(k, k + after)).sqrt();
    ((k.saturating_sub(after)..=k).find(|&i| mean(i, i + short) > level).unwrap_or(k), contrast(k))
}

/// Strip Silence: ranges `(start, end)` (end exclusive) of material whose peak across channels
/// exceeds `threshold_db`. Silent gaps shorter than `min_len` samples do not split a region.
/// Each region is padded by `pad_before`/`pad_after` (clamped to the buffer) and overlapping
/// regions are merged.
pub fn non_silent_ranges(ch: &[Vec<f32>], threshold_db: f32, min_len: usize, pad_before: usize, pad_after: usize) -> Vec<(usize, usize)> {
    let mut scan = SilenceScan::new(threshold_db, min_len as u64);
    scan.push(ch);
    let at = |x: u64| usize::try_from(x).unwrap_or(usize::MAX);
    scan.finish(pad_before as u64, pad_after as u64).into_iter().map(|(s, e)| (at(s), at(e))).collect()
}

/// [`non_silent_ranges`] over audio that arrives a block at a time, so material of any length can
/// be scanned without holding all of it.
pub struct SilenceScan {
    thr: f32,
    min_len: u64,
    len: u64,
    start: Option<u64>,
    last_loud: u64,
    raw: Vec<(u64, u64)>,
}

impl SilenceScan {
    pub fn new(threshold_db: f32, min_len: u64) -> Self {
        let thr = db_to_gain(if threshold_db.is_nan() { -48.0 } else { threshold_db });
        SilenceScan { thr, min_len, len: 0, start: None, last_loud: 0, raw: Vec::new() }
    }

    /// Scan the next block (planar), which follows the audio scanned so far.
    pub fn push(&mut self, ch: &[Vec<f32>]) {
        let n = max_len(ch);
        for k in 0..n {
            if !ch.iter().any(|c| c.get(k).is_some_and(|v| v.is_finite() && v.abs() > self.thr)) {
                continue;
            }
            let i = self.len.saturating_add(k as u64);
            match self.start {
                Some(s) if i - self.last_loud > self.min_len.max(1) => {
                    self.raw.push((s, self.last_loud.saturating_add(1)));
                    self.start = Some(i);
                }
                None => self.start = Some(i),
                _ => {}
            }
            self.last_loud = i;
        }
        self.len = self.len.saturating_add(n as u64);
    }

    /// Pass over `n` samples known to be silent.
    pub fn skip(&mut self, n: u64) {
        self.len = self.len.saturating_add(n);
    }

    /// The regions found, padded and merged (see [`non_silent_ranges`]).
    pub fn finish(mut self, pad_before: u64, pad_after: u64) -> Vec<(u64, u64)> {
        if let Some(s) = self.start {
            self.raw.push((s, self.last_loud.saturating_add(1)));
        }
        let mut out: Vec<(u64, u64)> = Vec::with_capacity(self.raw.len());
        for (s, e) in self.raw {
            let s = s.saturating_sub(pad_before);
            let e = e.saturating_add(pad_after).min(self.len);
            match out.last_mut() {
                Some(last) if s <= last.1 => last.1 = last.1.max(e),
                _ => out.push((s, e)),
            }
        }
        out
    }
}

/// Runs a plugin over whole buffers in blocks, compensating its latency so the output lines up
/// with the input. The buffer length is unchanged (any tail past the end is dropped; see
/// [`apply_plugin_with_tail`]).
pub fn apply_plugin(ch: &mut [Vec<f32>], plugin: &mut dyn Plugin, sample_rate: f32) {
    let len = max_len(ch);
    let out = render(ch, plugin, sample_rate, len);
    for (c, o) in ch.iter_mut().zip(out) {
        for (d, s) in c.iter_mut().zip(o) {
            *d = s;
        }
    }
}

/// Like [`apply_plugin`] but returns new buffers extended by the plugin's tail (capped at 60 s).
pub fn apply_plugin_with_tail(ch: &[Vec<f32>], plugin: &mut dyn Plugin, sample_rate: f32) -> Vec<Vec<f32>> {
    let len = max_len(ch);
    plugin.prepare(sane_sr(sample_rate), 1024, ch.len());
    let tail = plugin.tail_samples().min((sane_sr(sample_rate) * 60.0) as usize);
    render(ch, plugin, sample_rate, (len + tail).min(MAX_OFFLINE_SAMPLES))
}

fn render(ch: &[Vec<f32>], plugin: &mut dyn Plugin, sample_rate: f32, out_len: usize) -> Vec<Vec<f32>> {
    const BLOCK: usize = 1024;
    let sr = sane_sr(sample_rate);
    let channels = ch.len();
    plugin.prepare(sr, BLOCK, channels);
    plugin.reset();
    let lat = plugin.latency().min(MAX_OFFLINE_SAMPLES);
    let mut out: Vec<Vec<f32>> = (0..channels).map(|_| vec![0.0; out_len]).collect();
    let mut scratch: Vec<Vec<f32>> = (0..channels).map(|_| vec![0.0; BLOCK]).collect();
    let total = out_len + lat;
    let mut done = 0usize;
    while done < total {
        let n = BLOCK.min(total - done);
        for (s, c) in scratch.iter_mut().zip(ch) {
            for (i, v) in s.iter_mut().enumerate() {
                *v = if i < n { c.get(done + i).copied().unwrap_or(0.0) } else { 0.0 };
            }
        }
        plugin.process(&mut scratch, n);
        for (s, o) in scratch.iter().zip(out.iter_mut()) {
            for (i, v) in s.iter().take(n).enumerate() {
                let idx = done + i;
                if idx >= lat
                    && let Some(d) = o.get_mut(idx - lat)
                {
                    *d = if v.is_finite() { *v } else { 0.0 };
                }
            }
        }
        done += n;
    }
    out
}

/// Peak level of the buffers in dBFS (-144 for silence).
pub fn peak_db(ch: &[Vec<f32>]) -> f32 {
    gain_to_db(peak(ch))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The polyphase bank gives what the per-tap kernel gives (it only moves where the weights
    /// are computed), at common rate pairs, both directions, and at the signal's edges.
    #[test]
    fn polyphase_resample_matches_the_per_tap_kernel() {
        let x: Vec<f32> = (0..3_000).map(|i| (i as f32 * 0.071).sin() * 0.6 + (i as f32 * 0.53).sin() * 0.3).collect();
        let pairs = [(44_100, 48_000), (48_000, 44_100), (48_000, 96_000), (96_000, 48_000), (22_050, 48_000), (8_000, 44_100), (48_000, 22_050)];
        for (from, to) in pairs {
            let ratio = f64::from(to) / f64::from(from);
            let out_len = ((x.len() as f64 * ratio).round() as usize).min(MAX_OFFLINE_SAMPLES);
            let cutoff = if ratio < 1.0 { ratio * 0.94 } else { 0.97 };
            let table = SincTable::new(cutoff);
            let reference = resample_channel(&x, ratio, out_len, &table);
            let bank = Polyphase::new(from, to, &table).expect("small common ratio");
            let fast = bank.run(&x, out_len);
            assert_eq!(fast.len(), reference.len(), "{from}->{to}");
            let worst = fast.iter().zip(&reference).map(|(a, b)| (a - b).abs()).fold(0.0f32, f32::max);
            assert!(worst < 1e-5, "{from}->{to}: worst difference {worst}");
            assert_eq!(resample(std::slice::from_ref(&x), from, to)[0], fast, "resample uses the bank");
        }
        // Signals shorter than the kernel are all edge.
        let short = [0.5f32, -0.25, 0.75];
        let table = SincTable::new(0.97);
        let reference = resample_channel(&short, 48_000.0 / 44_100.0, 3, &table);
        let fast = Polyphase::new(44_100, 48_000, &table).expect("bank").run(&short, 3);
        assert!(fast.iter().zip(&reference).all(|(a, b)| (a - b).abs() < 1e-6), "{fast:?} {reference:?}");
        // Rates with no small common ratio fall back to the per-tap kernel.
        assert!(Polyphase::new(44_100, 96_001, &table).is_none());
        assert_eq!(resample(&[short.to_vec()], 44_100, 96_001)[0].len(), 7);
    }

    #[test]
    fn fade_shapes_endpoints() {
        for s in FadeShape::ALL {
            assert!(s.gain(0.0).abs() < 1e-6, "{s:?}");
            assert!((s.gain(1.0) - 1.0).abs() < 1e-5, "{s:?}");
            assert!(s.gain(f32::NAN) == 0.0);
        }
    }

    #[test]
    fn fade_hits_zero_at_edges() {
        let mut ch = vec![vec![1.0f32; 100]];
        fade(&mut ch, 10, 10, FadeShape::Linear);
        assert_eq!(ch[0][0], 0.0);
        assert_eq!(ch[0][99], 0.0);
        assert_eq!(ch[0][50], 1.0);
        fade(&mut ch, 1000, 1000, FadeShape::SCurve);
    }

    #[test]
    fn remove_dc_ignores_non_finite_samples_in_the_mean() {
        let mut ch = vec![vec![f32::NAN, 1.0, 1.0, 1.0]];
        remove_dc(&mut ch);
        assert!(ch[0][1..].iter().all(|x| x.abs() < 1e-6), "left over dc {:?}", ch[0]);
        let mut all_bad = vec![vec![f32::NAN, f32::INFINITY]];
        remove_dc(&mut all_bad);
        assert!(all_bad[0][0].is_nan() && all_bad[0][1].is_infinite());
    }
}
