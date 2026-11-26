//! MIDI output ports belong to the workstation, across sessions: the native port each label was
//! bound to, the MIDI Beat Clock routing with each port's calibration, and the delay-compensation
//! choices. Which port and channel a track plays to is in the session
//! (`soundcraft_model::MidiOutput`, the `track.midi_output` command).
use crate::{SoundApp, Value, json};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct MidiSyncPort {
    pub enabled: bool,
    pub offset_samples: i64,
}

/// Setup › Preferences › MIDI › Delay Compensation for External Devices: which external MIDI
/// outputs are delayed by the audio output latency and the mixer's plug-in delay, so they sound
/// with the audio.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Compensation {
    pub beat_clock: bool,
    pub mtc: bool,
    /// MIDI and instrument tracks' external output.
    pub notes: bool,
}

impl Default for Compensation {
    fn default() -> Self {
        Self { beat_clock: true, mtc: true, notes: true }
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
    /// Routing was configured here or imported from a project. An empty route list then
    /// means the user cleared it, so an old project must not enable Beat Clock again.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub configured: bool,
}

pub const MAX_SYNC_PORTS: usize = 256;

pub fn valid_saved_offset(samples: i64) -> bool {
    samples.unsigned_abs() <= 10_000_000
}

pub fn valid_port(name: &str) -> bool {
    !name.trim().is_empty() && name.len() <= 1024
}

impl MidiSync {
    fn bind_configured(&mut self, available: BTreeMap<String, String>, mtc: Option<&str>) {
        for (label, id) in available {
            if (self.ports.contains_key(&label) || mtc == Some(label.as_str())) && valid_port(&id) {
                self.port_ids.entry(label).or_insert(id);
            }
        }
    }

    /// Remember the native port behind each label Beat Clock, MIDI Time Code or a track uses, the
    /// first time it opens and when its device came back under a new native ID (see
    /// `soundcraft_playback` port assignment), and the labels the output worker bound beside it
    /// for other ports of the same name ("DIN (2)" beside "DIN"): saved too, a twin never takes
    /// the label of an unplugged port after a restart. A label already saved that nothing uses
    /// is left as it is. When few entries are left, the same-name labels beside a used one are
    /// saved first, then the labels in use, then the others: a saved twin is what keeps a used
    /// label from moving to that twin (its port still gets the label by name when it is listed),
    /// and the others cannot take the last free entries.
    fn bind_used(&mut self, available: BTreeMap<String, String>, used: &BTreeSet<String>, mtc: Option<&str>) {
        let in_use = |label: &str| used.contains(label) || self.ports.contains_key(label) || mtc == Some(label);
        let names: BTreeSet<_> = used.iter().map(String::as_str).chain(self.ports.keys().map(String::as_str)).chain(mtc).map(port_name).collect();
        let (in_use_now, rest): (Vec<_>, Vec<_>) = available.into_iter().partition(|(label, _)| in_use(label));
        let (twins, beside): (Vec<_>, Vec<_>) = rest.into_iter().partition(|(label, _)| names.contains(port_name(label)));
        for (label, id) in twins.into_iter().chain(in_use_now).chain(beside) {
            let room = self.port_ids.contains_key(&label) || self.port_ids.len() < MAX_SYNC_PORTS;
            if !valid_port(&id) || !room {
                continue;
            }
            if used.contains(&label) || self.ports.contains_key(&label) || mtc == Some(label.as_str()) {
                self.port_ids.insert(label, id);
            } else {
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

/// What the output warning bar says when the tracks' compensated MIDI output is sent
/// uncompensated.
pub(crate) const UNTIMED: &str = "MIDI output is not delay-compensated: the audio device reports no output timing.";

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
    pub(crate) fn sync_command(&mut self, id: &str, args: &Value) -> Result<Value, String> {
        if id == "setup.midi_sync_status" {
            let mut chase = json!(self.midi_inputs.as_ref().map(|inputs| inputs.feed().mtc_diagnostics()).unwrap_or_default());
            chase["state"] = match chase["freewheel_state"].as_str() {
                Some("freewheeling") => json!(format!("freewheeling ({}/{} frames)", chase["elapsed_frames"], chase["limit_frames"])),
                Some(s @ ("jam sync running on internal clock" | "stopped: timecode lost")) if self.mtc_online => json!(s),
                _ => json!(self.mtc_status),
            };
            return Ok(json!({"output": self.player.as_ref().map(|p|p.midi_sync_status()).unwrap_or_default(),
                "audio_device_open":self.player.is_some(), "settings":self.ui.midi_sync,
                "mtc_chase":chase}));
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
            if ids.len() - usize::from(self.ui.mtc_output_port.as_ref().is_some_and(|port| ids.contains_key(port))) > MAX_SYNC_PORTS
                || ids.iter().any(|(name, id)| !valid_port(name) || !valid_port(id))
            {
                return Err("Invalid MIDI output binding".into());
            }
            next.bind_configured(ids, self.ui.mtc_output_port.as_deref());
        } else if (args.contains_key("ports") || args.contains_key("destination"))
            && next.ports.keys().any(|label| !next.port_ids.contains_key(label))
            && let Ok(available) = soundcraft_playback::midi_sync_output_ids(&next.port_ids)
        {
            next.bind_configured(available, self.ui.mtc_output_port.as_deref());
        }
        // A label stays bound while Beat Clock or MIDI Time Code is configured for it or a track
        // plays to it.
        let used = self.midi_output_ports_in_use();
        next.port_ids.retain(|name, _| next.ports.contains_key(name) || used.contains(name) || self.ui.mtc_output_port.as_ref() == Some(name));
        let limit = soundcraft_playback::max_sync_offset_samples(self.engine.session().sample_rate);
        if next.ports.iter().any(|(name, port)| {
            port.offset_samples.unsigned_abs() > limit as u64
                && self.ui.midi_sync.ports.get(name).map(|previous| previous.offset_samples) != Some(port.offset_samples)
        }) {
            return Err("MIDI port calibration must be within 250 ms at the session sample rate".into());
        }
        next.configured |= !args.is_empty();
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
            let first = self.ui.midi_sync.ports.is_empty() && !self.ui.midi_sync.configured;
            self.ui.midi_sync.ports.insert(name.into(), MidiSyncPort { enabled: first && legacy.enabled, offset_samples: legacy.offset });
            if first {
                self.ui.midi_sync.enabled = legacy.enabled;
            }
            self.ui.midi_sync.configured = true;
        }
    }

    /// The MIDI output ports active MIDI and instrument tracks of this session play to.
    pub(crate) fn midi_output_ports_in_use(&self) -> BTreeSet<String> {
        self.midi_output_users().into_keys().collect()
    }

    /// The MIDI output ports in use, each with the names of the active MIDI and instrument tracks
    /// that play to it. An inactive track sends nothing, so its port is not opened and not
    /// reported missing.
    pub(crate) fn midi_output_users(&self) -> BTreeMap<String, Vec<String>> {
        let s = self.engine.session();
        let mut users = BTreeMap::<String, Vec<String>>::new();
        for (id, out) in soundcraft_model::MidiOutput::all(&s.edit) {
            if let Some(t) = s.track(id).filter(|t| t.kind.is_midi() && !t.inactive) {
                users.entry(out.device.to_string()).or_default().push(t.name.clone());
            }
        }
        users
    }

    /// The tracks' MIDI output would be delay-compensated but goes uncompensated, because the
    /// audio device reports no output timing (see [`compensation`]).
    pub(crate) fn midi_output_untimed(&self) -> bool {
        let routed = !self.midi_output_users().is_empty();
        self.player.as_ref().is_some_and(|p| compensation(self.ui.midi_sync.compensation.notes, p.silent, p.output_timing_unknown(), routed).1)
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

    /// Hand the Beat Clock routing to the audio engine's MIDI output, again until it accepts it.
    pub(crate) fn service_midi_sync(&mut self) {
        let Some(player) = &self.player else { return };
        let ids = &self.ui.midi_sync.port_ids;
        apply_beat_clock_routes(&self.ui.midi_sync, &mut self.applied_midi_sync, |ports, compensate| {
            player.restore_sync_output_ids(ids);
            player.try_set_beat_clock_outputs(ports, compensate)
        });
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
    /// every MIDI output failure (track output and Beat Clock) in the status line.
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
            self.ui.midi_sync.bind_used(ids, &used, self.ui.mtc_output_port.as_deref());
        }
        // The tracks' output goes uncompensated when the audio device reports no output timing;
        // that and other output timing trouble is shown by the output warning bar
        // (`output_warning`), not here.
        let (compensate, _) = compensation(self.ui.midi_sync.compensation.notes, player.silent, player.output_timing_unknown(), !users.is_empty());
        let error = player.midi_output_error().map(|e| name_tracks(&e, &users));
        let want = (used.into_iter().collect::<Vec<_>>(), compensate);
        if self.applied_midi_output.as_ref() != Some(&want) {
            player.restore_sync_output_ids(&self.ui.midi_sync.port_ids);
            let accepted = player.try_set_midi_note_outputs(want.0.clone(), want.1);
            self.applied_midi_output = accepted.then_some(want);
        }
        self.show_midi_sync_error(error);
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
    fn mtc_sync_route_keeps_its_device_id_after_beat_clock_is_disabled() {
        let mut app = SoundApp::new(soundcraft_engine::Engine::default(), None, crate::Services::default());
        let label = "DIN (2)";
        app.sync_command("setup.midi_beat_clock", &json!({"ports": {(label): {"enabled":true}}, "port_ids": {(label):"midi-b"}})).unwrap();
        app.mtc_output_command(&json!({"port":label})).unwrap();
        app.sync_command("setup.midi_beat_clock", &json!({"ports":{}})).unwrap();
        assert_eq!(app.ui.midi_sync.port_ids.get(label).map(String::as_str), Some("midi-b"));
        app.mtc_output_command(&json!({"port":null})).unwrap();
        assert!(app.ui.midi_sync.port_ids.is_empty());
    }

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
        sync.bind_used(opened, &BTreeSet::from(["DIN".to_string()]), None);
        assert_eq!(sync.port_ids.get("DIN (2)").map(String::as_str), Some("b"), "the twin's label is kept apart");
        // A saved label no track uses is not moved by a later binding.
        sync.bind_used(BTreeMap::from([("DIN (2)".to_string(), "c".to_string())]), &BTreeSet::new(), None);
        assert_eq!(sync.port_ids["DIN (2)"], "b");
    }

    #[test]
    fn a_used_label_is_saved_before_unused_ones_take_the_last_free_entries() {
        let port_ids = (0..MAX_SYNC_PORTS - 1).map(|i| (format!("Old {i:03}"), format!("old-{i}"))).collect();
        let mut sync = MidiSync { port_ids, ..MidiSync::default() };
        // "A twin" sorts before "B used": it must not take the one entry left.
        let opened = BTreeMap::from([("A twin".to_string(), "t".to_string()), ("B used".to_string(), "u".to_string())]);
        sync.bind_used(opened, &BTreeSet::from(["B used".to_string()]), None);
        assert_eq!(sync.port_ids.get("B used").map(String::as_str), Some("u"));
        assert!(!sync.port_ids.contains_key("A twin"));
        assert_eq!(sync.port_ids.len(), MAX_SYNC_PORTS);
    }

    #[test]
    fn the_twin_of_a_used_label_is_saved_before_it_when_one_entry_is_left() {
        let port_ids = (0..MAX_SYNC_PORTS - 1).map(|i| (format!("Old {i:03}"), format!("old-{i}"))).collect();
        let mut sync = MidiSync { port_ids, ..MidiSync::default() };
        let opened = BTreeMap::from([("DIN".to_string(), "a".to_string()), ("DIN (2)".to_string(), "b".to_string())]);
        sync.bind_used(opened, &BTreeSet::from(["DIN".to_string()]), None);
        // Saved, "DIN (2)" keeps B apart: with A unplugged, B does not take "DIN" after a restart.
        // A, the only other port named "DIN", still gets that name when it is listed.
        assert_eq!(sync.port_ids.get("DIN (2)").map(String::as_str), Some("b"));
        assert!(!sync.port_ids.contains_key("DIN"));
        assert_eq!(sync.port_ids.len(), MAX_SYNC_PORTS);
        // A label of another name is not a twin: the used one is saved first.
        let port_ids = (0..MAX_SYNC_PORTS - 1).map(|i| (format!("Old {i:03}"), format!("old-{i}"))).collect();
        let mut sync = MidiSync { port_ids, ..MidiSync::default() };
        let opened = BTreeMap::from([("DIN".to_string(), "a".to_string()), ("DINO (2)".to_string(), "b".to_string())]);
        sync.bind_used(opened, &BTreeSet::from(["DIN".to_string()]), None);
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
            app.service_midi_output(0.0);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(app.ui.status.contains(port), "{}", app.ui.status);
        assert!(app.ui.status.contains("Audio output requires attention"));
        assert!(app.sync_command("setup.midi_sync_status", &json!({})).unwrap()["output"]["error"].as_str().unwrap().contains(port));
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

    #[test]
    fn an_old_project_does_not_enable_routes_the_user_cleared() {
        let mut app = SoundApp::new(Default::default(), None, crate::Services::default());
        app.sync_command("setup.midi_beat_clock", &json!({"enabled":true,"destination":"DIN"})).unwrap();
        app.sync_command("setup.midi_beat_clock", &json!({"enabled":false,"ports":{"DIN":null}})).unwrap();
        assert!(app.ui.midi_sync.ports.is_empty());
        let saved: MidiSync = serde_json::from_str(&serde_json::to_string(&app.ui.midi_sync).unwrap()).unwrap();
        app.ui.midi_sync = saved;
        let edit = &mut app.engine.session_mut().edit;
        edit.flags.insert("midi.beat_clock.destination=DIN".into());
        edit.set_flag("midi.beat_clock", true);
        app.document_replaced();
        app.migrate_midi_sync();
        assert!(!app.ui.midi_sync.enabled);
        assert!(app.ui.midi_sync.ports.values().all(|port| !port.enabled));
    }

    #[test]
    fn beat_clock_routing_keeps_the_port_bindings_tracks_use() {
        let mut app = SoundApp::new(soundcraft_engine::Engine::default(), None, crate::Services::default());
        let keys = app.run("track.new", json!({"name": "Keys", "kind": "midi"})).unwrap()["tracks"][0].clone();
        app.run("track.midi_output", json!({"tracks": [keys], "output": "MIDI:1:UM-ONE"})).unwrap();
        // The track's port was bound when it opened; "Old" is used by nothing.
        app.ui.midi_sync.port_ids = BTreeMap::from([("UM-ONE".into(), "device-um".into()), ("Old".into(), "device-old".into())]);
        let ids = json!({"DIN": "device-din"});
        app.sync_command("setup.midi_beat_clock", &json!({"enabled": true, "ports": {"DIN": {"enabled": true}}, "port_ids": ids})).unwrap();
        assert_eq!(app.ui.midi_sync.port_ids.keys().collect::<Vec<_>>(), ["DIN", "UM-ONE"]);
        app.sync_command("setup.midi_beat_clock", &json!({"ports": {"DIN": null}})).unwrap();
        assert_eq!(app.ui.midi_sync.port_ids, BTreeMap::from([("UM-ONE".into(), "device-um".into())]));
    }
}
