//! A windowed-sinc sample-rate converter, for session rates MPEG-1 Layer III cannot carry (it
//! holds 32, 44.1 and 48 kHz).

/// Zero crossings of the kernel on each side.
const CROSSINGS: f64 = 24.0;
/// Kernel table entries per input sample.
const STEPS: usize = 256;
/// The most output phases kept as precomputed filters (the rest interpolate the kernel table).
const MAX_PHASES: u64 = 1024;

/// The windowed-sinc kernel at `u` input samples from its centre.
fn kernel_at(u: f64, cutoff: f64, half: i64) -> f32 {
    let u = u.abs();
    if u >= half as f64 {
        return 0.0;
    }
    let x = std::f64::consts::PI * cutoff * u;
    let sinc = if x == 0.0 { 1.0 } else { x.sin() / x };
    // Blackman window over [-half, half].
    let w = std::f64::consts::PI * (u / half as f64 + 1.0);
    (cutoff * sinc * (0.42 - 0.5 * w.cos() + 0.08 * (2.0 * w).cos())) as f32
}

fn gcd(a: u64, b: u64) -> u64 {
    if b == 0 { a } else { gcd(b, a % b) }
}

pub(super) struct Resampler {
    from: u64,
    to: u64,
    /// Kernel half-width in input samples.
    half: i64,
    /// The kernel, sampled every 1/[`STEPS`] input samples from 0 to `half`.
    kernel: Vec<f32>,
    /// When the rates' ratio has few phases: `2 * half` taps per phase, and the input-rate step
    /// between phases.
    phases: Option<(Vec<f32>, u64)>,
    /// Pending input per channel; `input[c][0]` is input sample `base`.
    input: Vec<Vec<f32>>,
    base: i64,
    /// Input samples received.
    received: u64,
    /// Output samples produced.
    produced: u64,
}

impl Resampler {
    pub(super) fn new(channels: usize, from: u32, to: u32) -> Self {
        let (from, to) = (u64::from(from.max(1)), u64::from(to.max(1)));
        // Cut off a little below the lower Nyquist frequency.
        let cutoff = (to as f64 / from as f64).min(1.0) * 0.94;
        let half = (CROSSINGS / cutoff).ceil() as i64;
        let len = half as usize * STEPS + 2;
        let g = gcd(from, to);
        let phases = (to / g <= MAX_PHASES).then(|| {
            let taps = 2 * half;
            let filters = (0..to / g)
                .flat_map(|p| {
                    let frac = (p * g) as f64 / to as f64;
                    (0..taps).map(move |j| kernel_at((half - 1 - j) as f64 + frac, cutoff, half))
                })
                .collect();
            (filters, g)
        });
        let kernel = if phases.is_some() { Vec::new() } else { (0..len).map(|i| kernel_at(i as f64 / STEPS as f64, cutoff, half)).collect() };
        // Silence before the first sample.
        let input = vec![vec![0.0; half as usize]; channels];
        Resampler { from, to, half, kernel, phases, input, base: -half, received: 0, produced: 0 }
    }

    /// Output samples per channel for `frames` input samples, at most.
    pub(super) fn output_len(&self, frames: u64) -> u64 {
        (u128::from(frames) * u128::from(self.to)).div_ceil(u128::from(self.from)).try_into().unwrap_or(u64::MAX)
    }

    /// Takes `frames` input samples of `block` (missing samples are silence) and appends the
    /// output they complete to `out`.
    pub(super) fn push(&mut self, block: &[Vec<f32>], frames: usize, out: &mut [Vec<f32>]) {
        for (c, input) in self.input.iter_mut().enumerate() {
            let src = block.get(c).map_or(&[][..], |b| b.get(..frames.min(b.len())).unwrap_or_default());
            input.extend_from_slice(src);
            input.resize(input.len() + (frames - src.len()), 0.0);
        }
        self.received += frames as u64;
        self.run(out, false);
    }

    /// Appends the rest of the output to `out`.
    pub(super) fn finish(&mut self, out: &mut [Vec<f32>]) {
        let half = self.half as usize;
        for input in &mut self.input {
            input.resize(input.len() + half + 1, 0.0);
        }
        self.run(out, true);
    }

    fn run(&mut self, out: &mut [Vec<f32>], last: bool) {
        let have = self.input.first().map_or(0, Vec::len) as i64;
        let total = self.output_len(self.received);
        loop {
            if last && self.produced >= total {
                break;
            }
            let num = u128::from(self.produced) * u128::from(self.from);
            let k = i64::try_from(num / u128::from(self.to)).unwrap_or(i64::MAX);
            let frac = (num % u128::from(self.to)) as f64 / self.to as f64;
            // Needs input samples k - half + 1 ..= k + half.
            if k.saturating_add(self.half) >= self.base + have {
                break;
            }
            let first = k - self.half + 1 - self.base;
            let taps = 2 * self.half as usize;
            if let Some((filters, g)) = &self.phases {
                let phase = (num % u128::from(self.to)) as usize / *g as usize;
                let filter = filters.get(phase * taps..(phase + 1) * taps).unwrap_or_default();
                for (input, out) in self.input.iter().zip(out.iter_mut()) {
                    let window = input.get(first.max(0) as usize..).unwrap_or_default();
                    out.push(window.iter().zip(filter).map(|(x, h)| x * h).sum());
                }
                self.produced += 1;
                continue;
            }
            for (input, out) in self.input.iter().zip(out.iter_mut()) {
                let mut acc = 0.0f32;
                for (j, x) in input.iter().skip(first.max(0) as usize).take(2 * self.half as usize).enumerate() {
                    // Distance of this input sample from the output instant.
                    let d = ((self.half - 1 - j as i64) as f64 + frac).abs() * STEPS as f64;
                    let i = d as usize;
                    let t = (d - i as f64) as f32;
                    let (a, b) = (self.kernel.get(i).copied().unwrap_or(0.0), self.kernel.get(i + 1).copied().unwrap_or(0.0));
                    acc += x * (a + (b - a) * t);
                }
                out.push(acc);
            }
            self.produced += 1;
        }
        // Drop the input no later output needs.
        let num = u128::from(self.produced) * u128::from(self.from);
        let next = i64::try_from(num / u128::from(self.to)).unwrap_or(i64::MAX);
        let drop = (next - self.half + 1 - self.base).clamp(0, have);
        if drop > 4096 {
            for input in &mut self.input {
                input.drain(..drop as usize);
            }
            self.base += drop;
        }
    }
}
