use super::{SoundApp, Value, json};
use crate::localization::text;
use soundcraft_playback::mtc::{self, MtcOutput};
use std::time::{Duration, Instant};

impl SoundApp {
    pub(crate) fn mtc_output_settings(&mut self, ui: &mut egui::Ui) {
        let mut chosen = self.ui.mtc_output_port.clone();
        craft_ui::ComboBox::from_label(text("MTC generator output")).selected_text(chosen.as_deref().unwrap_or_else(|| text("Off"))).show_ui(
            ui,
            |ui| {
                ui.selectable_value(&mut chosen, None, text("Off"));
                match mtc::output_ports() {
                    Ok(ports) => {
                        for port in ports {
                            ui.selectable_value(&mut chosen, Some(port.clone()), port);
                        }
                    }
                    Err(e) => {
                        ui.label(e);
                    }
                }
            },
        );
        if chosen != self.ui.mtc_output_port
            && let Err(e) = self.set_mtc_output_port(chosen)
        {
            self.ui.status = e;
        }
    }

    fn set_mtc_output_port(&mut self, port: Option<String>) -> Result<(), String> {
        let mut next = None;
        if let Some(name) = &port {
            if self.mtc_online {
                return Err("Take MTC chase offline before enabling the generator".into());
            }
            if !mtc::supported(self.engine.session().frame_rate) {
                return Err("MTC output supports 24, 25, 29.97 drop-frame and 30 fps".into());
            }
            let player = self.player.as_ref().ok_or("MTC output needs a playback engine")?;
            if self.ui.mtc_output_port == port && self.mtc_output.as_ref().is_some_and(|p| p.error().is_none()) {
                return Ok(());
            }
            let mut output = MtcOutput::open(name)?;
            output.attach(player);
            next = Some(output);
        } else if let Some(player) = &self.player {
            player.set_mtc_output(None);
        }
        self.mtc_output = next;
        self.ui.mtc_output_port = port;
        self.mtc_output_retry = None;
        Ok(())
    }

    /// Reattach after changing audio devices, or reopen the saved output after hotplug.
    pub(crate) fn service_mtc_output(&mut self) {
        let Some(port) = self.ui.mtc_output_port.as_ref() else { return };
        let Some(player) = &self.player else { return };
        if self.mtc_online || !mtc::supported(self.engine.session().frame_rate) {
            if self.mtc_output.take().is_some() {
                player.set_mtc_output(None);
                self.ui.status = "MTC output suspended: check the session frame rate and chase mode".into();
            }
            return;
        }
        if let Some(output) = &mut self.mtc_output {
            if let Some(e) = output.error() {
                self.ui.status = format!("MTC output: {e}");
                player.set_mtc_output(None);
                self.mtc_output = None;
            } else {
                output.attach(player);
                return;
            }
        }
        if self.mtc_output_retry.is_some_and(|last| last.elapsed() < Duration::from_secs(2)) {
            return;
        }
        self.mtc_output_retry = Some(Instant::now());
        if let Err(e) = self.set_mtc_output_port(Some(port.clone())) {
            self.ui.status = e;
        }
    }

    /// With no arguments returns the current selection. `list_ports` enumerates device names.
    pub(crate) fn mtc_output_command(&mut self, p: &Value) -> Result<Value, String> {
        let enabled = p.get("enabled").map(|v| v.as_bool().ok_or("enabled must be a boolean")).transpose()?;
        let list = p.get("list_ports").map(|v| v.as_bool().ok_or("list_ports must be a boolean")).transpose()?.unwrap_or(false);
        let port = match p.get("port") {
            None => self.ui.mtc_output_port.as_deref(),
            Some(Value::Null) => None,
            Some(Value::String(port)) if !port.trim().is_empty() => Some(port.as_str()),
            Some(_) => return Err("port must be a MIDI output name or null".into()),
        };
        if enabled == Some(true) && port.is_none() {
            return Err("Choose an MTC output port".into());
        }
        if enabled == Some(false) || p.get("port") == Some(&Value::Null) {
            self.set_mtc_output_port(None)?;
        } else if (p.get("port").is_some() || enabled == Some(true))
            && let Some(port) = port
        {
            self.set_mtc_output_port(Some(port.to_owned()))?;
        }
        let mut result = json!({
            "port": self.ui.mtc_output_port,
            "active": self.mtc_output.as_ref().is_some_and(|p| p.error().is_none()),
            "frame_rate": self.engine.session().frame_rate.label()
        });
        if list {
            result["ports"] = json!(mtc::output_ports()?);
        }
        Ok(result)
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
        let err = app.run("setup.mtc_output", json!({"port":"loop"})).unwrap_err();
        assert!(err.contains("29.97"));
        app.engine.session_mut().frame_rate = FrameRate::Fps25;
        app.mtc_online = true;
        let err = app.run("setup.mtc_output", json!({"port":"loop"})).unwrap_err();
        assert!(err.contains("offline"));
        assert!(app.ui.mtc_output_port.is_none());
        app.ui.mtc_output_port = Some("Saved output".into());
        app.run("setup.mtc_output", json!({"enabled":false})).unwrap();
        assert!(app.ui.mtc_output_port.is_none());
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
