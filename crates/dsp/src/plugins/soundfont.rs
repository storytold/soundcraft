//! SoundFont player: plays a preset of a SoundFont 2 bank, 64 voices.
//!
//! The host hands the parsed bank over with [`Plugin::set_soundfont`] (parsing a bank of hundreds
//! of megabytes is never done here); without one the player is silent. Each note starts a voice
//! for every region of the current preset whose key and velocity ranges hold it (a stereo sample
//! is two regions panned apart, a layered sound several). A voice reads its sample with linear
//! interpolation at the pitch the region and the key give, loops as the region says, and follows
//! the region's delay–attack–hold–decay–sustain–release volume envelope. Sustain pedal (CC 64),
//! pitch bend (±2 semitones), All Notes Off and exclusive classes (an open hi-hat cut by a closed
//! one) are honoured. A Program Change picks the bank's preset for that program.

use std::sync::Arc;

use super::synth::{Event, EventKind, EventQueue, MAX_EVENTS};
use crate::params::{Params, lin, param_plumbing};
use crate::soundfont::{LoopMode, Region, SoundFont};
use crate::util::{Smoother, frames_in, sane_channels, sane_sr};
use crate::{Category, ParamInfo, Plugin, PluginInfo, Unit, db_to_gain};

const PRESET: usize = 0;
const TRANSPOSE: usize = 1;
const VEL_SENS: usize = 2;
const LEVEL: usize = 3;

static SF_PARAMS: [ParamInfo; 4] = [
    lin("preset", "Preset", 0.0, 1023.0, 0.0, Unit::None),
    lin("transpose", "Transpose", -24.0, 24.0, 0.0, Unit::Semitones),
    lin("velocity", "Velocity Sens", 0.0, 100.0, 100.0, Unit::Percent),
    lin("level", "Level", -48.0, 12.0, 0.0, Unit::Db),
];

pub static SOUNDFONT_INFO: PluginInfo = PluginInfo {
    id: "soundfont",
    name: "SoundFont Player",
    short_name: "SF2",
    category: Category::Instrument,
    params: &SF_PARAMS,
    is_instrument: true,
    audiosuite: false,
};

const VOICES: usize = 64;
/// Pitch bend range, semitones each way.
const BEND_RANGE: f32 = 2.0;
/// Below this an envelope is silent (-100 dB).
const SILENT: f32 = 1.0e-5;
/// How fast a voice cut by its exclusive class or stolen fades out.
const CUT_SECONDS: f32 = 0.005;
/// Fastest playback step in sample points per output frame (keeps hostile pitch bounded).
const MAX_STEP: f64 = 256.0;

#[derive(Debug, Clone, Copy, PartialEq)]
enum Stage {
    Idle,
    Delay,
    Attack,
    Hold,
    Decay,
    Sustain,
    Release,
}

#[derive(Debug, Clone, Copy)]
struct Voice {
    stage: Stage,
    /// Frames left in a timed stage (delay, attack, hold).
    left: u32,
    level: f32,
    /// Per-frame change: added during attack, multiplied during decay and release.
    rate: f32,
    note: u8,
    /// Key still down.
    held: bool,
    /// Key up while the sustain pedal is down: released when the pedal lifts.
    sustained: bool,
    region: Region,
    /// Read position in sample points, and the step for one output frame (before bend).
    pos: f64,
    step: f64,
    gain_l: f32,
    gain_r: f32,
    /// Order the voice started in, to steal the oldest.
    age: u64,
}

impl Voice {
    fn idle(region: Region) -> Self {
        Voice {
            stage: Stage::Idle,
            left: 0,
            level: 0.0,
            rate: 0.0,
            note: 0,
            held: false,
            sustained: false,
            region,
            pos: 0.0,
            step: 1.0,
            gain_l: 0.0,
            gain_r: 0.0,
            age: 0,
        }
    }

    fn active(&self) -> bool {
        self.stage != Stage::Idle
    }

    fn looping(&self) -> bool {
        match self.region.loop_mode {
            LoopMode::Continuous => true,
            LoopMode::UntilRelease => self.stage != Stage::Release,
            LoopMode::None => false,
        }
    }
}

/// Seconds to whole frames, at least one.
fn frames(secs: f32, sr: f32) -> u32 {
    let n = secs * sr;
    if n.is_finite() { n.clamp(1.0, 1.0e9) as u32 } else { 1 }
}

/// Per-frame multiplier that falls by 100 dB in `secs` (the SoundFont decay and release slope).
fn fall(secs: f32, sr: f32) -> f32 {
    (SILENT.ln() / frames(secs, sr) as f32).exp()
}

/// Plays a SoundFont preset.
pub struct SoundFontPlayer {
    p: Params,
    sr: f32,
    ch: usize,
    bank: Option<Arc<SoundFont>>,
    /// Index into the bank's presets (clamped to it when used).
    preset: usize,
    transpose: f32,
    sens: f32,
    level: Smoother,
    sustain: bool,
    /// Pitch bend as a frequency ratio.
    bend: f32,
    voices: Vec<Voice>,
    events: EventQueue,
    started: u64,
}

impl Default for SoundFontPlayer {
    fn default() -> Self {
        Self::new()
    }
}

/// A placeholder region for idle voices (never played).
const NO_REGION: Region = Region {
    keys: (0, 0),
    vels: (0, 0),
    start: 0,
    end: 0,
    loop_start: 0,
    loop_end: 0,
    loop_mode: LoopMode::None,
    sample_rate: 44_100,
    root_key: 60,
    tune_cents: 0.0,
    key_cents: 100.0,
    gain_db: 0.0,
    pan: 0.0,
    env: crate::soundfont::Envelope { delay: 0.0, attack: 0.0, hold: 0.0, decay: 0.0, sustain: 1.0, release: 0.0 },
    exclusive_class: 0,
};

impl SoundFontPlayer {
    pub fn new() -> Self {
        let mut s = Self {
            p: Params::new(&SF_PARAMS),
            sr: 48_000.0,
            ch: 0,
            bank: None,
            preset: 0,
            transpose: 0.0,
            sens: 1.0,
            level: Smoother::new(1.0),
            sustain: false,
            bend: 1.0,
            voices: vec![Voice::idle(NO_REGION); VOICES],
            events: EventQueue::new(),
            started: 0,
        };
        s.update();
        s
    }

    fn update(&mut self) {
        let v = self.p.v(PRESET);
        self.preset = if v.is_finite() { v.round().clamp(0.0, 1023.0) as usize } else { 0 };
        self.transpose = self.p.v(TRANSPOSE).clamp(-24.0, 24.0);
        self.sens = (self.p.v(VEL_SENS) / 100.0).clamp(0.0, 1.0);
        self.level.set(db_to_gain(self.p.v(LEVEL)));
    }

    /// The preset index in use, within the bank.
    fn current(&self) -> Option<usize> {
        let n = self.bank.as_ref()?.presets.len();
        (n > 0).then(|| self.preset.min(n - 1))
    }

    fn release(v: &mut Voice, secs: f32, sr: f32) {
        if v.active() && v.stage != Stage::Release {
            v.stage = Stage::Release;
            v.rate = fall(secs, sr);
            if v.level <= 0.0 {
                v.stage = Stage::Idle;
            }
        }
    }

    fn start_note(&mut self, note: u8, vel: u8) {
        if vel == 0 {
            self.stop_note(note);
            return;
        }
        let (Some(bank), Some(idx)) = (self.bank.clone(), self.current()) else { return };
        let Some(preset) = bank.presets.get(idx) else { return };
        let (note, vel) = (note.min(127), vel.min(127));
        let sr = self.sr;
        // Retriggering a held key releases the earlier voices instead of stacking them.
        self.stop_note(note);
        let vel_gain = (1.0 - self.sens) + self.sens * (f32::from(vel) / 127.0).powi(2);
        for r in preset.regions.iter().filter(|r| (r.keys.0..=r.keys.1).contains(&note) && (r.vels.0..=r.vels.1).contains(&vel)) {
            if r.exclusive_class != 0 {
                let class = r.exclusive_class;
                for v in self.voices.iter_mut().filter(|v| v.active() && v.region.exclusive_class == class) {
                    Self::release(v, CUT_SECONDS, sr);
                }
            }
            let cents = (f32::from(note) + self.transpose - f32::from(r.root_key)) * r.key_cents + r.tune_cents;
            let step = f64::from(r.sample_rate) / f64::from(sr) * 2f64.powf(f64::from(cents) / 1200.0);
            if !step.is_finite() || step <= 0.0 {
                continue;
            }
            let angle = (r.pan.clamp(-1.0, 1.0) + 1.0) * std::f32::consts::FRAC_PI_4;
            let gain = db_to_gain(r.gain_db) * vel_gain;
            let slot = self.free_voice();
            self.started = self.started.wrapping_add(1);
            let env = r.env;
            let (stage, left) = if env.delay * sr >= 1.0 { (Stage::Delay, frames(env.delay, sr)) } else { (Stage::Attack, frames(env.attack, sr)) };
            if let Some(v) = self.voices.get_mut(slot) {
                *v = Voice {
                    stage,
                    left,
                    level: 0.0,
                    rate: 1.0 / frames(env.attack, sr) as f32,
                    note,
                    held: true,
                    sustained: false,
                    region: *r,
                    pos: r.start as f64,
                    step: step.min(MAX_STEP),
                    gain_l: gain * angle.cos(),
                    gain_r: gain * angle.sin(),
                    age: self.started,
                };
            }
        }
    }

    /// An idle voice, else the quietest releasing one, else the oldest.
    fn free_voice(&self) -> usize {
        if let Some(i) = self.voices.iter().position(|v| !v.active()) {
            return i;
        }
        if let Some((i, _)) = self
            .voices
            .iter()
            .enumerate()
            .filter(|(_, v)| v.stage == Stage::Release)
            .min_by(|(_, a), (_, b)| a.level.partial_cmp(&b.level).unwrap_or(std::cmp::Ordering::Equal))
        {
            return i;
        }
        self.voices.iter().enumerate().min_by_key(|(_, v)| v.age).map_or(0, |(i, _)| i)
    }

    fn stop_note(&mut self, note: u8) {
        let (sr, pedal) = (self.sr, self.sustain);
        for v in self.voices.iter_mut().filter(|v| v.active() && v.held && v.note == note) {
            v.held = false;
            if pedal {
                v.sustained = true;
            } else {
                let rel = v.region.env.release;
                Self::release(v, rel, sr);
            }
        }
    }

    fn apply(&mut self, kind: EventKind) {
        let sr = self.sr;
        match kind {
            EventKind::On(n, v) => self.start_note(n, v),
            EventKind::Off(n) => self.stop_note(n),
            EventKind::AllOff => {
                self.sustain = false;
                for v in &mut self.voices {
                    v.held = false;
                    v.sustained = false;
                    let rel = v.region.env.release;
                    Self::release(v, rel, sr);
                }
            }
            EventKind::Sustain(on) => {
                self.sustain = on;
                if !on {
                    for v in self.voices.iter_mut().filter(|v| v.sustained) {
                        v.sustained = false;
                        let rel = v.region.env.release;
                        Self::release(v, rel, sr);
                    }
                }
            }
            EventKind::Bend(ratio) => self.bend = if ratio.is_finite() && ratio > 0.0 { ratio } else { 1.0 },
            EventKind::ResetControllers => {
                self.bend = 1.0;
                self.apply(EventKind::Sustain(false));
            }
        }
    }

    /// Adds every voice's output for frame `n` into `io` and advances the voices.
    #[inline]
    fn render_frame(&mut self, io: &mut [Vec<f32>], ch: usize, n: usize, level: f32) {
        let Some(bank) = self.bank.as_deref() else { return };
        let data = &bank.samples;
        let sr = self.sr;
        let bend = f64::from(self.bend);
        let (mut l, mut r) = (0.0f32, 0.0f32);
        for v in &mut self.voices {
            if !v.active() {
                continue;
            }
            let a = envelope(v, sr);
            if a > 0.0 {
                let i = v.pos as usize;
                let t = (v.pos - i as f64) as f32;
                let looping = v.looping();
                let next = if looping && i + 1 >= v.region.loop_end { v.region.loop_start } else { i + 1 };
                let at = |k: usize| f32::from(data.get(k).copied().unwrap_or(0)) / 32_768.0;
                let x = at(i) + (at(next) - at(i)) * t;
                l += x * a * v.gain_l;
                r += x * a * v.gain_r;
            }
            // Advance, looping or ending.
            v.pos += v.step * bend;
            if v.looping() {
                let (ls, le) = (v.region.loop_start as f64, v.region.loop_end as f64);
                if v.pos >= le {
                    let width = (le - ls).max(1.0);
                    v.pos = ls + (v.pos - ls) % width;
                }
            } else if v.pos >= v.region.end.saturating_sub(1) as f64 || !v.pos.is_finite() {
                v.stage = Stage::Idle;
            }
        }
        let (l, r) = (l * level, r * level);
        match ch {
            0 => {}
            1 => {
                if let Some(x) = io.first_mut().and_then(|b| b.get_mut(n)) {
                    *x += 0.5 * (l + r);
                }
            }
            _ => {
                for (c, b) in io.iter_mut().take(ch).enumerate() {
                    if let Some(x) = b.get_mut(n) {
                        // Channels past the first two get nothing (the player is stereo).
                        *x += match c {
                            0 => l,
                            1 => r,
                            _ => 0.0,
                        };
                    }
                }
            }
        }
    }
}

/// The voice's envelope level for this frame, advanced one frame.
#[inline]
fn envelope(v: &mut Voice, sr: f32) -> f32 {
    let env = v.region.env;
    match v.stage {
        Stage::Idle => v.level = 0.0,
        Stage::Delay => {
            v.left = v.left.saturating_sub(1);
            if v.left == 0 {
                v.stage = Stage::Attack;
                v.left = frames(env.attack, sr);
                v.rate = 1.0 / v.left as f32;
            }
        }
        Stage::Attack => {
            v.level = (v.level + v.rate).min(1.0);
            v.left = v.left.saturating_sub(1);
            if v.left == 0 || v.level >= 1.0 {
                v.level = 1.0;
                v.stage = Stage::Hold;
                v.left = frames(env.hold, sr);
            }
        }
        Stage::Hold => {
            v.left = v.left.saturating_sub(1);
            if v.left == 0 {
                v.stage = Stage::Decay;
                v.rate = fall(env.decay, sr);
            }
        }
        Stage::Decay => {
            v.level *= v.rate;
            if v.level <= env.sustain {
                v.level = env.sustain;
                v.stage = if env.sustain <= SILENT { Stage::Idle } else { Stage::Sustain };
            }
        }
        Stage::Sustain => v.level = env.sustain,
        Stage::Release => {
            v.level *= v.rate;
            if v.level < SILENT {
                v.level = 0.0;
                v.stage = Stage::Idle;
            }
        }
    }
    v.level
}

impl Plugin for SoundFontPlayer {
    param_plumbing!(SOUNDFONT_INFO);

    fn prepare(&mut self, sample_rate: f32, _max_block: usize, channels: usize) {
        self.sr = sane_sr(sample_rate);
        self.ch = sane_channels(channels);
        self.level.set_time(20.0, self.sr);
        self.update();
        self.reset();
    }

    fn reset(&mut self) {
        for v in &mut self.voices {
            v.stage = Stage::Idle;
        }
        self.events.clear();
        self.sustain = false;
        self.bend = 1.0;
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
        // The longest release of the preset's regions (the envelope's own time to silence).
        let secs = self
            .current()
            .and_then(|i| self.bank.as_ref()?.presets.get(i))
            .map_or(0.0, |p| p.regions.iter().map(|r| r.env.release).fold(0.0f32, f32::max))
            .min(30.0);
        (secs * self.sr) as usize
    }

    fn note_on(&mut self, offset: usize, note: u8, velocity: u8) {
        self.events.push(offset, EventKind::On(note, velocity));
    }

    fn note_off(&mut self, offset: usize, note: u8) {
        self.events.push(offset, EventKind::Off(note));
    }

    fn midi(&mut self, offset: usize, msg: [u8; 3]) {
        let [status, d1, d2] = msg;
        let kind = match (status & 0xF0, d1) {
            (0xB0, 64) => EventKind::Sustain(d2 >= 64),
            (0xB0, 120 | 123) => EventKind::AllOff,
            (0xB0, 121) => EventKind::ResetControllers,
            (0xE0, _) => {
                let v = (i32::from(d2 & 0x7F) << 7 | i32::from(d1 & 0x7F)) - 8192;
                EventKind::Bend(2f32.powf(v as f32 / 8192.0 * BEND_RANGE / 12.0))
            }
            (0xC0, program) => {
                // New notes use the program's preset; sounding ones finish with theirs.
                if let Some(i) = self.bank.as_ref().and_then(|b| b.preset_for_program(program & 0x7F)) {
                    self.preset = i;
                }
                return;
            }
            _ => return,
        };
        self.events.push(offset, kind);
    }

    fn all_notes_off(&mut self) {
        self.events.push(0, EventKind::AllOff);
    }

    fn set_soundfont(&mut self, bank: Option<Arc<SoundFont>>) -> bool {
        for v in &mut self.voices {
            v.stage = Stage::Idle;
        }
        self.bank = bank;
        true
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::soundfont::Envelope;

    const SR: f32 = 48_000.0;

    /// A bank whose samples are generated here: 4 800 points of a 480 Hz sine at 48 kHz, looped
    /// over whole periods, root key 69. Preset 0 (program 0) plays it everywhere; preset 1
    /// (program 7) only from key 72, with a slow attack.
    fn bank() -> Arc<SoundFont> {
        let samples: Vec<i16> = (0..4_800).map(|i| ((i as f32 * std::f32::consts::TAU * 480.0 / SR).sin() * 16_000.0) as i16).collect();
        let env = Envelope { delay: 0.0, attack: 0.001, hold: 0.0, decay: 0.0, sustain: 1.0, release: 0.01 };
        let region = Region {
            keys: (0, 127),
            vels: (1, 127),
            start: 0,
            end: 4_800,
            loop_start: 1_000,
            loop_end: 1_100,
            loop_mode: LoopMode::Continuous,
            sample_rate: 48_000,
            root_key: 69,
            tune_cents: 0.0,
            key_cents: 100.0,
            gain_db: 0.0,
            pan: 0.0,
            env,
            exclusive_class: 0,
        };
        let slow = Region { keys: (72, 127), env: Envelope { attack: 1.0, ..env }, ..region };
        Arc::new(SoundFont {
            name: "test".into(),
            samples,
            presets: vec![
                crate::soundfont::Preset { name: "Sine".into(), bank: 0, program: 0, regions: vec![region] },
                crate::soundfont::Preset { name: "High".into(), bank: 0, program: 7, regions: vec![slow] },
            ],
        })
    }

    fn player() -> SoundFontPlayer {
        let mut p = SoundFontPlayer::new();
        p.prepare(SR, 512, 2);
        assert!(p.set_soundfont(Some(bank())));
        p
    }

    fn run(p: &mut SoundFontPlayer, len: usize) -> Vec<f32> {
        let mut out = Vec::new();
        let mut buf = vec![vec![0.0f32; 512]; 2];
        let mut done = 0;
        while done < len {
            let n = 512.min(len - done);
            p.process(&mut buf, n);
            out.extend_from_slice(&buf[0][..n]);
            done += n;
        }
        out
    }

    fn peak(x: &[f32]) -> f32 {
        x.iter().fold(0.0f32, |m, v| m.max(v.abs()))
    }

    fn freq(x: &[f32]) -> f32 {
        let ups: Vec<usize> = x.windows(2).enumerate().filter(|(_, w)| w[0] < 0.0 && w[1] >= 0.0).map(|(i, _)| i).collect();
        let (Some(a), Some(b)) = (ups.first(), ups.last()) else { return 0.0 };
        (ups.len() - 1) as f32 * SR / (b - a) as f32
    }

    #[test]
    fn silent_without_a_bank() {
        let mut p = SoundFontPlayer::new();
        p.prepare(SR, 512, 2);
        p.note_on(0, 69, 100);
        assert_eq!(peak(&run(&mut p, 4_800)), 0.0);
    }

    #[test]
    fn the_root_key_plays_at_pitch_an_octave_up_doubles_and_the_loop_sustains() {
        let mut p = player();
        p.note_on(0, 69, 127);
        let out = run(&mut p, 48_000);
        // Long past the 4 800-point sample: the loop keeps it sounding.
        assert!(peak(&out[40_000..]) > 0.2, "{}", peak(&out[40_000..]));
        assert!((freq(&out[..4_000]) - 480.0).abs() < 5.0, "{}", freq(&out[..4_000]));
        let mut p = player();
        p.note_on(0, 81, 127);
        let out = run(&mut p, 2_000);
        assert!((freq(&out) - 960.0).abs() < 10.0, "{}", freq(&out));
    }

    #[test]
    fn note_off_releases_and_the_pedal_holds() {
        let mut p = player();
        p.note_on(0, 69, 100);
        run(&mut p, 1_000);
        p.note_off(0, 69);
        let out = run(&mut p, 4_800);
        assert!(peak(&out[3_000..]) < 1e-3, "released");
        // Pedal down: the note rings past its note-off until the pedal lifts.
        p.midi(0, [0xB0, 64, 127]);
        p.note_on(0, 69, 100);
        run(&mut p, 1_000);
        p.note_off(0, 69);
        assert!(peak(&run(&mut p, 4_800)[3_000..]) > 0.1, "sustained by the pedal");
        p.midi(0, [0xB0, 64, 0]);
        assert!(peak(&run(&mut p, 4_800)[3_000..]) < 1e-3, "released with the pedal");
    }

    #[test]
    fn a_program_change_picks_the_preset_and_its_regions() {
        let mut p = player();
        p.midi(0, [0xC0, 7, 0]);
        // Preset "High" has no region below key 72.
        p.note_on(0, 60, 100);
        assert_eq!(peak(&run(&mut p, 2_000)), 0.0);
        p.note_on(0, 72, 100);
        let out = run(&mut p, 24_000);
        // Its one-second attack: still rising half a second in.
        assert!(peak(&out[..2_400]) < peak(&out[20_000..]) * 0.5);
        // The preset parameter picks it too; an unknown program leaves the preset alone.
        let mut q = player();
        q.set_param("preset", 1.0);
        q.midi(0, [0xC0, 99, 0]);
        q.note_on(0, 60, 100);
        assert_eq!(peak(&run(&mut q, 2_000)), 0.0);
    }

    #[test]
    fn hostile_input_stays_finite() {
        let mut p = player();
        p.set_param("transpose", f32::NAN);
        p.set_param("preset", 1.0e9);
        for n in [0u8, 127, 200] {
            p.note_on(0, n, 255);
        }
        p.midi(0, [0xE0, 0x7F, 0x7F]);
        let out = run(&mut p, 9_600);
        assert!(out.iter().all(|x| x.is_finite()));
    }
}
