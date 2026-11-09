//! Realtime Pitch Shifter: two crossfaded read taps sweeping through a short delay (granular
//! "rotating tape head" design). Latency is half the grain window.

use crate::params::{Params, lin, param_plumbing};
use crate::util::{DelayLine, Smoother, frames_in, ms_to_samples, sane_channels, sane_sr};
use crate::{Category, ParamInfo, Plugin, PluginInfo, Unit};

const WINDOW_MS: f32 = 50.0;

static PITCH_PARAMS: [ParamInfo; 3] = [
    lin("semitones", "Semitones", -24.0, 24.0, 0.0, Unit::Semitones),
    lin("cents", "Fine", -100.0, 100.0, 0.0, Unit::Cents),
    lin("mix", "Mix", 0.0, 100.0, 100.0, Unit::Percent),
];

pub static PITCH_INFO: PluginInfo = PluginInfo {
    id: "pitch_shifter",
    name: "Pitch Shifter",
    short_name: "Pitch",
    category: Category::PitchShift,
    params: &PITCH_PARAMS,
    is_instrument: false,
    is_midi_processor: false,
    midi_out: false,
    audiosuite: true,
};

pub struct PitchShifter {
    p: Params,
    sr: f32,
    lines: Vec<DelayLine>,
    window: f32,
    phase: f32,
    shift: Smoother,
    mix: Smoother,
}

impl Default for PitchShifter {
    fn default() -> Self {
        Self::new()
    }
}

impl PitchShifter {
    pub fn new() -> Self {
        let mut p = Self {
            p: Params::new(&PITCH_PARAMS),
            sr: 48_000.0,
            lines: Vec::new(),
            window: 2400.0,
            phase: 0.0,
            shift: Smoother::new(0.0),
            mix: Smoother::new(1.0),
        };
        p.update();
        p
    }

    fn update(&mut self) {
        self.shift.set(self.p.v(0) + self.p.v(1) / 100.0);
        self.mix.set(self.p.v(2) / 100.0);
    }
}

impl Plugin for PitchShifter {
    param_plumbing!(PITCH_INFO);

    fn prepare(&mut self, sample_rate: f32, _max_block: usize, channels: usize) {
        self.sr = sane_sr(sample_rate);
        self.window = ms_to_samples(WINDOW_MS, self.sr).max(16.0);
        self.lines = (0..sane_channels(channels))
            .map(|_| {
                let mut d = DelayLine::default();
                d.allocate(self.window as usize + 8);
                d
            })
            .collect();
        self.shift.set_time(30.0, self.sr);
        self.mix.set_time(20.0, self.sr);
        self.update();
        self.reset();
    }

    fn reset(&mut self) {
        self.lines.iter_mut().for_each(DelayLine::clear);
        self.phase = 0.0;
        self.shift.snap();
        self.mix.snap();
    }

    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        let ch = self.lines.len().min(io.len());
        let frames = frames_in(io, frames, ch);
        let w = self.window;
        let half = w * 0.5 + 2.0;
        for n in 0..frames {
            let ratio = 2f32.powf(self.shift.next() / 12.0);
            let m = self.mix.next();
            self.phase += (1.0 - ratio) / w;
            self.phase -= self.phase.floor();
            if !self.phase.is_finite() {
                self.phase = 0.0;
            }
            let p2 = if self.phase >= 0.5 { self.phase - 0.5 } else { self.phase + 0.5 };
            let g1 = (std::f32::consts::PI * self.phase).sin().powi(2);
            let g2 = 1.0 - g1;
            let (d1, d2) = (self.phase * w + 2.0, p2 * w + 2.0);
            for c in 0..ch {
                let (Some(buf), Some(line)) = (io.get_mut(c), self.lines.get_mut(c)) else { continue };
                let Some(x) = buf.get_mut(n) else { continue };
                line.push(if x.is_finite() { *x } else { 0.0 });
                let wet = g1 * line.read(d1) + g2 * line.read(d2);
                let dry = line.read(half);
                *x = dry * (1.0 - m) + wet * m;
            }
        }
    }

    fn latency(&self) -> usize {
        (self.window * 0.5 + 1.0).round() as usize
    }
}
