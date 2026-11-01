//! Dynamics: Compressor/Limiter, Expander/Gate, De-Esser and Maximizer.
//!
//! All detectors are stereo-linked (the loudest channel drives a shared gain) so the image does
//! not wander. Gain computers work in the log domain with smoothed gain reduction.

use crate::biquad::{Biquad, Coeffs, FilterType};
use crate::params::{Params, lin, log, param_plumbing, toggle};
use crate::util::{DelayLine, Smoother, frames_in, ms_to_samples, sane_channels, sane_sr, time_coef};
use crate::{Category, ParamInfo, Plugin, PluginInfo, Unit, db_to_gain, gain_to_db};

/// Static soft-knee gain computer: returns the gain reduction amount in dB (≥ 0).
#[inline]
pub(crate) fn compress_gr(level_db: f32, threshold: f32, ratio: f32, knee: f32) -> f32 {
    let slope = 1.0 - 1.0 / ratio.max(1.0);
    let over = level_db - threshold;
    if knee > 1e-3 && 2.0 * over.abs() <= knee {
        let t = over + knee / 2.0;
        slope * t * t / (2.0 * knee)
    } else if over > 0.0 {
        slope * over
    } else {
        0.0
    }
}

#[inline]
fn ballistic(state: f32, target: f32, att: f32, rel: f32) -> f32 {
    let c = if target > state { att } else { rel };
    target + (state - target) * c
}

// ---------------------------------------------------------------------------------------------

static COMP_PARAMS: [ParamInfo; 11] = [
    lin("threshold", "Threshold", -60.0, 0.0, -20.0, Unit::Db),
    log("ratio", "Ratio", 1.0, 100.0, 4.0, Unit::Ratio),
    log("attack", "Attack", 0.01, 300.0, 10.0, Unit::Ms),
    log("release", "Release", 5.0, 3000.0, 100.0, Unit::Ms),
    lin("knee", "Knee", 0.0, 24.0, 6.0, Unit::Db),
    lin("makeup", "Makeup", 0.0, 40.0, 0.0, Unit::Db),
    toggle("sc_hpf_on", "SC HPF", false),
    log("sc_hpf_freq", "SC HPF Freq", 20.0, 500.0, 100.0, Unit::Hz),
    toggle("limiter", "Limit", false),
    lin("lookahead", "Lookahead", 0.0, 10.0, 0.0, Unit::Ms),
    lin("mix", "Mix", 0.0, 100.0, 100.0, Unit::Percent),
];

pub static COMP_INFO: PluginInfo = PluginInfo {
    id: "compressor",
    name: "Compressor/Limiter",
    short_name: "Comp",
    category: Category::Dynamics,
    params: &COMP_PARAMS,
    is_instrument: false,
    audiosuite: true,
};

/// Feed-forward compressor / limiter with optional lookahead and sidechain high-pass.
pub struct Compressor {
    p: Params,
    sr: f32,
    sc: Vec<Biquad>,
    delay: Vec<DelayLine>,
    gr: f32,
    att: f32,
    rel: f32,
    makeup: Smoother,
    mix: Smoother,
    lookahead: usize,
    gr_meter: f32,
}

impl Default for Compressor {
    fn default() -> Self {
        Self::new()
    }
}

impl Compressor {
    pub fn new() -> Self {
        let mut c = Self {
            p: Params::new(&COMP_PARAMS),
            sr: 48_000.0,
            sc: Vec::new(),
            delay: Vec::new(),
            gr: 0.0,
            att: 0.0,
            rel: 0.0,
            makeup: Smoother::new(1.0),
            mix: Smoother::new(1.0),
            lookahead: 0,
            gr_meter: 0.0,
        };
        c.update();
        c
    }

    fn limiting(&self) -> bool {
        self.p.on(8)
    }

    fn update(&mut self) {
        let sr = self.sr;
        self.att = time_coef(self.p.v(2), sr);
        self.rel = time_coef(self.p.v(3), sr);
        self.makeup.set(db_to_gain(self.p.v(5)));
        self.mix.set(self.p.v(10) / 100.0);
        let f = self.p.v(7);
        for b in &mut self.sc {
            b.set(FilterType::HighPass, f, std::f32::consts::FRAC_1_SQRT_2, 0.0, sr);
        }
        let max = self.delay.first().map(|d| d.max_delay() as usize).unwrap_or(0);
        self.lookahead = (ms_to_samples(self.p.v(9), sr).round() as usize).min(max);
    }

    /// Processes one frame of gain computation from a sidechain level (linear).
    #[inline]
    fn gain_for(&mut self, level: f32) -> f32 {
        let ratio = if self.limiting() { 1000.0 } else { self.p.v(1) };
        let target = compress_gr(gain_to_db(level), self.p.v(0), ratio, self.p.v(4));
        self.gr = ballistic(self.gr, target, self.att, self.rel);
        if !self.gr.is_finite() {
            self.gr = 0.0;
        }
        self.gr
    }
}

impl Plugin for Compressor {
    param_plumbing!(COMP_INFO);

    fn prepare(&mut self, sample_rate: f32, _max_block: usize, channels: usize) {
        self.sr = sane_sr(sample_rate);
        let ch = sane_channels(channels);
        self.sc = vec![Biquad::default(); ch];
        let max = ms_to_samples(10.0, self.sr).ceil() as usize + 2;
        self.delay = (0..ch)
            .map(|_| {
                let mut d = DelayLine::default();
                d.allocate(max);
                d
            })
            .collect();
        self.makeup.set_time(20.0, self.sr);
        self.mix.set_time(20.0, self.sr);
        self.update();
        self.reset();
    }

    fn reset(&mut self) {
        self.sc.iter_mut().for_each(Biquad::reset);
        self.delay.iter_mut().for_each(DelayLine::clear);
        self.gr = 0.0;
        self.gr_meter = 0.0;
        self.makeup.snap();
        self.mix.snap();
    }

    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        let ch = self.sc.len().min(io.len());
        let frames = frames_in(io, frames, ch);
        let hpf = self.p.on(6);
        let la = self.lookahead;
        let mut max_gr = 0.0f32;
        for n in 0..frames {
            let mut level = 0.0f32;
            for c in 0..ch {
                let x = io.get(c).and_then(|b| b.get(n)).copied().unwrap_or(0.0);
                let s = match self.sc.get_mut(c) {
                    Some(f) if hpf => f.process_sample(x),
                    _ => x,
                };
                if s.is_finite() {
                    level = level.max(s.abs());
                }
            }
            let gr = self.gain_for(level);
            max_gr = max_gr.max(gr);
            let g = db_to_gain(-gr) * self.makeup.next();
            let m = self.mix.next();
            for c in 0..ch {
                let (Some(buf), Some(d)) = (io.get_mut(c), self.delay.get_mut(c)) else { continue };
                let Some(x) = buf.get_mut(n) else { continue };
                d.push(*x);
                let dry = if la == 0 { *x } else { d.tap(la + 1) };
                *x = dry * (1.0 - m) + dry * g * m;
            }
        }
        self.gr_meter = max_gr;
    }

    fn latency(&self) -> usize {
        self.lookahead
    }

    fn gain_reduction_db(&self) -> f32 {
        self.gr_meter
    }
}

// ---------------------------------------------------------------------------------------------

static GATE_PARAMS: [ParamInfo; 6] = [
    lin("threshold", "Threshold", -80.0, 0.0, -40.0, Unit::Db),
    log("ratio", "Ratio", 1.0, 100.0, 100.0, Unit::Ratio),
    lin("range", "Range", 0.0, 80.0, 80.0, Unit::Db),
    log("attack", "Attack", 0.01, 100.0, 0.5, Unit::Ms),
    lin("hold", "Hold", 0.0, 2000.0, 50.0, Unit::Ms),
    log("release", "Release", 1.0, 4000.0, 100.0, Unit::Ms),
];

pub static GATE_INFO: PluginInfo = PluginInfo {
    id: "expander_gate",
    name: "Expander/Gate",
    short_name: "Gate",
    category: Category::Dynamics,
    params: &GATE_PARAMS,
    is_instrument: false,
    audiosuite: true,
};

/// Downward expander / gate with hold.
pub struct ExpanderGate {
    p: Params,
    sr: f32,
    ch: usize,
    env: f32,
    env_rel: f32,
    gr: f32,
    att: f32,
    rel: f32,
    hold: usize,
    hold_left: usize,
    gr_meter: f32,
}

impl Default for ExpanderGate {
    fn default() -> Self {
        Self::new()
    }
}

impl ExpanderGate {
    pub fn new() -> Self {
        let mut g = Self {
            p: Params::new(&GATE_PARAMS),
            sr: 48_000.0,
            ch: 0,
            env: 0.0,
            env_rel: 0.0,
            gr: 0.0,
            att: 0.0,
            rel: 0.0,
            hold: 0,
            hold_left: 0,
            gr_meter: 0.0,
        };
        g.update();
        g
    }

    fn update(&mut self) {
        self.att = time_coef(self.p.v(3), self.sr);
        self.rel = time_coef(self.p.v(5), self.sr);
        self.hold = ms_to_samples(self.p.v(4), self.sr) as usize;
        self.env_rel = time_coef(5.0, self.sr);
    }

    /// Returns the gain reduction (dB ≥ 0) for one frame given the linked peak level.
    #[inline]
    pub(crate) fn tick(&mut self, level: f32) -> f32 {
        let level = if level.is_finite() { level } else { 0.0 };
        self.env = level.max(self.env * self.env_rel);
        let ldb = gain_to_db(self.env);
        let thr = self.p.v(0);
        let mut target = if ldb < thr { ((thr - ldb) * (self.p.v(1) - 1.0)).min(self.p.v(2)) } else { 0.0 };
        if target <= 0.0 {
            self.hold_left = self.hold;
        } else if target > self.gr && self.hold_left > 0 {
            self.hold_left -= 1;
            target = self.gr;
        }
        // Opening (less reduction) uses attack, closing uses release.
        let c = if target < self.gr { self.att } else { self.rel };
        self.gr = target + (self.gr - target) * c;
        if !self.gr.is_finite() {
            self.gr = 0.0;
        }
        self.gr
    }
}

impl Plugin for ExpanderGate {
    param_plumbing!(GATE_INFO);

    fn prepare(&mut self, sample_rate: f32, _max_block: usize, channels: usize) {
        self.sr = sane_sr(sample_rate);
        self.ch = sane_channels(channels);
        self.update();
        self.reset();
    }

    fn reset(&mut self) {
        self.env = 0.0;
        self.gr = 0.0;
        self.hold_left = 0;
        self.gr_meter = 0.0;
    }

    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        let ch = self.ch.min(io.len());
        let frames = frames_in(io, frames, ch);
        let mut max_gr = 0.0f32;
        for n in 0..frames {
            let level = io.iter().take(ch).filter_map(|b| b.get(n)).fold(0.0f32, |m, v| m.max(v.abs()));
            let gr = self.tick(level);
            max_gr = max_gr.max(gr);
            let g = db_to_gain(-gr);
            for b in io.iter_mut().take(ch) {
                if let Some(x) = b.get_mut(n) {
                    *x *= g;
                }
            }
        }
        self.gr_meter = max_gr;
    }

    fn gain_reduction_db(&self) -> f32 {
        self.gr_meter
    }
}

// ---------------------------------------------------------------------------------------------

static DEESS_PARAMS: [ParamInfo; 4] = [
    log("freq", "Freq", 2000.0, 16000.0, 6000.0, Unit::Hz),
    lin("range", "Range", 0.0, 40.0, 10.0, Unit::Db),
    lin("threshold", "Threshold", -60.0, 0.0, -30.0, Unit::Db),
    toggle("listen", "Listen", false),
];

pub static DEESS_INFO: PluginInfo = PluginInfo {
    id: "de_esser",
    name: "De-Esser",
    short_name: "DeEss",
    category: Category::Dynamics,
    params: &DEESS_PARAMS,
    is_instrument: false,
    audiosuite: true,
};

/// Dynamic-EQ de-esser: a high-pass sidechain detects sibilance and a high shelf at `freq`
/// dips by up to `range` dB while it exceeds the threshold. At rest the shelf is flat, so the
/// signal passes untouched.
pub struct DeEsser {
    p: Params,
    sr: f32,
    hp: Vec<Biquad>,
    shelf: Vec<Biquad>,
    env: f32,
    att: f32,
    rel: f32,
    shelf_gr: f32,
    gr_meter: f32,
}

impl Default for DeEsser {
    fn default() -> Self {
        Self::new()
    }
}

impl DeEsser {
    pub fn new() -> Self {
        let mut d = Self {
            p: Params::new(&DEESS_PARAMS),
            sr: 48_000.0,
            hp: Vec::new(),
            shelf: Vec::new(),
            env: 0.0,
            att: 0.0,
            rel: 0.0,
            shelf_gr: 0.0,
            gr_meter: 0.0,
        };
        d.update();
        d
    }

    fn update(&mut self) {
        self.att = time_coef(0.5, self.sr);
        self.rel = time_coef(60.0, self.sr);
        let f = self.p.v(0);
        let sr = self.sr;
        for b in &mut self.hp {
            b.set(FilterType::HighPass, f, std::f32::consts::FRAC_1_SQRT_2, 0.0, sr);
        }
        self.set_shelf(self.shelf_gr);
    }

    fn set_shelf(&mut self, gr: f32) {
        self.shelf_gr = gr;
        let c = if gr < 0.01 { Coeffs::IDENTITY } else { Coeffs::design(FilterType::HighShelf, self.p.v(0) * 0.8, 0.8, -gr, self.sr) };
        for b in &mut self.shelf {
            b.coeffs = c;
        }
    }
}

impl Plugin for DeEsser {
    param_plumbing!(DEESS_INFO);

    fn prepare(&mut self, sample_rate: f32, _max_block: usize, channels: usize) {
        self.sr = sane_sr(sample_rate);
        let ch = sane_channels(channels);
        self.hp = vec![Biquad::default(); ch];
        self.shelf = vec![Biquad::default(); ch];
        self.update();
        self.reset();
    }

    fn reset(&mut self) {
        self.hp.iter_mut().for_each(Biquad::reset);
        self.shelf.iter_mut().for_each(Biquad::reset);
        self.env = 0.0;
        self.gr_meter = 0.0;
        self.set_shelf(0.0);
    }

    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        const CHUNK: usize = 16;
        let ch = self.hp.len().min(io.len());
        let frames = frames_in(io, frames, ch);
        let (thr, range, listen) = (self.p.v(2), self.p.v(1), self.p.on(3));
        let mut max_gr = 0.0f32;
        let mut n = 0;
        while n < frames {
            let len = CHUNK.min(frames - n);
            for i in n..n + len {
                let mut level = 0.0f32;
                for c in 0..ch {
                    let (Some(b), Some(f)) = (io.get_mut(c), self.hp.get_mut(c)) else { continue };
                    let Some(x) = b.get_mut(i) else { continue };
                    let h = f.process_sample(*x);
                    level = level.max(h.abs());
                    if listen {
                        *x = h;
                    }
                }
                self.env = ballistic(self.env, level, self.att, self.rel);
                if !self.env.is_finite() {
                    self.env = 0.0;
                }
            }
            let gr = (gain_to_db(self.env) - thr).clamp(0.0, range);
            max_gr = max_gr.max(gr);
            if (gr - self.shelf_gr).abs() > 0.05 || (gr == 0.0 && self.shelf_gr != 0.0) {
                self.set_shelf(gr);
            }
            if !listen {
                for (b, f) in io.iter_mut().zip(self.shelf.iter_mut()).take(ch) {
                    if let Some(seg) = b.get_mut(n..n + len) {
                        f.process_block(seg);
                    }
                }
            }
            n += len;
        }
        self.gr_meter = max_gr;
    }

    fn gain_reduction_db(&self) -> f32 {
        self.gr_meter
    }
}

// ---------------------------------------------------------------------------------------------

static MAX_PARAMS: [ParamInfo; 3] = [
    lin("threshold", "Threshold", -30.0, 0.0, 0.0, Unit::Db),
    lin("ceiling", "Ceiling", -30.0, 0.0, -0.3, Unit::Db),
    log("release", "Release", 1.0, 1000.0, 50.0, Unit::Ms),
];

pub static MAX_INFO: PluginInfo = PluginInfo {
    id: "maximizer",
    name: "Maximizer",
    short_name: "Max",
    category: Category::Dynamics,
    params: &MAX_PARAMS,
    is_instrument: false,
    audiosuite: true,
};

const MAX_LOOKAHEAD_MS: f32 = 1.5;

/// Lookahead brickwall limiter: the input is driven by `-threshold` dB and never exceeds the
/// ceiling. Gain is a min-hold over the lookahead window followed by an equal-length moving
/// average, which ramps the gain down before each peak reaches the output.
pub struct Maximizer {
    p: Params,
    sr: f32,
    la: usize,
    delay: Vec<DelayLine>,
    win: Vec<(u64, f32)>,
    win_head: usize,
    win_len: usize,
    clock: u64,
    smooth: Vec<f32>,
    idx: usize,
    sum: f64,
    held: f32,
    rel: f32,
    drive: Smoother,
    ceiling: Smoother,
    gr_meter: f32,
}

impl Default for Maximizer {
    fn default() -> Self {
        Self::new()
    }
}

impl Maximizer {
    pub fn new() -> Self {
        let mut m = Self {
            p: Params::new(&MAX_PARAMS),
            sr: 48_000.0,
            la: 1,
            delay: Vec::new(),
            win: vec![(0, 1.0)],
            win_head: 0,
            win_len: 0,
            clock: 0,
            smooth: vec![1.0],
            idx: 0,
            sum: 1.0,
            held: 1.0,
            rel: 0.0,
            drive: Smoother::new(1.0),
            ceiling: Smoother::new(1.0),
            gr_meter: 0.0,
        };
        m.update();
        m
    }

    fn update(&mut self) {
        self.drive.set(db_to_gain(-self.p.v(0)));
        // The ceiling is a hard guarantee, so it changes immediately.
        self.ceiling.set(db_to_gain(self.p.v(1)));
        self.ceiling.snap();
        self.rel = time_coef(self.p.v(2), self.sr);
    }

    fn window_min(&mut self, req: f32, la: usize) -> f32 {
        let cap = self.win.len();
        let t = self.clock;
        self.clock = t.saturating_add(1);
        if cap == 0 {
            return req.min(1.0);
        }
        while self.win_len > 0 && self.win.get(self.win_head).is_some_and(|&(at, _)| at.saturating_add(la as u64) <= t) {
            self.win_head = wrap(self.win_head + 1, cap);
            self.win_len -= 1;
        }
        if !req.is_nan() {
            while self.win_len > 0 && self.win.get(wrap(self.win_head + self.win_len - 1, cap)).is_some_and(|&(_, v)| v >= req) {
                self.win_len -= 1;
            }
            if self.win_len == cap {
                self.win_head = wrap(self.win_head + 1, cap);
                self.win_len -= 1;
            }
            if let Some(slot) = self.win.get_mut(wrap(self.win_head + self.win_len, cap)) {
                *slot = (t, req);
                self.win_len += 1;
            }
        }
        match self.win.get(self.win_head) {
            Some(&(_, v)) if self.win_len > 0 => v.min(1.0),
            _ => 1.0,
        }
    }
}

fn wrap(i: usize, n: usize) -> usize {
    if i < n { i } else { i % n }
}

impl Plugin for Maximizer {
    param_plumbing!(MAX_INFO);

    fn prepare(&mut self, sample_rate: f32, _max_block: usize, channels: usize) {
        self.sr = sane_sr(sample_rate);
        self.la = (ms_to_samples(MAX_LOOKAHEAD_MS, self.sr).round() as usize).max(1);
        self.delay = (0..sane_channels(channels))
            .map(|_| {
                let mut d = DelayLine::default();
                d.allocate(self.la + 2);
                d
            })
            .collect();
        self.win = vec![(0, 1.0); self.la];
        self.smooth = vec![1.0; self.la];
        self.drive.set_time(20.0, self.sr);
        self.ceiling.set_time(20.0, self.sr);
        self.update();
        self.reset();
    }

    fn reset(&mut self) {
        self.delay.iter_mut().for_each(DelayLine::clear);
        self.win_head = 0;
        self.win_len = 0;
        self.clock = 0;
        self.smooth.iter_mut().for_each(|v| *v = 1.0);
        self.sum = self.la as f64;
        self.held = 1.0;
        self.idx = 0;
        self.drive.snap();
        self.ceiling.snap();
        self.gr_meter = 0.0;
    }

    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        let ch = self.delay.len().min(io.len());
        let frames = frames_in(io, frames, ch);
        let la = self.la.max(1);
        let mut min_gain = 1.0f32;
        for n in 0..frames {
            let drive = self.drive.next();
            let ceil = self.ceiling.next();
            let mut peak = 0.0f32;
            for c in 0..ch {
                let (Some(buf), Some(d)) = (io.get(c), self.delay.get_mut(c)) else { continue };
                let x = buf.get(n).copied().filter(|v| v.is_finite()).unwrap_or(0.0) * drive;
                d.push(x);
                peak = peak.max(x.abs());
            }
            let req = if peak > ceil { ceil / peak } else { 1.0 };
            let hold = self.window_min(req, la);
            self.held = if hold < self.held { hold } else { hold + (self.held - hold) * self.rel };
            if let Some(s) = self.smooth.get_mut(self.idx) {
                self.sum += f64::from(self.held) - f64::from(*s);
                *s = self.held;
            }
            self.idx = wrap(self.idx + 1, la);
            if self.idx == 0 {
                // Re-sum once per window to cancel floating-point drift.
                self.sum = self.smooth.iter().map(|&v| f64::from(v)).sum();
            }
            let g = ((self.sum / la as f64) as f32).clamp(0.0, 1.0);
            min_gain = min_gain.min(g);
            for c in 0..ch {
                let (Some(buf), Some(d)) = (io.get_mut(c), self.delay.get(c)) else { continue };
                if let Some(x) = buf.get_mut(n) {
                    *x = (d.tap(la) * g).clamp(-ceil, ceil);
                }
            }
        }
        self.gr_meter = -gain_to_db(min_gain);
    }

    fn latency(&self) -> usize {
        self.la - 1
    }

    fn gain_reduction_db(&self) -> f32 {
        self.gr_meter
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The window minimum as first written: the last `la` requests in a ring, scanned whole.
    #[test]
    fn window_min_matches_a_scan_of_the_last_requests() {
        let mut x = 0x2545_f491_4f6c_dd1du64;
        let mut next = move || {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x
        };
        for la in [1usize, 2, 3, 7, 64, 480] {
            let mut m = Maximizer::new();
            m.win = vec![(0, 1.0); la];
            let mut ring = vec![1.0f32; la];
            let mut at = 0;
            for i in 0..6_000 {
                if i == 3_000 {
                    m.reset();
                    ring.iter_mut().for_each(|v| *v = 1.0);
                    at = 0;
                }
                let r = next();
                let req = match r % 16 {
                    0 => f32::NAN,
                    1 => 1.0,
                    2 => 0.25,
                    _ => (r >> 40) as f32 / (1u64 << 24) as f32 * 1.5,
                };
                ring[at] = req;
                at = (at + 1) % la;
                let want = ring.iter().copied().fold(1.0f32, f32::min);
                assert_eq!(m.window_min(req, la).to_bits(), want.to_bits(), "la {la}, sample {i}");
            }
        }
    }
}
