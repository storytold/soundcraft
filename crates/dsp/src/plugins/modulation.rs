//! Delay and modulation effects: Mod Delay, Chorus, Flanger, Phaser, Ring Modulator.

use crate::params::{Params, choice, lin, log, param_plumbing};
use crate::util::{DelayLine, Lfo, Rng, Smoother, flush, frames_in, ms_to_samples, sane_channels, sane_sr, time_coef};
use crate::{Category, ParamInfo, Plugin, PluginInfo, Unit};

/// Per-channel LFO phase offset for stereo movement (channel 1 is offset by `spread` cycles).
#[inline]
fn channel_phase(phase: f32, c: usize, spread: f32) -> f32 {
    let p = phase + if c % 2 == 1 { spread } else { 0.0 };
    if p >= 1.0 { p - 1.0 } else { p }
}

// ---------------------------------------------------------------------------------------------

const MAX_DELAY_MS: f32 = 2000.0;
const MOD_DELAY_DEPTH_MS: f32 = 5.0;

static DELAY_PARAMS: [ParamInfo; 6] = [
    log("time", "Time", 1.0, MAX_DELAY_MS, 250.0, Unit::Ms),
    lin("feedback", "Feedback", -99.0, 99.0, 30.0, Unit::Percent),
    lin("mix", "Mix", 0.0, 100.0, 30.0, Unit::Percent),
    log("mod_rate", "Mod Rate", 0.01, 10.0, 0.5, Unit::Hz),
    lin("mod_depth", "Mod Depth", 0.0, 100.0, 0.0, Unit::Percent),
    log("lpf", "LPF", 500.0, 20000.0, 12000.0, Unit::Hz),
];

pub static DELAY_INFO: PluginInfo = PluginInfo {
    id: "mod_delay",
    name: "Mod Delay",
    short_name: "ModDly",
    category: Category::Delay,
    params: &DELAY_PARAMS,
    is_instrument: false,
    audiosuite: true,
};

/// Feedback delay with modulation and a low-pass in the feedback path.
pub struct ModDelay {
    p: Params,
    sr: f32,
    lines: Vec<DelayLine>,
    lp: Vec<f32>,
    lfo: Lfo,
    time: Smoother,
    fb: Smoother,
    mix: Smoother,
    depth: Smoother,
    lp_coef: f32,
}

impl Default for ModDelay {
    fn default() -> Self {
        Self::new()
    }
}

impl ModDelay {
    pub fn new() -> Self {
        let mut d = Self {
            p: Params::new(&DELAY_PARAMS),
            sr: 48_000.0,
            lines: Vec::new(),
            lp: Vec::new(),
            lfo: Lfo::new(),
            time: Smoother::new(250.0),
            fb: Smoother::new(0.0),
            mix: Smoother::new(0.0),
            depth: Smoother::new(0.0),
            lp_coef: 0.0,
        };
        d.update();
        d
    }

    fn update(&mut self) {
        self.time.set(self.p.v(0));
        self.fb.set(self.p.v(1) / 100.0);
        self.mix.set(self.p.v(2) / 100.0);
        self.depth.set(self.p.v(4) / 100.0 * MOD_DELAY_DEPTH_MS);
        self.lp_coef = (-std::f32::consts::TAU * self.p.v(5) / self.sr).exp();
    }
}

impl Plugin for ModDelay {
    param_plumbing!(DELAY_INFO);

    fn prepare(&mut self, sample_rate: f32, _max_block: usize, channels: usize) {
        self.sr = sane_sr(sample_rate);
        let max = ms_to_samples(MAX_DELAY_MS + 2.0 * MOD_DELAY_DEPTH_MS, self.sr) as usize + 8;
        self.lines = (0..sane_channels(channels))
            .map(|_| {
                let mut d = DelayLine::default();
                d.allocate(max);
                d
            })
            .collect();
        self.lp = vec![0.0; self.lines.len()];
        self.time.set_time(120.0, self.sr);
        for s in [&mut self.fb, &mut self.mix, &mut self.depth] {
            s.set_time(20.0, self.sr);
        }
        self.update();
        self.reset();
    }

    fn reset(&mut self) {
        self.lines.iter_mut().for_each(DelayLine::clear);
        self.lp.iter_mut().for_each(|v| *v = 0.0);
        self.lfo.reset();
        for s in [&mut self.time, &mut self.fb, &mut self.mix, &mut self.depth] {
            s.snap();
        }
    }

    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        let ch = self.lines.len().min(io.len());
        let frames = frames_in(io, frames, ch);
        let inc = self.p.v(3) / self.sr;
        let a = self.lp_coef;
        for n in 0..frames {
            let phase = self.lfo.tick(inc);
            let t = self.time.next();
            let fb = self.fb.next();
            let m = self.mix.next();
            let depth = self.depth.next();
            for c in 0..ch {
                let (Some(buf), Some(line), Some(lp)) = (io.get_mut(c), self.lines.get_mut(c), self.lp.get_mut(c)) else { continue };
                let Some(x) = buf.get_mut(n) else { continue };
                let dry = if x.is_finite() { *x } else { 0.0 };
                let modv = depth * 0.5 * (1.0 + Lfo::sin_at(channel_phase(phase, c, 0.25)));
                let d = ms_to_samples(t + modv, self.sr);
                let y = line.read_cubic(d);
                *lp = flush(y + (*lp - y) * a);
                line.push(flush(dry + fb * *lp));
                *x = dry * (1.0 - m) + *lp * m;
            }
        }
    }

    fn tail_samples(&self) -> usize {
        // Time for feedback to fall by 60 dB.
        let fb = (self.p.v(1).abs() / 100.0).clamp(0.0, 0.99);
        let repeats = if fb < 1e-3 { 1.0 } else { (-60.0 / (20.0 * fb.log10())).min(500.0) + 1.0 };
        ms_to_samples(self.p.v(0) * repeats, self.sr) as usize
    }
}

// ---------------------------------------------------------------------------------------------

static CHORUS_PARAMS: [ParamInfo; 6] = [
    log("rate", "Rate", 0.05, 5.0, 0.8, Unit::Hz),
    lin("depth", "Depth", 0.0, 100.0, 50.0, Unit::Percent),
    lin("delay", "Delay", 5.0, 30.0, 12.0, Unit::Ms),
    lin("feedback", "Feedback", 0.0, 90.0, 0.0, Unit::Percent),
    lin("mix", "Mix", 0.0, 100.0, 50.0, Unit::Percent),
    lin("spread", "Spread", 0.0, 100.0, 100.0, Unit::Percent),
];

pub static CHORUS_INFO: PluginInfo = PluginInfo {
    id: "chorus",
    name: "Chorus",
    short_name: "Chorus",
    category: Category::Modulation,
    params: &CHORUS_PARAMS,
    is_instrument: false,
    audiosuite: true,
};

static FLANGER_PARAMS: [ParamInfo; 5] = [
    log("rate", "Rate", 0.02, 5.0, 0.25, Unit::Hz),
    lin("depth", "Depth", 0.0, 100.0, 70.0, Unit::Percent),
    lin("delay", "Delay", 0.1, 10.0, 2.0, Unit::Ms),
    lin("feedback", "Feedback", -95.0, 95.0, 50.0, Unit::Percent),
    lin("mix", "Mix", 0.0, 100.0, 50.0, Unit::Percent),
];

pub static FLANGER_INFO: PluginInfo = PluginInfo {
    id: "flanger",
    name: "Flanger",
    short_name: "Flanger",
    category: Category::Modulation,
    params: &FLANGER_PARAMS,
    is_instrument: false,
    audiosuite: true,
};

#[derive(Clone, Copy, PartialEq)]
enum ModKind {
    Chorus,
    Flanger,
}

/// Modulated short delay used by both Chorus and Flanger.
pub struct ModFx {
    kind: ModKind,
    p: Params,
    sr: f32,
    lines: Vec<DelayLine>,
    fbs: Vec<f32>,
    lfo: Lfo,
    delay: Smoother,
    depth: Smoother,
    fb: Smoother,
    mix: Smoother,
}

impl ModFx {
    pub fn chorus() -> Self {
        Self::build(ModKind::Chorus)
    }

    pub fn flanger() -> Self {
        Self::build(ModKind::Flanger)
    }

    fn build(kind: ModKind) -> Self {
        let info = if kind == ModKind::Chorus { &CHORUS_INFO } else { &FLANGER_INFO };
        let mut m = Self {
            kind,
            p: Params::new(info.params),
            sr: 48_000.0,
            lines: Vec::new(),
            fbs: Vec::new(),
            lfo: Lfo::new(),
            delay: Smoother::new(0.0),
            depth: Smoother::new(0.0),
            fb: Smoother::new(0.0),
            mix: Smoother::new(0.0),
        };
        m.update();
        m
    }

    fn update(&mut self) {
        self.delay.set(self.p.v(2));
        self.depth.set(self.p.v(1) / 100.0);
        self.fb.set(self.p.v(3) / 100.0);
        self.mix.set(self.p.v(4) / 100.0);
    }
}

impl Plugin for ModFx {
    fn info(&self) -> &'static PluginInfo {
        if self.kind == ModKind::Chorus { &CHORUS_INFO } else { &FLANGER_INFO }
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
        let max = ms_to_samples(45.0, self.sr) as usize + 8;
        self.lines = (0..sane_channels(channels))
            .map(|_| {
                let mut d = DelayLine::default();
                d.allocate(max);
                d
            })
            .collect();
        self.fbs = vec![0.0; self.lines.len()];
        for s in [&mut self.delay, &mut self.depth, &mut self.fb, &mut self.mix] {
            s.set_time(30.0, self.sr);
        }
        self.update();
        self.reset();
    }

    fn reset(&mut self) {
        self.lines.iter_mut().for_each(DelayLine::clear);
        self.fbs.iter_mut().for_each(|v| *v = 0.0);
        self.lfo.reset();
        for s in [&mut self.delay, &mut self.depth, &mut self.fb, &mut self.mix] {
            s.snap();
        }
    }

    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        let ch = self.lines.len().min(io.len());
        let frames = frames_in(io, frames, ch);
        let inc = self.p.v(0) / self.sr;
        let spread = if self.kind == ModKind::Chorus { 0.25 * self.p.v(5) / 100.0 } else { 0.25 };
        for n in 0..frames {
            let phase = self.lfo.tick(inc);
            let base = self.delay.next();
            let depth = self.depth.next();
            let fb = self.fb.next();
            let m = self.mix.next();
            for c in 0..ch {
                let (Some(buf), Some(line), Some(fbs)) = (io.get_mut(c), self.lines.get_mut(c), self.fbs.get_mut(c)) else { continue };
                let Some(x) = buf.get_mut(n) else { continue };
                let dry = if x.is_finite() { *x } else { 0.0 };
                let s = Lfo::sin_at(channel_phase(phase, c, spread));
                let ms = match self.kind {
                    // Up to ±6 ms of sweep around the base delay.
                    ModKind::Chorus => base + depth * 6.0 * 0.5 * (1.0 + s),
                    // Sweep from (1 - depth)·delay up to delay.
                    ModKind::Flanger => (base * (1.0 - depth * 0.5 * (1.0 - s))).max(0.05),
                };
                line.push(flush(dry + fb * *fbs));
                let y = line.read_cubic(ms_to_samples(ms, self.sr).max(2.0));
                *fbs = flush(y);
                *x = dry * (1.0 - m) + y * m;
            }
        }
    }

    fn tail_samples(&self) -> usize {
        // Time for feedback to fall by 60 dB at the longest delay of the sweep, as for the delay
        // above; never shorter than the 200 ms this used to report.
        let longest = match self.kind {
            ModKind::Chorus => self.p.v(2) + self.p.v(1) / 100.0 * 6.0,
            ModKind::Flanger => self.p.v(2),
        };
        let fb = (self.p.v(3).abs() / 100.0).clamp(0.0, 0.99);
        let repeats = if fb < 1e-3 { 1.0 } else { (-60.0 / (20.0 * fb.log10())).min(500.0) + 1.0 };
        ms_to_samples((longest * repeats).max(200.0), self.sr) as usize
    }
}

// ---------------------------------------------------------------------------------------------

const STAGE_CHOICES: &[&str] = &["2", "4", "6", "8", "12"];
const STAGE_COUNTS: [usize; 5] = [2, 4, 6, 8, 12];
const MAX_STAGES: usize = 12;

static PHASER_PARAMS: [ParamInfo; 6] = [
    log("rate", "Rate", 0.02, 5.0, 0.5, Unit::Hz),
    lin("depth", "Depth", 0.0, 100.0, 70.0, Unit::Percent),
    choice("stages", "Stages", STAGE_CHOICES, 2),
    lin("feedback", "Feedback", -95.0, 95.0, 40.0, Unit::Percent),
    log("center", "Center", 100.0, 4000.0, 800.0, Unit::Hz),
    lin("mix", "Mix", 0.0, 100.0, 50.0, Unit::Percent),
];

pub static PHASER_INFO: PluginInfo = PluginInfo {
    id: "phaser",
    name: "Phaser",
    short_name: "Phaser",
    category: Category::Modulation,
    params: &PHASER_PARAMS,
    is_instrument: false,
    audiosuite: true,
};

/// Cascade of modulated first-order allpasses with feedback.
pub struct Phaser {
    p: Params,
    sr: f32,
    ch: usize,
    z: Vec<[f32; MAX_STAGES]>,
    fbs: Vec<f32>,
    lfo: Lfo,
    depth: Smoother,
    fb: Smoother,
    center: Smoother,
    mix: Smoother,
}

impl Default for Phaser {
    fn default() -> Self {
        Self::new()
    }
}

impl Phaser {
    pub fn new() -> Self {
        let mut p = Self {
            p: Params::new(&PHASER_PARAMS),
            sr: 48_000.0,
            ch: 0,
            z: Vec::new(),
            fbs: Vec::new(),
            lfo: Lfo::new(),
            depth: Smoother::new(0.0),
            fb: Smoother::new(0.0),
            center: Smoother::new(800.0),
            mix: Smoother::new(0.0),
        };
        p.update();
        p
    }

    fn update(&mut self) {
        self.depth.set(self.p.v(1) / 100.0);
        self.fb.set(self.p.v(3) / 100.0);
        self.center.set(self.p.v(4));
        self.mix.set(self.p.v(5) / 100.0);
    }
}

impl Plugin for Phaser {
    param_plumbing!(PHASER_INFO);

    fn prepare(&mut self, sample_rate: f32, _max_block: usize, channels: usize) {
        self.sr = sane_sr(sample_rate);
        self.ch = sane_channels(channels);
        self.z = vec![[0.0; MAX_STAGES]; self.ch];
        self.fbs = vec![0.0; self.ch];
        for s in [&mut self.depth, &mut self.fb, &mut self.center, &mut self.mix] {
            s.set_time(30.0, self.sr);
        }
        self.update();
        self.reset();
    }

    fn reset(&mut self) {
        self.z.iter_mut().for_each(|z| *z = [0.0; MAX_STAGES]);
        self.fbs.iter_mut().for_each(|v| *v = 0.0);
        self.lfo.reset();
        for s in [&mut self.depth, &mut self.fb, &mut self.center, &mut self.mix] {
            s.snap();
        }
    }

    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        let ch = self.ch.min(io.len());
        let frames = frames_in(io, frames, ch);
        let inc = self.p.v(0) / self.sr;
        let stages = STAGE_COUNTS.get(self.p.choice(2)).copied().unwrap_or(6);
        let nyq = self.sr * 0.45;
        for n in 0..frames {
            let phase = self.lfo.tick(inc);
            let depth = self.depth.next();
            let fb = self.fb.next();
            let center = self.center.next();
            let m = self.mix.next();
            for c in 0..ch {
                let (Some(buf), Some(z), Some(fbs)) = (io.get_mut(c), self.z.get_mut(c), self.fbs.get_mut(c)) else { continue };
                let Some(x) = buf.get_mut(n) else { continue };
                let dry = if x.is_finite() { *x } else { 0.0 };
                let s = Lfo::sin_at(channel_phase(phase, c, 0.25));
                let f = (center * 2f32.powf(2.0 * depth * s)).clamp(20.0, nyq);
                let t = (std::f32::consts::PI * f / self.sr).tan();
                let a = (t - 1.0) / (t + 1.0);
                let mut y = dry + fb * *fbs;
                for zi in z.iter_mut().take(stages) {
                    let out = a * y + *zi;
                    *zi = flush(y - a * out);
                    y = out;
                }
                *fbs = flush(y);
                *x = dry * (1.0 - m) + y * m;
            }
        }
    }

    fn tail_samples(&self) -> usize {
        ms_to_samples(100.0, self.sr) as usize
    }
}

// ---------------------------------------------------------------------------------------------

const RING_EFFECTS: &[&str] = &["Ring", "Frequency Mod", "Resonator +", "Resonator -"];
const RING_SOURCES: &[&str] = &["LFO", "Envelope", "Sample & Hold"];
const RING_MIN_HZ: f32 = 20.0;
/// Octaves the frequency moves at full modulation depth.
const RING_MOD_OCTAVES: f32 = 2.0;
/// Peak delay swing of Frequency Mod.
const RING_FM_SWING_MS: f32 = 1.0;
/// Sample-and-hold noise seed (the same sequence after every reset).
const RING_SEED: u32 = 0x5EED_1234;

static RING_PARAMS: [ParamInfo; 7] = [
    choice("effect", "Effect", RING_EFFECTS, 0),
    log("frequency", "Frequency", RING_MIN_HZ, 5000.0, 440.0, Unit::Hz),
    lin("resonance", "Resonance", 0.0, 95.0, 50.0, Unit::Percent),
    choice("mod_source", "Mod Source", RING_SOURCES, 0),
    log("mod_rate", "Mod Rate", 0.05, 20.0, 1.0, Unit::Hz),
    lin("mod_depth", "Mod Depth", 0.0, 100.0, 0.0, Unit::Percent),
    lin("mix", "Mix", 0.0, 100.0, 100.0, Unit::Percent),
];

pub static RING_INFO: PluginInfo = PluginInfo {
    id: "ring_mod",
    name: "Ring Modulator",
    short_name: "RingMod",
    category: Category::Modulation,
    params: &RING_PARAMS,
    is_instrument: false,
    audiosuite: true,
};

/// Ring modulation, frequency modulation and positive or negative comb resonators, with the
/// frequency moved by an LFO, an envelope follower or a sample-and-hold source.
///
/// - Ring multiplies the input by a sine carrier at the frequency.
/// - Frequency Mod reads the input through a delay that the carrier swings by ±1 ms.
/// - Resonator + and Resonator - feed the output back after one period of the frequency, with
///   a positive or a negative sign. The output is scaled by `1 - resonance`, so the peak gain at
///   resonance is unity.
///
/// At 100 % depth the modulation moves the frequency by up to two octaves either way. The
/// envelope follower moves it up only.
pub struct RingMod {
    p: Params,
    sr: f32,
    lines: Vec<DelayLine>,
    lfo: Lfo,
    rng: Rng,
    carrier: f32,
    env: f32,
    env_attack: f32,
    env_release: f32,
    held: f32,
    hold_left: usize,
    last_effect: usize,
    freq: Smoother,
    res: Smoother,
    depth: Smoother,
    mix: Smoother,
}

impl Default for RingMod {
    fn default() -> Self {
        Self::new()
    }
}

impl RingMod {
    pub fn new() -> Self {
        let mut r = Self {
            p: Params::new(&RING_PARAMS),
            sr: 48_000.0,
            lines: Vec::new(),
            lfo: Lfo::new(),
            rng: Rng::new(RING_SEED),
            carrier: 0.0,
            env: 0.0,
            env_attack: 0.0,
            env_release: 0.0,
            held: 0.0,
            hold_left: 0,
            last_effect: 0,
            freq: Smoother::new(440.0),
            res: Smoother::new(0.0),
            depth: Smoother::new(0.0),
            mix: Smoother::new(1.0),
        };
        r.update();
        r
    }

    fn update(&mut self) {
        self.freq.set(self.p.v(1));
        self.res.set(self.p.v(2) / 100.0);
        self.depth.set(self.p.v(5) / 100.0);
        self.mix.set(self.p.v(6) / 100.0);
    }

    /// Samples in one period of the lowest frequency the modulation can reach.
    fn longest_period(&self) -> f32 {
        self.sr / (self.p.v(1) * 2f32.powf(-RING_MOD_OCTAVES * self.p.v(5) / 100.0)).max(1.0)
    }
}

impl Plugin for RingMod {
    param_plumbing!(RING_INFO);

    fn prepare(&mut self, sample_rate: f32, _max_block: usize, channels: usize) {
        self.sr = sane_sr(sample_rate);
        let comb = self.sr / (RING_MIN_HZ * 2f32.powf(-RING_MOD_OCTAVES));
        let max = comb.max(ms_to_samples(2.0 * RING_FM_SWING_MS, self.sr)) as usize + 8;
        self.lines = (0..sane_channels(channels))
            .map(|_| {
                let mut d = DelayLine::default();
                d.allocate(max);
                d
            })
            .collect();
        self.env_attack = time_coef(5.0, self.sr);
        self.env_release = time_coef(100.0, self.sr);
        for s in [&mut self.freq, &mut self.res, &mut self.depth, &mut self.mix] {
            s.set_time(20.0, self.sr);
        }
        self.update();
        self.reset();
    }

    fn reset(&mut self) {
        self.lines.iter_mut().for_each(DelayLine::clear);
        self.lfo.reset();
        self.rng = Rng::new(RING_SEED);
        self.carrier = 0.0;
        self.env = 0.0;
        self.held = 0.0;
        self.hold_left = 0;
        self.last_effect = self.p.choice(0);
        for s in [&mut self.freq, &mut self.res, &mut self.depth, &mut self.mix] {
            s.snap();
        }
    }

    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        let ch = self.lines.len().min(io.len());
        let frames = frames_in(io, frames, ch);
        let effect = self.p.choice(0);
        if effect != self.last_effect {
            // The lines hold the previous effect's state.
            self.lines.iter_mut().for_each(DelayLine::clear);
            self.last_effect = effect;
        }
        let source = self.p.choice(3);
        let rate = self.p.v(4);
        let inc = rate / self.sr;
        let hold = ((self.sr / rate.max(0.05)) as usize).max(1);
        let nyq = self.sr * 0.45;
        let swing = ms_to_samples(RING_FM_SWING_MS, self.sr);
        for n in 0..frames {
            let level = io.iter().take(ch).filter_map(|b| b.get(n)).filter(|x| x.is_finite()).fold(0.0f32, |m, x| m.max(x.abs()));
            let k = if level > self.env { self.env_attack } else { self.env_release };
            self.env = flush(level + (self.env - level) * k);
            let lfo = Lfo::sin_at(self.lfo.tick(inc));
            let m = match source {
                1 => self.env.min(1.0),
                2 => {
                    if self.hold_left == 0 {
                        self.held = self.rng.bipolar();
                        self.hold_left = hold;
                    }
                    self.hold_left -= 1;
                    self.held
                }
                _ => lfo,
            };
            let f = (self.freq.next() * 2f32.powf(RING_MOD_OCTAVES * self.depth.next() * m)).clamp(1.0, nyq);
            let fb = self.res.next();
            let mix = self.mix.next();
            self.carrier += f / self.sr;
            if self.carrier >= 1.0 {
                self.carrier -= 1.0;
            }
            let carrier = Lfo::sin_at(self.carrier);
            for c in 0..ch {
                let (Some(buf), Some(line)) = (io.get_mut(c), self.lines.get_mut(c)) else { continue };
                let Some(x) = buf.get_mut(n) else { continue };
                let dry = if x.is_finite() { *x } else { 0.0 };
                let wet = match effect {
                    1 => {
                        line.push(dry);
                        line.read_cubic(2.0 + swing * (1.0 + carrier))
                    }
                    2 | 3 => {
                        let sign = if effect == 2 { 1.0 } else { -1.0 };
                        let y = flush(dry + sign * fb * line.read_cubic(self.sr / f));
                        line.push(y);
                        y * (1.0 - fb)
                    }
                    _ => dry * carrier,
                };
                *x = dry * (1.0 - mix) + wet * mix;
            }
        }
    }

    fn tail_samples(&self) -> usize {
        match self.p.choice(0) {
            1 => ms_to_samples(2.0 * RING_FM_SWING_MS, self.sr) as usize + 4,
            2 | 3 => {
                // Time for the feedback to fall by 60 dB at the longest period the modulation reaches.
                let fb = (self.p.v(2) / 100.0).clamp(0.0, 0.99);
                let repeats = if fb < 1e-3 { 1.0 } else { (-60.0 / (20.0 * fb.log10())).min(500.0) + 1.0 };
                (self.longest_period() * repeats) as usize
            }
            _ => 0,
        }
    }
}
