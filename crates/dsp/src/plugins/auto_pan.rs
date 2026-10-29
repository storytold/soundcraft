//! Auto Pan: moves a signal between the left and right channels with an LFO or an envelope.

use std::f32::consts::{FRAC_PI_4, PI, SQRT_2, TAU};

use crate::params::{Params, choice, lin, log, param_plumbing, toggle};
use crate::util::{Smoother, frames_in, ms_to_samples, sane_channels, sane_sr, time_coef};
use crate::{Category, ParamInfo, Plugin, PluginInfo, Transport, Unit, db_to_gain, gain_to_db};

const SOURCE_CHOICES: &[&str] = &["LFO", "Envelope"];
const PATH_CHOICES: &[&str] = &["Left to Right", "Right to Left"];
const WAVEFORM_CHOICES: &[&str] = &["Sine", "Triangle", "Square", "Saw Up", "Saw Down", "Half Sine", "4 Step Triangle"];
const TRIGGER_CHOICES: &[&str] = &["Free", "Single", "Beat Clock", "Envelope", "Manual"];
const DURATION_CHOICES: &[&str] = &["16th Note", "8th Note", "1 Beat", "2 Beats", "1 Bar", "2 Bars", "4 Bars"];

const SOURCE_ENVELOPE: usize = 1;
const PATH_RIGHT_TO_LEFT: usize = 1;
const TRIGGER_FREE: usize = 0;
const TRIGGER_SINGLE: usize = 1;
const TRIGGER_BEAT_CLOCK: usize = 2;
const TRIGGER_ENVELOPE: usize = 3;
const TRIGGER_MANUAL: usize = 4;

static AUTOPAN_PARAMS: [ParamInfo; 14] = [
    lin("output", "Output", -24.0, 12.0, 0.0, Unit::Db),
    lin("width", "Width", 0.0, 100.0, 100.0, Unit::Percent),
    lin("manual", "Manual", -100.0, 100.0, 0.0, Unit::Percent),
    choice("source", "Source", SOURCE_CHOICES, 0),
    choice("path", "Path", PATH_CHOICES, 0),
    log("rate", "Rate (BPM)", 1.0, 960.0, 120.0, Unit::None),
    choice("waveform", "Waveform", WAVEFORM_CHOICES, 0),
    choice("trigger", "Trigger", TRIGGER_CHOICES, 0),
    toggle("retrigger", "Manual Trigger", false),
    toggle("link_tempo", "Link to Tempo", false),
    choice("duration", "Duration", DURATION_CHOICES, 4),
    lin("threshold", "Threshold", -60.0, -1.0, -30.0, Unit::Db),
    log("attack", "Attack", 0.1, 100.0, 5.0, Unit::Ms),
    log("release", "Release", 5.0, 2000.0, 150.0, Unit::Ms),
];

pub static AUTOPAN_INFO: PluginInfo = PluginInfo {
    id: "auto_pan",
    name: "Auto Pan",
    short_name: "Auto Pan",
    category: Category::Modulation,
    params: &AUTOPAN_PARAMS,
    is_instrument: false,
    audiosuite: true,
};

/// The LFO shape at `t` in 0..=1 of a cycle, from -1 (the start of the path) to 1 (its end).
fn wave_at(waveform: usize, t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    let tri = if t < 0.5 { 4.0 * t - 1.0 } else { 3.0 - 4.0 * t };
    match waveform {
        1 => tri,
        2 => {
            if t < 0.5 {
                -1.0
            } else {
                1.0
            }
        }
        3 => 2.0 * t - 1.0,
        4 => 1.0 - 2.0 * t,
        5 => 2.0 * (PI * t).sin() - 1.0,
        6 => {
            let step = ((tri + 1.0) * 2.0).floor().clamp(0.0, 3.0);
            step * 2.0 / 3.0 - 1.0
        }
        _ => -(TAU * t).cos(),
    }
}

/// Length of one Duration choice in quarter notes, for a bar of `bar` quarter notes.
fn duration_quarters(duration: usize, bar: f64) -> f64 {
    match duration {
        0 => 0.25,
        1 => 0.5,
        2 => 1.0,
        3 => 2.0,
        5 => 2.0 * bar,
        6 => 4.0 * bar,
        _ => bar,
    }
}

/// Number of bars in a bar-based Duration choice.
fn duration_bars(duration: usize) -> Option<i64> {
    match duration {
        4 => Some(1),
        5 => Some(2),
        6 => Some(4),
        _ => None,
    }
}

/// Writes one frame: a constant-power pan for stereo and wider, plain gain for mono. Channels past
/// the second only get the output gain.
fn write_frame(io: &mut [Vec<f32>], n: usize, ch: usize, left: f32, right: f32, position: f32, out: f32) {
    if ch >= 2 {
        // Constant power: sin² + cos² = 1 across the field, and both gains are 1 at the center.
        let theta = (position.clamp(-1.0, 1.0) + 1.0) * FRAC_PI_4;
        let (l, r) = (theta.cos().max(0.0), theta.sin().max(0.0));
        if let Some(x) = io.get_mut(0).and_then(|b| b.get_mut(n)) {
            *x = left * l * SQRT_2 * out;
        }
        if let Some(x) = io.get_mut(1).and_then(|b| b.get_mut(n)) {
            *x = right * r * SQRT_2 * out;
        }
        for c in 2..ch {
            if let Some(x) = io.get_mut(c).and_then(|b| b.get_mut(n)) {
                *x = if x.is_finite() { *x * out } else { 0.0 };
            }
        }
    } else if let Some(x) = io.get_mut(0).and_then(|b| b.get_mut(n)) {
        *x = left * out;
    }
}

/// Moves the balance of the first two channels with a constant-power law: as the pan position
/// moves right the left channel's gain falls and the right's rises, and each channel stays on its
/// own side. The pan position is `manual · (1 - width) + modulator · width`, where the modulator is
/// the LFO or the envelope follower in -1..=1. At the center both gains are 1, so a stereo signal
/// passes unchanged; a signal that is the same on both channels pans across the field at a
/// constant level.
pub struct AutoPan {
    p: Params,
    sr: f32,
    ch: usize,
    transport: Transport,
    /// Samples processed since the last `set_transport`, for hosts that call it less than once a block.
    since_transport: u64,
    was_playing: bool,
    /// LFO phase in cycles, in `f64`: at the slowest rates an `f32` step is below half an ULP.
    phase: f64,
    /// A Single, Envelope or Manual trigger has finished its cycle and holds the last position.
    holding: bool,
    env: f32,
    armed: bool,
    /// Samples the envelope has spent below the threshold since the last Envelope trigger. The
    /// trigger re-arms once this reaches the Release time, so a longer Release spaces triggers out.
    below: u64,
    last_retrigger: bool,
    last_trigger: usize,
    output: Smoother,
    width: Smoother,
    manual: Smoother,
    /// Short declicking smoother on the pan position.
    position: Smoother,
    /// The next sample starts the position at its target instead of gliding to it.
    snap_position: bool,
}

impl Default for AutoPan {
    fn default() -> Self {
        Self::new()
    }
}

impl AutoPan {
    pub fn new() -> Self {
        let mut a = Self {
            p: Params::new(&AUTOPAN_PARAMS),
            sr: 48_000.0,
            ch: 0,
            transport: Transport::default(),
            since_transport: 0,
            was_playing: false,
            phase: 0.0,
            holding: false,
            env: 0.0,
            armed: true,
            below: 0,
            last_retrigger: false,
            last_trigger: TRIGGER_FREE,
            output: Smoother::new(1.0),
            width: Smoother::new(1.0),
            manual: Smoother::new(0.0),
            position: Smoother::new(0.0),
            snap_position: true,
        };
        a.update();
        a.restart();
        a
    }

    fn update(&mut self) {
        self.output.set(db_to_gain(self.p.v(0)));
        self.width.set(self.p.v(1) / 100.0);
        self.manual.set(self.p.v(2) / 100.0);
        let trigger = self.p.choice(7);
        if trigger != self.last_trigger {
            self.last_trigger = trigger;
            self.restart();
        }
        let retrigger = self.p.on(8);
        if retrigger != self.last_retrigger {
            self.last_retrigger = retrigger;
            if trigger == TRIGGER_MANUAL {
                self.phase = 0.0;
                self.holding = false;
            }
        }
    }

    /// Puts the LFO back at the start of the path. Single runs one cycle from there; Envelope and
    /// Manual wait at the start for their first trigger.
    fn restart(&mut self) {
        self.phase = 0.0;
        self.holding = matches!(self.p.choice(7), TRIGGER_ENVELOPE | TRIGGER_MANUAL);
        self.armed = true;
        self.below = 0;
    }

    /// LFO frequency in cycles per second.
    fn lfo_hz(&self) -> f64 {
        let tempo = if self.p.on(9) && self.transport.tempo.is_finite() && self.transport.tempo > 0.0 {
            self.transport.tempo
        } else {
            f64::from(self.p.v(5))
        };
        tempo / 60.0 / duration_quarters(self.p.choice(10), self.bar_quarters())
    }

    fn bar_quarters(&self) -> f64 {
        let (num, den) = (self.transport.numerator, self.transport.denominator);
        if num == 0 || den == 0 { 4.0 } else { (f64::from(num) * 4.0 / f64::from(den)).clamp(0.25, 64.0) }
    }

    /// The LFO phase at `n` samples into the block when Beat Clock follows a playing transport.
    /// Beat Clock restarts the LFO at the start of every Duration and runs it at Rate in between,
    /// so a Rate slower than the session tempo plays only part of each cycle before the restart.
    fn beat_clock_phase(&self, n: usize, hz: f64) -> Option<f64> {
        let t = &self.transport;
        if !(t.playing && t.tempo.is_finite() && t.tempo > 0.0 && t.quarters.is_finite()) {
            return None;
        }
        let duration = self.p.choice(10);
        let bar = self.bar_quarters();
        let dur = duration_quarters(duration, bar);
        let elapsed = self.since_transport.saturating_add(n as u64) as f64;
        let quarters = t.quarters + elapsed * t.tempo / 60.0 / f64::from(self.sr);
        // Bar-based cycles start on the bar line, so a change of time signature does not shift them;
        // note and beat cycles follow the song position in quarter notes.
        let into = match duration_bars(duration) {
            Some(bars) => {
                let in_bar = if t.bar_start.is_finite() { quarters - t.bar_start } else { quarters };
                ((t.bar.rem_euclid(bars) as f64 + in_bar / bar) / bars as f64).rem_euclid(1.0)
            }
            None => (quarters / dur).rem_euclid(1.0),
        };
        // Cycles of the LFO per Duration: 1 when its rate matches the session tempo.
        let ratio = hz * dur * 60.0 / t.tempo;
        Some((into * ratio).rem_euclid(1.0))
    }

    /// Advances the envelope follower by one sample. `level` is the louder channel's magnitude.
    fn follow_envelope(&mut self, level: f32, attack: f32, release: f32) {
        let k = if level > self.env { attack } else { release };
        self.env = level + (self.env - level) * k;
        if self.env < 1.0e-12 {
            self.env = 0.0;
        }
    }

    /// Arms the Envelope trigger on a rising threshold crossing and re-arms it after a Release of silence.
    fn envelope_trigger(&mut self, threshold: f32, release_samples: u64) {
        if self.armed && self.env >= threshold {
            self.armed = false;
            self.phase = 0.0;
            self.holding = false;
            self.below = 0;
        } else if !self.armed {
            if self.env < threshold {
                self.below = self.below.saturating_add(1);
                self.armed = self.below >= release_samples;
            } else {
                self.below = 0;
            }
        }
    }

    /// The LFO phase for sample `n` of the block. Advances the phase unless a held or Beat Clock position sets it.
    fn lfo_phase(&mut self, n: usize, trigger: usize, hz: f64, inc: f64) -> f64 {
        match (trigger == TRIGGER_BEAT_CLOCK).then(|| self.beat_clock_phase(n, hz)).flatten() {
            Some(phase) => phase,
            None if self.holding => self.phase,
            None => {
                let current = self.phase;
                self.phase += inc;
                if self.phase >= 1.0 {
                    if matches!(trigger, TRIGGER_FREE | TRIGGER_BEAT_CLOCK) {
                        self.phase -= 1.0;
                    } else {
                        self.phase = 1.0;
                        self.holding = true;
                    }
                }
                current
            }
        }
    }

    /// The modulator in -1..=1: the envelope level above the threshold, or the LFO shape.
    fn modulator(&self, envelope_source: bool, threshold_db: f32, waveform: usize, lfo_phase: f64) -> f32 {
        if envelope_source {
            let above = (gain_to_db(self.env) - threshold_db) / -threshold_db;
            2.0 * above.clamp(0.0, 1.0) - 1.0
        } else {
            wave_at(waveform, lfo_phase as f32)
        }
    }
}

impl Plugin for AutoPan {
    param_plumbing!(AUTOPAN_INFO);

    fn prepare(&mut self, sample_rate: f32, _max_block: usize, channels: usize) {
        self.sr = sane_sr(sample_rate);
        self.ch = sane_channels(channels);
        for s in [&mut self.output, &mut self.width, &mut self.manual] {
            s.set_time(20.0, self.sr);
        }
        self.position.set_time(2.0, self.sr);
        self.update();
        self.reset();
    }

    fn reset(&mut self) {
        self.restart();
        self.env = 0.0;
        for s in [&mut self.output, &mut self.width, &mut self.manual] {
            s.snap();
        }
        self.snap_position = true;
    }

    fn set_transport(&mut self, transport: &Transport) {
        self.transport = *transport;
        self.since_transport = 0;
        if transport.playing && !self.was_playing && self.p.choice(7) == TRIGGER_SINGLE {
            self.restart();
        }
        self.was_playing = transport.playing;
    }

    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        let ch = self.ch.min(io.len());
        let frames = frames_in(io, frames, ch);
        let hz = self.lfo_hz();
        let inc = if hz.is_finite() { (hz / f64::from(self.sr)).clamp(0.0, 0.5) } else { 0.0 };
        let waveform = self.p.choice(6);
        let trigger = self.p.choice(7);
        let envelope_source = self.p.choice(3) == SOURCE_ENVELOPE;
        let sign = if self.p.choice(4) == PATH_RIGHT_TO_LEFT { -1.0 } else { 1.0 };
        let threshold_db = self.p.v(11);
        let threshold = db_to_gain(threshold_db);
        let attack = time_coef(self.p.v(12), self.sr);
        let release = time_coef(self.p.v(13), self.sr);
        let release_samples = ms_to_samples(self.p.v(13), self.sr) as u64;
        for n in 0..frames {
            let sample = |io: &[Vec<f32>], c: usize| io.get(c).and_then(|b| b.get(n)).copied().filter(|v| v.is_finite()).unwrap_or(0.0);
            let left = sample(io, 0);
            let right = if ch >= 2 { sample(io, 1) } else { left };
            // The louder channel drives the follower, so anti-phase material still has a level.
            self.follow_envelope(left.abs().max(right.abs()), attack, release);
            if trigger == TRIGGER_ENVELOPE {
                self.envelope_trigger(threshold, release_samples);
            }
            let lfo_phase = self.lfo_phase(n, trigger, hz, inc);
            let modulator = self.modulator(envelope_source, threshold_db, waveform, lfo_phase);
            let width = self.width.next();
            let manual = self.manual.next();
            self.position.set(manual * (1.0 - width) + sign * modulator * width);
            if self.snap_position {
                self.snap_position = false;
                self.position.snap();
            }
            let position = self.position.next();
            let out = self.output.next();
            write_frame(io, n, ch, left, right, position, out);
        }
        self.since_transport = self.since_transport.saturating_add(frames as u64);
    }
}
