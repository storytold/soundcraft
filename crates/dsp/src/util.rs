//! Small realtime building blocks shared by the plugins.

use std::f32::consts::TAU;

/// Default sample rate used when a host passes something unusable.
pub(crate) const FALLBACK_SR: f32 = 48_000.0;

/// Sanitizes a host sample rate into 1 kHz ..= 1.536 MHz.
pub(crate) fn sane_sr(sr: f32) -> f32 {
    if sr.is_finite() && sr >= 1000.0 { sr.min(1_536_000.0) } else { FALLBACK_SR }
}

/// Upper bound on channels any plugin will allocate state for.
pub(crate) const MAX_CHANNELS: usize = 64;

pub(crate) fn sane_channels(ch: usize) -> usize {
    ch.min(MAX_CHANNELS)
}

/// Converts milliseconds to (fractional) samples, never negative or non-finite.
#[inline]
pub(crate) fn ms_to_samples(ms: f32, sr: f32) -> f32 {
    let s = ms * 0.001 * sr;
    if s.is_finite() { s.max(0.0) } else { 0.0 }
}

/// One-pole coefficient for a time constant in milliseconds.
#[inline]
pub(crate) fn time_coef(ms: f32, sr: f32) -> f32 {
    let n = ms_to_samples(ms, sr);
    if n < 1.0e-3 { 0.0 } else { (-1.0 / n).exp() }
}

/// Flushes denormals and non-finite values in feedback paths to zero.
#[inline]
pub(crate) fn flush(x: f32) -> f32 {
    let a = x.abs();
    if (a > 1.0e-25) & (a < f32::INFINITY) { x } else { 0.0 }
}

/// Number of frames a plugin may safely touch.
pub(crate) fn frames_in(io: &[Vec<f32>], frames: usize, channels: usize) -> usize {
    io.iter().take(channels).map(Vec::len).fold(frames, usize::min)
}

/// One-pole exponential parameter smoother (no zipper noise).
#[derive(Debug, Clone, Copy)]
pub(crate) struct Smoother {
    cur: f32,
    target: f32,
    coef: f32,
}

impl Smoother {
    pub(crate) fn new(v: f32) -> Self {
        Self { cur: v, target: v, coef: 0.0 }
    }

    /// Sets the time constant (default ~20 ms is typical).
    pub(crate) fn set_time(&mut self, ms: f32, sr: f32) {
        self.coef = time_coef(ms, sr);
    }

    pub(crate) fn set(&mut self, v: f32) {
        if v.is_finite() {
            self.target = v;
        }
    }

    pub(crate) fn snap(&mut self) {
        self.cur = self.target;
    }

    pub(crate) fn value(&self) -> f32 {
        self.cur
    }

    pub(crate) fn settling(&self) -> bool {
        self.cur != self.target
    }

    #[inline]
    pub(crate) fn next(&mut self) -> f32 {
        if self.cur != self.target {
            self.cur = self.target + (self.cur - self.target) * self.coef;
            if (self.cur - self.target).abs() <= 1.0e-6 * (1.0 + self.target.abs()) {
                self.cur = self.target;
            }
        }
        self.cur
    }

    /// Advances `n` samples at once (used for block-rate coefficient updates).
    pub(crate) fn advance(&mut self, n: usize) -> f32 {
        if self.cur != self.target {
            let k = self.coef.powi(n.min(i32::MAX as usize) as i32);
            self.cur = self.target + (self.cur - self.target) * k;
            if (self.cur - self.target).abs() <= 1.0e-6 * (1.0 + self.target.abs()) {
                self.cur = self.target;
            }
        }
        self.cur
    }
}

/// Power-of-two circular delay line with fractional reads.
#[derive(Debug, Clone, Default)]
pub(crate) struct DelayLine {
    buf: Vec<f32>,
    mask: usize,
    pos: usize,
}

impl DelayLine {
    /// Allocates room for at least `max_delay` samples of delay.
    pub(crate) fn allocate(&mut self, max_delay: usize) {
        let len = max_delay.saturating_add(4).clamp(4, 1 << 26).next_power_of_two();
        self.buf = vec![0.0; len];
        self.mask = len - 1;
        self.pos = 0;
    }

    pub(crate) fn clear(&mut self) {
        self.buf.iter_mut().for_each(|x| *x = 0.0);
    }

    /// Largest delay (in samples) that `read` supports.
    pub(crate) fn max_delay(&self) -> f32 {
        self.buf.len().saturating_sub(3) as f32
    }

    #[inline]
    pub(crate) fn push(&mut self, x: f32) {
        if let Some(s) = self.buf.get_mut(self.pos) {
            *s = x;
        }
        self.pos = (self.pos + 1) & self.mask;
    }

    /// Reads the sample written `d` pushes ago (`d = 1` is the last written sample).
    #[inline]
    pub(crate) fn tap(&self, d: usize) -> f32 {
        let idx = self.pos.wrapping_sub(d) & self.mask;
        self.buf.get(idx).copied().unwrap_or(0.0)
    }

    /// Linear-interpolated read `d` samples back (clamped to 1..=max_delay).
    #[inline]
    pub(crate) fn read(&self, d: f32) -> f32 {
        let d = if d.is_finite() { d.clamp(1.0, self.max_delay().max(1.0)) } else { 1.0 };
        let i = d as usize;
        let f = d - i as f32;
        let a = self.tap(i);
        let b = self.tap(i + 1);
        a + (b - a) * f
    }

    /// Cubic (Hermite) interpolated read, for modulated delays.
    #[inline]
    pub(crate) fn read_cubic(&self, d: f32) -> f32 {
        let d = if d.is_finite() { d.clamp(2.0, (self.max_delay() - 1.0).max(2.0)) } else { 2.0 };
        let i = d as usize;
        let f = d - i as f32;
        let xm1 = self.tap(i - 1);
        let x0 = self.tap(i);
        let x1 = self.tap(i + 1);
        let x2 = self.tap(i + 2);
        let c1 = 0.5 * (x1 - xm1);
        let c2 = xm1 - 2.5 * x0 + 2.0 * x1 - 0.5 * x2;
        let c3 = 0.5 * (x2 - xm1) + 1.5 * (x0 - x1);
        ((c3 * f + c2) * f + c1) * f + x0
    }
}

/// Xorshift32 noise source (deterministic, allocation free).
#[derive(Debug, Clone, Copy)]
pub(crate) struct Rng(u32);

impl Rng {
    pub(crate) fn new(seed: u32) -> Self {
        Self(if seed == 0 { 0x9E37_79B9 } else { seed })
    }

    #[inline]
    pub(crate) fn next_u32(&mut self) -> u32 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.0 = x;
        x
    }

    /// Uniform white noise in -1..1.
    #[inline]
    pub(crate) fn bipolar(&mut self) -> f32 {
        (self.next_u32() >> 8) as f32 * (2.0 / 16_777_216.0) - 1.0
    }
}

/// Pink noise via a sum of first-order filtered white noise (Kellet-style economy filter).
#[derive(Debug, Clone, Copy)]
pub(crate) struct PinkNoise {
    b: [f32; 7],
}

impl PinkNoise {
    pub(crate) fn new() -> Self {
        Self { b: [0.0; 7] }
    }

    #[inline]
    pub(crate) fn next(&mut self, white: f32) -> f32 {
        let b = &mut self.b;
        b[0] = 0.99886 * b[0] + white * 0.0555179;
        b[1] = 0.99332 * b[1] + white * 0.0750759;
        b[2] = 0.96900 * b[2] + white * 0.153_852;
        b[3] = 0.86650 * b[3] + white * 0.3104856;
        b[4] = 0.55000 * b[4] + white * 0.5329522;
        b[5] = -0.7616 * b[5] - white * 0.016_898;
        let out = b[0] + b[1] + b[2] + b[3] + b[4] + b[5] + b[6] + white * 0.5362;
        b[6] = white * 0.115926;
        out * 0.11
    }
}

/// Sine LFO with phase in 0..1.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Lfo {
    phase: f32,
}

impl Lfo {
    pub(crate) fn new() -> Self {
        Self { phase: 0.0 }
    }

    pub(crate) fn reset(&mut self) {
        self.phase = 0.0;
    }

    /// Advances by `inc` cycles and returns sin at `offset` cycles ahead (for stereo spread).
    #[inline]
    pub(crate) fn tick(&mut self, inc: f32) -> f32 {
        let inc = if inc.is_finite() { inc.clamp(0.0, 0.5) } else { 0.0 };
        self.phase += inc;
        if self.phase >= 1.0 {
            self.phase -= 1.0;
        }
        self.phase
    }

    #[inline]
    pub(crate) fn sin_at(phase: f32) -> f32 {
        (phase * TAU).sin()
    }
}

/// DC blocking one-pole high-pass.
#[derive(Debug, Clone, Copy)]
pub(crate) struct DcBlocker {
    r: f32,
    x1: f32,
    y1: f32,
}

impl DcBlocker {
    pub(crate) fn new(cutoff_hz: f32, sr: f32) -> Self {
        let mut d = Self { r: 0.995, x1: 0.0, y1: 0.0 };
        d.set_cutoff(cutoff_hz, sr);
        d
    }

    pub(crate) fn set_cutoff(&mut self, cutoff_hz: f32, sr: f32) {
        let fc = if cutoff_hz.is_finite() { cutoff_hz.clamp(0.1, 1000.0) } else { 5.0 };
        self.r = (-TAU * fc / sane_sr(sr)).exp();
    }

    pub(crate) fn reset(&mut self) {
        self.x1 = 0.0;
        self.y1 = 0.0;
    }

    #[inline]
    pub(crate) fn process(&mut self, x: f32) -> f32 {
        let y = x - self.x1 + self.r * self.y1;
        self.x1 = x;
        self.y1 = flush(y);
        self.y1
    }
}

/// Sorts pending note events by offset in place (insertion sort; tiny lists, no allocation).
pub(crate) fn sort_events<T: Copy>(ev: &mut [T], key: impl Fn(&T) -> usize) {
    for i in 1..ev.len() {
        let mut j = i;
        while j > 0 {
            let (Some(a), Some(b)) = (ev.get(j - 1), ev.get(j)) else { break };
            if key(a) <= key(b) {
                break;
            }
            ev.swap(j - 1, j);
            j -= 1;
        }
    }
}

/// Converts a MIDI note number to Hz (A4 = 440).
#[inline]
pub(crate) fn note_hz(note: f32) -> f32 {
    440.0 * 2f32.powf((note - 69.0) / 12.0)
}

#[cfg(test)]
mod tests {
    #[test]
    fn flush_matches_the_finite_and_magnitude_test() {
        let edge = 1.0e-25f32.to_bits();
        let special = [0, 0x8000_0000, 1, edge - 1, edge, edge + 1, 0x7f7f_ffff, 0x7f80_0000, 0xff80_0000, 0x7fc0_0000, 0xffc0_0001];
        for b in (0..=u32::MAX).step_by(997).chain(special).chain(special.map(|b| b | 0x8000_0000)) {
            let x = f32::from_bits(b);
            let want = if x.is_finite() && x.abs() > 1.0e-25 { x } else { 0.0 };
            assert_eq!(super::flush(x).to_bits(), want.to_bits(), "{b:#x}");
        }
    }
}
