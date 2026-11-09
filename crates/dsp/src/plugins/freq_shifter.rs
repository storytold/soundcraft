//! Frequency Shifter: moves every frequency in the signal by the same number of hertz.
//!
//! Unlike a pitch shifter, which multiplies frequencies, a frequency shifter adds a constant to
//! them, so harmonic sounds become inharmonic. The signal goes through two chains of allpass
//! filters whose outputs differ in phase by 90° (the in-phase and quadrature parts of the
//! analytic signal). Multiplying them by a cosine and a sine at the shift frequency and adding or
//! subtracting the products keeps only the upper or only the lower sideband (single-sideband
//! modulation). The filters run in `f64`.

use std::f64::consts::{PI, TAU};

use crate::params::{Params, choice, lin, log, param_plumbing};
use crate::util::{FALLBACK_SR, Smoother, frames_in, sane_channels, sane_sr};
use crate::{Category, ParamInfo, Plugin, PluginInfo, Unit};

const MODE_CHOICES: &[&str] = &["Up", "Down", "Up & Down", "Stereo"];
const MODE_DOWN: usize = 1;
const MODE_BOTH: usize = 2;
const MODE_STEREO: usize = 3;

/// Allpass sections in each of the two chains.
const SECTIONS: usize = 10;
/// The 90° phase difference holds from this frequency to the same distance below Nyquist.
const QUADRATURE_LOW_HZ: f64 = 20.0;
/// Largest Feedback, in percent. The shift raises a signal's energy by at most √2 (the
/// quadrature pair has twice the energy of its input, and the output is never larger than
/// their magnitude), so with feedback below 1/√2 the loop gain stays under one and the
/// feedback loop is stable for any input, mode and frequency (small-gain theorem).
const MAX_FEEDBACK: f32 = 70.0;
/// The level, relative to the input, at which `tail_samples` considers the ringing over (-80 dB).
const TAIL_FLOOR: f64 = 1.0e-4;

static FS_PARAMS: [ParamInfo; 4] = [
    log("frequency", "Frequency", 0.01, 5000.0, 100.0, Unit::Hz),
    choice("mode", "Mode", MODE_CHOICES, 0),
    lin("feedback", "Feedback", 0.0, MAX_FEEDBACK, 0.0, Unit::Percent),
    lin("mix", "Mix", 0.0, 100.0, 100.0, Unit::Percent),
];

pub static FREQ_SHIFTER_INFO: PluginInfo = PluginInfo {
    id: "freq_shifter",
    name: "Frequency Shifter",
    short_name: "FreqShft",
    category: Category::Modulation,
    params: &FS_PARAMS,
    is_instrument: false,
    audiosuite: true,
};

/// Arithmetic-geometric mean of `a` and `b` (both positive).
fn agm(mut a: f64, mut b: f64) -> f64 {
    for _ in 0..64 {
        let (m, g) = (0.5 * (a + b), (a * b).sqrt());
        a = m;
        b = g;
        if (a - b).abs() <= 1.0e-15 * a {
            break;
        }
    }
    a
}

/// Allpass coefficients for a 90° phase-difference network at `sr`, accurate from
/// [`QUADRATURE_LOW_HZ`] to `sr / 2 - QUADRATURE_LOW_HZ`.
///
/// They are the coefficients of an elliptic half-band filter built from two allpass chains
/// (Valenzuela and Constantinides, "Digital signal processing schemes for efficient
/// interpolation and decimation", IEE Proceedings G 130(6), 1983). Moving that filter's
/// response by a quarter of the sample rate turns its two branches into a phase splitter: each
/// section becomes `(a - z⁻²) / (1 - a z⁻²)`. Even-numbered coefficients go to the in-phase
/// chain, odd-numbered ones to the quadrature chain, which also has a one-sample delay.
///
/// The half-band filter has a transition band of `tb = 2 · low / sr` (as a fraction of the
/// sample rate) centred on `sr / 4`, so after the shift the phase error is equiripple between
/// `low` and `sr / 2 - low`. Its selectivity is `k = tan²(π (1 - 2 tb) / 4)` and its nome
/// `q = exp(-π K(k') / K(k))`, with `K` the complete elliptic integral of the first kind computed
/// by the arithmetic-geometric mean. Coefficient `i` of `n` comes from the theta-function sums
///
/// ```text
/// w = q^¼ · Σₘ (-1)ᵐ q^(m(m+1)) sin((2m+1) i π / (2n+1)) / (½ + Σₘ₌₁ (-1)ᵐ q^(m²) cos(2 m i π / (2n+1)))
/// x = √((1 - w² k)(1 - w² / k)) / (1 + w²),   a = (1 - x) / (1 + x).
/// ```
///
/// `None` when the rate is unusable or a coefficient comes out of `[0, 1)`.
fn design_quadrature(sr: f64) -> Option<[f64; 2 * SECTIONS]> {
    let tb = 2.0 * QUADRATURE_LOW_HZ / sr;
    if !tb.is_finite() || tb <= 0.0 || tb >= 0.25 {
        return None;
    }
    let k = ((1.0 - 2.0 * tb) * PI / 4.0).tan().powi(2);
    let kc = ((1.0 - k) * (1.0 + k)).sqrt();
    // K(k') / K(k) = agm(1, k') / agm(1, k), since K(k) = π / (2 agm(1, k')).
    let q = (-PI * agm(1.0, kc) / agm(1.0, k)).exp();
    // The half-band filter has one coefficient per section of both chains: order 2 · 2 SECTIONS + 1.
    let order = (4 * SECTIONS + 1) as f64;
    let mut coefs = [0.0f64; 2 * SECTIONS];
    for (idx, coef) in coefs.iter_mut().enumerate() {
        let i = (idx + 1) as f64;
        let mut num = 0.0;
        for m in 0..64i32 {
            let sign = if m % 2 == 0 { 1.0 } else { -1.0 };
            let term = sign * q.powi(m * (m + 1)) * ((2 * m + 1) as f64 * i * PI / order).sin();
            num += term;
            if term.abs() < 1.0e-30 {
                break;
            }
        }
        let mut den = 0.5;
        for m in 1..64i32 {
            let sign = if m % 2 == 0 { 1.0 } else { -1.0 };
            let term = sign * q.powi(m * m) * (2.0 * f64::from(m) * i * PI / order).cos();
            den += term;
            if term.abs() < 1.0e-30 {
                break;
            }
        }
        let w = q.powf(0.25) * num / den;
        let w2 = w * w;
        let x = ((1.0 - w2 * k) * (1.0 - w2 / k)).max(0.0).sqrt() / (1.0 + w2);
        *coef = (1.0 - x) / (1.0 + x);
    }
    coefs.iter().all(|a| a.is_finite() && (0.0..1.0).contains(a)).then_some(coefs)
}

/// The network for `sr`, falling back to the default rate's (and, failing that, to plain
/// delays) so that a strange host rate can never leave the plugin without coefficients.
fn quadrature_coefs(sr: f64) -> [f64; 2 * SECTIONS] {
    design_quadrature(sr).or_else(|| design_quadrature(f64::from(FALLBACK_SR))).unwrap_or([0.0; 2 * SECTIONS])
}

/// Flushes values too small to hear (and any non-finite value) to zero, so silence does not
/// leave the filter states running on denormals.
#[inline]
fn flush64(x: f64) -> f64 {
    if x.abs() > 1.0e-30 && x.is_finite() { x } else { 0.0 }
}

/// One `(a - z⁻²) / (1 - a z⁻²)` allpass section.
#[derive(Debug, Clone, Copy, Default)]
struct Section {
    x1: f64,
    x2: f64,
    y1: f64,
    y2: f64,
}

impl Section {
    #[inline]
    fn tick(&mut self, a: f64, x: f64) -> f64 {
        let y = flush64(a * (x + self.y2) - self.x2);
        self.x2 = self.x1;
        self.x1 = x;
        self.y2 = self.y1;
        self.y1 = y;
        y
    }
}

/// Runs `x` through a chain of sections with the coefficients `coefs`.
#[inline]
fn chain(sections: &mut [Section; SECTIONS], coefs: &[f64; SECTIONS], x: f64) -> f64 {
    sections.iter_mut().zip(coefs.iter()).fold(x, |v, (s, &a)| s.tick(a, v))
}

/// The state of one channel.
#[derive(Debug, Clone, Copy, Default)]
struct Channel {
    in_phase: [Section; SECTIONS],
    quadrature: [Section; SECTIONS],
    /// The quadrature chain's input from the previous sample (its one-sample delay).
    delayed: f64,
    /// The previous wet output, fed back into the input.
    last: f64,
}

/// Which sideband `channel` of `channels` keeps, as the weight of the quadrature product:
/// `1` keeps the upper sideband, `-1` the lower one and `0` both at half level. The output is
/// `i cos - s q sin`, so it is linear in `s`, and moving `s` smoothly from one mode's value to
/// another's crossfades the two modes without a click.
///
/// Stereo shifts the left channel down and the right one up. Only a two-channel insert has a
/// known left and right; a plugin is not told the speaker layout of a wider one (channel 1 is
/// the centre in LCR and 5.1, the right in quad), so mono and multichannel inserts play both
/// sidebands on every channel.
fn sideband(mode: usize, channel: usize, channels: usize) -> f32 {
    match mode {
        MODE_DOWN => -1.0,
        MODE_BOTH => 0.0,
        MODE_STEREO if channels != 2 => 0.0,
        MODE_STEREO if channel == 0 => -1.0,
        _ => 1.0,
    }
}

/// Single-sideband frequency shifter with feedback.
///
/// - **Up** adds Frequency to every component and **Down** subtracts it; a component that goes
///   below 0 Hz comes back mirrored, and one pushed past Nyquist folds back from it.
/// - **Up & Down** plays both sidebands at half level each (the in-phase signal times the
///   carrier), which is a ring modulation without the quadrature chain's error.
/// - **Stereo** shifts the left channel down and the right channel up (see [`sideband`]).
/// - **Feedback** sends the shifted output back into the input, so a tone leaves a trail of
///   components one more shift away each time, each `feedback` times the level of the last.
///
/// The phase difference is accurate to within a few millionths of a radian from 20 Hz to 20 Hz
/// below Nyquist at 44.1 to 96 kHz, which leaves the unwanted sideband more than 100 dB down;
/// below 20 Hz both sidebands come through. The plugin has no latency.
///
/// Frequency, Feedback and Mix glide to a new value over about 20 ms, and the carrier phase runs
/// on, so a change does not click. A new Mode crossfades from the old sidebands in the same time.
pub struct FreqShifter {
    p: Params,
    sr: f64,
    in_phase_coefs: [f64; SECTIONS],
    quadrature_coefs: [f64; SECTIONS],
    channels: Vec<Channel>,
    /// Carrier phase in cycles, in `[0, 1)`.
    phase: f64,
    freq: Smoother,
    feedback: Smoother,
    mix: Smoother,
    /// Each channel's sideband weight (see [`sideband`]).
    sides: Vec<Smoother>,
    tail: usize,
}

impl Default for FreqShifter {
    fn default() -> Self {
        Self::new()
    }
}

impl FreqShifter {
    pub fn new() -> Self {
        let mut s = Self {
            p: Params::new(&FS_PARAMS),
            sr: f64::from(FALLBACK_SR),
            in_phase_coefs: [0.0; SECTIONS],
            quadrature_coefs: [0.0; SECTIONS],
            channels: Vec::new(),
            phase: 0.0,
            freq: Smoother::new(100.0),
            feedback: Smoother::new(0.0),
            mix: Smoother::new(1.0),
            sides: Vec::new(),
            tail: 0,
        };
        s.design();
        s.update();
        s
    }

    fn design(&mut self) {
        let coefs = quadrature_coefs(self.sr);
        for (i, a) in coefs.iter().enumerate() {
            let side = if i % 2 == 0 { &mut self.in_phase_coefs } else { &mut self.quadrature_coefs };
            if let Some(slot) = side.get_mut(i / 2) {
                *slot = *a;
            }
        }
    }

    fn update(&mut self) {
        self.freq.set(self.p.v(0));
        self.feedback.set(self.p.v(2) / 100.0);
        self.mix.set(self.p.v(3) / 100.0);
        let (mode, channels) = (self.p.choice(1), self.sides.len());
        for (c, s) in self.sides.iter_mut().enumerate() {
            s.set(sideband(mode, c, channels));
        }
        self.tail = self.ring_samples();
    }

    /// How long the network rings after an impulse until it falls below [`TAIL_FLOOR`], from its
    /// slowest pole (the section `a` closest to 1 decays by `a` every two samples), stretched by
    /// the feedback, which sends each pass through the network again.
    fn ring_samples(&self) -> usize {
        let slowest = self.in_phase_coefs.iter().chain(self.quadrature_coefs.iter()).fold(0.0f64, |m, &a| m.max(a));
        let ring = if slowest > 0.0 && slowest < 1.0 { 2.0 * TAIL_FLOOR.ln() / slowest.ln() } else { 0.0 };
        let fb = f64::from(self.p.v(2) / 100.0).clamp(0.0, 0.99);
        let n = (ring / (1.0 - fb)).ceil();
        if n.is_finite() && n > 0.0 { n.min(f64::from(u32::MAX)) as usize } else { 0 }
    }
}

impl Plugin for FreqShifter {
    param_plumbing!(FREQ_SHIFTER_INFO);

    fn prepare(&mut self, sample_rate: f32, _max_block: usize, channels: usize) {
        self.sr = f64::from(sane_sr(sample_rate));
        self.design();
        let channels = sane_channels(channels);
        self.channels = vec![Channel::default(); channels];
        self.sides = vec![Smoother::new(0.0); channels];
        let sr = self.sr as f32;
        for s in [&mut self.freq, &mut self.feedback, &mut self.mix].into_iter().chain(self.sides.iter_mut()) {
            s.set_time(20.0, sr);
        }
        self.update();
        self.reset();
    }

    fn reset(&mut self) {
        self.channels.iter_mut().for_each(|c| *c = Channel::default());
        self.phase = 0.0;
        for s in [&mut self.freq, &mut self.feedback, &mut self.mix].into_iter().chain(self.sides.iter_mut()) {
            s.snap();
        }
    }

    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        let ch = self.channels.len().min(io.len());
        let frames = frames_in(io, frames, ch);
        for n in 0..frames {
            let (sin, cos) = (TAU * self.phase).sin_cos();
            let step = f64::from(self.freq.next()) / self.sr;
            self.phase = (self.phase + step).fract();
            let fb = f64::from(self.feedback.next());
            let mix = self.mix.next();
            for c in 0..ch {
                let (Some(buf), Some(st), Some(side)) = (io.get_mut(c), self.channels.get_mut(c), self.sides.get_mut(c)) else {
                    continue;
                };
                let Some(x) = buf.get_mut(n) else { continue };
                let dry = if x.is_finite() { *x } else { 0.0 };
                let input = f64::from(dry) + fb * st.last;
                let i = chain(&mut st.in_phase, &self.in_phase_coefs, input);
                let q = chain(&mut st.quadrature, &self.quadrature_coefs, st.delayed);
                st.delayed = input;
                let wet = i * cos - f64::from(side.next()) * q * sin;
                st.last = flush64(wet);
                *x = dry * (1.0 - mix) + st.last as f32 * mix;
            }
        }
    }

    fn tail_samples(&self) -> usize {
        self.tail
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The in-phase and quadrature responses of the network at `f` cycles per sample.
    fn response(coefs: &[f64; 2 * SECTIONS], f: f64) -> ((f64, f64), (f64, f64)) {
        let mul = |(a, b): (f64, f64), (c, d): (f64, f64)| (a * c - b * d, a * d + b * c);
        let div = |(a, b): (f64, f64), (c, d): (f64, f64)| {
            let m = c * c + d * d;
            ((a * c + b * d) / m, (b * c - a * d) / m)
        };
        // z⁻² = e^(-j 4π f)
        let z2 = ((2.0 * TAU * f).cos(), -(2.0 * TAU * f).sin());
        let mut paths = [(1.0, 0.0), (1.0, 0.0)];
        for (i, &a) in coefs.iter().enumerate() {
            let h = div((a - z2.0, -z2.1), (1.0 - a * z2.0, -a * z2.1));
            if let Some(p) = paths.get_mut(i % 2) {
                *p = mul(*p, h);
            }
        }
        let [i, q] = paths;
        (i, mul(q, ((TAU * f).cos(), -(TAU * f).sin())))
    }

    /// The level of the unwanted sideband, in dB, for a phase error of `err` radians.
    fn image_db(err: f64) -> f64 {
        20.0 * (err / 2.0).abs().tan().max(1e-300).log10()
    }

    #[test]
    fn network_is_ninety_degrees_across_the_band_at_every_rate() {
        // Worst unwanted sideband at each rate from 20 Hz to 20 Hz below Nyquist.
        for (sr, limit) in [(8_000.0, -140.0), (44_100.0, -110.0), (48_000.0, -110.0), (96_000.0, -100.0), (192_000.0, -90.0), (384_000.0, -85.0)] {
            let coefs = design_quadrature(sr).unwrap();
            let (lo, hi) = (QUADRATURE_LOW_HZ / sr, 0.5 - QUADRATURE_LOW_HZ / sr);
            let mut worst = f64::MIN;
            for k in 0..=4000 {
                let f = lo * (hi / lo).powf(k as f64 / 4000.0);
                let ((ir, ii), (qr, qi)) = response(&coefs, f);
                // Both chains are allpass: unit gain everywhere.
                assert!(((ir * ir + ii * ii).sqrt() - 1.0).abs() < 1e-9, "{sr} Hz rate: in-phase gain at {f}");
                assert!(((qr * qr + qi * qi).sqrt() - 1.0).abs() < 1e-9, "{sr} Hz rate: quadrature gain at {f}");
                // The quadrature chain lags the in-phase chain by 90°.
                let diff = qi.atan2(qr) - ii.atan2(ir);
                let diff = (diff + PI).rem_euclid(TAU) - PI;
                worst = worst.max(image_db(diff + PI / 2.0));
            }
            assert!(worst < limit, "{sr} Hz rate: unwanted sideband at {worst:.1} dB, limit {limit} dB");
        }
    }

    #[test]
    fn network_fails_outside_its_band_as_designed() {
        // At 5 Hz, a quarter of the lowest accurate frequency, the phase difference is far off 90°:
        // the band edge really is at 20 Hz, not somewhere lower by accident.
        let coefs = design_quadrature(48_000.0).unwrap();
        let ((ir, ii), (qr, qi)) = response(&coefs, 5.0 / 48_000.0);
        let diff = (qi.atan2(qr) - ii.atan2(ir) + PI).rem_euclid(TAU) - PI;
        assert!(image_db(diff + PI / 2.0) > -40.0, "5 Hz: {diff}");
    }

    #[test]
    fn coefficients_exist_for_every_host_rate() {
        for sr in [1_000.0f32, 8_000.0, 22_050.0, 44_100.0, 48_000.0, 88_200.0, 176_400.0, 352_800.0, 768_000.0, 1_536_000.0] {
            let coefs = design_quadrature(f64::from(sr)).unwrap_or_else(|| panic!("no network at {sr}"));
            assert!(coefs.windows(2).all(|w| w[0] < w[1]), "{sr}: coefficients not increasing: {coefs:?}");
        }
        // Rates the design cannot serve fall back to the default network, never to nothing.
        assert_eq!(design_quadrature(100.0), None);
        assert_eq!(design_quadrature(f64::NAN), None);
        assert_eq!(quadrature_coefs(100.0), quadrature_coefs(48_000.0));
    }

    #[test]
    fn silence_after_sound_flushes_the_filter_states() {
        let mut p = FreqShifter::new();
        p.prepare(48_000.0, 512, 2);
        let mut io = vec![vec![0.0f32; 512]; 2];
        io[0][0] = 1.0;
        io[1][0] = 1.0;
        p.process(&mut io, 512);
        for _ in 0..2_000 {
            let mut io = vec![vec![0.0f32; 512]; 2];
            p.process(&mut io, 512);
        }
        for c in &p.channels {
            for s in c.in_phase.iter().chain(c.quadrature.iter()) {
                for v in [s.x1, s.x2, s.y1, s.y2] {
                    assert!(v == 0.0 || v.is_normal(), "state {v:e} is denormal");
                }
            }
            assert_eq!(c.last, 0.0);
        }
    }

    #[test]
    fn stereo_mode_shifts_left_down_and_right_up_on_two_channels_only() {
        assert_eq!(sideband(MODE_STEREO, 0, 2), -1.0);
        assert_eq!(sideband(MODE_STEREO, 1, 2), 1.0);
        for channels in [1, 3, 4, 6, 16] {
            for c in 0..channels {
                assert_eq!(sideband(MODE_STEREO, c, channels), 0.0, "{c} of {channels}");
            }
        }
        for channels in [1, 2, 6] {
            for c in 0..channels {
                assert_eq!(sideband(0, c, channels), 1.0);
                assert_eq!(sideband(MODE_DOWN, c, channels), -1.0);
                assert_eq!(sideband(MODE_BOTH, c, channels), 0.0);
            }
        }
    }
}
