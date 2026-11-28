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
    /// Remember the native port behind each label a track uses, the first time it opens and when
    /// its device came back under a new native ID (see `soundcraft_playback` port assignment),
    /// and the labels the output worker bound beside it for other ports of the same name ("DIN
    /// (2)" beside "DIN"): saved too, a twin never takes the label of an unplugged port after a
    /// restart. A label already saved that no track uses is left as it is. When few entries are
    /// left, the same-name labels beside a used one are saved first, then the labels in use, then
    /// the others: a saved twin is what keeps a used label from moving to that twin (its port
    /// still gets the label by name when it is listed), and the others cannot take the last free
    /// entries.
    fn bind_used(&mut self, available: BTreeMap<String, String>, used: &BTreeSet<String>) {
        let names: BTreeSet<_> = used.iter().map(|label| port_name(label)).collect();
        let (in_use, rest): (Vec<_>, Vec<_>) = available.into_iter().partition(|(label, _)| used.contains(label));
        let (twins, beside): (Vec<_>, Vec<_>) = rest.into_iter().partition(|(label, _)| names.contains(port_name(label)));
        for (label, id) in twins.into_iter().chain(in_use).chain(beside) {
            let room = self.port_ids.contains_key(&label) || self.port_ids.len() < MAX_SYNC_PORTS;
            if !valid_port(&id) || !room {
                continue;
            }
            if used.contains(&label) {
                self.port_ids.insert(label, id);
            } else {
                self.port_ids.entry(label).or_insert(id);
            }
        }
    }
}

/// The port name a selection label numbers: "DIN" for "DIN (2)", the label itself otherwise.
fn port_name(label: &str) -> &str {
    label
        .strip_suffix(')')
        .and_then(|rest| rest.rsplit_once(" ("))
        .filter(|(_, n)| n.parse::<usize>().is_ok_and(|n| n >= 2))
        .map_or(label, |(name, _)| name)
}

/// What joins the failures of several MIDI output ports (`MidiSyncStatus::error`).
const FAILURES: &str = "; ";

/// Name the tracks that play to each port the MIDI output failure says is unavailable: "MIDI
/// output UM-ONE unavailable for Synth 1, Pad".
fn name_tracks(error: &str, users: &BTreeMap<String, Vec<String>>) -> String {
    let named = error.split(FAILURES).map(|failure| {
        match failure.strip_prefix("MIDI output ").and_then(|rest| rest.strip_suffix(" unavailable")).and_then(|port| users.get(port)) {
            Some(tracks) => format!("{failure} for {}", tracks.join(", ")),
            None => failure.to_owned(),
        }
    });
    named.collect::<Vec<_>>().join(FAILURES)
}

/// A MIDI output failure as the status line shows it, each port's failure translated by itself.
fn shown(message: &str) -> String {
    message.split(FAILURES).map(crate::localization::error_text).collect::<Vec<_>>().join(FAILURES)
}

/// What the status line says when compensated MIDI output is sent uncompensated.
const UNTIMED: &str = "MIDI output is not delay-compensated: the audio device reports no output timing.";

/// Whether the tracks' MIDI output is delay-compensated, and whether the preference asks for it
/// but cannot have it because the audio device reports no output timing. The silent clock that
/// stands in for a missing audio device has no output latency to wait for. A device that reports
/// no timing cannot time a compensated route, which would then send nothing; the output goes
/// uncompensated instead, as with the silent clock, and the status line says so while a track
/// plays to an external port (`routed`).
fn compensation(preference: bool, silent: bool, timing_unknown: bool, routed: bool) -> (bool, bool) {
    let untimed = preference && !silent && timing_unknown && routed;
    (preference && !silent && !timing_unknown, untimed)
}

/// How often the list of MIDI output ports is read again, in seconds.
const RESCAN_SECONDS: f64 = 2.0;

impl SoundApp {
    /// The MIDI output ports in use, each with the names of the active MIDI and instrument tracks
    /// that play to it. An inactive track sends nothing, so its port is not opened and not
    /// reported missing.
    fn midi_output_users(&self) -> BTreeMap<String, Vec<String>> {
        let s = self.engine.session();
        let mut users = BTreeMap::<String, Vec<String>>::new();
        for (id, out) in soundcraft_model::MidiOutput::all(&s.edit) {
            if let Some(t) = s.track(id).filter(|t| t.kind.is_midi() && !t.inactive) {
                users.entry(out.device.to_string()).or_default().push(t.name.clone());
            }
        }
        users
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

    /// Replace the MIDI output failure in the status line. The text removed is the text that was
    /// inserted, not the message translated again, so a language switch in between can't strand it.
    fn update_midi_sync_error(&mut self, error: Option<String>) {
        if let Some((_, shown)) = self.last_midi_sync_error.take() {
            if self.ui.status == shown {
                self.ui.status.clear();
            } else if let Some(other) = self.ui.status.strip_suffix(&format!(" · {shown}")) {
                self.ui.status.truncate(other.len());
            }
        }
        self.last_midi_sync_error = error.map(|message| {
            let shown = shown(&message);
            if !self.ui.status.is_empty() {
                self.ui.status.push_str(" · ");
            }
            self.ui.status.push_str(&shown);
            (message, shown)
        });
    }

    fn show_midi_sync_error(&mut self, error: Option<String>) {
        // Another message can replace the status line while the same port error persists, and a
        // language switch changes the text the error should read.
        let hidden = error.as_ref().is_some_and(|message| !self.ui.status.contains(&shown(message)));
        if error.as_ref() != self.last_midi_sync_error.as_ref().map(|(message, _)| message) || hidden {
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
        let users = self.midi_output_users();
        let used: BTreeSet<String> = users.keys().cloned().collect();
        if let Some(ids) = player.take_opened_sync_output_ids() {
            self.ui.midi_sync.bind_used(ids, &used);
        }
        let (compensate, untimed) =
            compensation(self.ui.midi_sync.compensation.notes, player.silent, player.output_timing_unknown(), !users.is_empty());
        let error = player.midi_output_error().map(|e| name_tracks(&e, &users)).or_else(|| {
            if untimed {
                return Some(crate::localization::text(UNTIMED).to_string());
            }
            Some(match player.output_timing_issue()? {
                // Only until the route above is applied: it then sends uncompensated.
                soundcraft_playback::OutputTimingIssue::Unknown => crate::localization::text(UNTIMED).to_string(),
                soundcraft_playback::OutputTimingIssue::Retained { lead_us } => crate::localized_format!(
                    "The audio driver stopped reporting output timing. Compensated MIDI outputs use the last measured lead ({ms} ms).",
                    ms = format!("{:.2}", lead_us as f64 / 1000.)
                ),
            })
        });
        let want = (used.into_iter().collect::<Vec<_>>(), compensate);
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
    fn a_language_switch_replaces_the_status_line_error_and_fixing_the_port_clears_it() {
        use crate::localization::{Language, enter};
        let mut app = SoundApp::new(soundcraft_engine::Engine::default(), None, crate::Services::default());
        app.ui.status = "Audio output requires attention".into();
        let error = "MIDI output Gone Port unavailable";
        app.show_midi_sync_error(Some(error.into()));
        {
            let _pt = enter(Language::PortugueseBrazil);
            app.show_midi_sync_error(Some(error.into()));
            assert_eq!(app.ui.status, "Audio output requires attention · Saída MIDI Gone Port indisponível");
        }
        app.show_midi_sync_error(Some(error.into()));
        assert_eq!(app.ui.status, "Audio output requires attention · MIDI output Gone Port unavailable");
        {
            let _pt = enter(Language::PortugueseBrazil);
            app.show_midi_sync_error(Some(error.into()));
        }
        app.show_midi_sync_error(None);
        assert_eq!(app.ui.status, "Audio output requires attention");
    }

    #[test]
    fn the_status_line_names_the_tracks_of_each_unavailable_port_in_the_current_language() {
        let users = BTreeMap::from([
            ("Gone A".to_string(), vec!["Synth 1".to_string(), "Pad".to_string()]),
            ("Gone B".to_string(), vec!["Bass".to_string()]),
        ]);
        let error = name_tracks("MIDI output Gone A unavailable; MIDI output Gone B unavailable; DIN: device error", &users);
        assert_eq!(error, "MIDI output Gone A unavailable for Synth 1, Pad; MIDI output Gone B unavailable for Bass; DIN: device error");
        let mut app = SoundApp::new(soundcraft_engine::Engine::default(), None, crate::Services::default());
        let _pt = crate::localization::enter(crate::localization::Language::PortugueseBrazil);
        app.show_midi_sync_error(Some(error));
        assert_eq!(app.ui.status, "Saída MIDI Gone A indisponível para Synth 1, Pad; Saída MIDI Gone B indisponível para Bass; DIN: device error");
    }

    #[test]
    fn an_inactive_track_does_not_hold_its_port_open() {
        let mut engine = soundcraft_engine::Engine::default();
        for (name, port) in [("A", "Gone A"), ("B", "Gone B")] {
            engine.execute("track.new", &json!({"name": name, "kind": "midi"})).unwrap();
            engine.execute("track.midi_output", &json!({"tracks": [name], "output": format!("MIDI:1:{port}")})).unwrap();
        }
        engine.execute("track.make_inactive", &json!({"tracks": ["B"], "inactive": true})).unwrap();
        let app = SoundApp::new(engine, None, crate::Services::default());
        assert_eq!(app.midi_output_users().into_keys().collect::<Vec<_>>(), ["Gone A"]);
    }

    #[test]
    fn output_without_device_timing_goes_uncompensated_and_says_so() {
        assert_eq!(compensation(true, false, false, true), (true, false));
        assert_eq!(compensation(true, false, true, true), (false, true), "sent uncompensated, with a note");
        assert_eq!(compensation(false, false, true, true), (false, false), "nothing to say when compensation is off");
        assert_eq!(compensation(true, true, true, true), (false, false), "the silent clock is uncompensated already");
        assert_eq!(compensation(true, false, true, false), (false, false), "nothing to say when no track plays to a port");
        let _pt = crate::localization::enter(crate::localization::Language::PortugueseBrazil);
        assert_ne!(crate::localization::text(UNTIMED), UNTIMED, "translated");
    }

    #[test]
    fn the_same_name_ports_bound_beside_a_used_one_are_saved_too() {
        let mut sync = MidiSync::default();
        let opened = BTreeMap::from([("DIN".to_string(), "a".to_string()), ("DIN (2)".to_string(), "b".to_string())]);
        sync.bind_used(opened, &BTreeSet::from(["DIN".to_string()]));
        assert_eq!(sync.port_ids.get("DIN (2)").map(String::as_str), Some("b"), "the twin's label is kept apart");
        // A saved label no track uses is not moved by a later binding.
        sync.bind_used(BTreeMap::from([("DIN (2)".to_string(), "c".to_string())]), &BTreeSet::new());
        assert_eq!(sync.port_ids["DIN (2)"], "b");
    }

    #[test]
    fn a_used_label_is_saved_before_unused_ones_take_the_last_free_entries() {
        let port_ids = (0..MAX_SYNC_PORTS - 1).map(|i| (format!("Old {i:03}"), format!("old-{i}"))).collect();
        let mut sync = MidiSync { port_ids, ..MidiSync::default() };
        // "A twin" sorts before "B used": it must not take the one entry left.
        let opened = BTreeMap::from([("A twin".to_string(), "t".to_string()), ("B used".to_string(), "u".to_string())]);
        sync.bind_used(opened, &BTreeSet::from(["B used".to_string()]));
        assert_eq!(sync.port_ids.get("B used").map(String::as_str), Some("u"));
        assert!(!sync.port_ids.contains_key("A twin"));
        assert_eq!(sync.port_ids.len(), MAX_SYNC_PORTS);
    }

    #[test]
    fn the_twin_of_a_used_label_is_saved_before_it_when_one_entry_is_left() {
        let port_ids = (0..MAX_SYNC_PORTS - 1).map(|i| (format!("Old {i:03}"), format!("old-{i}"))).collect();
        let mut sync = MidiSync { port_ids, ..MidiSync::default() };
        let opened = BTreeMap::from([("DIN".to_string(), "a".to_string()), ("DIN (2)".to_string(), "b".to_string())]);
        sync.bind_used(opened, &BTreeSet::from(["DIN".to_string()]));
        // Saved, "DIN (2)" keeps B apart: with A unplugged, B does not take "DIN" after a restart.
        // A, the only other port named "DIN", still gets that name when it is listed.
        assert_eq!(sync.port_ids.get("DIN (2)").map(String::as_str), Some("b"));
        assert!(!sync.port_ids.contains_key("DIN"));
        assert_eq!(sync.port_ids.len(), MAX_SYNC_PORTS);
        // A label of another name is not a twin: the used one is saved first.
        let port_ids = (0..MAX_SYNC_PORTS - 1).map(|i| (format!("Old {i:03}"), format!("old-{i}"))).collect();
        let mut sync = MidiSync { port_ids, ..MidiSync::default() };
        let opened = BTreeMap::from([("DIN".to_string(), "a".to_string()), ("DINO (2)".to_string(), "b".to_string())]);
        sync.bind_used(opened, &BTreeSet::from(["DIN".to_string()]));
        assert_eq!(sync.port_ids.get("DIN").map(String::as_str), Some("a"));
    }

    #[test]
    fn recalling_a_window_configuration_keeps_the_midi_output_settings() {
        let mut app = SoundApp::new(soundcraft_engine::Engine::default(), None, crate::Services::default());
        app.ui.midi_sync.port_ids.insert("DIN".into(), "device-a".into());
        let saved = serde_json::to_value(&app.ui).unwrap();
        app.ui.midi_sync.compensation.notes = false;
        app.ui.midi_sync.port_ids.insert("DIN".into(), "device-b".into());
        crate::windows::recall_configuration(&mut app, &saved).unwrap();
        assert!(!app.ui.midi_sync.compensation.notes);
        assert_eq!(app.ui.midi_sync.port_ids["DIN"], "device-b");
        // A configuration saved before these settings existed leaves them too.
        crate::windows::recall_configuration(&mut app, &serde_json::to_value(crate::UiState::default()).unwrap()).unwrap();
        assert_eq!(app.ui.midi_sync.port_ids["DIN"], "device-b");
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
