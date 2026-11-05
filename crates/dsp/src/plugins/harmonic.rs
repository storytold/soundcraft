//! Harmonic effects: Saturator (tanh), Lo-Fi (bit/sample-rate reduction), Rectifier and
//! Subharmonic (octave-down bass synthesis).

use crate::biquad::{Biquad, FilterType};
use crate::params::{Params, choice, lin, log, param_plumbing, toggle};
use crate::util::{DcBlocker, Rng, Smoother, flush, frames_in, sane_channels, sane_sr, time_coef};
use crate::{Category, ParamInfo, Plugin, PluginInfo, Unit, db_to_gain};

static SAT_PARAMS: [ParamInfo; 5] = [
    lin("drive", "Drive", 0.0, 36.0, 6.0, Unit::Db),
    lin("bias", "Asymmetry", 0.0, 100.0, 0.0, Unit::Percent),
    log("tone", "Tone", 1000.0, 20000.0, 20000.0, Unit::Hz),
    lin("mix", "Mix", 0.0, 100.0, 100.0, Unit::Percent),
    lin("output", "Output", -24.0, 12.0, 0.0, Unit::Db),
];

pub static SAT_INFO: PluginInfo = PluginInfo {
    id: "saturator",
    name: "Saturator",
    short_name: "Satur",
    category: Category::Harmonic,
    params: &SAT_PARAMS,
    is_instrument: false,
    audiosuite: true,
};

/// Smooth tanh waveshaper, normalized so a full-scale input still peaks near full scale (more
/// drive = more harmonics and density). Asymmetry adds even harmonics (the DC is removed).
pub struct Saturator {
    p: Params,
    sr: f32,
    dc: Vec<DcBlocker>,
    lp: Vec<f32>,
    drive: Smoother,
    bias: Smoother,
    mix: Smoother,
    out: Smoother,
    tone: f32,
}

impl Default for Saturator {
    fn default() -> Self {
        Self::new()
    }
}

impl Saturator {
    pub fn new() -> Self {
        let mut s = Self {
            p: Params::new(&SAT_PARAMS),
            sr: 48_000.0,
            dc: Vec::new(),
            lp: Vec::new(),
            drive: Smoother::new(1.0),
            bias: Smoother::new(0.0),
            mix: Smoother::new(1.0),
            out: Smoother::new(1.0),
            tone: 0.0,
        };
        s.update();
        s
    }

    fn update(&mut self) {
        self.drive.set(db_to_gain(self.p.v(0)));
        self.bias.set(self.p.v(1) / 100.0 * 0.5);
        self.mix.set(self.p.v(3) / 100.0);
        self.out.set(db_to_gain(self.p.v(4)));
        let f = self.p.v(2);
        self.tone = if f >= 19_999.0 { 0.0 } else { (-std::f32::consts::TAU * f / self.sr).exp() };
    }
}

impl Plugin for Saturator {
    param_plumbing!(SAT_INFO);

    fn prepare(&mut self, sample_rate: f32, _max_block: usize, channels: usize) {
        self.sr = sane_sr(sample_rate);
        let ch = sane_channels(channels);
        self.dc = vec![DcBlocker::new(10.0, self.sr); ch];
        self.lp = vec![0.0; ch];
        for s in [&mut self.drive, &mut self.bias, &mut self.mix, &mut self.out] {
            s.set_time(20.0, self.sr);
        }
        self.update();
        self.reset();
    }

    fn reset(&mut self) {
        self.dc.iter_mut().for_each(DcBlocker::reset);
        self.lp.iter_mut().for_each(|v| *v = 0.0);
        for s in [&mut self.drive, &mut self.bias, &mut self.mix, &mut self.out] {
            s.snap();
        }
    }

    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        let ch = self.dc.len().min(io.len());
        let frames = frames_in(io, frames, ch);
        let a = self.tone;
        for n in 0..frames {
            let g = self.drive.next();
            let b = self.bias.next();
            let m = self.mix.next();
            let o = self.out.next();
            let tb = b.tanh();
            let norm = 1.0 / ((g + b).tanh() - tb).max(1e-3);
            for c in 0..ch {
                let (Some(buf), Some(dc), Some(lp)) = (io.get_mut(c), self.dc.get_mut(c), self.lp.get_mut(c)) else { continue };
                let Some(x) = buf.get_mut(n) else { continue };
                let dry = if x.is_finite() { *x } else { 0.0 };
                let mut y = ((g * dry + b).tanh() - tb) * norm;
                if b > 0.0 {
                    y = dc.process(y);
                }
                if a > 0.0 {
                    *lp = flush(y + (*lp - y) * a);
                    y = *lp;
                }
                *x = (dry * (1.0 - m) + y * m) * o;
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------

static LOFI_PARAMS: [ParamInfo; 4] = [
    lin("bits", "Bit Depth", 1.0, 24.0, 8.0, Unit::None),
    log("sample_rate", "Sample Rate", 500.0, 48000.0, 11025.0, Unit::Hz),
    lin("noise", "Noise", 0.0, 100.0, 0.0, Unit::Percent),
    lin("mix", "Mix", 0.0, 100.0, 100.0, Unit::Percent),
];

pub static LOFI_INFO: PluginInfo = PluginInfo {
    id: "lofi",
    name: "Lo-Fi",
    short_name: "LoFi",
    category: Category::Harmonic,
    params: &LOFI_PARAMS,
    is_instrument: false,
    audiosuite: true,
};

/// Bit crusher and sample-and-hold decimator with optional hiss.
pub struct LoFi {
    p: Params,
    sr: f32,
    held: Vec<f32>,
    phase: f32,
    rng: Rng,
    mix: Smoother,
    noise: Smoother,
}

impl Default for LoFi {
    fn default() -> Self {
        Self::new()
    }
}

impl LoFi {
    pub fn new() -> Self {
        let mut l = Self {
            p: Params::new(&LOFI_PARAMS),
            sr: 48_000.0,
            held: Vec::new(),
            phase: 1.0,
            rng: Rng::new(77),
            mix: Smoother::new(1.0),
            noise: Smoother::new(0.0),
        };
        l.update();
        l
    }

    fn update(&mut self) {
        self.mix.set(self.p.v(3) / 100.0);
        let n = self.p.v(2) / 100.0;
        self.noise.set(n * n * 0.1);
    }
}

impl Plugin for LoFi {
    param_plumbing!(LOFI_INFO);

    fn prepare(&mut self, sample_rate: f32, _max_block: usize, channels: usize) {
        self.sr = sane_sr(sample_rate);
        self.held = vec![0.0; sane_channels(channels)];
        self.mix.set_time(20.0, self.sr);
        self.noise.set_time(20.0, self.sr);
        self.update();
        self.reset();
    }

    fn reset(&mut self) {
        self.held.iter_mut().for_each(|v| *v = 0.0);
        self.phase = 1.0;
        self.mix.snap();
        self.noise.snap();
    }

    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        let ch = self.held.len().min(io.len());
        let frames = frames_in(io, frames, ch);
        let step = 2f32.powf(1.0 - self.p.v(0));
        let inc = (self.p.v(1) / self.sr).min(1.0);
        for n in 0..frames {
            let m = self.mix.next();
            let nz = self.noise.next();
            self.phase += inc;
            let grab = self.phase >= 1.0;
            if grab {
                self.phase -= 1.0;
            }
            for c in 0..ch {
                let (Some(buf), Some(h)) = (io.get_mut(c), self.held.get_mut(c)) else { continue };
                let Some(x) = buf.get_mut(n) else { continue };
                let dry = if x.is_finite() { *x } else { 0.0 };
                if grab {
                    let noisy = dry + if nz > 0.0 { self.rng.bipolar() * nz } else { 0.0 };
                    *h = (noisy / step).round() * step;
                }
                *x = dry * (1.0 - m) + *h * m;
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------

static RECT_PARAMS: [ParamInfo; 3] = [
    choice("mode", "Mode", &["Half Wave", "Full Wave"], 1),
    lin("mix", "Mix", 0.0, 100.0, 100.0, Unit::Percent),
    lin("output", "Output", -24.0, 12.0, 0.0, Unit::Db),
];

pub static RECT_INFO: PluginInfo = PluginInfo {
    id: "rectifier",
    name: "Rectifier",
    short_name: "Rectify",
    category: Category::Harmonic,
    params: &RECT_PARAMS,
    is_instrument: false,
    audiosuite: true,
};

/// Half/full-wave rectifier (octave-up harmonics) with DC removal.
pub struct Rectifier {
    p: Params,
    sr: f32,
    dc: Vec<DcBlocker>,
    mix: Smoother,
    out: Smoother,
}

impl Default for Rectifier {
    fn default() -> Self {
        Self::new()
    }
}

impl Rectifier {
    pub fn new() -> Self {
        let mut r = Self { p: Params::new(&RECT_PARAMS), sr: 48_000.0, dc: Vec::new(), mix: Smoother::new(1.0), out: Smoother::new(1.0) };
        r.update();
        r
    }

    fn update(&mut self) {
        self.mix.set(self.p.v(1) / 100.0);
        self.out.set(db_to_gain(self.p.v(2)));
    }
}

impl Plugin for Rectifier {
    param_plumbing!(RECT_INFO);

    fn prepare(&mut self, sample_rate: f32, _max_block: usize, channels: usize) {
        self.sr = sane_sr(sample_rate);
        self.dc = vec![DcBlocker::new(10.0, self.sr); sane_channels(channels)];
        self.mix.set_time(20.0, self.sr);
        self.out.set_time(20.0, self.sr);
        self.update();
        self.reset();
    }

    fn reset(&mut self) {
        self.dc.iter_mut().for_each(DcBlocker::reset);
        self.mix.snap();
        self.out.snap();
    }

    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        let ch = self.dc.len().min(io.len());
        let frames = frames_in(io, frames, ch);
        let full = self.p.choice(0) == 1;
        for n in 0..frames {
            let m = self.mix.next();
            let o = self.out.next();
            for c in 0..ch {
                let (Some(buf), Some(dc)) = (io.get_mut(c), self.dc.get_mut(c)) else { continue };
                let Some(x) = buf.get_mut(n) else { continue };
                let dry = if x.is_finite() { *x } else { 0.0 };
                let r = if full { dry.abs() } else { dry.max(0.0) };
                let wet = dc.process(r);
                *x = (dry * (1.0 - m) + wet * m) * o;
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------

const SUB_RANGES: &[&str] = &["120-180 Hz", "80-120 Hz", "60-90 Hz", "40-60 Hz"];
/// Lower and upper band frequency of each range.
const SUB_RANGE_HZ: [(f32, f32); 4] = [(120.0, 180.0), (80.0, 120.0), (60.0, 90.0), (40.0, 60.0)];
const SUB_CHARACTERS: &[&str] = &["Clean I", "Clean II", "Resonant I", "Resonant II", "Distort I", "Distort II"];
/// Band gains at or below this are off.
const SUB_OFF_DB: f32 = -120.0;
/// Q of the band-pass that picks each band out of the input.
const SUB_BAND_Q: f32 = 2.0;
/// Cutoff of the low-pass after each divider, as a fraction of the band frequency.
const SUB_SMOOTH: f32 = 0.75;
/// Zero-crossing hysteresis, as a fraction of the band's envelope.
const SUB_HYSTERESIS: f32 = 0.1;
/// Release of the band envelope that sets the hysteresis.
const SUB_ENV_RELEASE_MS: f32 = 50.0;
/// Bias of the asymmetric Distort I curve.
const SUB_ASYM: f32 = 0.25;

static SUB_PARAMS: [ParamInfo; 17] = [
    lin("input", "Input", -18.0, 18.0, 0.0, Unit::Db),
    choice("range", "Range", SUB_RANGES, 2),
    lin("lower_gain", "Lower Gain", SUB_OFF_DB, 24.0, 0.0, Unit::Db),
    lin("upper_gain", "Upper Gain", SUB_OFF_DB, 24.0, -6.0, Unit::Db),
    lin("direct_gain", "Direct Gain", SUB_OFF_DB, 24.0, SUB_OFF_DB, Unit::Db),
    toggle("lower_solo", "Lower Solo", false),
    toggle("upper_solo", "Upper Solo", false),
    toggle("direct_solo", "Direct Solo", false),
    lin("drive", "Drive", -18.0, 18.0, 0.0, Unit::Db),
    choice("character", "Character", SUB_CHARACTERS, 0),
    log("hp_freq", "HP Freq", 5.0, 1000.0, 20.0, Unit::Hz),
    log("hp_q", "HP Q", 0.1, 5.0, 0.707, Unit::None),
    log("lp_freq", "LP Freq", 5.0, 1000.0, 200.0, Unit::Hz),
    log("lp_q", "LP Q", 0.1, 5.0, 0.707, Unit::None),
    lin("mix", "Mix", 0.0, 100.0, 50.0, Unit::Percent),
    toggle("wet_solo", "Wet Solo", false),
    lin("output", "Output", -18.0, 18.0, 0.0, Unit::Db),
];

pub static SUB_INFO: PluginInfo = PluginInfo {
    id: "subharmonic",
    name: "Subharmonic",
    short_name: "SubHarm",
    category: Category::Harmonic,
    params: &SUB_PARAMS,
    is_instrument: false,
    audiosuite: true,
};

/// One synthesized band: a band-pass, a divide-by-two flip-flop and a smoothing low-pass.
#[derive(Debug, Clone, Copy)]
struct SubBand {
    band: Biquad,
    smooth: Biquad,
    /// Divider output: ±1, flipped on every rising zero crossing of the band.
    flip: f32,
    /// The band has gone below the negative threshold since the last flip.
    armed: bool,
    env: f32,
}

impl SubBand {
    fn new() -> Self {
        Self { band: Biquad::default(), smooth: Biquad::default(), flip: 1.0, armed: false, env: 0.0 }
    }

    fn reset(&mut self) {
        self.band.reset();
        self.smooth.reset();
        self.flip = 1.0;
        self.armed = false;
        self.env = 0.0;
    }

    /// The octave below the band's part of `x`; `rel` is the envelope's release coefficient.
    #[inline]
    fn process(&mut self, x: f32, rel: f32) -> f32 {
        let b = self.band.process_sample(x);
        let a = b.abs();
        self.env = flush(if a > self.env { a } else { self.env * rel });
        let th = self.env * SUB_HYSTERESIS;
        if b < -th {
            self.armed = true;
        } else if self.armed && b > th {
            self.armed = false;
            self.flip = -self.flip;
        }
        self.smooth.process_sample(b * self.flip)
    }
}

/// Filter and divider state of one channel.
#[derive(Debug, Clone, Copy)]
struct SubChannel {
    /// The lower and the upper band.
    bands: [SubBand; 2],
    /// Low-pass that takes the direct bass out of the input.
    direct: Biquad,
    hp: Biquad,
    lp: Biquad,
    dc: DcBlocker,
}

impl SubChannel {
    fn new(sr: f32) -> Self {
        Self { bands: [SubBand::new(); 2], direct: Biquad::default(), hp: Biquad::default(), lp: Biquad::default(), dc: DcBlocker::new(5.0, sr) }
    }

    fn reset(&mut self) {
        self.bands.iter_mut().for_each(SubBand::reset);
        self.direct.reset();
        self.hp.reset();
        self.lp.reset();
        self.dc.reset();
    }
}

/// Subharmonic synthesizer. The Range sets a lower and an upper band frequency. Each band
/// band-passes the input at its frequency, and a divide-by-two flip-flop on the band's rising zero
/// crossings multiplies the band, which moves it an octave down at a level that follows the band.
/// The direct band is the input low-passed at the upper band frequency.
///
/// - The three bands are summed with their gains, raised by Drive and saturated, then
///   shaped by the HP and LP filters. A soloed band plays alone and mutes the dry signal.
/// - Clean I saturates with tanh and Clean II with a softer algebraic curve. Resonant I and II use
///   tanh and raise the Q of the low-pass after each divider to 2 and 4. Distort I bends the curve
///   asymmetrically (even harmonics, DC removed) and Distort II clips hard at full scale.
/// - Below 50 % Mix the dry signal stays at full level and the wet level falls; above 50 % the wet
///   signal stays at full level and the dry level falls. Wet Solo mutes the dry signal.
pub struct SubHarmonic {
    p: Params,
    sr: f32,
    ch: Vec<SubChannel>,
    input: Smoother,
    bands: [Smoother; 3],
    drive: Smoother,
    dry: Smoother,
    wet: Smoother,
    out: Smoother,
    env_rel: f32,
}

impl Default for SubHarmonic {
    fn default() -> Self {
        Self::new()
    }
}

impl SubHarmonic {
    pub fn new() -> Self {
        let mut s = Self {
            p: Params::new(&SUB_PARAMS),
            sr: 48_000.0,
            ch: Vec::new(),
            input: Smoother::new(1.0),
            bands: [Smoother::new(0.0); 3],
            drive: Smoother::new(1.0),
            dry: Smoother::new(1.0),
            wet: Smoother::new(1.0),
            out: Smoother::new(1.0),
            env_rel: 0.0,
        };
        s.update();
        s
    }

    /// The lower and upper band frequencies of the current range.
    fn range_hz(&self) -> (f32, f32) {
        SUB_RANGE_HZ.get(self.p.choice(1)).copied().unwrap_or((60.0, 90.0))
    }

    /// Q of the low-pass after each divider for the current Drive Character.
    fn smooth_q(&self) -> f32 {
        match self.p.choice(9) {
            2 => 2.0,
            3 => 4.0,
            _ => std::f32::consts::FRAC_1_SQRT_2,
        }
    }

    fn update(&mut self) {
        let gain = |db: f32| if db <= SUB_OFF_DB { 0.0 } else { db_to_gain(db) };
        let solo = [self.p.on(5), self.p.on(6), self.p.on(7)];
        let any_solo = solo.contains(&true);
        for (k, s) in self.bands.iter_mut().enumerate() {
            let heard = !any_solo || solo.get(k).copied().unwrap_or(false);
            s.set(if heard { gain(self.p.v(2 + k)) } else { 0.0 });
        }
        self.input.set(db_to_gain(self.p.v(0)));
        self.drive.set(db_to_gain(self.p.v(8)));
        self.out.set(db_to_gain(self.p.v(16)));
        let m = self.p.v(14) / 100.0;
        self.wet.set((2.0 * m).min(1.0));
        self.dry.set(if any_solo || self.p.on(15) { 0.0 } else { (2.0 * (1.0 - m)).min(1.0) });
        self.env_rel = time_coef(SUB_ENV_RELEASE_MS, self.sr);

        let (lo, hi) = self.range_hz();
        let sq = self.smooth_q();
        let sr = self.sr;
        let (hp_f, hp_q, lp_f, lp_q) = (self.p.v(10), self.p.v(11), self.p.v(12), self.p.v(13));
        for c in &mut self.ch {
            for (b, f) in c.bands.iter_mut().zip([lo, hi]) {
                b.band.set(FilterType::BandPass, f, SUB_BAND_Q, 0.0, sr);
                b.smooth.set(FilterType::LowPass, f * SUB_SMOOTH, sq, 0.0, sr);
            }
            c.direct.set(FilterType::LowPass, hi, std::f32::consts::FRAC_1_SQRT_2, 0.0, sr);
            c.hp.set(FilterType::HighPass, hp_f, hp_q, 0.0, sr);
            c.lp.set(FilterType::LowPass, lp_f, lp_q, 0.0, sr);
        }
    }

    fn smoothers(&mut self) -> impl Iterator<Item = &mut Smoother> {
        [&mut self.input, &mut self.drive, &mut self.dry, &mut self.wet, &mut self.out].into_iter().chain(self.bands.iter_mut())
    }
}

/// Saturation curve of each Drive Character (index into `SUB_CHARACTERS`).
fn sub_saturate(character: usize, x: f32) -> f32 {
    match character {
        1 => x / (1.0 + x * x).sqrt(),
        4 => ((x + SUB_ASYM).tanh() - SUB_ASYM.tanh()) / (1.0 + SUB_ASYM.tanh()),
        5 => x.clamp(-1.0, 1.0),
        _ => x.tanh(),
    }
}

impl Plugin for SubHarmonic {
    param_plumbing!(SUB_INFO);

    fn prepare(&mut self, sample_rate: f32, _max_block: usize, channels: usize) {
        self.sr = sane_sr(sample_rate);
        self.ch = vec![SubChannel::new(self.sr); sane_channels(channels)];
        let sr = self.sr;
        for s in self.smoothers() {
            s.set_time(20.0, sr);
        }
        self.update();
        self.reset();
    }

    fn reset(&mut self) {
        self.ch.iter_mut().for_each(SubChannel::reset);
        for s in self.smoothers() {
            s.snap();
        }
    }

    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        let ch = self.ch.len().min(io.len());
        let frames = frames_in(io, frames, ch);
        let character = self.p.choice(9);
        let rel = self.env_rel;
        for n in 0..frames {
            let gin = self.input.next();
            let g = [self.bands[0].next(), self.bands[1].next(), self.bands[2].next()];
            let drive = self.drive.next();
            let dg = self.dry.next();
            let wg = self.wet.next();
            let o = self.out.next();
            for c in 0..ch {
                let (Some(buf), Some(st)) = (io.get_mut(c), self.ch.get_mut(c)) else { continue };
                let Some(x) = buf.get_mut(n) else { continue };
                let dry = if x.is_finite() { *x * gin } else { 0.0 };
                let mut sum = st.direct.process_sample(dry) * g[2];
                for (band, gain) in st.bands.iter_mut().zip(g) {
                    sum += band.process(dry, rel) * gain;
                }
                let mut y = sub_saturate(character, sum * drive);
                if character == 4 {
                    y = st.dc.process(y);
                }
                let wet = st.lp.process_sample(st.hp.process_sample(y));
                *x = (dry * dg + wet * wg) * o;
            }
        }
    }

    fn tail_samples(&self) -> usize {
        // Time for the slowest, most resonant filter to ring down by 60 dB (6.9 time constants).
        let (lo, _) = self.range_hz();
        let f = self.p.v(10).min(self.p.v(12)).min(lo * SUB_SMOOTH).max(1.0);
        let q = self.p.v(11).max(self.p.v(13)).max(self.smooth_q()).max(SUB_BAND_Q);
        (6.9 * q / (std::f32::consts::PI * f) * self.sr) as usize
    }
}
