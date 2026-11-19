//! Per-session assignments from MIDI CC to existing mixer controls.

use serde::{Deserialize, Serialize};
use soundcraft_model::{Session, TrackId, TrackKind};

// Like the engine's other app-defined settings, these entries travel in edit.flags.
// Store one binding per entry so tracks, MIDI ports and values need no schema change.
pub(crate) const PREFIX: &str = "midi.learn.v1=";

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Parameter {
    Volume,
    Pan(usize),
    Mute,
    Solo,
    SendLevel { slot: usize, bus: soundcraft_model::BusId },
    SendPan { slot: usize, bus: soundcraft_model::BusId },
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Target {
    pub track: TrackId,
    pub parameter: Parameter,
}

impl Target {
    pub fn available(self, session: &Session) -> bool {
        session.track(self.track).is_some_and(|track| match self.parameter {
            Parameter::Volume => true,
            Parameter::Mute | Parameter::Solo => track.kind != TrackKind::Master,
            Parameter::SendLevel { slot, bus } | Parameter::SendPan { slot, bus } => {
                track.mixer.sends.get(slot).and_then(Option::as_ref).is_some_and(|send| send.target == soundcraft_model::Route::Bus(bus))
            }
            Parameter::Pan(index) => index < 2 && track.kind != TrackKind::Vca && index < track.mixer.pan.len(),
        })
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Source {
    pub device: String,
    pub channel: u8,
    pub cc: u8,
}

impl Source {
    pub(crate) fn valid(&self) -> bool {
        !self.device.trim().is_empty()
            && self.device.len() <= 256
            && !self.device.chars().any(char::is_control)
            && (1..=16).contains(&self.channel)
            && soundcraft_model::is_mixer_cc(self.cc)
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ControlMode {
    #[default]
    Absolute,
    #[serde(rename = "relative_64")]
    Relative64,
    Toggle,
}
impl ControlMode {
    pub fn supports(self, parameter: Parameter) -> bool {
        let switch = matches!(parameter, Parameter::Mute | Parameter::Solo);
        match self {
            Self::Absolute => true,
            Self::Relative64 => !switch,
            Self::Toggle => switch,
        }
    }
    fn is_absolute(&self) -> bool {
        *self == Self::Absolute
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Binding {
    #[serde(default, skip_serializing_if = "ControlMode::is_absolute")]
    pub mode: ControlMode,
    pub source: Source,
    pub target: Target,
}

pub(crate) fn stored(session: &Session) -> impl Iterator<Item = (&String, Binding)> {
    session
        .edit
        .flags
        .range(PREFIX.to_owned()..)
        .map_while(|flag| Some((flag, flag.strip_prefix(PREFIX)?)))
        .filter_map(|(flag, json)| Some((flag, serde_json::from_str(json).ok()?)))
}

pub fn configured(session: &Session) -> bool {
    bindings(session).next().is_some()
}

pub fn bindings(session: &Session) -> impl Iterator<Item = Binding> {
    stored(session)
        .map(|(_, binding)| binding)
        .filter(|binding| binding.source.valid() && binding.mode.supports(binding.target.parameter) && binding.target.available(session))
}
