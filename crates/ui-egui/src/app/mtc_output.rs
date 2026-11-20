use super::{SoundApp, Value, json};
use crate::localization::text;
use soundcraft_playback::mtc;

impl SoundApp {
    pub(crate) fn mtc_output_settings(&mut self, ui: &mut egui::Ui) {
        let mut chosen = self.ui.mtc_output_port.clone();
        craft_ui::ComboBox::from_label(text("MTC Generator Port")).selected_text(chosen.as_deref().unwrap_or(text("None"))).show_ui(ui, |ui| {
            ui.selectable_value(&mut chosen, None, text("None"));
            match soundcraft_playback::midi_sync_outputs() {
                Ok(ports) => {
                    for port in ports {
                        ui.selectable_value(&mut chosen, Some(port.clone()), port);
                    }
                }
                Err(e) => {
                    ui.label(crate::localization::error_text(&e));
                }
            }
        });
        if chosen != self.ui.mtc_output_port
            && let Err(e) = self.mtc_output_command(&json!({"port":chosen}))
        {
            self.ui.status = crate::localization::error_text(&e);
        }
        if let Some(name) = &self.ui.mtc_output_port {
            let mut offset = self.ui.midi_sync.ports.get(name).map_or(0, |cfg| cfg.offset_samples);
            if ui.horizontal(|ui| crate::dialogs::port_offset_field(ui, &mut offset, self.engine.session().sample_rate)).inner
                && let Err(err) = self.mtc_output_command(&json!({"offset_samples":offset}))
            {
                self.ui.status = crate::localization::error_text(&err);
            }
        }
    }

    pub(crate) fn service_mtc_output(&mut self) {
        let Some(player) = self.player.as_ref() else { return };
        let port =
            self.ui.mtc_output_port.as_ref().filter(|_| self.ui.gen_mtc && !self.mtc_online && mtc::supported(self.engine.session().frame_rate));
        let desired = port.map(|name| (name.as_str(), self.ui.midi_sync.ports.get(name).map_or(0, |p| p.offset_samples)));
        let compensate = self.ui.midi_sync.compensation.mtc;
        if let Some((applied, previous_compensation)) = &self.applied_mtc_output
            && applied.as_ref().map(|(name, offset)| (name.as_str(), *offset)) == desired
            && *previous_compensation == compensate
        {
            return;
        }
        let owned = desired.map(|(name, offset)| (name.to_owned(), offset));
        player.set_mtc_generator(owned.clone(), compensate);
        self.applied_mtc_output = Some((owned, compensate));
    }

    pub(crate) fn mtc_output_command(&mut self, p: &Value) -> Result<Value, String> {
        let args = p.as_object().ok_or("Expected MTC generator settings")?;
        if args.keys().any(|k| !matches!(k.as_str(), "port" | "enabled" | "list_ports" | "offset_samples")) {
            return Err("Unknown MTC generator setting".into());
        }
        let selected = match args.get("port") {
            None => self.ui.mtc_output_port.clone(),
            Some(Value::Null) => None,
            Some(Value::String(s)) if !s.trim().is_empty() && s.len() <= 1024 => Some(s.clone()),
            _ => return Err("port must be a MIDI output name or null".into()),
        };
        let enabled = match args.get("enabled") {
            None => self.ui.gen_mtc && selected.is_some(),
            Some(v) => v.as_bool().ok_or("enabled must be a boolean")?,
        };
        let list = args.get("list_ports").map(|v| v.as_bool().ok_or("list_ports must be a boolean")).transpose()?.unwrap_or(false);
        if enabled {
            if selected.is_none() {
                return Err("Choose an MTC output port".into());
            }
            if self.mtc_online {
                return Err("Take MTC chase offline before enabling the generator".into());
            }
            if !mtc::supported(self.engine.session().frame_rate) {
                return Err(mtc::UNSUPPORTED_RATE.into());
            }
        }
        let ports = if list { Some(soundcraft_playback::midi_sync_outputs()?) } else { None };
        if let Some(value) = args.get("offset_samples") {
            let name = selected.as_ref().ok_or("Choose an MTC output port")?;
            let limit = soundcraft_playback::max_sync_offset_samples(self.engine.session().sample_rate);
            let offset = value
                .as_i64()
                .filter(|n| n.unsigned_abs() <= limit as u64 || self.ui.midi_sync.ports.get(name).is_some_and(|p| p.offset_samples == *n))
                .ok_or("MIDI port calibration must be within 250 ms at the session sample rate")?;
            let ports = &mut self.ui.midi_sync.ports;
            if !ports.contains_key(name) && ports.len() >= 256 {
                return Err("Invalid MIDI sync port name, count or sample offset".into());
            }
            ports.entry(name.clone()).or_default().offset_samples = offset;
        }
        self.ui.gen_mtc = enabled;
        self.ui.mtc_output_port = selected;
        self.service_mtc_output();
        let active = enabled
            && self.player.as_ref().zip(self.ui.mtc_output_port.as_ref()).is_some_and(|(player, name)| {
                player.midi_sync_status().ports.get(name).is_some_and(|port| port.connected && port.mtc_total_us.is_some())
            });
        let mut state = json!({"port":self.ui.mtc_output_port,"enabled":enabled,"active":active,"offset_samples":self.ui.mtc_output_port.as_ref().and_then(|n|self.ui.midi_sync.ports.get(n)).map_or(0,|p|p.offset_samples),"frame_rate":self.engine.session().frame_rate.label()});
        if let Some(ports) = ports {
            state["ports"] = json!(ports);
        }
        Ok(state)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use soundcraft_time::FrameRate;

    #[test]
    fn saved_output_calibration_survives_a_lower_rate_and_rejects_new_out_of_range_values() {
        let mut host=SoundApp::new(crate::Engine::default(),None,crate::Services::default());
        host.engine.session_mut().sample_rate=soundcraft_time::SampleRate::new(96000).unwrap();
        host.run("setup.mtc_output",json!({"port":"interface","offset_samples":20000})).unwrap();
        host.engine.session_mut().sample_rate=soundcraft_time::SampleRate::HZ_48000;
        assert_eq!(host.run("setup.mtc_output",json!({"offset_samples":20000})).unwrap()["offset_samples"],20000);
        assert!(host.run("setup.mtc_output",json!({"offset_samples":20001})).is_err());
        assert_eq!(host.run("setup.mtc_output",json!({})).unwrap()["offset_samples"],20000);
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn generator_calibration_is_shared_with_beat_settings_and_output_diagnostics() {
        let engine = crate::Engine::default();
        let player = soundcraft_playback::Player::new(engine.session_arc());
        let mut app = SoundApp::new(engine, Some(player), crate::Services::default());
        app.ui.midi_sync.compensation.mtc = false;
        app.run("setup.mtc_output", json!({"port":"calibration test","enabled":true,"offset_samples":240})).unwrap();
        let port = app.run("setup.midi_beat_clock", json!({})).unwrap()["ports"]["calibration test"].clone();
        assert_eq!(port["offset_samples"], 240);
        assert_eq!(port["enabled"], false);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        loop {
            let status = app.run("setup.midi_sync_status", json!({})).unwrap();
            if status["output"]["ports"]["calibration test"]["mtc_total_us"] == 5000 {
                break;
            }
            assert!(std::time::Instant::now() < deadline, "{status}");
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(app.run("setup.mtc_output", json!({"offset_samples":12001})).is_err());
    }

    #[test]
    fn mtc_output_validates_setup_commands_before_touching_a_port() {
        let mut app = SoundApp::new(Default::default(), None, crate::Services::default());
        let status = app.run("setup.mtc_output", json!({})).unwrap();
        assert_eq!(status["port"], Value::Null);
        assert_eq!(status["active"], false);
        for args in [json!({"enabled":true}), json!({"port":37}), json!({"enabled":"yes"}), json!({"list_ports":2})] {
            assert!(app.run("setup.mtc_output", args).is_err());
        }
        app.engine.session_mut().frame_rate = FrameRate::Fps59_94;
        let err = app.run("setup.mtc_output", json!({"port":"loop","enabled":true})).unwrap_err();
        assert!(err.contains("29.97"));
        app.engine.session_mut().frame_rate = FrameRate::Fps25;
        app.mtc_online = true;
        let err = app.run("setup.mtc_output", json!({"port":"loop","enabled":true})).unwrap_err();
        assert!(err.contains("offline"));
        assert!(app.ui.mtc_output_port.is_none());
        app.ui.mtc_output_port = Some("Saved output".into());
        app.run("setup.mtc_output", json!({"enabled":false})).unwrap();
        assert_eq!(app.ui.mtc_output_port.as_deref(), Some("Saved output"));
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn stopped_locate_reaches_the_playback_cursor_that_drives_mtc() {
        let engine = crate::Engine::default();
        let player = soundcraft_playback::Player::new(engine.session_arc());
        let mut app = SoundApp::new(engine, Some(player), crate::Services::default());
        app.run("transport.locate", json!({"at":240_000})).unwrap();
        app.handle_transport(0.);
        let player = app.player.as_ref().unwrap();
        assert!(!player.is_playing());
        assert_eq!(player.position(), 240_000);
        assert_eq!(app.position(), 240_000);
    }
}
