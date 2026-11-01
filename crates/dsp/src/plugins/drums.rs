//! Drum Synth: fully synthesized kit on a General-MIDI-style note map.
//!
//! 35/36 kick, 37/39 clap, 38/40 snare, 42/44 closed hat (chokes open hat), 46 open hat,
//! 41/43 floor tom, 45 low tom, 47/48 mid tom, 50 high tom, 49/57 crash, 51/59 ride.

use std::f32::consts::TAU;

use super::synth::{EventKind, EventQueue, MAX_EVENTS};
use crate::biquad::{Biquad, FilterType};
use crate::params::{Params, lin, log, param_plumbing};
use crate::util::{Rng, Smoother, frames_in, ms_to_samples, sane_channels, sane_sr};
use crate::{Category, ParamInfo, Plugin, PluginInfo, Unit, db_to_gain};

static DRUM_PARAMS: [ParamInfo; 11] = [
    lin("level", "Level", -48.0, 6.0, -6.0, Unit::Db),
    lin("kick_tune", "Kick Tune", -12.0, 12.0, 0.0, Unit::Semitones),
    log("kick_decay", "Kick Decay", 50.0, 2000.0, 450.0, Unit::Ms),
    lin("snare_tune", "Snare Tune", -12.0, 12.0, 0.0, Unit::Semitones),
    lin("snare_snappy", "Snare Snappy", 0.0, 100.0, 60.0, Unit::Percent),
    log("snare_decay", "Snare Decay", 50.0, 1000.0, 220.0, Unit::Ms),
    log("hat_decay", "Closed Hat Decay", 20.0, 500.0, 80.0, Unit::Ms),
    log("open_hat_decay", "Open Hat Decay", 100.0, 3000.0, 600.0, Unit::Ms),
    lin("tom_tune", "Tom Tune", -12.0, 12.0, 0.0, Unit::Semitones),
    log("tom_decay", "Tom Decay", 50.0, 2000.0, 500.0, Unit::Ms),
    log("cymbal_decay", "Cymbal Decay", 200.0, 6000.0, 2000.0, Unit::Ms),
];

pub static DRUM_INFO: PluginInfo = PluginInfo {
    id: "drum_synth",
    name: "Drum Synth",
    short_name: "Drums",
    category: Category::Instrument,
    params: &DRUM_PARAMS,
    is_instrument: true,
    audiosuite: false,
};

#[derive(Debug, Clone, Copy, PartialEq)]
enum Kind {
    Kick,
    Snare,
    Clap,
    ClosedHat,
    OpenHat,
    Tom(f32),
    Crash,
    Ride,
}

fn kind_for(note: u8) -> Option<Kind> {
    Some(match note {
        35 | 36 => Kind::Kick,
        38 | 40 => Kind::Snare,
        37 | 39 => Kind::Clap,
        42 | 44 => Kind::ClosedHat,
        46 => Kind::OpenHat,
        41 | 43 => Kind::Tom(82.0),
        45 => Kind::Tom(110.0),
        47 | 48 => Kind::Tom(147.0),
        50 => Kind::Tom(196.0),
        49 | 57 => Kind::Crash,
        51 | 59 => Kind::Ride,
        _ => return None,
    })
}

/// Inharmonic partials (Hz) for the metallic cymbal/hat oscillator bank.
const METAL: [f32; 6] = [263.0, 400.0, 421.0, 474.0, 587.0, 845.0];
const VOICES: usize = 24;
const OFF: f32 = 1.0e-4;

#[derive(Debug, Clone, Copy)]
struct DrumVoice {
    kind: Option<Kind>,
    vel: f32,
    t: usize,
    amp: f32,
    amp_coef: f32,
    noise_amp: f32,
    noise_coef: f32,
    pitch_env: f32,
    pitch_coef: f32,
    f_start: f32,
    f_end: f32,
    phase: f32,
    metal: [f32; 6],
    filt: Biquad,
    filt2: Biquad,
    rng: Rng,
    age: u64,
}

impl DrumVoice {
    fn new(seed: u32) -> Self {
        Self {
            kind: None,
            vel: 0.0,
            t: 0,
            amp: 0.0,
            amp_coef: 0.0,
            noise_amp: 0.0,
            noise_coef: 0.0,
            pitch_env: 0.0,
            pitch_coef: 0.0,
            f_start: 0.0,
            f_end: 0.0,
            phase: 0.0,
            metal: [0.0; 6],
            filt: Biquad::default(),
            filt2: Biquad::default(),
            rng: Rng::new(seed),
            age: 0,
        }
    }
}

/// Coefficient that decays by 60 dB over `ms`.
fn decay60(ms: f32, sr: f32) -> f32 {
    (0.001f32.ln() / ms_to_samples(ms, sr).max(1.0)).exp()
}

pub struct DrumSynth {
    p: Params,
    sr: f32,
    ch: usize,
    voices: [DrumVoice; VOICES],
    events: EventQueue,
    counter: u64,
    level: Smoother,
}

impl Default for DrumSynth {
    fn default() -> Self {
        Self::new()
    }
}

/// One output sample of a sounding voice of `kind`, advancing its envelopes.
fn voice_sample(v: &mut DrumVoice, kind: Kind, sr: f32, clap_gap: usize) -> f32 {
    match kind {
        Kind::Kick | Kind::Tom(_) => {
            v.pitch_env *= v.pitch_coef;
            let f = v.f_end + (v.f_start - v.f_end) * v.pitch_env;
            v.phase += f / sr;
            v.phase -= v.phase.floor();
            let tone = (v.phase * TAU).sin() * v.amp;
            let click = v.filt.process_sample(v.rng.bipolar()) * v.noise_amp;
            v.amp *= v.amp_coef;
            v.noise_amp *= v.noise_coef;
            tone + click
        }
        Kind::Snare => {
            v.pitch_env *= v.pitch_coef;
            let f = v.f_end + (v.f_start - v.f_end) * v.pitch_env;
            v.phase += f / sr;
            v.phase -= v.phase.floor();
            let tone = (v.phase * TAU).sin() * v.amp * 0.6;
            let n = v.filt2.process_sample(v.filt.process_sample(v.rng.bipolar())) * v.noise_amp * 0.7;
            v.amp *= v.amp_coef;
            v.noise_amp *= v.noise_coef;
            tone + n
        }
        Kind::Clap => {
            // Three quick bursts then a diffuse tail.
            let burst = if v.t < clap_gap * 3 {
                if v.t.is_multiple_of(clap_gap) {
                    v.amp = 1.0;
                }
                v.amp
            } else {
                0.0
            };
            let env = burst.max(if v.t >= clap_gap * 2 { v.noise_amp * 0.6 } else { 0.0 });
            let y = v.filt.process_sample(v.rng.bipolar()) * env * 2.0;
            v.amp *= v.amp_coef;
            if v.t >= clap_gap * 2 {
                v.noise_amp *= v.noise_coef;
            }
            y
        }
        Kind::ClosedHat | Kind::OpenHat | Kind::Crash | Kind::Ride => {
            let scale = if matches!(kind, Kind::Crash | Kind::Ride) { 1.0 } else { 1.9 };
            let mut metal = 0.0;
            for (m, f) in v.metal.iter_mut().zip(METAL) {
                *m += f * scale / sr;
                *m -= m.floor();
                metal += if *m < 0.5 { 1.0 } else { -1.0 };
            }
            let raw = metal / 6.0 * (1.0 - v.noise_amp * 0.5) + v.rng.bipolar() * v.noise_amp;
            let y = v.filt2.process_sample(v.filt.process_sample(raw)) * v.amp;
            v.amp *= v.amp_coef;
            y
        }
    }
}

impl DrumSynth {
    pub fn new() -> Self {
        let mut voices = [DrumVoice::new(1); VOICES];
        for (i, v) in voices.iter_mut().enumerate() {
            *v = DrumVoice::new(0xBEEF + i as u32 * 977);
        }
        let mut d =
            Self { p: Params::new(&DRUM_PARAMS), sr: 48_000.0, ch: 0, voices, events: EventQueue::new(), counter: 0, level: Smoother::new(0.5) };
        d.update();
        d
    }

    fn update(&mut self) {
        self.level.set(db_to_gain(self.p.v(0)));
    }

    fn trigger(&mut self, note: u8, vel: u8) {
        let Some(kind) = kind_for(note) else { return };
        if vel == 0 {
            return;
        }
        let sr = self.sr;
        if kind == Kind::ClosedHat {
            let choke = decay60(15.0, sr);
            for v in self.voices.iter_mut().filter(|v| v.kind == Some(Kind::OpenHat)) {
                v.amp_coef = v.amp_coef.min(choke);
                v.noise_coef = v.noise_coef.min(choke);
            }
        }
        self.counter += 1;
        let idx = self
            .voices
            .iter()
            .position(|v| v.kind.is_none())
            .or_else(|| self.voices.iter().enumerate().min_by_key(|(_, v)| v.age).map(|(i, _)| i))
            .unwrap_or(0);
        let v = |i| self.p.v(i);
        let semis = |st: f32| 2f32.powf(st / 12.0);
        let (kick_tune, kick_decay, snare_tune, snappy, snare_decay) = (semis(v(1)), v(2), semis(v(3)), v(4) / 100.0, v(5));
        let (hat, open, tom_tune, tom_decay, cym) = (v(6), v(7), semis(v(8)), v(9), v(10));
        let counter = self.counter;
        let Some(dv) = self.voices.get_mut(idx) else { return };
        let seed = dv.rng.next_u32();
        *dv = DrumVoice::new(seed);
        dv.kind = Some(kind);
        dv.vel = f32::from(vel.min(127)) / 127.0;
        dv.age = counter;
        dv.amp = 1.0;
        dv.pitch_env = 1.0;
        match kind {
            Kind::Kick => {
                dv.f_start = 170.0 * kick_tune;
                dv.f_end = 48.0 * kick_tune;
                dv.pitch_coef = decay60(120.0, sr);
                dv.amp_coef = decay60(kick_decay, sr);
                dv.noise_amp = 0.5;
                dv.noise_coef = decay60(6.0, sr);
                dv.filt = Biquad::design(FilterType::LowPass, 3000.0, 0.7, 0.0, sr);
            }
            Kind::Snare => {
                dv.f_start = 260.0 * snare_tune;
                dv.f_end = 185.0 * snare_tune;
                dv.pitch_coef = decay60(40.0, sr);
                dv.amp_coef = decay60(snare_decay * 0.6, sr);
                dv.noise_amp = 0.3 + snappy;
                dv.noise_coef = decay60(snare_decay, sr);
                dv.filt = Biquad::design(FilterType::HighPass, 1200.0, 0.7, 0.0, sr);
                dv.filt2 = Biquad::design(FilterType::LowPass, 9000.0, 0.7, 0.0, sr);
            }
            Kind::Clap => {
                dv.amp = 0.0;
                dv.noise_amp = 1.0;
                dv.noise_coef = decay60(250.0, sr);
                dv.amp_coef = decay60(25.0, sr);
                dv.filt = Biquad::design(FilterType::BandPass, 1150.0, 1.6, 0.0, sr);
            }
            Kind::ClosedHat | Kind::OpenHat | Kind::Crash | Kind::Ride => {
                let (ms, hp, noise) = match kind {
                    Kind::ClosedHat => (hat, 7000.0, 0.6),
                    Kind::OpenHat => (open, 6500.0, 0.6),
                    Kind::Crash => (cym, 4500.0, 0.9),
                    _ => (cym * 0.6, 5500.0, 0.25),
                };
                dv.amp_coef = decay60(ms, sr);
                dv.noise_amp = noise;
                dv.noise_coef = dv.amp_coef;
                dv.filt = Biquad::design(FilterType::HighPass, hp, 0.7, 0.0, sr);
                dv.filt2 = Biquad::design(FilterType::HighPass, hp, 0.7, 0.0, sr);
                for (i, m) in dv.metal.iter_mut().enumerate() {
                    *m = i as f32 * 0.13;
                }
            }
            Kind::Tom(base) => {
                dv.f_start = base * 1.6 * tom_tune;
                dv.f_end = base * tom_tune;
                dv.pitch_coef = decay60(180.0, sr);
                dv.amp_coef = decay60(tom_decay, sr);
                dv.noise_amp = 0.15;
                dv.noise_coef = decay60(10.0, sr);
                dv.filt = Biquad::design(FilterType::LowPass, 4000.0, 0.7, 0.0, sr);
            }
        }
    }

    #[inline]
    fn render(&mut self) -> f32 {
        let sr = self.sr;
        let clap_gap = ms_to_samples(11.0, sr).max(1.0) as usize;
        let mut out = 0.0;
        for v in &mut self.voices {
            let Some(kind) = v.kind else { continue };
            let y = voice_sample(v, kind, sr, clap_gap);
            v.t += 1;
            let alive = match kind {
                Kind::Clap => v.t < clap_gap * 3 || v.noise_amp > OFF,
                _ => v.amp > OFF || v.noise_amp * v.noise_coef > OFF && v.t < (sr * 0.05) as usize,
            };
            if !alive {
                v.kind = None;
            }
            out += y * v.vel;
        }
        out * 0.5
    }
}

impl Plugin for DrumSynth {
    param_plumbing!(DRUM_INFO);

    fn prepare(&mut self, sample_rate: f32, _max_block: usize, channels: usize) {
        self.sr = sane_sr(sample_rate);
        self.ch = sane_channels(channels);
        self.level.set_time(20.0, self.sr);
        self.update();
        self.reset();
    }

    fn reset(&mut self) {
        for v in &mut self.voices {
            v.kind = None;
        }
        self.events.clear();
        self.level.snap();
    }

    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        let ch = self.ch.min(io.len());
        let frames = frames_in(io, frames, ch);
        let mut evs = [(0usize, EventKind::AllOff); MAX_EVENTS];
        let count = {
            let sorted = self.events.sorted();
            for (d, s) in evs.iter_mut().zip(sorted) {
                *d = (s.offset, s.kind);
            }
            sorted.len().min(MAX_EVENTS)
        };
        self.events.clear();
        let mut next = 0;
        for n in 0..frames {
            while next < count {
                let Some(&(off, kind)) = evs.get(next) else { break };
                if off > n && n + 1 < frames {
                    break;
                }
                self.apply(kind);
                next += 1;
            }
            let y = self.render() * self.level.next();
            for b in io.iter_mut().take(ch) {
                if let Some(x) = b.get_mut(n) {
                    *x = y;
                }
            }
        }
        for &(_, kind) in evs.iter().take(count).skip(next) {
            self.apply(kind);
        }
    }

    fn tail_samples(&self) -> usize {
        ms_to_samples(self.p.v(10).max(self.p.v(2)), self.sr) as usize
    }

    fn note_on(&mut self, offset: usize, note: u8, velocity: u8) {
        self.events.push(offset, EventKind::On(note, velocity));
    }

    fn note_off(&mut self, _offset: usize, _note: u8) {}

    fn all_notes_off(&mut self) {
        self.events.push(0, EventKind::AllOff);
    }
}

impl DrumSynth {
    fn apply(&mut self, kind: EventKind) {
        match kind {
            EventKind::On(n, v) => self.trigger(n, v),
            EventKind::Off(_) | EventKind::Sustain(_) | EventKind::Bend(_) | EventKind::ResetControllers => {}
            EventKind::AllOff => {
                let fast = decay60(30.0, self.sr);
                for v in &mut self.voices {
                    v.amp_coef = v.amp_coef.min(fast);
                    v.noise_coef = v.noise_coef.min(fast);
                }
            }
        }
    }
}
