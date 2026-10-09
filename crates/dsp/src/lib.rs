#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
//! SoundCraft DSP.
//!
//! Realtime plugins (EQ, dynamics, reverb, delay, modulation, harmonic, pitch, utilities and two
//! instruments), metering (peak, RMS, ITU-R BS.1770 loudness), spectrum analysis and offline
//! AudioSuite-style whole-buffer processing.
//!
//! Every algorithm here is an original implementation of textbook DSP (RBJ cookbook biquads,
//! feed-forward log-domain dynamics, feedback-delay-network reverbs, WSOLA, windowed-sinc
//! resampling). Plugins never panic on any parameter value: NaN falls back to the default and
//! infinities clamp to the range. `Plugin::process` never allocates; buffers are sized in
//! `Plugin::prepare`.
//!
//! Audio is planar `f32`: `io[channel][frame]`.

pub mod biquad;
pub mod meter;
pub mod offline;
mod osc;
pub mod pan;
mod params;
pub mod pitch_detect;
pub mod plugins;
pub mod spectrum;
mod util;

pub use offline::FadeShape;
pub use osc::Waveform;
pub use plugins::eq::{eq1_response, eq7_response};

/// Plugin category, used to group the insert menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum Category {
    Eq,
    Dynamics,
    PitchShift,
    Reverb,
    Delay,
    Modulation,
    Harmonic,
    Dither,
    Instrument,
    Other,
}

/// Display unit of a parameter.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize)]
pub enum Unit {
    None,
    Db,
    Hz,
    Ms,
    Percent,
    Ratio,
    Semitones,
    Cents,
    Seconds,
    Toggle,
    Choice,
}

/// How a control maps its 0..1 travel onto the parameter range.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize)]
pub enum Taper {
    Linear,
    Log,
}

/// Static description of one plugin parameter.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct ParamInfo {
    pub id: &'static str,
    pub name: &'static str,
    pub min: f32,
    pub max: f32,
    pub default: f32,
    pub unit: Unit,
    pub taper: Taper,
    /// Labels for `Unit::Choice` parameters (value is the index).
    pub choices: &'static [&'static str],
}

impl ParamInfo {
    /// Clamps any value (including NaN/inf) into this parameter's legal range.
    /// NaN becomes the default; toggles and choices round to whole numbers.
    pub fn clamp(&self, value: f32) -> f32 {
        let v = if value.is_nan() { self.default } else { value };
        let v = v.clamp(self.min, self.max);
        match self.unit {
            Unit::Toggle | Unit::Choice => v.round().clamp(self.min, self.max),
            _ => v,
        }
    }

    /// Maps a value to 0..1 control travel honoring the taper.
    pub fn to_normalized(&self, value: f32) -> f32 {
        let v = self.clamp(value);
        let span = self.max - self.min;
        if span <= 0.0 {
            return 0.0;
        }
        match self.taper {
            Taper::Log if self.min > 0.0 => ((v / self.min).ln() / (self.max / self.min).ln()).clamp(0.0, 1.0),
            _ => ((v - self.min) / span).clamp(0.0, 1.0),
        }
    }

    /// Maps 0..1 control travel back to a value honoring the taper.
    pub fn from_normalized(&self, t: f32) -> f32 {
        let t = if t.is_nan() { 0.0 } else { t.clamp(0.0, 1.0) };
        let v = match self.taper {
            Taper::Log if self.min > 0.0 => self.min * (self.max / self.min).powf(t),
            _ => self.min + (self.max - self.min) * t,
        };
        self.clamp(v)
    }

    /// Human-readable label for a value (choice label, On/Off, or number with unit).
    pub fn format(&self, value: f32) -> String {
        let v = self.clamp(value);
        match self.unit {
            Unit::Choice => self.choices.get(v as usize).map(|s| (*s).to_string()).unwrap_or_else(|| format!("{v}")),
            Unit::Toggle => if v >= 0.5 { "On" } else { "Off" }.to_string(),
            Unit::Db => {
                if v <= -95.9 {
                    "-inf dB".to_string()
                } else {
                    format!("{v:.1} dB")
                }
            }
            Unit::Hz => {
                if v >= 1000.0 {
                    format!("{:.2} kHz", v / 1000.0)
                } else {
                    format!("{v:.1} Hz")
                }
            }
            Unit::Ms => format!("{v:.1} ms"),
            Unit::Percent => format!("{v:.0}%"),
            Unit::Ratio => format!("{v:.1}:1"),
            Unit::Semitones => format!("{v:.0} st"),
            Unit::Cents => format!("{v:.0} ct"),
            Unit::Seconds => format!("{v:.2} s"),
            Unit::None => format!("{v:.2}"),
        }
    }
}

/// Static description of a plugin.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct PluginInfo {
    pub id: &'static str,
    pub name: &'static str,
    /// At most 8 characters, for insert slots.
    pub short_name: &'static str,
    pub category: Category,
    pub params: &'static [ParamInfo],
    pub is_instrument: bool,
    /// Usable as an offline (AudioSuite-style) process.
    pub audiosuite: bool,
}

impl PluginInfo {
    /// Looks up a parameter description by id.
    pub fn param(&self, id: &str) -> Option<&'static ParamInfo> {
        self.params.iter().find(|p| p.id == id)
    }
}

/// A realtime audio processor or instrument.
///
/// Contract: `process` never allocates or panics. It processes `min(frames, io[c].len())` frames
/// of at most the channel count given to `prepare`; extra channels are left untouched.
/// Instruments replace the buffer contents with their output.
pub trait Plugin: Send {
    fn info(&self) -> &'static PluginInfo;
    fn prepare(&mut self, sample_rate: f32, max_block: usize, channels: usize);
    fn reset(&mut self);
    /// Sets a parameter (clamped into range; NaN becomes the default). `false` if `id` is unknown.
    fn set_param(&mut self, id: &str, value: f32) -> bool;
    fn param(&self, id: &str) -> Option<f32>;
    /// In-place processing of `channels` planar buffers each `frames` long.
    fn process(&mut self, io: &mut [Vec<f32>], frames: usize);
    /// Processing latency in samples (for delay compensation).
    fn latency(&self) -> usize {
        0
    }
    /// How long the output keeps ringing after the input stops, in samples.
    fn tail_samples(&self) -> usize {
        0
    }
    /// Gain reduction in dB for metering (a non-negative amount); 0 for non-dynamics plugins.
    fn gain_reduction_db(&self) -> f32 {
        0.0
    }
    /// Instruments: note events at sample offsets within the next block.
    fn note_on(&mut self, _offset: usize, _note: u8, _velocity: u8) {}
    fn note_off(&mut self, _offset: usize, _note: u8) {}
    fn all_notes_off(&mut self) {}
    /// The plugin's complete internal state as an opaque blob (third-party plugins), for saving
    /// in the session. `None` when the plugin has no state beyond its parameters (built-ins) or
    /// the plugin refused. May allocate: never call it in steady-state audio processing.
    fn save_state(&mut self) -> Option<Vec<u8>> {
        None
    }
    /// Restores a blob from [`Plugin::save_state`]. `false` if unsupported or the plugin rejected
    /// it (the plugin keeps working with its previous state). The data is hostile input.
    fn load_state(&mut self, _data: &[u8]) -> bool {
        false
    }
    /// Opens the plugin's own editor window (GUI). Main (UI) thread only.
    fn open_editor(&mut self) -> Result<(), String> {
        Err("no editor".into())
    }
    /// Closes the editor opened with [`Plugin::open_editor`] (no-op when none is open).
    fn close_editor(&mut self) {}
    /// A handle to this instance's editor that can be used on the UI thread while the instance
    /// itself is processing on the audio thread (`None` when the plugin has no editor). The
    /// handle stays valid after the instance is dropped (its calls then do nothing).
    fn editor(&mut self) -> Option<Box<dyn PluginEditor>> {
        None
    }
}

/// A plugin's editor (GUI), driven from the main (UI) thread. Obtained from
/// [`Plugin::editor`]; the plugin instance may meanwhile run on the audio thread (plugin formats
/// allow GUI calls on the main thread concurrently with processing on the audio thread).
pub trait PluginEditor: Send {
    /// Opens (or raises) the editor window. Main thread only; an error says why it can't.
    fn open(&mut self) -> Result<(), String>;
    /// Closes the editor window.
    fn close(&mut self);
    fn is_open(&self) -> bool;
    /// Call every UI frame while open: services the plugin's GUI requests, notices a window the
    /// user closed, and returns parameter edits made in the editor as `(param id, value)` in
    /// SoundCraft units (ids and ranges of [`PluginInfo::params`]).
    fn idle(&mut self) -> Vec<(String, f32)>;
    /// Tells the editor that a parameter changed elsewhere (generic editor, automation), so the
    /// GUI shows it. Values are in SoundCraft units.
    fn set_param(&mut self, _id: &str, _value: f32) {}
}

/// The plugin registry.
pub fn plugins() -> &'static [&'static PluginInfo] {
    plugins::REGISTRY
}

/// Creates a plugin by id, prepared for 48 kHz stereo with 1024-frame blocks.
/// Hosts should call `prepare` again with their real settings.
pub fn create(id: &str) -> Option<Box<dyn Plugin>> {
    let mut p = plugins::instantiate(id)?;
    p.prepare(48_000.0, 1024, 2);
    Some(p)
}

/// Looks up a plugin description by id.
pub fn plugin_info(id: &str) -> Option<&'static PluginInfo> {
    plugins::REGISTRY.iter().copied().find(|i| i.id == id)
}

/// Silence floor used by the dB helpers.
pub const MIN_DB: f32 = -144.0;

/// Converts decibels to linear gain. Anything at or below -144 dB (or NaN) is silence.
pub fn db_to_gain(db: f32) -> f32 {
    if db.is_nan() || db <= MIN_DB { 0.0 } else { 10f32.powf(db.min(200.0) / 20.0) }
}

/// Converts linear gain (sign ignored) to decibels, floored at -144 dB.
pub fn gain_to_db(g: f32) -> f32 {
    let a = g.abs();
    if a.is_nan() {
        return MIN_DB;
    }
    if a.is_infinite() {
        return f32::MAX;
    }
    (20.0 * a.log10()).max(MIN_DB)
}

/// MIDI note number to frequency in Hz (equal temperament, A4 = note 69 = 440 Hz).
pub fn midi_to_hz(note: f64) -> f64 {
    440.0 * 2f64.powf((note - 69.0) / 12.0)
}

#[cfg(test)]
mod trait_default_tests {
    use super::*;

    #[test]
    fn built_ins_have_no_state_or_editor() {
        for info in plugins() {
            let mut p = create(info.id).unwrap();
            assert!(p.save_state().is_none(), "{}", info.id);
            assert!(!p.load_state(b"anything"), "{}", info.id);
            assert!(p.open_editor().is_err(), "{}", info.id);
            p.close_editor();
            assert!(p.editor().is_none(), "{}", info.id);
        }
    }
}
