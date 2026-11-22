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
    /// Saved device IDs keep same-name occurrence labels stable across restart and unplug.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub port_ids: BTreeMap<String, String>,
    pub compensation: Compensation,
}

pub const MAX_SYNC_PORTS: usize = 256;

pub fn valid_saved_offset(samples: i64) -> bool {
    samples.unsigned_abs() <= 10_000_000
}

pub fn valid_port(name: &str) -> bool {
    !name.trim().is_empty() && name.len() <= 1024
}
impl MidiSync {
    fn bind_configured(&mut self, available: BTreeMap<String, String>) {
        for (label, id) in available {
            if self.ports.contains_key(&label) && valid_port(&id) {
                self.port_ids.entry(label).or_insert(id);
            }
        }
    }

    fn validate(&self) -> Result<(), String> {
        if self.ports.len() > MAX_SYNC_PORTS || self.ports.iter().any(|(k, v)| !valid_port(k) || !valid_saved_offset(v.offset_samples)) {
            return Err("Invalid MIDI sync port name, count or sample offset".into());
        }
        Ok(())
    }
}

impl SoundApp {
    pub(crate) fn sync_command(&mut self, id: &str, args: &Value) -> Result<Value, String> {
        if id == "setup.midi_sync_status" {
            return Ok(json!({"output": self.player.as_ref().map(|p|p.midi_sync_status()).unwrap_or_default(),
                "audio_device_open":self.player.is_some(), "settings":self.ui.midi_sync}));
        }
        self.migrate_midi_sync();
        let mut next = self.ui.midi_sync.clone();
        let args = args.as_object().ok_or("Expected MIDI sync settings")?;
        if args.keys().any(|k| !matches!(k.as_str(), "enabled" | "ports" | "destination" | "offset" | "compensation" | "port_ids")) {
            return Err("Unknown MIDI sync setting".into());
        }
        if let Some(v) = args.get("enabled") {
            next.enabled = v.as_bool().ok_or("enabled must be a boolean")?;
        }
        if let Some(v) = args.get("ports") {
            let destinations: BTreeMap<String, Option<MidiSyncPort>> = serde_json::from_value(v.clone()).map_err(|e| e.to_string())?;
            // Beat Clock routing can change independently of another protocol's calibration.
            for port in next.ports.values_mut() {
                port.enabled = false;
            }
            for (name, port) in destinations {
                if !valid_port(&name) {
                    return Err("Invalid MIDI sync port name, count or sample offset".into());
                }
                if let Some(port) = port {
                    next.ports.insert(name, port);
                } else {
                    next.ports.remove(&name);
                }
            }
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
        if args.contains_key("ports") || args.contains_key("destination") {
            next.ports.retain(|_, port| port.enabled || port.offset_samples != 0);
        }
        next.validate()?;
        if let Some(value) = args.get("port_ids") {
            let ids: BTreeMap<String, String> = serde_json::from_value(value.clone()).map_err(|e| e.to_string())?;
            if ids.len() > 256 || ids.iter().any(|(name, id)| !valid_port(name) || !valid_port(id)) {
                return Err("Invalid MIDI output binding".into());
            }
            next.bind_configured(ids);
        } else if (args.contains_key("ports") || args.contains_key("destination"))
            && next.ports.keys().any(|label| !next.port_ids.contains_key(label))
            && let Ok(available) = soundcraft_playback::midi_sync_output_ids(&next.port_ids)
        {
            next.bind_configured(available);
        }
        next.port_ids.retain(|name, _| next.ports.contains_key(name));
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
            && self.ui.midi_sync.ports.len() < MAX_SYNC_PORTS
        {
            let first = self.ui.midi_sync.ports.is_empty();
            self.ui.midi_sync.ports.insert(name.into(), MidiSyncPort { enabled: first && legacy.enabled, offset_samples: legacy.offset });
            if first {
                self.ui.midi_sync.enabled = legacy.enabled;
            }
        }
    }

    fn update_midi_sync_error(&mut self, error: Option<String>) {
        if let Some(previous) = self.last_midi_sync_error.take() {
            let previous = crate::localization::error_text(&previous);
            if self.ui.status == previous {
                self.ui.status.clear();
            } else if let Some(other) = self.ui.status.strip_suffix(&format!(" · {previous}")) {
                self.ui.status.truncate(other.len());
            }
        }
        if let Some(message) = &error {
            if !self.ui.status.is_empty() {
                self.ui.status.push_str(" · ");
            }
            self.ui.status.push_str(&crate::localization::error_text(message));
        }
        self.last_midi_sync_error = error;
    }

    pub(crate) fn service_midi_sync(&mut self) {
        let error = self.player.as_ref().and_then(|player| player.midi_clock_error());
        if error != self.last_midi_sync_error {
            self.update_midi_sync_error(error);
        }
        let Some(player) = &self.player else { return };
        if let Some(ids) = player.take_opened_sync_output_ids() {
            self.ui.midi_sync.bind_configured(ids);
        }
        let ids = &self.ui.midi_sync.port_ids;
        apply_beat_clock_routes(&self.ui.midi_sync, &mut self.applied_midi_sync, |ports, compensate| {
            player.restore_sync_output_ids(ids);
            player.try_set_beat_clock_outputs(ports, compensate)
        });
    }
}

fn apply_beat_clock_routes(config: &MidiSync, applied: &mut Option<MidiSync>, enqueue: impl FnOnce(Vec<(String, i64)>, bool) -> bool) {
    if applied.as_ref() == Some(config) {
        return;
    }
    let outputs = config
        .ports
        .iter()
        .filter(|(name, p)| config.enabled && p.enabled && valid_port(name))
        .map(|(k, p)| (k.clone(), p.offset_samples.clamp(-10_000_000, 10_000_000)))
        .collect();
    *applied = enqueue(outputs, config.compensation.beat_clock).then(|| config.clone());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duplicate_calibrations_roundtrip_as_independent_enabled_routes() {
        let mut app = SoundApp::new(soundcraft_engine::Engine::default(), None, crate::Services::default());
        let settings = json!({"enabled": true, "ports": {
            "DIN": {"enabled":true,"offset_samples":701}, "DIN (2)": {"enabled":true,"offset_samples":302}
        }, "port_ids": {"DIN": "device-a", "DIN (2)": "device-b"}});
        app.sync_command("setup.midi_beat_clock", &settings).unwrap();
        let saved: MidiSync = serde_json::from_str(&serde_json::to_string(&app.ui.midi_sync).unwrap()).unwrap();
        apply_beat_clock_routes(&saved, &mut None, |routes, _| {
            let by_id: BTreeMap<_, _> = routes.into_iter().map(|(label, offset)| (saved.port_ids[&label].as_str(), offset)).collect();
            assert_eq!(by_id, BTreeMap::from([("device-a", 701), ("device-b", 302)]));
            true
        });
        app.sync_command("setup.midi_beat_clock", &json!({"ports":{"DIN":null}})).unwrap();
        assert_eq!(app.ui.midi_sync.port_ids, BTreeMap::from([("DIN (2)".into(), "device-b".into())]));
    }

    #[test]
    fn recurring_sync_retries_unchanged_routes_until_the_worker_accepts_them() {
        let mut config = MidiSync { enabled: true, ..MidiSync::default() };
        config.ports.insert("DIN".into(), MidiSyncPort { enabled: true, offset_samples: -20 });
        let mut applied = Some(MidiSync::default());
        for accepted in [false, true] {
            apply_beat_clock_routes(&config, &mut applied, |ports, _| {
                assert_eq!(ports, [("DIN".into(), -20)]);
                accepted
            });
            assert_eq!(applied.is_some(), accepted);
        }
        apply_beat_clock_routes(&config, &mut applied, |_, _| panic!("accepted routes must be cached"));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn recurring_sync_poll_reports_a_late_port_failure() {
        let engine = soundcraft_engine::Engine::default();
        let player = soundcraft_playback::Player::new(engine.session_arc());
        let mut app = SoundApp::new(engine, Some(player), crate::Services::default());
        app.engine.transport.playing = true;
        let port = "SoundCraft test unavailable sync destination";
        app.sync_command("setup.midi_beat_clock", &json!({"enabled":true,"ports":{(port):{"enabled":true}}})).unwrap();
        app.ui.status = "Audio output requires attention".into();
        let until = std::time::Instant::now() + std::time::Duration::from_secs(4);
        while !app.ui.status.contains(port) && std::time::Instant::now() < until {
            app.service_midi_sync();
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(app.ui.status.contains(port), "{}", app.ui.status);
        assert!(app.ui.status.contains("Audio output requires attention"));
        assert!(app.sync_command("setup.midi_sync_status", &json!({})).unwrap()["output"]["error"].as_str().unwrap().contains(port));
    }

    #[test]
    fn midi_sync_errors_after_play_update_the_status_line() {
        let mut app = SoundApp::new(soundcraft_engine::Engine::default(), None, crate::Services::default());
        app.engine.transport.playing = true;
        app.ui.status = "Audio output requires attention".into();
        let error = "MIDI output DIN unavailable";
        app.update_midi_sync_error(Some(error.into()));
        assert!(app.ui.status.contains(error));
        assert!(app.ui.status.starts_with("Audio output requires attention"));
        app.update_midi_sync_error(None);
        assert_eq!(app.ui.status, "Audio output requires attention");
        app.update_midi_sync_error(Some(error.into()));
        app.ui.status = "Recording stopped".into();
        app.update_midi_sync_error(None);
        assert_eq!(app.ui.status, "Recording stopped");
    }

    #[test]
    fn unused_destinations_release_their_slots_and_calibration_can_be_removed() {
        let mut app = SoundApp::new(soundcraft_engine::Engine::default(), None, crate::Services::default());
        let mut ports = serde_json::Map::new();
        for i in 0..257 {
            ports.insert(format!("unused {i}"), json!({"enabled":false,"offset_samples":0}));
        }
        ports.insert("DIN".into(), json!({"enabled":true,"offset_samples":16}));
        let result = app.sync_command("setup.midi_beat_clock", &json!({"ports":ports})).unwrap();
        assert_eq!(result["ports"].as_object().unwrap().len(), 1);
        let result = app.sync_command("setup.midi_beat_clock", &json!({"ports":{"DIN":null}})).unwrap();
        assert_eq!(result["ports"], json!({}));
    }

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
    fn replacing_clock_destinations_retains_other_ports_calibration() {
        let mut host = SoundApp::new(Default::default(), None, crate::Services::default());
        host.sync_command(
            "setup.midi_beat_clock",
            &json!({"enabled":true, "ports":{
                "DIN":{"enabled":true,"offset_samples":-201},
                "MTC":{"enabled":false,"offset_samples":72}
            }}),
        )
        .unwrap();
        let result = host.sync_command("setup.midi_beat_clock", &json!({"ports":{"USB":{"enabled":true,"offset_samples":33}}})).unwrap();
        assert_eq!(result["destination"], "USB");
        assert_eq!(result["ports"]["DIN"], json!({"enabled":false,"offset_samples":-201}));
        assert_eq!(result["ports"]["MTC"], json!({"enabled":false,"offset_samples":72}));
        let result = host.sync_command("setup.midi_beat_clock", &json!({"ports":{}})).unwrap();
        assert!(result["destination"].is_null());
        assert_eq!(result["ports"]["USB"], json!({"enabled":false,"offset_samples":33}));
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
