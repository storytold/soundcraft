//! MIDI port calibration and routing belong to the workstation, across sessions.
use crate::{SoundApp, Value, json};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct MidiSyncPort {
    pub enabled: bool,
    pub offset_samples: i64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Compensation {
    pub beat_clock: bool,
    pub mtc: bool,
    pub notes: bool,
}

impl Default for Compensation {
    fn default() -> Self {
        Self { beat_clock: true, mtc: true, notes: false }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct MidiSync {
    pub enabled: bool,
    pub ports: BTreeMap<String, MidiSyncPort>,
    pub compensation: Compensation,
}

fn valid_port(name: &str) -> bool {
    !name.trim().is_empty() && name.len() <= 1024
}
impl MidiSync {
    fn validate(&self) -> Result<(), String> {
        if self.ports.len() > 256 || self.ports.iter().any(|(k, v)| !valid_port(k) || v.offset_samples.unsigned_abs() > 10_000_000) {
            return Err("Invalid MIDI sync port name, count or sample offset".into());
        }
        Ok(())
    }
}

impl SoundApp {
    pub(crate) fn sync_command(&mut self, id: &str, args: &Value) -> Result<Value, String> {
        if id == "setup.midi_sync_status" {
            return Ok(json!({"output": self.player.as_ref().map(|p|p.midi_sync_status()).unwrap_or_default(),
                "audio_device_open":self.player.is_some(), "settings":self.ui.midi_sync,
                "clock_follow":self.clock_follow_state(),
                "mtc_chase":self.midi_inputs.as_ref().map(|inputs|inputs.feed().mtc_diagnostics())}));
        }
        self.migrate_midi_sync();
        let mut next = self.ui.midi_sync.clone();
        let args = args.as_object().ok_or("Expected MIDI sync settings")?;
        if args.keys().any(|k| !matches!(k.as_str(), "enabled" | "ports" | "destination" | "offset" | "compensation")) {
            return Err("Unknown MIDI sync setting".into());
        }
        if let Some(v) = args.get("enabled") {
            next.enabled = v.as_bool().ok_or("enabled must be a boolean")?;
        }
        if let Some(v) = args.get("ports") {
            next.ports = serde_json::from_value(v.clone()).map_err(|e| e.to_string())?;
        }
        if let Some(v) = args.get("compensation") {
            next.compensation = serde_json::from_value(v.clone()).map_err(|e| e.to_string())?;
        }
        // Older clients configure one destination. Retain each other port's calibration.
        if let Some(v) = args.get("destination") {
            let name = v.as_str().ok_or("destination must be a string")?;
            for port in next.ports.values_mut() {
                port.enabled = false;
            }
            if !name.is_empty() {
                next.ports.entry(name.to_owned()).or_default().enabled = true;
            }
        }
        if let Some(v) = args.get("offset") {
            let mut enabled = next.ports.values_mut().filter(|p| p.enabled);
            let port = enabled.next().ok_or("Choose a MIDI output")?;
            if enabled.next().is_some() {
                return Err("Choose a single MIDI output for this offset".into());
            }
            port.offset_samples = v.as_i64().ok_or("offset must be an integer sample count")?;
        }
        next.validate()?;
        if self.clock_follow.enabled
            && next.enabled
            && self.ui.clock_follow_input.as_deref().is_some_and(|name| next.ports.get(name).is_some_and(|p| p.enabled))
        {
            return Err("Choose separate MIDI ports for Beat Clock output and external clock follow".into());
        }
        let limit = soundcraft_playback::max_sync_offset_samples(self.engine.session().sample_rate);
        if next.ports.iter().any(|(name, port)| {
            port.offset_samples.unsigned_abs() > limit as u64
                && self.ui.midi_sync.ports.get(name).map(|previous| previous.offset_samples) != Some(port.offset_samples)
        }) {
            return Err("MIDI port calibration must be within 250 ms at the session sample rate".into());
        }
        self.ui.midi_sync = next;
        let config = &self.ui.midi_sync;
        let first = config.ports.iter().find(|(_, p)| p.enabled);
        Ok(json!({"enabled":config.enabled,"ports":config.ports,"compensation":config.compensation,
                 "destination":first.map(|(name,_)|name), "offset":first.map_or(0,|(_,p)|p.offset_samples)}))
    }

    pub(crate) fn migrate_midi_sync(&mut self) {
        if !std::mem::take(&mut self.sync_migration_pending) {
            return;
        }
        let legacy = soundcraft_model::MidiBeatClock::from_session(self.engine.session());
        if let Some(name) = legacy.destination.filter(|s| valid_port(s))
            && !self.ui.midi_sync.ports.contains_key(name)
            && self.ui.midi_sync.ports.len() < 256
        {
            let first = self.ui.midi_sync.ports.is_empty();
            self.ui.midi_sync.ports.insert(name.into(), MidiSyncPort { enabled: first && legacy.enabled, offset_samples: legacy.offset });
            if first {
                self.ui.midi_sync.enabled = legacy.enabled;
            }
        }
    }

    pub(crate) fn service_midi_sync(&mut self) {
        let Some(player) = &self.player else { return };
        if self.applied_midi_sync.as_ref() == Some(&self.ui.midi_sync) {
            return;
        }
        let config = &self.ui.midi_sync;
        let outputs = config
            .ports
            .iter()
            .filter(|(name, p)| config.enabled && p.enabled && valid_port(name))
            .map(|(k, p)| (k.clone(), p.offset_samples.clamp(-10_000_000, 10_000_000)))
            .collect();
        player.set_beat_clock_outputs(outputs, config.compensation.beat_clock);
        self.applied_midi_sync = Some(config.clone());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn port_calibration_accepts_quarter_second_at_session_rate_and_rejects_one_sample_more() {
        let mut host = SoundApp::new(Default::default(), None, crate::Services::default());
        let limit = host.engine.session().sample_rate.samples(0.25);
        for value in [-limit, limit] {
            host.sync_command("setup.midi_beat_clock", &json!({"destination":"port","offset":value})).unwrap();
        }
        for value in [-limit - 1, limit + 1] {
            assert!(host.sync_command("setup.midi_beat_clock", &json!({"offset":value})).is_err());
        }
    }

    #[test]
    fn old_project_imports_calibration_once_and_never_overwrites_machine_settings() {
        let mut app = SoundApp::new(Default::default(), None, crate::Services::default());
        let edit = &mut app.engine.session_mut().edit;
        edit.flags.insert("midi.beat_clock.destination=DIN".into());
        edit.set_flag("midi.beat_clock", true);
        edit.values.insert("midi.beat_clock.offset".into(), -211.);
        app.migrate_midi_sync();
        assert_eq!(app.ui.midi_sync.ports["DIN"].offset_samples, -211);
        assert!(app.ui.midi_sync.enabled);
        app.sync_command("setup.midi_beat_clock", &json!({"destination":"DIN","offset":-99})).unwrap();
        app.document_replaced();
        app.migrate_midi_sync();
        assert_eq!(app.ui.midi_sync.ports["DIN"].offset_samples, -99);
        for invalid in [json!({"enabled":false,"destination":"x".repeat(1025)}), json!({"ports":{"A":{"enabled":true,"offset_samples":i64::MAX}}})] {
            assert!(app.sync_command("setup.midi_beat_clock", &invalid).is_err());
            assert!(app.ui.midi_sync.enabled);
            assert_eq!(app.ui.midi_sync.ports["DIN"].offset_samples, -99);
        }
    }
}
