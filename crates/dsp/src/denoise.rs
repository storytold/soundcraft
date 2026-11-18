//! Noise reduction from a learned noise profile.
//!
//! [`NoiseProfile::learn`] measures the average spectrum of a passage that holds only the
//! background noise (hiss, hum, room tone). [`reduce`] then removes that noise from other audio by
//! short-time spectral subtraction: each frequency bin whose level is close to the learned noise
//! is turned down, never by more than the reduction amount, and bins well above it pass through.
//! Gains are smoothed across neighbouring bins and over time, which keeps the residual noise from
//! turning into "musical" chirps.
//!
//! The analysis is a textbook short-time Fourier transform: a square-root Hann window on both
//! analysis and synthesis, 75 % overlap, overlap-add. With every gain at 1 the output equals the
//! input. Like the rest of [`crate::offline`], these functions allocate and never panic.

use rustfft::{FftPlanner, num_complex::Complex};

use crate::{db_to_gain, gain_to_db};

/// FFT size of the analysis, in samples (≈ 43 ms at 48 kHz).
pub const FFT_SIZE: usize = 2048;
/// Distance between analysis frames: a quarter of the frame, 75 % overlap.
const HOP: usize = FFT_SIZE / 4;
/// Frequency bins in a profile: `FFT_SIZE / 2 + 1`.
pub const BINS: usize = FFT_SIZE / 2 + 1;
/// Most channels a profile holds.
pub const MAX_PROFILE_CHANNELS: usize = 64;
/// Most samples per channel that learning reads; a longer passage is analysed from its start.
pub const MAX_LEARN_SAMPLES: usize = 1 << 24;
/// The shortest passage that can be learned: one analysis frame.
pub const MIN_LEARN_SAMPLES: usize = FFT_SIZE;

/// The learned spectrum of a passage of background noise.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct NoiseProfile {
    /// Sample rate of the audio it was learned from, in Hz.
    pub sample_rate: u32,
    /// FFT size it was learned with; [`FFT_SIZE`] for every profile this crate makes.
    pub fft_size: usize,
    /// Length of the learned passage, in samples per channel.
    pub frames: u64,
    /// Mean magnitude of the noise in each frequency bin, per channel (`[channel][bin]`).
    pub magnitudes: Vec<Vec<f32>>,
}

/// How strongly [`reduce`] works.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ReduceSettings {
    /// Most a bin is turned down, in dB (0 leaves the audio as it is, 48 at most).
    pub reduction_db: f32,
    /// How far above the learned noise level a bin still counts as noise, in dB (0..=24). Higher
    /// removes more noise and risks more of the wanted sound.
    pub sensitivity_db: f32,
    /// Smoothing of the gains across frequency and time, 0 (none) to 1 (most).
    pub smoothing: f32,
}

impl Default for ReduceSettings {
    fn default() -> Self {
        ReduceSettings { reduction_db: 12.0, sensitivity_db: 6.0, smoothing: 0.5 }
    }
}

impl ReduceSettings {
    /// The settings with every value in range: NaN takes the default, the rest is clamped.
    pub fn sanitized(self) -> Self {
        let d = ReduceSettings::default();
        let fix = |v: f32, def: f32, lo: f32, hi: f32| if v.is_nan() { def } else { v.clamp(lo, hi) };
        ReduceSettings {
            reduction_db: fix(self.reduction_db, d.reduction_db, 0.0, 48.0),
            sensitivity_db: fix(self.sensitivity_db, d.sensitivity_db, 0.0, 24.0),
            smoothing: fix(self.smoothing, d.smoothing, 0.0, 1.0),
        }
    }
}

/// `window[i]`: the square root of a periodic Hann window, used for analysis and synthesis alike.
fn window() -> Vec<f32> {
    (0..FFT_SIZE).map(|i| (0.5 - 0.5 * (std::f64::consts::TAU * i as f64 / FFT_SIZE as f64).cos()).sqrt() as f32).collect()
}

fn finite(x: f32) -> f32 {
    if x.is_finite() { x } else { 0.0 }
}

impl NoiseProfile {
    /// Learns the noise in `ch` (planar audio at `sample_rate`). `None` when there is no audio,
    /// more than [`MAX_PROFILE_CHANNELS`] channels, or fewer than [`MIN_LEARN_SAMPLES`] samples.
    pub fn learn(ch: &[Vec<f32>], sample_rate: u32) -> Option<NoiseProfile> {
        if ch.is_empty() || ch.len() > MAX_PROFILE_CHANNELS || sample_rate == 0 {
            return None;
        }
        let len = ch.iter().map(Vec::len).min().unwrap_or(0).min(MAX_LEARN_SAMPLES);
        if len < MIN_LEARN_SAMPLES {
            return None;
        }
        let w = window();
        let fft = FftPlanner::<f32>::new().plan_fft_forward(FFT_SIZE);
        let mut buf = vec![Complex::new(0.0f32, 0.0); FFT_SIZE];
        let mut magnitudes = Vec::with_capacity(ch.len());
        for x in ch {
            let mut sum = vec![0.0f64; BINS];
            let mut count = 0u32;
            let mut pos = 0;
            while pos + FFT_SIZE <= len {
                for (i, b) in buf.iter_mut().enumerate() {
                    let s = x.get(pos + i).copied().map_or(0.0, finite);
                    *b = Complex::new(s * w.get(i).copied().unwrap_or(0.0), 0.0);
                }
                fft.process(&mut buf);
                for (s, b) in sum.iter_mut().zip(&buf) {
                    *s += f64::from(b.norm());
                }
                count += 1;
                pos += HOP;
            }
            let n = f64::from(count.max(1));
            magnitudes.push(sum.iter().map(|s| (s / n) as f32).collect());
        }
        Some(NoiseProfile { sample_rate, fft_size: FFT_SIZE, frames: len as u64, magnitudes })
    }

    /// Channels in the profile.
    pub fn channels(&self) -> usize {
        self.magnitudes.len()
    }

    /// Length of the learned passage in seconds.
    pub fn seconds(&self) -> f64 {
        if self.sample_rate == 0 { 0.0 } else { self.frames as f64 / f64::from(self.sample_rate) }
    }

    /// Whether the profile can be used: it was made with [`FFT_SIZE`], has 1 to
    /// [`MAX_PROFILE_CHANNELS`] channels of [`BINS`] bins, and every magnitude is finite and not
    /// negative. Check this on a profile read from outside (a file, a command) before using it.
    pub fn is_valid(&self) -> bool {
        self.fft_size == FFT_SIZE
            && self.sample_rate > 0
            && (1..=MAX_PROFILE_CHANNELS).contains(&self.magnitudes.len())
            && self.magnitudes.iter().all(|c| c.len() == BINS && c.iter().all(|m| m.is_finite() && *m >= 0.0))
    }

    /// The noise level in each bin in dBFS, averaged over the channels: a full-scale sine reads
    /// about 0 dB. For displays.
    pub fn spectrum_db(&self) -> Vec<f32> {
        let n = self.magnitudes.len().max(1) as f32;
        // The square-root Hann window sums to 2N/π; a sine's energy splits across ±f.
        let scale = std::f32::consts::PI / FFT_SIZE as f32;
        (0..BINS).map(|k| gain_to_db(self.magnitudes.iter().filter_map(|c| c.get(k)).sum::<f32>() / n * scale)).collect()
    }
}

/// Removes the noise described by `profile` from `ch`, returning audio of the same shape.
///
/// Channel `c` uses the profile's channel `c`, or its last channel when it has fewer (a mono
/// profile serves every channel). An invalid profile (see [`NoiseProfile::is_valid`]) returns the
/// audio unchanged. Non-finite samples are read as silence.
pub fn reduce(ch: &[Vec<f32>], profile: &NoiseProfile, settings: ReduceSettings) -> Vec<Vec<f32>> {
    if !profile.is_valid() {
        return ch.to_vec();
    }
    let s = settings.sanitized();
    let floor = db_to_gain(-s.reduction_db);
    let over = db_to_gain(s.sensitivity_db);
    // Up to four bins either side, and a release that holds a bin open for a few frames.
    let radius = (s.smoothing * 4.0).round() as usize;
    let release = s.smoothing * 0.9;
    let w = window();
    let mut planner = FftPlanner::<f32>::new();
    let fwd = planner.plan_fft_forward(FFT_SIZE);
    let inv = planner.plan_fft_inverse(FFT_SIZE);
    let mut buf = vec![Complex::new(0.0f32, 0.0); FFT_SIZE];
    let mut raw = vec![0.0f32; BINS];
    let mut gains = vec![1.0f32; BINS];
    let mut prev = vec![1.0f32; BINS];
    // Zeros before the audio so its first sample is covered by four frames, like every other.
    let pre = FFT_SIZE - HOP;
    let mut out = Vec::with_capacity(ch.len());
    for (c, x) in ch.iter().enumerate() {
        let len = x.len().min(crate::offline::MAX_OFFLINE_SAMPLES);
        let noise = profile.magnitudes.get(c).or_else(|| profile.magnitudes.last());
        let (Some(noise), true) = (noise, len > 0) else {
            out.push(x.iter().take(len).copied().map(finite).collect());
            continue;
        };
        let mut acc = vec![0.0f32; pre + len + FFT_SIZE];
        prev.fill(1.0);
        let mut pos = 0;
        while pos < pre + len {
            for (i, b) in buf.iter_mut().enumerate() {
                let s = (pos + i).checked_sub(pre).and_then(|j| x.get(j).filter(|_| j < len)).copied().map_or(0.0, finite);
                *b = Complex::new(s * w.get(i).copied().unwrap_or(0.0), 0.0);
            }
            fwd.process(&mut buf);
            for ((g, b), n) in raw.iter_mut().zip(&buf).zip(noise) {
                let m = b.norm();
                let t = over * n;
                // Power subtraction: what is left of the bin once the noise's share is taken out.
                let keep = if m > t { (1.0 - (t / m) * (t / m)).max(0.0).sqrt() } else { 0.0 };
                *g = keep.max(floor);
            }
            for k in 0..BINS {
                let lo = k.saturating_sub(radius);
                let hi = (k + radius).min(BINS - 1);
                let span = raw.get(lo..=hi).unwrap_or_default();
                let mean = span.iter().sum::<f32>() / span.len().max(1) as f32;
                let p = prev.get(k).copied().unwrap_or(1.0);
                // Open at once, close gradually.
                let g = if mean < p { release * p + (1.0 - release) * mean } else { mean };
                if let Some(slot) = gains.get_mut(k) {
                    *slot = g;
                }
            }
            prev.copy_from_slice(&gains);
            for (k, b) in buf.iter_mut().enumerate() {
                // Bin k and its mirror N - k share a gain, so the result stays real.
                let g = gains.get(k.min(FFT_SIZE - k)).copied().unwrap_or(1.0);
                *b *= g;
            }
            inv.process(&mut buf);
            for (i, b) in buf.iter().enumerate() {
                if let Some(a) = acc.get_mut(pos + i) {
                    // 1/N undoes the unnormalised inverse FFT. The two windows multiply to a Hann
                    // window, and Hann windows a quarter apart sum to 2.
                    *a += b.re * w.get(i).copied().unwrap_or(0.0) / (FFT_SIZE as f32 * 2.0);
                }
            }
            pos += HOP;
        }
        out.push(acc.get(pre..pre + len).map(<[f32]>::to_vec).unwrap_or_default());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Deterministic white noise in -amp..amp.
    fn noise(len: usize, amp: f32, seed: u32) -> Vec<f32> {
        let mut s = seed;
        (0..len)
            .map(|_| {
                s = s.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                ((s >> 8) as f32 / (1u32 << 24) as f32 * 2.0 - 1.0) * amp
            })
            .collect()
    }

    fn rms(x: &[f32]) -> f32 {
        (x.iter().map(|v| v * v).sum::<f32>() / x.len().max(1) as f32).sqrt()
    }

    #[test]
    fn learn_needs_one_frame_of_audio() {
        assert!(NoiseProfile::learn(&[], 48_000).is_none());
        assert!(NoiseProfile::learn(&[vec![0.1; MIN_LEARN_SAMPLES - 1]], 48_000).is_none());
        assert!(NoiseProfile::learn(&[vec![0.1; MIN_LEARN_SAMPLES]], 0).is_none());
        let p = NoiseProfile::learn(&[noise(MIN_LEARN_SAMPLES, 0.1, 1), noise(9000, 0.1, 2)], 48_000).unwrap();
        assert!(p.is_valid());
        assert_eq!((p.channels(), p.frames), (2, MIN_LEARN_SAMPLES as u64));
    }

    #[test]
    fn no_reduction_gives_back_the_input() {
        let x = noise(10_000, 0.5, 3);
        let p = NoiseProfile::learn(std::slice::from_ref(&x), 48_000).unwrap();
        let y = reduce(std::slice::from_ref(&x), &p, ReduceSettings { reduction_db: 0.0, ..Default::default() });
        assert_eq!(y[0].len(), x.len());
        let err = x.iter().zip(&y[0]).map(|(a, b)| (a - b).abs()).fold(0.0f32, f32::max);
        assert!(err < 1e-4, "reconstruction error {err}");
    }

    #[test]
    fn removes_the_learned_noise_and_keeps_the_tone() {
        let sr = 48_000.0;
        let len = 96_000;
        let hiss = noise(len, 0.05, 7);
        let tone: Vec<f32> = (0..len).map(|i| if i >= 48_000 { 0.5 * (std::f32::consts::TAU * 1000.0 * i as f32 / sr).sin() } else { 0.0 }).collect();
        let x: Vec<f32> = hiss.iter().zip(&tone).map(|(n, t)| n + t).collect();
        // The first half is only hiss: learn it there.
        let p = NoiseProfile::learn(&[x[..24_000].to_vec()], 48_000).unwrap();
        let y = reduce(std::slice::from_ref(&x), &p, ReduceSettings { reduction_db: 24.0, ..Default::default() });
        let before = rms(&x[4_000..44_000]);
        let after = rms(&y[0][4_000..44_000]);
        let drop = gain_to_db(after / before);
        assert!(drop < -18.0, "hiss only dropped {drop} dB");
        // The tone comes through within a decibel.
        let tone_in = rms(&tone[52_000..92_000]);
        let tone_out = rms(&y[0][52_000..92_000]);
        let change = gain_to_db(tone_out / tone_in);
        assert!(change.abs() < 1.0, "tone changed {change} dB");
    }

    #[test]
    fn a_mono_profile_serves_every_channel_and_bad_input_is_safe() {
        let p = NoiseProfile::learn(&[noise(4096, 0.1, 9)], 48_000).unwrap();
        let x = vec![noise(5000, 0.1, 10), vec![f32::NAN; 5000], Vec::new()];
        let y = reduce(&x, &p, ReduceSettings { reduction_db: f32::NAN, sensitivity_db: f32::INFINITY, smoothing: -3.0 });
        assert_eq!(y.iter().map(Vec::len).collect::<Vec<_>>(), vec![5000, 5000, 0]);
        assert!(y.iter().flatten().all(|v| v.is_finite()));
        assert!(rms(&y[0]) < rms(&x[0]));

        let mut bad = p.clone();
        bad.magnitudes[0].pop();
        assert!(!bad.is_valid());
        assert_eq!(reduce(&x[..1], &bad, ReduceSettings::default()), x[..1].to_vec());
    }

    #[test]
    fn profile_spectrum_reads_a_sine_near_its_level() {
        let sr = 48_000.0;
        let k = 100;
        let f = k as f32 * sr / FFT_SIZE as f32;
        let x: Vec<f32> = (0..8192).map(|i| 0.5 * (std::f32::consts::TAU * f * i as f32 / sr).sin()).collect();
        let p = NoiseProfile::learn(&[x], 48_000).unwrap();
        let db = p.spectrum_db();
        assert_eq!(db.len(), BINS);
        assert!((db[k] - gain_to_db(0.5)).abs() < 0.5, "{}", db[k]);
    }

    #[test]
    fn profiles_round_trip_through_json() {
        let p = NoiseProfile::learn(&[noise(4096, 0.1, 11)], 44_100).unwrap();
        let back: NoiseProfile = serde_json::from_value(serde_json::to_value(&p).unwrap()).unwrap();
        assert_eq!(back, p);
    }
}
