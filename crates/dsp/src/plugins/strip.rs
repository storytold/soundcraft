//! Channel Strip: input trim → gate → compressor → 3-band EQ with HPF → output.

use super::dynamics::{Compressor, ExpanderGate};
use super::eq::Eq7;
use crate::params::{Params, lin, log, toggle};
use crate::util::{Smoother, frames_in, sane_channels, sane_sr};
use crate::{Category, ParamInfo, Plugin, PluginInfo, Unit, db_to_gain};

static STRIP_PARAMS: [ParamInfo; 22] = [
    lin("input_gain", "Input", -24.0, 24.0, 0.0, Unit::Db),
    toggle("gate_on", "Gate", false),
    lin("gate_threshold", "Gate Thresh", -80.0, 0.0, -50.0, Unit::Db),
    lin("gate_range", "Gate Range", 0.0, 80.0, 40.0, Unit::Db),
    log("gate_release", "Gate Release", 1.0, 4000.0, 150.0, Unit::Ms),
    toggle("comp_on", "Comp", false),
    lin("comp_threshold", "Comp Thresh", -60.0, 0.0, -18.0, Unit::Db),
    log("comp_ratio", "Comp Ratio", 1.0, 100.0, 3.0, Unit::Ratio),
    log("comp_attack", "Comp Attack", 0.01, 300.0, 10.0, Unit::Ms),
    log("comp_release", "Comp Release", 5.0, 3000.0, 120.0, Unit::Ms),
    lin("comp_makeup", "Comp Makeup", 0.0, 40.0, 0.0, Unit::Db),
    toggle("eq_on", "EQ", true),
    toggle("hpf_on", "HPF", false),
    log("hpf_freq", "HPF Freq", 10.0, 2000.0, 80.0, Unit::Hz),
    lin("low_gain", "Low Gain", -24.0, 24.0, 0.0, Unit::Db),
    log("low_freq", "Low Freq", 20.0, 1000.0, 100.0, Unit::Hz),
    lin("mid_gain", "Mid Gain", -24.0, 24.0, 0.0, Unit::Db),
    log("mid_freq", "Mid Freq", 100.0, 8000.0, 1000.0, Unit::Hz),
    log("mid_q", "Mid Q", 0.1, 10.0, 1.0, Unit::None),
    lin("high_gain", "High Gain", -24.0, 24.0, 0.0, Unit::Db),
    log("high_freq", "High Freq", 1000.0, 20000.0, 8000.0, Unit::Hz),
    lin("output_gain", "Output", -24.0, 24.0, 0.0, Unit::Db),
];

pub static STRIP_INFO: PluginInfo = PluginInfo {
    id: "channel_strip",
    name: "Channel Strip",
    short_name: "Strip",
    category: Category::Dynamics,
    params: &STRIP_PARAMS,
    is_instrument: false,
    is_midi_processor: false,
    midi_out: false,
    audiosuite: true,
};

#[derive(Clone, Copy)]
enum Target {
    Gate,
    Comp,
    Eq,
}

/// Parameter forwarding: strip index → (module, module parameter id).
const ROUTES: &[(usize, Target, &str)] = &[
    (2, Target::Gate, "threshold"),
    (3, Target::Gate, "range"),
    (4, Target::Gate, "release"),
    (6, Target::Comp, "threshold"),
    (7, Target::Comp, "ratio"),
    (8, Target::Comp, "attack"),
    (9, Target::Comp, "release"),
    (10, Target::Comp, "makeup"),
    (12, Target::Eq, "hpf_on"),
    (13, Target::Eq, "hpf_freq"),
    (14, Target::Eq, "low_shelf_gain"),
    (15, Target::Eq, "low_shelf_freq"),
    (16, Target::Eq, "mid_gain"),
    (17, Target::Eq, "mid_freq"),
    (18, Target::Eq, "mid_q"),
    (19, Target::Eq, "high_shelf_gain"),
    (20, Target::Eq, "high_shelf_freq"),
];

/// Gate + compressor + EQ in one insert.
pub struct ChannelStrip {
    p: Params,
    gate: ExpanderGate,
    comp: Compressor,
    eq: Eq7,
    input: Smoother,
    output: Smoother,
    ch: usize,
}

impl Default for ChannelStrip {
    fn default() -> Self {
        Self::new()
    }
}

impl ChannelStrip {
    pub fn new() -> Self {
        let mut s = Self {
            p: Params::new(&STRIP_PARAMS),
            gate: ExpanderGate::new(),
            comp: Compressor::new(),
            eq: Eq7::new(),
            input: Smoother::new(1.0),
            output: Smoother::new(1.0),
            ch: 0,
        };
        s.comp.set_param("knee", 6.0);
        for &(idx, _, _) in ROUTES {
            s.forward(idx);
        }
        s.update();
        s
    }

    fn forward(&mut self, idx: usize) {
        let v = self.p.v(idx);
        for &(i, t, id) in ROUTES {
            if i == idx {
                match t {
                    Target::Gate => self.gate.set_param(id, v),
                    Target::Comp => self.comp.set_param(id, v),
                    Target::Eq => self.eq.set_param(id, v),
                };
            }
        }
    }

    fn update(&mut self) {
        self.input.set(db_to_gain(self.p.v(0)));
        self.output.set(db_to_gain(self.p.v(21)));
    }
}

impl Plugin for ChannelStrip {
    fn info(&self) -> &'static PluginInfo {
        &STRIP_INFO
    }

    fn set_param(&mut self, id: &str, value: f32) -> bool {
        let Some(idx) = self.p.set(id, value) else { return false };
        self.forward(idx);
        self.update();
        true
    }

    fn param(&self, id: &str) -> Option<f32> {
        self.p.get(id)
    }

    fn prepare(&mut self, sample_rate: f32, max_block: usize, channels: usize) {
        let sr = sane_sr(sample_rate);
        self.ch = sane_channels(channels);
        self.gate.prepare(sr, max_block, channels);
        self.comp.prepare(sr, max_block, channels);
        self.eq.prepare(sr, max_block, channels);
        self.input.set_time(20.0, sr);
        self.output.set_time(20.0, sr);
        self.reset();
    }

    fn reset(&mut self) {
        self.gate.reset();
        self.comp.reset();
        self.eq.reset();
        self.input.snap();
        self.output.snap();
    }

    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        let ch = self.ch.min(io.len());
        let frames = frames_in(io, frames, ch);
        for n in 0..frames {
            let g = self.input.next();
            for b in io.iter_mut().take(ch) {
                if let Some(x) = b.get_mut(n) {
                    *x *= g;
                }
            }
        }
        if self.p.on(1) {
            self.gate.process(io, frames);
        }
        if self.p.on(5) {
            self.comp.process(io, frames);
        }
        if self.p.on(11) {
            self.eq.process(io, frames);
        }
        for n in 0..frames {
            let g = self.output.next();
            for b in io.iter_mut().take(ch) {
                if let Some(x) = b.get_mut(n) {
                    *x *= g;
                }
            }
        }
    }

    fn gain_reduction_db(&self) -> f32 {
        let mut gr = 0.0;
        if self.p.on(1) {
            gr += self.gate.gain_reduction_db();
        }
        if self.p.on(5) {
            gr += self.comp.gain_reduction_db();
        }
        gr
    }
}
