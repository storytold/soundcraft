//! Sampler: plays one recorded sample across the keyboard, 32 voices.
//!
//! Each note plays the sample pitched from its root note (an octave up plays twice as fast) with
//! 4-point cubic interpolation, which also converts the sample's own rate to the host's. Key and
//! velocity ranges, an amp ADSR with velocity sensitivity, a sample start point, forward or
//! ping-pong loops, and gate or one-shot playback. The host hands the audio over with
//! [`Plugin::set_sample`]; without one the Sampler is silent.

use std::sync::Arc;

use super::synth::{Adsr, AdsrRates, Event, EventKind, EventQueue, MAX_EVENTS, Stage};
use crate::params::{Params, choice, lin, log, param_plumbing, toggle};
use crate::util::{Smoother, frames_in, ms_to_samples, sane_channels, sane_sr};
use crate::{Category, ParamInfo, Plugin, PluginInfo, SampleSource, Unit, db_to_gain};

const PLAY_MODES: &[&str] = &["Gate", "One Shot"];
const LOOP_MODES: &[&str] = &["Off", "Forward", "Ping-Pong"];

const ROOT: usize = 0;
const TRANSPOSE: usize = 1;
const FINE: usize = 2;
const KEY_TRACK: usize = 3;
const KEY_LOW: usize = 4;
const KEY_HIGH: usize = 5;
const VEL_LOW: usize = 6;
const VEL_HIGH: usize = 7;
const VEL_SENS: usize = 8;
const ATTACK: usize = 9;
const DECAY: usize = 10;
const SUSTAIN: usize = 11;
const RELEASE: usize = 12;
const PLAY_MODE: usize = 13;
const START: usize = 14;
const LOOP_MODE: usize = 15;
const LOOP_START: usize = 16;
const LOOP_END: usize = 17;
const LEVEL: usize = 18;

static SAMPLER_PARAMS: [ParamInfo; 19] = [
    lin("root_note", "Root Note", 0.0, 127.0, 60.0, Unit::None),
    lin("transpose", "Transpose", -48.0, 48.0, 0.0, Unit::Semitones),
    lin("fine_tune", "Fine Tune", -100.0, 100.0, 0.0, Unit::Cents),
    toggle("key_track", "Key Tracking", true),
    lin("key_low", "Lowest Key", 0.0, 127.0, 0.0, Unit::None),
    lin("key_high", "Highest Key", 0.0, 127.0, 127.0, Unit::None),
    lin("vel_low", "Lowest Velocity", 1.0, 127.0, 1.0, Unit::None),
    lin("vel_high", "Highest Velocity", 1.0, 127.0, 127.0, Unit::None),
    lin("velocity", "Velocity Sens", 0.0, 100.0, 70.0, Unit::Percent),
    log("attack", "Attack", 0.1, 5000.0, 1.0, Unit::Ms),
    log("decay", "Decay", 1.0, 10000.0, 300.0, Unit::Ms),
    lin("sustain", "Sustain", 0.0, 100.0, 100.0, Unit::Percent),
    log("release", "Release", 1.0, 10000.0, 150.0, Unit::Ms),
    choice("play_mode", "Play Mode", PLAY_MODES, 0),
    lin("start", "Sample Start", 0.0, 100.0, 0.0, Unit::Percent),
    choice("loop_mode", "Loop", LOOP_MODES, 0),
    lin("loop_start", "Loop Start", 0.0, 100.0, 0.0, Unit::Percent),
    lin("loop_end", "Loop End", 0.0, 100.0, 100.0, Unit::Percent),
    lin("level", "Level", -48.0, 12.0, 0.0, Unit::Db),
];

pub static SAMPLER_INFO: PluginInfo = PluginInfo {
    id: "sampler",
    name: "Sampler",
    short_name: "Sampler",
    category: Category::Instrument,
    params: &SAMPLER_PARAMS,
    is_instrument: true,
    audiosuite: false,
};

const VOICES: usize = 32;
/// Fastest playback step in sample frames per output frame (keeps hostile pitch bounded).
const MAX_STEP: f64 = 256.0;

#[derive(Debug, Clone, Copy, PartialEq)]
enum LoopMode {
    Off,
    Forward,
    PingPong,
}

#[derive(Debug, Clone, Copy)]
struct Voice {
    note: u8,
    /// Key still down (gate mode only; one-shot voices are never held).
    held: bool,
    gain: f32,
    /// Read position in sample frames.
    pos: f64,
    step: f64,
    forward: bool,
    looping: bool,
    env: Adsr,
}

impl Voice {
    const fn new() -> Self {
        Self { note: 0, held: false, gain: 0.0, pos: 0.0, step: 1.0, forward: true, looping: false, env: Adsr::new() }
    }

    fn active(&self) -> bool {
        self.env.stage != Stage::Idle
    }

    fn stop(&mut self) {
        *self = Voice::new();
    }
}

/// Values derived from the parameters (refreshed on every change).
#[derive(Debug, Clone, Copy)]
struct Derived {
    root: f64,
    /// Transpose plus fine tune, in semitones.
    offset: f64,
    key_track: bool,
    keys: (u8, u8),
    vels: (u8, u8),
    sens: f32,
    one_shot: bool,
    start: f64,
    loop_mode: LoopMode,
    /// Loop start and end as fractions of the sample (start <= end).
    loop_span: (f64, f64),
}

/// A MIDI key or velocity parameter as a whole number in `lo..=127`.
fn whole(v: f32, lo: u8) -> u8 {
    if v.is_finite() { v.round().clamp(f32::from(lo), 127.0) as u8 } else { lo }
}

fn ordered(a: u8, b: u8) -> (u8, u8) {
    (a.min(b), a.max(b))
}

/// 4-point cubic (Catmull-Rom) interpolation of `x` at fractional frame `pos` (>= 0). Reads
/// outside the slice are silence.
#[inline]
fn hermite(x: &[f32], pos: f64) -> f32 {
    let i = pos as usize;
    let t = (pos - i as f64) as f32;
    let at = |k: usize| x.get(k).copied().unwrap_or(0.0);
    let x0 = at(i);
    if t == 0.0 {
        return if x0.is_finite() { x0 } else { 0.0 };
    }
    let xm1 = at(i.saturating_sub(1));
    let x1 = at(i.saturating_add(1));
    let x2 = at(i.saturating_add(2));
    let c1 = 0.5 * (x1 - xm1);
    let c2 = xm1 - 2.5 * x0 + 2.0 * x1 - 0.5 * x2;
    let c3 = 0.5 * (x2 - xm1) + 1.5 * (x0 - x1);
    let y = ((c3 * t + c2) * t + c1) * t + x0;
    // A float file can hold NaN or infinities; they play as silence.
    if y.is_finite() { y } else { 0.0 }
}

/// Plays a sample across the keyboard.
pub struct Sampler {
    p: Params,
    d: Derived,
    sr: f32,
    ch: usize,
    sample: Option<Arc<dyn SampleSource>>,
    /// Frames in the sample (0 = nothing to play).
    len: usize,
    /// The sample's own rate.
    sample_sr: f32,
    voices: [Voice; VOICES],
    events: EventQueue,
    rates: AdsrRates,
    level: Smoother,
}

impl Default for Sampler {
    fn default() -> Self {
        Self::new()
    }
}

impl Sampler {
    pub fn new() -> Self {
        let mut s = Self {
            p: Params::new(&SAMPLER_PARAMS),
            d: Derived {
                root: 60.0,
                offset: 0.0,
                key_track: true,
                keys: (0, 127),
                vels: (1, 127),
                sens: 0.7,
                one_shot: false,
                start: 0.0,
                loop_mode: LoopMode::Off,
                loop_span: (0.0, 1.0),
            },
            sr: 48_000.0,
            ch: 0,
            sample: None,
            len: 0,
            sample_sr: 48_000.0,
            voices: [Voice::new(); VOICES],
            events: EventQueue::new(),
            rates: AdsrRates::default(),
            level: Smoother::new(1.0),
        };
        s.update();
        s
    }

    fn update(&mut self) {
        let v = |i| self.p.v(i);
        let pct = |i| f64::from(v(i) / 100.0).clamp(0.0, 1.0);
        let (ls, le) = (pct(LOOP_START), pct(LOOP_END));
        self.d = Derived {
            root: f64::from(whole(v(ROOT), 0)),
            offset: f64::from(v(TRANSPOSE)) + f64::from(v(FINE)) / 100.0,
            key_track: self.p.on(KEY_TRACK),
            keys: ordered(whole(v(KEY_LOW), 0), whole(v(KEY_HIGH), 0)),
            vels: ordered(whole(v(VEL_LOW), 1), whole(v(VEL_HIGH), 1)),
            sens: (v(VEL_SENS) / 100.0).clamp(0.0, 1.0),
            one_shot: self.p.choice(PLAY_MODE) == 1,
            start: pct(START),
            loop_mode: match self.p.choice(LOOP_MODE) {
                1 => LoopMode::Forward,
                2 => LoopMode::PingPong,
                _ => LoopMode::Off,
            },
            loop_span: (ls.min(le), ls.max(le)),
        };
        self.rates = AdsrRates::new(v(ATTACK), v(DECAY), v(SUSTAIN) / 100.0, v(RELEASE), self.sr);
        self.level.set(db_to_gain(v(LEVEL)));
    }

    /// The loop region in sample frames, when it is long enough to loop.
    fn loop_frames(&self) -> Option<(f64, f64)> {
        if self.d.loop_mode == LoopMode::Off {
            return None;
        }
        let len = self.len as f64;
        let (a, b) = (self.d.loop_span.0 * len, self.d.loop_span.1 * len);
        (b - a >= 1.0).then_some((a, b))
    }

    fn start_note(&mut self, note: u8, vel: u8) {
        if vel == 0 {
            self.stop_note(note);
            return;
        }
        let d = self.d;
        let (note, vel) = (note.min(127), vel.min(127));
        if self.len == 0 || note < d.keys.0 || note > d.keys.1 || vel < d.vels.0 || vel > d.vels.1 {
            return;
        }
        let semis = if d.key_track { f64::from(note) - d.root } else { 0.0 } + d.offset;
        let step = f64::from(self.sample_sr) / f64::from(self.sr) * 2f64.powf(semis / 12.0);
        if !step.is_finite() || step <= 0.0 {
            return;
        }
        if !d.one_shot {
            // Retriggering a held key releases the earlier voice instead of stacking it.
            self.stop_note(note);
        }
        let idx = self
            .voices
            .iter()
            .position(|v| !v.active())
            .or_else(|| {
                self.voices
                    .iter()
                    .enumerate()
                    .min_by(|(_, a), (_, b)| (a.held, a.env.level).partial_cmp(&(b.held, b.env.level)).unwrap_or(std::cmp::Ordering::Equal))
                    .map(|(i, _)| i)
            })
            .unwrap_or(0);
        let start = d.start * self.len.saturating_sub(1) as f64;
        let looping = self.loop_frames().is_some_and(|(_, end)| start < end);
        let gain = (1.0 - d.sens) + d.sens * f32::from(vel) / 127.0;
        if let Some(v) = self.voices.get_mut(idx) {
            *v = Voice { note, held: !d.one_shot, gain, pos: start, step: step.min(MAX_STEP), forward: true, looping, env: Adsr::new() };
            v.env.gate_on();
        }
    }

    fn stop_note(&mut self, note: u8) {
        for v in self.voices.iter_mut().filter(|v| v.held && v.note == note) {
            v.held = false;
            v.env.gate_off();
        }
    }

    fn apply(&mut self, kind: EventKind) {
        match kind {
            EventKind::On(n, v) => self.start_note(n, v),
            EventKind::Off(n) => self.stop_note(n),
            EventKind::AllOff => {
                for v in &mut self.voices {
                    v.held = false;
                    v.env.gate_off();
                }
            }
            EventKind::Sustain(_) | EventKind::Bend(_) | EventKind::ResetControllers => {}
        }
    }

    /// Adds every voice's output for frame `n` into `io` and advances the voices.
    #[inline]
    fn render_frame(&mut self, io: &mut [Vec<f32>], ch: usize, n: usize, level: f32) {
        let Some(sample) = self.sample.as_deref() else { return };
        let nch = sample.num_channels();
        let len = self.len as f64;
        let mode = self.d.loop_mode;
        let region = self.loop_frames();
        for v in &mut self.voices {
            if !v.active() {
                continue;
            }
            let a = v.env.next(&self.rates) * v.gain * level;
            for (c, b) in io.iter_mut().take(ch).enumerate() {
                let src = if nch == 1 { 0 } else { c };
                if let (Some(x), Some(data)) = (b.get_mut(n), sample.channel(src)) {
                    *x += hermite(data, v.pos) * a;
                }
            }
            let r = if v.looping { region } else { None };
            advance(v, len, r, mode);
        }
    }
}

/// Moves a voice's read position one output frame on, looping or ending it.
#[inline]
fn advance(v: &mut Voice, len: f64, region: Option<(f64, f64)>, mode: LoopMode) {
    let was_forward = v.forward;
    v.pos = if was_forward { v.pos + v.step } else { v.pos - v.step };
    match region {
        Some((ls, le)) if mode == LoopMode::PingPong => {
            // How far past the edge it was heading for, measured from the edge it left. A step of
            // more than the loop's width crosses it more than once: fold the distance into the
            // loop (even crossings keep the direction, odd ones turn it round) instead of
            // reflecting once and dropping the rest of the travel.
            let width = le - ls;
            let dist = if was_forward { v.pos - ls } else { le - v.pos };
            if width > 0.0 && dist >= width && dist.is_finite() {
                let crossings = (dist / width).floor();
                let into = dist - crossings * width;
                let odd = crossings % 2.0 == 1.0;
                (v.pos, v.forward) = match (was_forward, odd) {
                    (true, false) => (ls + into, true),
                    (true, true) => (le - into, false),
                    (false, false) => (le - into, false),
                    (false, true) => (ls + into, true),
                };
            }
        }
        Some((ls, le)) => {
            if v.pos >= le {
                v.pos = ls + (v.pos - ls) % (le - ls);
            }
        }
        None => {
            if v.pos >= len || !v.pos.is_finite() {
                v.stop();
            }
        }
    }
    if !v.pos.is_finite() || v.pos < 0.0 {
        v.stop();
    }
}

impl Plugin for Sampler {
    param_plumbing!(SAMPLER_INFO);

    fn prepare(&mut self, sample_rate: f32, _max_block: usize, channels: usize) {
        self.sr = sane_sr(sample_rate);
        self.ch = sane_channels(channels);
        self.level.set_time(20.0, self.sr);
        self.update();
        self.reset();
    }

    fn reset(&mut self) {
        for v in &mut self.voices {
            v.stop();
        }
        self.events.clear();
        self.level.snap();
    }

    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        let ch = self.ch.min(io.len());
        let frames = frames_in(io, frames, ch);
        for b in io.iter_mut().take(ch) {
            b.iter_mut().take(frames).for_each(|x| *x = 0.0);
        }
        let mut evs = [Event { offset: 0, kind: EventKind::AllOff }; MAX_EVENTS];
        let count = {
            let sorted = self.events.sorted();
            for (d, s) in evs.iter_mut().zip(sorted) {
                *d = *s;
            }
            sorted.len().min(MAX_EVENTS)
        };
        self.events.clear();
        let mut next = 0;
        for n in 0..frames {
            while let Some(e) = evs.get(next).filter(|_| next < count) {
                if e.offset > n && n + 1 < frames {
                    break;
                }
                let kind = e.kind;
                self.apply(kind);
                next += 1;
            }
            let level = self.level.next();
            self.render_frame(io, ch, n, level);
        }
        // Events for an empty block take effect immediately.
        for e in evs.iter().take(count).skip(next) {
            self.apply(e.kind);
        }
    }

    fn tail_samples(&self) -> usize {
        let release = ms_to_samples(self.p.v(RELEASE), self.sr);
        // A one-shot keeps playing its sample after the note ends.
        let rest = if self.d.one_shot { self.len as f32 * self.sr / self.sample_sr } else { 0.0 };
        let t = release + rest;
        if t.is_finite() { t.min(1.0e9) as usize } else { 0 }
    }

    fn note_on(&mut self, offset: usize, note: u8, velocity: u8) {
        self.events.push(offset, EventKind::On(note, velocity));
    }

    fn note_off(&mut self, offset: usize, note: u8) {
        self.events.push(offset, EventKind::Off(note));
    }

    fn all_notes_off(&mut self) {
        self.events.push(0, EventKind::AllOff);
    }

    fn set_sample(&mut self, sample: Option<Arc<dyn SampleSource>>) -> bool {
        for v in &mut self.voices {
            v.stop();
        }
        self.len = sample.as_deref().map_or(0, |s| if s.num_channels() == 0 { 0 } else { s.frames() });
        let rate = sample.as_deref().map_or(self.sr, |s| s.sample_rate());
        // An unusable rate plays at the host rate rather than not at all.
        self.sample_sr = if rate.is_finite() && rate > 0.0 { rate.min(1_536_000.0) } else { self.sr };
        self.sample = sample;
        true
    }
}

#[cfg(test)]
mod tests;
