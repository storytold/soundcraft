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
        if args.keys().any(|k| !matches!(k.as_str(), "port" | "enabled" | "list_ports")) {
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
                return Err("MTC output supports 23.976, 24, 25, 29.97 and 30 fps".into());
            }
        }
        let ports = if list { Some(soundcraft_playback::midi_sync_outputs()?) } else { None };
        self.ui.gen_mtc = enabled;
        self.ui.mtc_output_port = selected;
        self.service_mtc_output();
        let active = enabled
            && self.player.as_ref().zip(self.ui.mtc_output_port.as_ref()).is_some_and(|(player, name)| {
                player.midi_sync_status().ports.get(name).is_some_and(|port| port.connected && port.mtc_total_us.is_some())
            });
        let mut state =
            json!({"port":self.ui.mtc_output_port,"enabled":enabled,"active":active,"frame_rate":self.engine.session().frame_rate.label()});
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
