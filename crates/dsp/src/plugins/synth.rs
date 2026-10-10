//! Subtractive Synth: 16-voice polyphonic, two PolyBLEP oscillators, resonant state-variable
//! low-pass (TPT form) with its own ADSR, amp ADSR, optional glide.

use crate::osc::{Osc, Waveform};
use crate::params::{Params, choice, lin, log, param_plumbing};
use crate::util::{Smoother, frames_in, ms_to_samples, note_hz, sane_channels, sane_sr, sort_events};
use crate::{Category, ParamInfo, Plugin, PluginInfo, Unit, db_to_gain};

pub(crate) const MAX_EVENTS: usize = 512;

#[derive(Debug, Clone, Copy)]
pub(crate) enum EventKind {
    On(u8, u8),
    Off(u8),
    AllOff,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct Event {
    pub(crate) offset: usize,
    pub(crate) kind: EventKind,
}

/// Fixed-capacity event queue (never reallocates on the audio thread).
#[derive(Debug)]
pub(crate) struct EventQueue {
    ev: Vec<Event>,
}

impl EventQueue {
    pub(crate) fn new() -> Self {
        Self { ev: Vec::with_capacity(MAX_EVENTS) }
    }

    pub(crate) fn push(&mut self, offset: usize, kind: EventKind) {
        if self.ev.len() < MAX_EVENTS {
            self.ev.push(Event { offset, kind });
        } else {
            log::warn!("instrument event queue full; dropping event");
        }
    }

    pub(crate) fn sorted(&mut self) -> &[Event] {
        sort_events(&mut self.ev, |e| e.offset);
        &self.ev
    }

    pub(crate) fn clear(&mut self) {
        self.ev.clear();
    }
}

const WAVES: &[&str] = &["Saw", "Square", "Sine", "Triangle"];

fn wave(i: usize) -> Waveform {
    match i {
        1 => Waveform::Square,
        2 => Waveform::Sine,
        3 => Waveform::Triangle,
        _ => Waveform::Saw,
    }
}

static SYNTH_PARAMS: [ParamInfo; 19] = [
    choice("osc1_wave", "Osc 1", WAVES, 0),
    choice("osc2_wave", "Osc 2", WAVES, 0),
    lin("osc2_semitones", "Osc 2 Pitch", -24.0, 24.0, 0.0, Unit::Semitones),
    lin("osc2_detune", "Osc 2 Detune", -100.0, 100.0, 7.0, Unit::Cents),
    lin("osc_mix", "Osc Mix", 0.0, 100.0, 50.0, Unit::Percent),
    log("cutoff", "Cutoff", 20.0, 20000.0, 2000.0, Unit::Hz),
    lin("resonance", "Resonance", 0.0, 100.0, 20.0, Unit::Percent),
    lin("filter_env_amount", "Filter Env", -100.0, 100.0, 30.0, Unit::Percent),
    log("amp_attack", "Amp Attack", 0.5, 5000.0, 5.0, Unit::Ms),
    log("amp_decay", "Amp Decay", 1.0, 5000.0, 200.0, Unit::Ms),
    lin("amp_sustain", "Amp Sustain", 0.0, 100.0, 70.0, Unit::Percent),
    log("amp_release", "Amp Release", 1.0, 10000.0, 300.0, Unit::Ms),
    log("filter_attack", "Filter Attack", 0.5, 5000.0, 5.0, Unit::Ms),
    log("filter_decay", "Filter Decay", 1.0, 5000.0, 300.0, Unit::Ms),
    lin("filter_sustain", "Filter Sustain", 0.0, 100.0, 30.0, Unit::Percent),
    log("filter_release", "Filter Release", 1.0, 10000.0, 300.0, Unit::Ms),
    lin("glide", "Glide", 0.0, 1000.0, 0.0, Unit::Ms),
    lin("velocity", "Velocity Sens", 0.0, 100.0, 70.0, Unit::Percent),
    lin("level", "Level", -48.0, 6.0, -6.0, Unit::Db),
];

pub static SYNTH_INFO: PluginInfo = PluginInfo {
    id: "subtractive_synth",
    name: "Subtractive Synth",
    short_name: "SubSynth",
    category: Category::Instrument,
    params: &SYNTH_PARAMS,
    is_instrument: true,
    audiosuite: false,
};

const VOICES: usize = 16;
const SILENT: f32 = 1.0e-5;

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Stage {
    Idle,
    Attack,
    Decay,
    Sustain,
    Release,
}

/// Linear-attack, exponential decay/release ADSR. Times are "to -60 dB" for decay/release.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Adsr {
    pub(crate) stage: Stage,
    pub(crate) level: f32,
}

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct AdsrRates {
    attack: f32,
    decay: f32,
    sustain: f32,
    release: f32,
}

impl AdsrRates {
    pub(crate) fn new(a_ms: f32, d_ms: f32, s: f32, r_ms: f32, sr: f32) -> Self {
        let exp60 = |ms: f32| (0.001f32.ln() / ms_to_samples(ms, sr).max(1.0)).exp();
        Self { attack: 1.0 / ms_to_samples(a_ms, sr).max(1.0), decay: exp60(d_ms), sustain: s.clamp(0.0, 1.0), release: exp60(r_ms) }
    }
}

impl Adsr {
    pub(crate) const fn new() -> Self {
        Self { stage: Stage::Idle, level: 0.0 }
    }

    pub(crate) fn gate_on(&mut self) {
        self.stage = Stage::Attack;
    }

    pub(crate) fn gate_off(&mut self) {
        if self.stage != Stage::Idle {
            self.stage = Stage::Release;
        }
    }

    #[inline]
    pub(crate) fn next(&mut self, r: &AdsrRates) -> f32 {
        match self.stage {
            Stage::Idle => self.level = 0.0,
            Stage::Attack => {
                self.level += r.attack;
                if self.level >= 1.0 {
                    self.level = 1.0;
                    self.stage = Stage::Decay;
                }
            }
            Stage::Decay => {
                self.level = r.sustain + (self.level - r.sustain) * r.decay;
                if (self.level - r.sustain).abs() < 1e-4 {
                    self.stage = Stage::Sustain;
                }
            }
            Stage::Sustain => self.level = r.sustain,
            Stage::Release => {
                self.level *= r.release;
                if self.level < SILENT {
                    self.level = 0.0;
                    self.stage = Stage::Idle;
                }
            }
        }
        self.level
    }
}

/// Topology-preserving-transform state-variable low-pass.
#[derive(Debug, Clone, Copy, Default)]
struct Svf {
    ic1: f32,
    ic2: f32,
}

impl Svf {
    #[inline]
    fn lowpass(&mut self, x: f32, g: f32, k: f32) -> f32 {
        let a1 = 1.0 / (1.0 + g * (g + k));
        let a2 = g * a1;
        let a3 = g * a2;
        let v3 = x - self.ic2;
        let v1 = a1 * self.ic1 + a2 * v3;
        let v2 = self.ic2 + a2 * self.ic1 + a3 * v3;
        self.ic1 = crate::util::flush(2.0 * v1 - self.ic1);
        self.ic2 = crate::util::flush(2.0 * v2 - self.ic2);
        if v2.is_finite() {
            v2
        } else {
            *self = Svf::default();
            0.0
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct Voice {
    note: u8,
    vel: f32,
    freq: f32,
    target: f32,
    osc1: Osc,
    osc2: Osc,
    amp: Adsr,
    filt: Adsr,
    svf: Svf,
    age: u64,
    held: bool,
}

impl Voice {
    fn new(seed: u32) -> Self {
        Self {
            note: 0,
            vel: 0.0,
            freq: 440.0,
            target: 440.0,
            osc1: Osc::new(seed),
            osc2: Osc::new(seed.wrapping_mul(7919)),
            amp: Adsr::new(),
            filt: Adsr::new(),
            svf: Svf::default(),
            age: 0,
            held: false,
        }
    }

    fn active(&self) -> bool {
        self.amp.stage != Stage::Idle
    }
}

/// Polyphonic subtractive synthesizer.
pub struct SubtractiveSynth {
    p: Params,
    sr: f32,
    ch: usize,
    voices: [Voice; VOICES],
    events: EventQueue,
    amp_rates: AdsrRates,
    filt_rates: AdsrRates,
    glide_coef: f32,
    last_freq: Option<f32>,
    counter: u64,
    level: Smoother,
    cutoff: Smoother,
}

impl Default for SubtractiveSynth {
    fn default() -> Self {
        Self::new()
    }
}

impl SubtractiveSynth {
    pub fn new() -> Self {
        let mut voices = [Voice::new(1); VOICES];
        for (i, v) in voices.iter_mut().enumerate() {
            *v = Voice::new(0x1000 + i as u32 * 31);
        }
        let mut s = Self {
            p: Params::new(&SYNTH_PARAMS),
            sr: 48_000.0,
            ch: 0,
            voices,
            events: EventQueue::new(),
            amp_rates: AdsrRates::default(),
            filt_rates: AdsrRates::default(),
            glide_coef: 0.0,
            last_freq: None,
            counter: 0,
            level: Smoother::new(0.5),
            cutoff: Smoother::new(2000.0),
        };
        s.update();
        s
    }

    fn update(&mut self) {
        let sr = self.sr;
        let v = |i| self.p.v(i);
        self.amp_rates = AdsrRates::new(v(8), v(9), v(10) / 100.0, v(11), sr);
        self.filt_rates = AdsrRates::new(v(12), v(13), v(14) / 100.0, v(15), sr);
        let glide = v(16);
        self.glide_coef = if glide < 0.5 { 0.0 } else { crate::util::time_coef(glide / 3.0, sr) };
        self.level.set(db_to_gain(v(18)));
        self.cutoff.set(v(5));
    }

    fn start_note(&mut self, note: u8, vel: u8) {
        if vel == 0 {
            self.stop_note(note);
            return;
        }
        self.counter += 1;
        let freq = note_hz(f32::from(note.min(127)));
        // Reuse a voice already playing this note, else a free one, else steal.
        let idx = self
            .voices
            .iter()
            .position(|v| v.active() && v.note == note)
            .or_else(|| self.voices.iter().position(|v| !v.active()))
            .or_else(|| {
                self.voices
                    .iter()
                    .enumerate()
                    .min_by(|(_, a), (_, b)| {
                        let ka = (a.held, a.amp.level);
                        let kb = (b.held, b.amp.level);
                        ka.partial_cmp(&kb).unwrap_or(std::cmp::Ordering::Equal)
                    })
                    .map(|(i, _)| i)
            })
            .unwrap_or(0);
        let sens = self.p.v(17) / 100.0;
        let glide = self.glide_coef > 0.0;
        let start = if glide { self.last_freq.unwrap_or(freq) } else { freq };
        let counter = self.counter;
        if let Some(v) = self.voices.get_mut(idx) {
            let was_active = v.active();
            v.note = note;
            v.vel = (1.0 - sens) + sens * f32::from(vel.min(127)) / 127.0;
            v.target = freq;
            v.freq = if was_active && glide { v.freq } else { start };
            if !was_active {
                v.osc1.reset();
                v.osc2.phase = 0.37;
                v.svf = Svf::default();
            }
            v.amp.gate_on();
            v.filt.gate_on();
            v.age = counter;
            v.held = true;
        }
        self.last_freq = Some(freq);
    }

    fn stop_note(&mut self, note: u8) {
        for v in self.voices.iter_mut().filter(|v| v.held && v.note == note) {
            v.held = false;
            v.amp.gate_off();
            v.filt.gate_off();
        }
    }

    fn apply(&mut self, kind: EventKind) {
        match kind {
            EventKind::On(n, v) => self.start_note(n, v),
            EventKind::Off(n) => self.stop_note(n),
            EventKind::AllOff => {
                for v in &mut self.voices {
                    v.held = false;
                    v.amp.gate_off();
                    v.filt.gate_off();
                }
            }
        }
    }

    #[inline]
    fn render_sample(&mut self) -> f32 {
        let sr = self.sr;
        let w1 = wave(self.p.choice(0));
        let w2 = wave(self.p.choice(1));
        let ratio2 = 2f32.powf((self.p.v(2) + self.p.v(3) / 100.0) / 12.0);
        let mix = self.p.v(4) / 100.0;
        let k = 2.0 - 1.95 * (self.p.v(6) / 100.0);
        let env_oct = self.p.v(7) / 100.0 * 6.0;
        let cutoff = self.cutoff.next();
        let nyq = sr * 0.45;
        let mut out = 0.0;
        for v in &mut self.voices {
            if !v.active() {
                continue;
            }
            if self.glide_coef > 0.0 {
                v.freq = v.target + (v.freq - v.target) * self.glide_coef;
            } else {
                v.freq = v.target;
            }
            let a = v.amp.next(&self.amp_rates);
            let fe = v.filt.next(&self.filt_rates);
            let s1 = v.osc1.next(w1, v.freq / sr);
            let s2 = v.osc2.next(w2, v.freq * ratio2 / sr);
            let raw = s1 * (1.0 - mix) + s2 * mix;
            let fc = (cutoff * 2f32.powf(env_oct * fe)).clamp(20.0, nyq);
            let g = (std::f32::consts::PI * fc / sr).tan();
            let y = v.svf.lowpass(raw, g, k);
            out += y * a * v.vel;
        }
        out * 0.3
    }
}

impl Plugin for SubtractiveSynth {
    param_plumbing!(SYNTH_INFO);

    fn prepare(&mut self, sample_rate: f32, _max_block: usize, channels: usize) {
        self.sr = sane_sr(sample_rate);
        self.ch = sane_channels(channels);
        self.level.set_time(20.0, self.sr);
        self.cutoff.set_time(20.0, self.sr);
        self.update();
        self.reset();
    }

    fn reset(&mut self) {
        for (i, v) in self.voices.iter_mut().enumerate() {
            *v = Voice::new(0x1000 + i as u32 * 31);
        }
        self.events.clear();
        self.last_freq = None;
        self.level.snap();
        self.cutoff.snap();
    }

    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        let ch = self.ch.min(io.len());
        let frames = frames_in(io, frames, ch);
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
            let y = self.render_sample() * self.level.next();
            for b in io.iter_mut().take(ch) {
                if let Some(x) = b.get_mut(n) {
                    *x = y;
                }
            }
        }
        // Events for an empty block take effect immediately.
        for e in evs.iter().take(count).skip(next) {
            self.apply(e.kind);
        }
    }

    fn tail_samples(&self) -> usize {
        ms_to_samples(self.p.v(11), self.sr) as usize
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
}
