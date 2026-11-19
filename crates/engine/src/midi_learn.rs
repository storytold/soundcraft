//! Per-session assignments from absolute MIDI CC to existing mixer controls.

use crate::cmd::{CommandSpec, always, bad};
use crate::{Engine, Result, cmd};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use soundcraft_model::{Session, TrackId, TrackKind};

// Like the engine's other app-defined settings, these entries travel in edit.flags.
// Store one binding per entry so tracks, MIDI ports and values need no schema change.
const PREFIX: &str = "midi.learn.v1=";

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Parameter {
    Volume,
    Pan(usize),
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
    fn valid(&self) -> bool {
        !self.device.trim().is_empty() && (1..=16).contains(&self.channel) && self.cc < 120
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Binding {
    pub source: Source,
    pub target: Target,
}

fn stored(session: &Session) -> impl Iterator<Item = (&String, Binding)> {
    session
        .edit
        .flags
        .range(PREFIX.to_owned()..)
        .map_while(|flag| Some((flag, flag.strip_prefix(PREFIX)?)))
        .filter_map(|(flag, json)| Some((flag, serde_json::from_str(json).ok()?)))
}

pub fn configured(session: &Session) -> bool {
    session.edit.flags.range(PREFIX.to_owned()..).next().is_some_and(|flag| flag.starts_with(PREFIX))
}

pub fn bindings(session: &Session) -> impl Iterator<Item = Binding> {
    stored(session).map(|(_, binding)| binding).filter(|binding| binding.source.valid() && binding.target.available(session))
}

pub(crate) fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!("midi.learn.bind", "Assign MIDI Controller", [], None, "{source: {device, channel, cc}, target: {track, parameter}}", always, bind),
        cmd!("midi.learn.remove", "Remove MIDI Assignment", [], None, "{track, parameter}", always, remove),
    ]
}

fn bind(e: &mut Engine, p: &Value) -> Result<Value> {
    let id = "midi.learn.bind";
    let binding: Binding = serde_json::from_value(p.clone()).map_err(|err| bad(id, err.to_string()))?;
    if !binding.target.available(e.session()) {
        return Err(bad(id, "the mixer control is unavailable"));
    }
    if !binding.source.valid() {
        return Err(bad(id, "expected a device name, MIDI channel 1–16 and CC 0–119"));
    }
    if bindings(e.session()).any(|old| old.source == binding.source && old.target != binding.target) {
        return Err(bad(id, "this MIDI control is already assigned"));
    }
    let encoded = serde_json::to_string(&binding).map_err(|err| bad(id, err.to_string()))?;
    remove_where(e, |old| old.target == binding.target || old.source == binding.source);
    e.session_mut().edit.flags.insert(format!("{PREFIX}{encoded}"));
    Ok(json!(binding))
}

fn remove(e: &mut Engine, p: &Value) -> Result<Value> {
    let target: Target = serde_json::from_value(p.clone()).map_err(|err| bad("midi.learn.remove", err.to_string()))?;
    Ok(json!({"removed": remove_where(e, |old| old.target == target)}))
}

fn remove_where(e: &mut Engine, matches: impl Fn(&Binding) -> bool) -> usize {
    let flags: Vec<_> = stored(e.session()).filter(|(_, binding)| matches(binding)).map(|(flag, _)| flag.clone()).collect();
    for flag in &flags {
        e.session_mut().edit.flags.remove(flag);
    }
    flags.len()
}
