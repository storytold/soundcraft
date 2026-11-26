//! MIDI output ports belong to the workstation, across sessions: the native port each label was
//! bound to and the delay-compensation choice. Which port and channel a track plays to is in the
//! session (`soundcraft_model::MidiOutput`, the `track.midi_output` command).
use crate::{SoundApp, Value, json};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Compensation {
    /// Delay MIDI and instrument tracks' external output by the audio output latency and the
    /// mixer's plug-in delay, so it sounds with the audio.
    pub notes: bool,
}

impl Default for Compensation {
    fn default() -> Self {
        Self { notes: true }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct MidiSync {
    /// Saved device IDs keep same-name occurrence labels stable across restart and unplug.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub port_ids: BTreeMap<String, String>,
    pub compensation: Compensation,
}

pub const MAX_SYNC_PORTS: usize = 256;

pub fn valid_port(name: &str) -> bool {
    !name.trim().is_empty() && name.len() <= 1024
}

impl MidiSync {
    /// Remember the native port behind each label a track uses, the first time it opens.
    fn bind_used(&mut self, available: BTreeMap<String, String>, used: &BTreeSet<String>) {
        for (label, id) in available {
            if used.contains(&label) && valid_port(&id) && self.port_ids.len() < MAX_SYNC_PORTS {
                self.port_ids.entry(label).or_insert(id);
            }
        }
    }
}

/// How often the list of MIDI output ports is read again, in seconds.
const RESCAN_SECONDS: f64 = 2.0;

impl SoundApp {
    /// The MIDI output ports MIDI and instrument tracks of this session play to.
    pub(crate) fn midi_output_ports_in_use(&self) -> BTreeSet<String> {
        let s = self.engine.session();
        soundcraft_model::MidiOutput::all(&s.edit)
            .filter(|(id, _)| s.track(*id).is_some_and(|t| t.kind.is_midi()))
            .map(|(_, out)| out.device.to_string())
            .collect()
    }

    /// Whether the port a track plays to is missing: `None` while the port list is not known.
    pub(crate) fn midi_output_missing(&self, device: &str) -> Option<bool> {
        match self.midi_output_ports.as_ref()? {
            Ok(ports) => Some(!ports.iter().any(|p| p == device)),
            Err(_) => Some(true),
        }
    }

    /// Each MIDI or instrument track's MIDI output for `ui.inspect`: the track, its output
    /// (`MIDI:<channel>:<device>`) and whether that port is listed (`null` while unknown).
    pub(crate) fn midi_outputs_inspect(&self) -> Value {
        let s = self.engine.session();
        let outputs: Vec<Value> = soundcraft_model::MidiOutput::all(&s.edit)
            .filter_map(|(id, out)| {
                let t = s.track(id).filter(|t| t.kind.is_midi())?;
                Some(json!({"track": id, "name": t.name, "output": out.route(), "connected": self.midi_output_missing(out.device).map(|m| !m)}))
            })
            .collect();
        Value::Array(outputs)
    }

    /// `ui.midi_output_compensation {value?: bool}`: delay-compensate the tracks' external MIDI
    /// output. Without `value` it reports the setting.
    pub(crate) fn midi_output_compensation_command(&mut self, args: &Value) -> Result<Value, String> {
        if let Some(value) = args.get("value") {
            self.ui.midi_sync.compensation.notes = value.as_bool().ok_or_else(|| format!("value must be true or false, not {value}"))?;
        }
        Ok(json!({"value": self.ui.midi_sync.compensation.notes}))
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

    fn show_midi_sync_error(&mut self, error: Option<String>) {
        // Another message can replace the status line while the same port error persists.
        let hidden = error.as_ref().is_some_and(|message| !self.ui.status.contains(&crate::localization::error_text(message)));
        if error != self.last_midi_sync_error || hidden {
            self.update_midi_sync_error(error);
        }
    }

    /// Keep the port list fresh, hand the ports the tracks use to the audio engine, and show
    /// their failures in the status line.
    pub(crate) fn service_midi_output(&mut self, now: f64) {
        let Some(player) = &self.player else { return };
        if self.midi_output_ports.is_none() || (now - self.midi_output_scan_at).abs() >= RESCAN_SECONDS {
            self.midi_output_scan_at = now;
            self.midi_output_ports =
                Some(soundcraft_playback::midi_sync_output_ids(&self.ui.midi_sync.port_ids).map(|ids| ids.into_keys().collect()));
        }
        let used = self.midi_output_ports_in_use();
        if let Some(ids) = player.take_opened_sync_output_ids() {
            self.ui.midi_sync.bind_used(ids, &used);
        }
        let error = player.midi_output_error().or_else(|| {
            let timing = player.output_timing_issue()?;
            Some(match timing {
                soundcraft_playback::OutputTimingIssue::Unknown => {
                    crate::localization::text("Audio output timing is unavailable. Compensated MIDI outputs need a valid measurement.").to_string()
                }
                soundcraft_playback::OutputTimingIssue::Retained { lead_us } => crate::localized_format!(
                    "The audio driver stopped reporting output timing. Compensated MIDI outputs use the last measured lead ({ms} ms).",
                    ms = format!("{:.2}", lead_us as f64 / 1000.)
                ),
            })
        });
        // The silent clock that stands in for a missing audio device has no output latency to
        // wait for, and no timing for a compensated route.
        let want = (used.into_iter().collect::<Vec<_>>(), self.ui.midi_sync.compensation.notes && !player.silent);
        if self.applied_midi_output.as_ref() != Some(&want) {
            player.restore_sync_output_ids(&self.ui.midi_sync.port_ids);
            let accepted = player.try_set_midi_note_outputs(want.0.clone(), want.1);
            self.applied_midi_output = accepted.then_some(want);
        }
        self.show_midi_sync_error(error);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn midi_output_errors_join_the_status_line_and_leave_it_when_fixed() {
        let mut app = SoundApp::new(soundcraft_engine::Engine::default(), None, crate::Services::default());
        app.ui.status = "Audio output requires attention".into();
        let error = "MIDI output UM-ONE unavailable";
        app.show_midi_sync_error(Some(error.into()));
        assert!(app.ui.status.starts_with("Audio output requires attention") && app.ui.status.contains(error), "{}", app.ui.status);
        app.show_midi_sync_error(None);
        assert_eq!(app.ui.status, "Audio output requires attention");
        // A persistent error comes back after another message replaced the status line.
        app.show_midi_sync_error(Some(error.into()));
        app.ui.status = "Recording stopped".into();
        app.show_midi_sync_error(Some(error.into()));
        assert!(app.ui.status.starts_with("Recording stopped") && app.ui.status.contains(error), "{}", app.ui.status);
    }

    #[test]
    fn the_compensation_command_takes_only_a_boolean() {
        let mut app = SoundApp::new(soundcraft_engine::Engine::default(), None, crate::Services::default());
        assert!(app.ui.midi_sync.compensation.notes, "compensated by default, as Beat Clock is");
        assert_eq!(app.run("ui.midi_output_compensation", json!({"value": false})).unwrap()["value"], false);
        assert!(app.run("ui.midi_output_compensation", json!({"value": "no"})).is_err());
        assert!(!app.ui.midi_sync.compensation.notes);
        assert_eq!(app.run("ui.midi_output_compensation", json!({})).unwrap()["value"], false);
    }
}
