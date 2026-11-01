//! Room Reverb and Plate Reverb: pre-delay → allpass input diffusion → 8-line feedback delay
//! network (Hadamard mixing, per-line RT60 gains, in-loop HF damping, gentle delay modulation).

use crate::biquad::{Biquad, FilterType};
use crate::params::{Params, lin, log};
use crate::util::{DelayLine, Smoother, flush, frames_in, ms_to_samples, sane_channels, sane_sr};
use crate::{Category, ParamInfo, Plugin, PluginInfo, Unit};

const LINES: usize = 8;
const DIFFUSERS: usize = 4;
const MAX_PREDELAY_MS: f32 = 250.0;
const SIZE_MIN: f32 = 0.35;
const SIZE_MAX: f32 = 1.6;

struct Config {
    delays_ms: [f32; LINES],
    diffusers_ms: [f32; DIFFUSERS],
    mod_ms: f32,
    mod_hz: f32,
}

const ROOM: Config = Config {
    delays_ms: [23.13, 27.71, 31.87, 36.29, 41.33, 45.67, 51.09, 56.93],
    diffusers_ms: [4.71, 3.59, 12.73, 9.31],
    mod_ms: 0.25,
    mod_hz: 0.7,
};

const PLATE: Config =
    Config { delays_ms: [13.71, 17.93, 21.07, 24.73, 28.31, 32.89, 36.13, 40.31], diffusers_ms: [3.13, 4.27, 7.93, 11.29], mod_ms: 0.4, mod_hz: 1.1 };

macro_rules! reverb_params {
    ($mix:expr, $pre:expr, $decay:expr, $damp:expr, $diff:expr) => {
        [
            lin("mix", "Mix", 0.0, 100.0, $mix, Unit::Percent),
            lin("pre_delay", "Pre-Delay", 0.0, MAX_PREDELAY_MS, $pre, Unit::Ms),
            log("decay", "Decay", 0.1, 20.0, $decay, Unit::Seconds),
            lin("size", "Size", 0.0, 100.0, 50.0, Unit::Percent),
            log("damping", "HF Damping", 1000.0, 20000.0, $damp, Unit::Hz),
            lin("diffusion", "Diffusion", 0.0, 100.0, $diff, Unit::Percent),
            lin("width", "Width", 0.0, 100.0, 100.0, Unit::Percent),
            log("low_cut", "Low Cut", 20.0, 1000.0, 20.0, Unit::Hz),
        ]
    };
}

static ROOM_PARAMS: [ParamInfo; 8] = reverb_params!(25.0, 10.0, 1.2, 6000.0, 70.0);
static PLATE_PARAMS: [ParamInfo; 8] = reverb_params!(30.0, 20.0, 2.5, 10000.0, 85.0);

pub static ROOM_INFO: PluginInfo = PluginInfo {
    id: "room_reverb",
    name: "Room Reverb",
    short_name: "Room",
    category: Category::Reverb,
    params: &ROOM_PARAMS,
    is_instrument: false,
    audiosuite: true,
};

pub static PLATE_INFO: PluginInfo = PluginInfo {
    id: "plate_reverb",
    name: "Plate Reverb",
    short_name: "Plate",
    category: Category::Reverb,
    params: &PLATE_PARAMS,
    is_instrument: false,
    audiosuite: true,
};

/// Schroeder allpass section with a fixed delay.
#[derive(Default)]
struct Allpass {
    buf: Vec<f32>,
    pos: usize,
}

impl Allpass {
    fn allocate(&mut self, len: usize) {
        self.buf = vec![0.0; len.max(1)];
        self.pos = 0;
    }

    #[inline]
    fn process(&mut self, x: f32, g: f32) -> f32 {
        let Some(slot) = self.buf.get_mut(self.pos) else { return x };
        let d = *slot;
        let v = x + g * d;
        *slot = flush(v);
        self.pos += 1;
        if self.pos >= self.buf.len() {
            self.pos = 0;
        }
        d - g * v
    }
}

/// In-place normalized 8-point fast Walsh–Hadamard transform (an orthogonal mixing matrix).
#[inline]
fn hadamard8(v: &mut [f32; LINES]) {
    let mut h = 1;
    while h < LINES {
        let mut i = 0;
        while i < LINES {
            for j in i..i + h {
                let (a, b) = (v[j], v[j + h]);
                v[j] = a + b;
                v[j + h] = a - b;
            }
            i += h * 2;
        }
        h *= 2;
    }
    let s = 1.0 / (LINES as f32).sqrt();
    v.iter_mut().for_each(|x| *x *= s);
}

/// FDN reverb shared by the room and plate models.
pub struct Reverb {
    info: &'static PluginInfo,
    cfg: &'static Config,
    p: Params,
    sr: f32,
    ch: usize,
    pre: [DelayLine; 2],
    diff: [[Allpass; DIFFUSERS]; 2],
    lines: [DelayLine; LINES],
    lp: [f32; LINES],
    gains: [f32; LINES],
    damp: f32,
    lowcut: [Biquad; 2],
    mix: Smoother,
    size: Smoother,
    pre_ms: Smoother,
    width: Smoother,
    lfo: f32,
}

impl Reverb {
    pub fn room() -> Self {
        Self::build(&ROOM_INFO, &ROOM)
    }

    pub fn plate() -> Self {
        Self::build(&PLATE_INFO, &PLATE)
    }

    fn build(info: &'static PluginInfo, cfg: &'static Config) -> Self {
        let mut r = Self {
            info,
            cfg,
            p: Params::new(info.params),
            sr: 48_000.0,
            ch: 0,
            pre: Default::default(),
            diff: Default::default(),
            lines: Default::default(),
            lp: [0.0; LINES],
            gains: [0.0; LINES],
            damp: 0.0,
            lowcut: [Biquad::default(); 2],
            mix: Smoother::new(0.0),
            size: Smoother::new(1.0),
            pre_ms: Smoother::new(0.0),
            width: Smoother::new(1.0),
            lfo: 0.0,
        };
        r.update();
        r.mix.snap();
        r.size.snap();
        r.pre_ms.snap();
        r.width.snap();
        r
    }

    fn size_factor(&self) -> f32 {
        SIZE_MIN + (SIZE_MAX - SIZE_MIN) * self.p.v(3) / 100.0
    }

    fn update(&mut self) {
        self.mix.set(self.p.v(0) / 100.0);
        self.pre_ms.set(self.p.v(1));
        self.size.set(self.size_factor());
        self.width.set(self.p.v(6) / 100.0);
        let lc = self.p.v(7);
        let sr = self.sr;
        for b in &mut self.lowcut {
            b.set(FilterType::HighPass, lc, std::f32::consts::FRAC_1_SQRT_2, 0.0, sr);
        }
        self.damp = (-std::f32::consts::TAU * self.p.v(4) / sr).exp();
        self.update_gains();
    }

    /// Per-line feedback gain for the target RT60 at the current (smoothed) size.
    fn update_gains(&mut self) {
        let rt60 = self.p.v(2).max(0.05);
        let size = self.size.value();
        for (g, ms) in self.gains.iter_mut().zip(self.cfg.delays_ms) {
            let secs = ms * size * 0.001;
            *g = 10f32.powf(-3.0 * secs / rt60).min(0.9999);
        }
    }
}

/// Input sample `n` of the left and right channels (a mono input feeds both); non-finite reads as 0.
fn input_pair(io: &[Vec<f32>], ch: usize, n: usize) -> (f32, f32) {
    let xl = io.first().and_then(|b| b.get(n)).copied().filter(|v| v.is_finite()).unwrap_or(0.0);
    let xr = if ch > 1 { io.get(1).and_then(|b| b.get(n)).copied().filter(|v| v.is_finite()).unwrap_or(0.0) } else { xl };
    (xl, xr)
}

/// Blend the wet signal `[mid, left, right]` into sample `n` of the first `ch` channels at mix `m`.
fn mix_out(io: &mut [Vec<f32>], ch: usize, n: usize, wet: [f32; 3], m: f32) {
    let [mid, wl, wr] = wet;
    for (c, b) in io.iter_mut().take(ch).enumerate() {
        let wet = if ch == 1 {
            mid
        } else if c % 2 == 0 {
            wl
        } else {
            wr
        };
        if let Some(x) = b.get_mut(n) {
            let dry = if x.is_finite() { *x } else { 0.0 };
            *x = dry * (1.0 - m) + wet * m;
        }
    }
}

impl Plugin for Reverb {
    fn info(&self) -> &'static PluginInfo {
        self.info
    }

    fn set_param(&mut self, id: &str, value: f32) -> bool {
        if self.p.set(id, value).is_some() {
            self.update();
            true
        } else {
            false
        }
    }

    fn param(&self, id: &str) -> Option<f32> {
        self.p.get(id)
    }

    fn prepare(&mut self, sample_rate: f32, _max_block: usize, channels: usize) {
        self.sr = sane_sr(sample_rate);
        self.ch = sane_channels(channels);
        let sr = self.sr;
        let pre_max = ms_to_samples(MAX_PREDELAY_MS, sr) as usize + 4;
        self.pre.iter_mut().for_each(|d| d.allocate(pre_max));
        for (side, aps) in self.diff.iter_mut().enumerate() {
            let spread = if side == 0 { 1.0 } else { 1.071 };
            for (ap, ms) in aps.iter_mut().zip(self.cfg.diffusers_ms) {
                ap.allocate(ms_to_samples(ms * spread, sr) as usize);
            }
        }
        let mod_max = ms_to_samples(self.cfg.mod_ms, sr) + 4.0;
        for (l, ms) in self.lines.iter_mut().zip(self.cfg.delays_ms) {
            l.allocate((ms_to_samples(ms * SIZE_MAX, sr) + mod_max) as usize);
        }
        self.mix.set_time(30.0, sr);
        self.size.set_time(200.0, sr);
        self.pre_ms.set_time(100.0, sr);
        self.width.set_time(30.0, sr);
        self.update();
        self.reset();
    }

    fn reset(&mut self) {
        self.pre.iter_mut().for_each(DelayLine::clear);
        for aps in &mut self.diff {
            for ap in aps.iter_mut() {
                ap.buf.iter_mut().for_each(|v| *v = 0.0);
            }
        }
        self.lines.iter_mut().for_each(DelayLine::clear);
        self.lp = [0.0; LINES];
        self.lowcut.iter_mut().for_each(Biquad::reset);
        self.mix.snap();
        self.size.snap();
        self.pre_ms.snap();
        self.width.snap();
        self.update_gains();
        self.lfo = 0.0;
    }

    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        let ch = self.ch.min(io.len());
        if ch == 0 {
            return;
        }
        let frames = frames_in(io, frames, ch);
        let sr = self.sr;
        let g_diff = 0.7 * self.p.v(5) / 100.0;
        let mod_depth = ms_to_samples(self.cfg.mod_ms, sr);
        let lfo_inc = self.cfg.mod_hz / sr;
        let damp = self.damp;
        if self.size.settling() {
            self.size.advance(frames);
            self.update_gains();
        }
        let size = self.size.value();
        let base: [f32; LINES] = self.cfg.delays_ms.map(|ms| ms_to_samples(ms * size, sr));
        for n in 0..frames {
            let (xl, xr) = input_pair(io, ch, n);
            let pre = ms_to_samples(self.pre_ms.next(), sr);
            let mut ins = [xl, xr];
            for (side, x) in ins.iter_mut().enumerate() {
                let d = &mut self.pre[side];
                d.push(*x);
                let mut v = if pre < 1.0 { *x } else { d.read(pre) };
                for ap in &mut self.diff[side] {
                    v = ap.process(v, g_diff);
                }
                *x = v;
            }
            self.lfo += lfo_inc;
            if self.lfo >= 1.0 {
                self.lfo -= 1.0;
            }
            let (s, c) = (self.lfo * std::f32::consts::TAU).sin_cos();
            let mods = [0.0, s, 0.0, c, 0.0, -s, 0.0, -c];
            let mut o = [0.0f32; LINES];
            for i in 0..LINES {
                let raw = self.lines[i].read(base[i] + mod_depth * (1.0 + mods[i]));
                self.lp[i] = flush(raw + (self.lp[i] - raw) * damp);
                o[i] = raw;
            }
            let mut fb: [f32; LINES] = std::array::from_fn(|i| self.lp[i] * self.gains[i]);
            hadamard8(&mut fb);
            for (i, (line, f)) in self.lines.iter_mut().zip(fb).enumerate() {
                let inj = if i % 2 == 0 { ins[0] } else { ins[1] };
                line.push(flush(f + inj * 0.5));
            }
            let wl = 0.4 * (o[0] - o[2] + o[4] - o[6]);
            let wr = 0.4 * (o[1] - o[3] + o[5] - o[7]);
            let wl = self.lowcut[0].process_sample(wl);
            let wr = self.lowcut[1].process_sample(wr);
            let w = self.width.next();
            let mid = 0.5 * (wl + wr);
            let side = 0.5 * (wl - wr) * w;
            let (wl, wr) = (mid + side, mid - side);
            let m = self.mix.next();
            mix_out(io, ch, n, [mid, wl, wr], m);
        }
    }

    fn tail_samples(&self) -> usize {
        let secs = self.p.v(2) * 1.3 + self.p.v(1) * 0.001 + 0.1;
        (secs * self.sr) as usize
    }
}
