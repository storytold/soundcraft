use super::{SoundApp, Value, json};
use crate::localization::text;
use soundcraft_engine::midi_learn::{self, Binding, Parameter, Source, Target};
use std::sync::Arc;

impl SoundApp {
    pub(super) fn reset_midi_learn(&mut self) {
        if self.midi_learn_target.take().is_some() {
            self.ui.status.clear();
        }
        if let Some(inputs) = &self.midi_inputs {
            inputs.feed().control_changes().for_each(drop);
        }
    }

    fn start_midi_learn(&mut self, target: Target) -> Result<(), String> {
        if !target.available(self.engine.session()) {
            return Err(text("The mixer control is unavailable").to_owned());
        }
        self.reset_midi_learn();
        self.midi_learn_target = Some(target);
        self.ui.status = text("MIDI Learn: turn an absolute CC control").to_owned();
        Ok(())
    }

    pub(crate) fn midi_learn_command(&mut self, params: &Value) -> Result<Value, String> {
        if params["cancel"] == true {
            self.reset_midi_learn();
        } else if let Some(target) = params.get("target") {
            self.start_midi_learn(serde_json::from_value(target.clone()).map_err(|err| err.to_string())?)?;
        }
        Ok(json!({
            "pending": self.midi_learn_target,
            "bindings": midi_learn::bindings(self.engine.session()).collect::<Vec<_>>(),
        }))
    }

    pub(crate) fn midi_learn_menu(&mut self, response: &egui::Response, target: Target) {
        if cfg!(target_arch = "wasm32") {
            return;
        }
        response.context_menu(|ui| {
            let binding = midi_learn::bindings(self.engine.session()).find(|binding| binding.target == target);
            if let Some(binding) = &binding {
                let source = &binding.source;
                ui.label(crate::localized_format!("{} · Ch {} · CC {}", source.device, source.channel, source.cc));
            }
            if self.midi_learn_target == Some(target) {
                if ui.button(text("Cancel MIDI Learn")).clicked() {
                    self.reset_midi_learn();
                    ui.close();
                }
            } else if ui.button(text("MIDI Learn")).clicked() {
                if let Err(err) = self.start_midi_learn(target) {
                    self.ui.status = err;
                }
                ui.close();
            }
            if binding.is_some() && ui.button(text("Remove MIDI Assignment")).clicked() {
                if self.midi_learn_target == Some(target) {
                    self.reset_midi_learn();
                }
                let _ = self.run("midi.learn.remove", json!(target));
                ui.close();
            }
        });
    }

    pub(super) fn service_midi_learn(&mut self) -> bool {
        let active = self.midi_learn_target.is_some() || midi_learn::configured(self.engine.session());
        let Some(feed) = self.midi_inputs.as_ref().map(|inputs| Arc::clone(inputs.feed())) else { return false };
        if !active {
            feed.control_changes().for_each(drop);
            return false;
        }
        let mut changes = feed.control_changes().peekable();
        if changes.peek().is_none() {
            return feed.connected() > 0;
        }
        let mut bindings: Vec<_> = midi_learn::bindings(self.engine.session()).collect();
        // Encoders may send many values between frames. Keep the last value for each target,
        // with updates in the order of those last values (groups may share mixer controls).
        let mut updates = Vec::new();
        for event in changes {
            let Some(device) = feed.ports().name(event.port) else { continue };
            let channel = (event.status & 0x0F) + 1;
            let cc = event.data1;
            let existing = bindings.iter().find(|b| b.source.cc == cc && b.source.channel == channel && b.source.device == device);
            if let Some(target) = self.midi_learn_target {
                if existing.is_some_and(|b| b.target != target) {
                    self.ui.status = text("MIDI Learn: this control is already assigned. Turn another control.").to_owned();
                    continue;
                }
                self.midi_learn_target = None;
                let binding = Binding { source: Source { device: device.to_owned(), channel, cc }, target };
                if self.run("midi.learn.bind", json!(binding)).is_ok() {
                    bindings.retain(|old| old.target != target && old.source != binding.source);
                    bindings.push(binding);
                    self.ui.status = crate::localized_format!("MIDI Learn: assigned {} · Ch {} · CC {}", device, channel, cc);
                }
                continue;
            }
            if let Some(binding) = existing {
                updates.retain(|(target, _)| *target != binding.target);
                updates.push((binding.target, event.data2));
            }
        }
        for (target, value) in updates {
            let track = target.track.0;
            let (id, params, merge_key) = match target.parameter {
                Parameter::Volume => {
                    let db = soundcraft_model::fader_pos_to_db(f32::from(value) / 127.0);
                    ("mix.volume", json!({"track": track, "db": db}), format!("fader:{track}"))
                }
                Parameter::Pan(index) => ("mix.pan", json!({"track": track, "index": index, "pan": pan(value)}), format!("pan:{track}:{index}")),
            };
            if let Err(err) = self.engine.execute_merged(id, &params, &merge_key) {
                self.ui.status = crate::localization::error_text(&err.to_string());
            }
        }
        feed.connected() > 0
    }
}

fn pan(value: u8) -> f32 {
    (f32::from(value) - 64.0) / if value < 64 { 64.0 } else { 63.0 }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Services;
    use soundcraft_engine::{Engine, autosave};
    use soundcraft_model::TrackId;
    use soundcraft_playback::midi::MidiInputs;

    #[test]
    fn learns_scoped_ccs_and_drives_the_same_controls_after_reopening_the_file() {
        let mut app = SoundApp::new(Engine::default(), None, Services::default());
        let made = app
            .run(
                "track.new",
                json!({"rows": [
                    {"name": "Target", "kind": "audio", "format": "mono"},
                    {"name": "Other", "kind": "audio", "format": "mono"}
                ]}),
            )
            .unwrap();
        let target = TrackId(made["tracks"][0].as_u64().unwrap());
        let other = TrackId(made["tracks"][1].as_u64().unwrap());
        let inputs = MidiInputs::with_port_names(Vec::new(), vec!["Controller".into(), "Other MIDI".into()]);
        let feed = Arc::clone(inputs.feed());
        let port = feed.ports().assign("Controller");
        let other_port = feed.ports().assign("Other MIDI");
        app.midi_inputs = Some(inputs);

        feed.receive(port, &[0xB0, 18, 127]);
        app.run("ui.midi_learn", json!({"target": {"track": target, "parameter": "volume"}})).unwrap();
        feed.receive(port, &[0x90, 60, 92]);
        feed.receive(port, &[0xB0, 123, 0]);
        feed.receive(port, &[0xB0, 19, 20]);
        app.service_midi_learn();
        let volume = midi_learn::bindings(app.engine.session()).next().unwrap();
        assert_eq!(volume.source, Source { device: "Controller".into(), channel: 1, cc: 19 });
        assert_eq!(app.engine.session().track(target).unwrap().mixer.volume_db, 0.0);

        app.run("ui.midi_learn", json!({"target": {"track": target, "parameter": {"pan": 0}}})).unwrap();
        feed.receive(port, &[0xB0, 19, 127]);
        app.service_midi_learn();
        assert_eq!(app.midi_learn_target, Some(Target { track: target, parameter: Parameter::Pan(0) }));
        assert_eq!(midi_learn::bindings(app.engine.session()).collect::<Vec<_>>(), [volume]);
        assert_eq!(app.engine.session().track(target).unwrap().mixer.volume_db, 0.0);
        feed.receive(port, &[0xB0, 20, 30]);
        app.service_midi_learn();
        assert_eq!(midi_learn::bindings(app.engine.session()).count(), 2);

        let dir = autosave::create_unique_folder(&std::env::temp_dir(), "soundcraft-midi-learn").unwrap();
        let path = dir.join("Learn.scraft");
        app.run("session.save_as", json!({"path": path})).unwrap();
        app.run("session.new", json!({"name": "Empty"})).unwrap();
        assert_eq!(midi_learn::bindings(app.engine.session()).count(), 0);
        feed.receive(port, &[0xB0, 19, 0]);
        app.run("session.open", json!({"path": path})).unwrap();
        feed.receive(other_port, &[0xB0, 19, 0]);
        feed.receive(port, &[0xB1, 19, 0]);
        app.service_midi_learn();
        assert_eq!(app.engine.session().track(target).unwrap().mixer.volume_db, 0.0);
        assert_eq!(midi_learn::bindings(app.engine.session()).count(), 2);

        feed.receive(port, &[0xB0, 19, 127]);
        feed.receive(port, &[0xB0, 20, 0]);
        app.service_midi_learn();
        let session = app.engine.session();
        assert_eq!(session.track(target).unwrap().mixer.volume_db, 12.0);
        assert_eq!(session.track(target).unwrap().mixer.pan, [-1.0]);
        assert_eq!(session.track(other).unwrap().mixer.volume_db, 0.0);
        assert_eq!(session.track(other).unwrap().mixer.pan, [0.0]);
        feed.receive(port, &[0xB0, 20, 64]);
        app.service_midi_learn();
        assert_eq!(app.engine.session().track(target).unwrap().mixer.pan, [0.0]);

        app.run("midi.learn.remove", json!({"track": target, "parameter": {"pan": 0}})).unwrap();
        feed.receive(port, &[0xB0, 20, 127]);
        app.service_midi_learn();
        assert_eq!(app.engine.session().track(target).unwrap().mixer.pan, [0.0]);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
