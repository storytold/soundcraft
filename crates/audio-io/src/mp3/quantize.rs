//! Quantization of one granule's spectral lines: the rate loop picks the global gain that fits
//! the bit budget, and the distortion loop amplifies (scalefactors) the bands whose noise is above
//! the allowed level, a simple masking estimate floored by the threshold in quiet.
#![allow(clippy::needless_range_loop)]

use super::analysis::GRANULE;
use super::huffman::{self, Coding, MAX_IX};
use super::tables::SLEN;

/// Signal-to-noise ratio each band is allowed, in dB, before masking from the band below.
const SNR_DB: f32 = 18.0;
/// How much of the band below's level masks a band, in dB below it.
const SPREAD_DB: f32 = 26.0;
/// Rounds of amplification the distortion loop tries.
const ROUNDS: usize = 12;
/// Spectral energy (sum of squared lines over a granule) of a full-scale sine.
pub(super) const FULL_SCALE_ENERGY: f32 = 1.0;

/// A quantized granule of one channel.
#[derive(Clone)]
pub(super) struct Granule {
    /// Quantized magnitudes.
    pub ix: [i32; GRANULE],
    pub neg: [bool; GRANULE],
    pub coding: Coding,
    pub global_gain: u32,
    /// Scalefactors of bands 0–20.
    pub sf: [u8; 21],
    pub scalefac_compress: u32,
    /// Bits of the scalefactors (part 2).
    pub part2: u32,
}

impl Granule {
    pub(super) fn new() -> Box<Self> {
        Box::new(Granule {
            ix: [0; GRANULE],
            neg: [false; GRANULE],
            coding: Coding::default(),
            global_gain: 0,
            sf: [0; 21],
            scalefac_compress: 0,
            part2: 0,
        })
    }

    /// Bits of the granule's main data (part 2 and part 3).
    pub(super) fn bits(&self) -> u32 {
        self.part2 + self.coding.bits
    }
}

/// The quantizer for one stream: its bands, threshold in quiet and lowpass.
pub(super) struct Quantizer {
    pub sfb: [usize; 23],
    pow43: Vec<f32>,
    /// Threshold in quiet per band, as spectral energy.
    ath: [f32; 22],
    /// The first line the lowpass removes.
    lowpass: usize,
}

/// The per-granule work buffers of [`Quantizer::quantize`].
pub(super) struct Work {
    xr34: [f32; GRANULE],
    trial: Box<Granule>,
}

impl Work {
    pub(super) fn new() -> Box<Self> {
        Box::new(Work { xr34: [0.0; GRANULE], trial: Granule::new() })
    }
}

/// The band's allowed noise and the granule's perceptual entropy (a bit-demand estimate).
pub(super) struct Mask {
    pub xmin: [f32; 22],
    pub pe: f32,
}

/// The threshold in quiet (Terhardt) in dB SPL at `khz`.
fn ath_db(khz: f32) -> f32 {
    let f = khz.max(0.02);
    3.64 * f.powf(-0.8) - 6.5 * (-0.6 * (f - 3.3) * (f - 3.3)).exp() + 1e-3 * f.powi(4)
}

impl Quantizer {
    /// A quantizer for `sample_rate` (32, 44.1 or 48 kHz, `sfb` its bands) passing up to
    /// `lowpass_hz`.
    pub(super) fn new(sfb: [usize; 23], sample_rate: u32, lowpass_hz: f32) -> Self {
        let hz_per_line = sample_rate as f32 / 2.0 / GRANULE as f32;
        let mut ath = [0.0f32; 22];
        for (b, a) in ath.iter_mut().enumerate() {
            let lines = sfb[b]..sfb[b + 1];
            let width = lines.len() as f32;
            // A full-scale sine is taken to play at 96 dB SPL.
            let quietest = lines.map(|i| ath_db((i as f32 + 0.5) * hz_per_line / 1000.0)).fold(f32::INFINITY, f32::min);
            *a = width * FULL_SCALE_ENERGY * 10f32.powf((quietest - 96.0) / 10.0);
        }
        let lowpass = ((lowpass_hz / hz_per_line) as usize).clamp(1, GRANULE);
        let pow43 = (0..=MAX_IX).map(|i| (i as f64).powf(4.0 / 3.0) as f32).collect();
        Quantizer { sfb, pow43, ath, lowpass }
    }

    /// Removes the lines above the lowpass and estimates the allowed noise per band.
    pub(super) fn mask(&self, xr: &mut [f32; GRANULE]) -> Mask {
        for x in &mut xr[self.lowpass..] {
            *x = 0.0;
        }
        let mut mask = Mask { xmin: [0.0; 22], pe: 0.0 };
        let (snr, spread) = (10f32.powf(-SNR_DB / 10.0), 10f32.powf(-SPREAD_DB / 10.0));
        let mut below = 0.0f32;
        for b in 0..22 {
            let lines = self.sfb[b]..self.sfb[b + 1];
            let width = lines.len() as f32;
            let energy: f32 = xr[lines].iter().map(|x| x * x).sum();
            let thr = (energy * snr).max(below * spread * width).max(self.ath[b]);
            below = energy / width;
            mask.xmin[b] = thr;
            mask.pe += width * (1.0 + energy / thr).log2() * 0.5;
        }
        mask
    }

    /// Quantizes `xr` into `out` in at most `max_bits` bits. When the noise is inaudible with
    /// fewer bits, it uses fewer, but not below `min_bits` (bits the reservoir couldn't keep).
    pub(super) fn quantize(&self, xr: &[f32; GRANULE], mask: &Mask, min_bits: u32, max_bits: u32, work: &mut Work, out: &mut Granule) {
        self.search(xr, mask, min_bits, max_bits, work, out);
        // The loops code with a quick estimate; the best coding only takes fewer bits.
        out.coding = huffman::plan(&out.ix, &self.sfb);
    }

    fn search(&self, xr: &[f32; GRANULE], mask: &Mask, min_bits: u32, max_bits: u32, work: &mut Work, out: &mut Granule) {
        let mut peak = 0.0f32;
        for i in 0..GRANULE {
            let a = xr[i].abs();
            let a = if a.is_finite() { a } else { 0.0 };
            work.xr34[i] = (a * a.sqrt()).sqrt();
            out.neg[i] = xr[i] < 0.0;
            peak = peak.max(work.xr34[i]);
        }
        out.sf = [0; 21];
        if peak == 0.0 {
            self.encode(&work.xr34, 210, out);
            return;
        }
        // The finest gain at which no line overflows the tables.
        let lowest = (210.0 + 16.0 / 3.0 * (peak / MAX_IX as f32).log2()).ceil().clamp(0.0, 255.0) as u32;
        let gain = self.rate_loop(&work.xr34, lowest, max_bits, out);
        let mut score = self.over_noise(xr, mask, out);
        if score.0 == 0 {
            // Nothing audible: spend no more bits than that takes (or than must be spent).
            let (mut lo, mut hi) = (gain, 255);
            work.trial.sf = [0; 21];
            while lo < hi {
                let mid = (lo + hi).div_ceil(2);
                self.encode(&work.xr34, mid, &mut work.trial);
                if work.trial.bits() >= min_bits && self.over_noise(xr, mask, &work.trial).0 == 0 { lo = mid } else { hi = mid - 1 }
            }
            self.encode(&work.xr34, lo, out);
            return;
        }
        // Amplify the bands with audible noise until none is left, the scalefactors run out, or
        // the rounds do; keep the best try.
        let trial = &mut work.trial;
        **trial = out.clone();
        for _ in 0..ROUNDS {
            let noise = self.band_noise(xr, trial);
            let mut amplified = false;
            for b in 0..21 {
                if noise[b] > mask.xmin[b] {
                    trial.sf[b] += 1;
                    amplified = true;
                }
            }
            let fits = trial.sf.iter().enumerate().all(|(b, &s)| s <= if b < 11 { 15 } else { 7 });
            if !amplified || !fits {
                break;
            }
            let mut gain = trial.global_gain;
            loop {
                self.encode(&work.xr34, gain, trial);
                if trial.bits() <= max_bits || gain >= 255 {
                    break;
                }
                gain += 1;
            }
            if trial.bits() > max_bits {
                break;
            }
            let s = self.over_noise(xr, mask, trial);
            if s.0 < score.0 || (s.0 == score.0 && s.1 < score.1) {
                score = s;
                *out = (**trial).clone();
            }
            if s.0 == 0 {
                break;
            }
        }
    }

    /// The finest global gain (with `out`'s scalefactors) whose coding fits `max_bits`; `out`
    /// holds that coding. If even the coarsest doesn't fit, the top lines are dropped until it
    /// does.
    fn rate_loop(&self, xr34: &[f32; GRANULE], lowest: u32, max_bits: u32, out: &mut Granule) -> u32 {
        let (mut lo, mut hi) = (lowest, 255);
        while lo < hi {
            let mid = (lo + hi) / 2;
            self.encode(xr34, mid, out);
            if out.bits() <= max_bits { hi = mid } else { lo = mid + 1 }
        }
        self.encode(xr34, lo, out);
        while out.bits() > max_bits && out.coding.count1_end > 0 {
            let end = out.coding.count1_end;
            for v in &mut out.ix[end.saturating_sub(2)..end] {
                *v = 0;
            }
            self.code(out);
        }
        lo
    }

    /// Quantizes at `gain` with `out`'s scalefactors and plans its coding.
    fn encode(&self, xr34: &[f32; GRANULE], gain: u32, out: &mut Granule) {
        out.global_gain = gain;
        for b in 0..22 {
            let sf = out.sf.get(b).copied().unwrap_or(0);
            let scale = (-0.1875 * (gain as f32 - 210.0) + 0.375 * f32::from(sf)).exp2();
            for i in self.sfb[b]..self.sfb[b + 1] {
                let v = xr34[i] * scale + 0.4054;
                out.ix[i] = if v >= MAX_IX as f32 { MAX_IX } else { v as i32 };
            }
        }
        self.code(out);
    }

    /// Estimates the coding of `out`'s lines and plans its scalefactors.
    fn code(&self, out: &mut Granule) {
        let (max1, max2) = (out.sf[..11].iter().copied().max().unwrap_or(0), out.sf[11..].iter().copied().max().unwrap_or(0));
        let (k, (s1, s2)) = SLEN
            .iter()
            .enumerate()
            .filter(|(_, (s1, s2))| u32::from(max1) < 1 << s1 && u32::from(max2) < 1 << s2)
            .min_by_key(|(_, (s1, s2))| 11 * s1 + 10 * s2)
            .map_or((15, (4, 3)), |(k, &s)| (k, s));
        out.scalefac_compress = k as u32;
        out.part2 = 11 * s1 + 10 * s2;
        out.coding = huffman::estimate(&out.ix, &self.sfb);
    }

    /// The quantization noise energy of each band.
    fn band_noise(&self, xr: &[f32; GRANULE], g: &Granule) -> [f32; 22] {
        let mut noise = [0.0f32; 22];
        for (b, n) in noise.iter_mut().enumerate() {
            let sf = g.sf.get(b).copied().unwrap_or(0);
            let step = ((g.global_gain as f32 - 210.0) * 0.25 - 0.5 * f32::from(sf)).exp2();
            for i in self.sfb[b]..self.sfb[b + 1] {
                let q = self.pow43.get(g.ix[i] as usize).copied().unwrap_or(0.0) * step;
                let d = xr[i].abs() - q;
                *n += d * d;
            }
        }
        noise
    }

    /// The bands (0–20) whose noise is above the allowed level, and by how much in total (dB).
    fn over_noise(&self, xr: &[f32; GRANULE], mask: &Mask, g: &Granule) -> (usize, f32) {
        let noise = self.band_noise(xr, g);
        let mut over = (0, 0.0);
        for b in 0..21 {
            if noise[b] > mask.xmin[b] {
                over.0 += 1;
                over.1 += 10.0 * (noise[b] / mask.xmin[b]).log10();
            }
        }
        over
    }
}
