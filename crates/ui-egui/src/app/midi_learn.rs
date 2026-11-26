use super::{SoundApp, Value, json};
use crate::localization::text;
use soundcraft_engine::midi_learn::{self, Binding, ControlMode, Parameter, Source, Target};
use std::sync::Arc;

pub(super) const GESTURE_KEY: &str = "midi_cc_gesture";
const RELEASE_SECONDS: f64 = 0.3;

impl SoundApp {
    pub(super) fn expire_midi_moves(&mut self) {
        self.midi_moves.retain(|(_, deadline)| *deadline > self.uptime);
        if self.midi_moves.is_empty() && self.engine.merge_key() == Some(GESTURE_KEY) {
            self.engine.end_merge();
        }
    }

    pub(super) fn sync_midi_routes(&mut self) {
        if self.midi_learn_target.is_some_and(|t| !t.available(self.engine.session())) {
            self.midi_learn_target = None;
            self.midi_route_revision = None;
            self.ui.status = text("The mixer control is unavailable").to_owned();
        }
        if self.midi_route_revision == Some(self.engine.revision) {
            return;
        }
        if let Some(inputs) = &self.midi_inputs {
            inputs.feed().set_control_sources(self.midi_sources(), self.midi_learn_target.is_some());
            self.midi_route_revision = Some(self.engine.revision);
        }
    }

    pub(super) fn midi_sources(&self) -> impl Iterator<Item = (String, u8, u8)> + '_ {
        midi_learn::bindings(self.engine.session()).map(|b| (b.source.device, b.source.channel, b.source.cc))
    }

    pub(super) fn reset_midi_learn(&mut self) {
        if self.midi_learn_target.take().is_some() {
            self.ui.status.clear();
        }
        if let Some(inputs) = &self.midi_inputs {
            inputs.feed().clear_control_capture();
        }
        self.midi_route_revision = None;
        self.sync_midi_routes();
    }

    fn start_midi_learn(&mut self, target: Target) -> Result<(), String> {
        if !target.available(self.engine.session()) {
            return Err(text("The mixer control is unavailable").to_owned());
        }
        if self.midi_learn_target.is_some() {
            // Input queued for the previous learn is stale.
            self.reset_midi_learn();
        } else {
            // Moves of assigned controls queued before Learn still reach the mixer.
            self.service_midi_learn();
        }
        self.midi_learn_mode =
            if matches!(target.parameter, Parameter::Mute | Parameter::Solo) { ControlMode::Toggle } else { ControlMode::Absolute };
        self.midi_learn_target = Some(target);
        self.midi_route_revision = None;
        self.sync_midi_routes();
        self.ui.status =
            text("MIDI Learn: move a control that sends a CC. Bank, parameter, switch pedal and channel-mode messages pass through.").to_owned();
        Ok(())
    }

    pub(crate) fn midi_learn_command(&mut self, params: &Value) -> Result<Value, String> {
        if params["cancel"] == true {
            self.reset_midi_learn();
        } else if let Some(target) = params.get("target") {
            let target: Target = serde_json::from_value(target.clone()).map_err(|err| err.to_string())?;
            let mode: Option<ControlMode> =
                params.get("mode").map(|v| serde_json::from_value(v.clone())).transpose().map_err(|err| err.to_string())?;
            if mode.is_some_and(|m| !m.supports(target.parameter)) {
                return Err(text("Unsupported control mode").into());
            }
            self.start_midi_learn(target)?;
            if let Some(m) = mode {
                self.midi_learn_mode = m;
            }
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
        response.context_menu(|ui| self.midi_learn_options(ui, target));
    }

    pub(crate) fn midi_learn_options(&mut self, ui: &mut egui::Ui, target: Target) {
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
        } else {
            let mut button = ui.button(text("MIDI Learn"));
            if matches!(target.parameter, Parameter::Mute | Parameter::Solo) {
                button = button.on_hover_text(text("Toggle requires a press and release. For a latching button, choose hold mode."));
            }
            if button.clicked() {
                if let Err(err) = self.start_midi_learn(target) {
                    self.ui.status = err;
                }
                ui.close();
            }
        }
        if self.midi_learn_target != Some(target) {
            let (label, mode) = if matches!(target.parameter, Parameter::Mute | Parameter::Solo) {
                ("MIDI Learn (hold button)", ControlMode::Absolute)
            } else {
                ("MIDI Learn (relative, center 64)", ControlMode::Relative64)
            };
            if ui.button(text(label)).clicked() {
                if let Err(err) = self.start_midi_learn(target) {
                    self.ui.status = err;
                } else {
                    self.midi_learn_mode = mode;
                }
                ui.close();
            }
        }
        if binding.is_some() && ui.button(text("Forget MIDI Assignment")).clicked() {
            if self.midi_learn_target == Some(target) {
                self.reset_midi_learn();
            }
            let _ = self.run("midi.learn.remove", json!(target));
            ui.close();
        }
    }

    pub(super) fn service_midi_learn(&mut self) -> bool {
        self.sync_midi_routes();
        let Some(feed) = self.midi_inputs.as_ref().map(|inputs| Arc::clone(inputs.feed())) else { return false };
        let mut changes = feed.control_changes().peekable();
        if changes.peek().is_none() {
            return false;
        }
        let mut bindings: Vec<_> = midi_learn::bindings(self.engine.session()).collect();
        // Encoders may send many values between frames. Keep the last value for each target,
        // with updates in the order of those last values (groups may share mixer controls).
        let mut updates = Vec::new();
        for change in changes {
            let event = change.event;
            let Some(device) = feed.ports().name(event.port) else { continue };
            let channel = (event.status & 0x0F) + 1;
            let cc = event.data1;
            let existing = bindings.iter().find(|b| b.source.cc == cc && b.source.channel == channel && b.source.device == device);
            // Only input captured while Learn waited is learn input: an assigned control moved
            // before Learn started (after `start_midi_learn` drained the feed) still moves the mixer.
            if let Some(target) = self.midi_learn_target.filter(|_| change.learning()) {
                if existing.is_some_and(|b| b.target != target) {
                    self.ui.status = text("MIDI Learn: this control is already assigned. Turn another control.").to_owned();
                    continue;
                }
                self.midi_learn_target = None;
                self.midi_route_revision = None;
                let binding = Binding { mode: self.midi_learn_mode, source: Source { device: device.to_owned(), channel, cc }, target };
                if self.run("midi.learn.bind", json!(binding)).is_ok() {
                    bindings.retain(|old| old.target != target && old.source != binding.source);
                    bindings.push(binding);
                    self.ui.status = crate::localized_format!("MIDI Learn: assigned {} · Ch {} · CC {}", device, channel, cc);
                }
                self.sync_midi_routes();
                continue;
            }
            if let Some(binding) = existing {
                updates.push((binding.clone(), change));
            }
        }
        if !updates.is_empty() {
            self.prepare_automation_merge();
        }
        for (binding, change) in updates {
            let target = binding.target;
            let Some((id, params)) = mixer_move(self.engine.session(), &binding, change) else { continue };
            // A switch is a discrete edit. Each press keeps its own undo step.
            if matches!(target.parameter, Parameter::Mute | Parameter::Solo) {
                if let Err(err) = self.engine.execute(id, &params) {
                    self.ui.status = crate::localization::error_text(&err.to_string());
                }
                continue;
            }
            // While the pointer drags a mixer control, join that gesture's undo step.
            let key = self
                .engine
                .merge_key()
                .filter(|key| {
                    self.pointer_down
                        && key.split_once(':').is_some_and(|(kind, _)| matches!(kind, "fader" | "pan" | "span" | "trim" | "send" | "send-pan"))
                })
                .unwrap_or(GESTURE_KEY)
                .to_owned();
            if let Err(err) = self.engine.execute_merged(id, &params, &key) {
                self.ui.status = crate::localization::error_text(&err.to_string());
            } else {
                self.midi_moves.retain(|(other, _)| *other != target);
                self.midi_moves.push((target, self.uptime + RELEASE_SECONDS));
            }
        }
        true
    }
}

fn mixer_move(
    session: &soundcraft_model::Session,
    binding: &Binding,
    change: soundcraft_playback::midi::ControlChange,
) -> Option<(&'static str, Value)> {
    let target = binding.target;
    if !target.available(session) {
        return None;
    }
    let track = target.track.0;
    let mix = &session.track(target.track)?.mixer;
    let parameter = target.parameter;
    if matches!(parameter, Parameter::Mute | Parameter::Solo) {
        let (cmd, old) = if parameter == Parameter::Mute { ("mix.mute", mix.mute) } else { ("mix.solo", mix.solo) };
        let value = if binding.mode == ControlMode::Toggle { old ^ (change.presses % 2 == 1) } else { change.event.data2 >= 64 };
        return (old != value).then(|| (cmd, json!({"track":track,"value":value})));
    }
    let value = if binding.mode == ControlMode::Relative64 {
        let pos = match parameter {
            Parameter::Volume => soundcraft_model::fader_db_to_pos(mix.volume_db) * 127.0,
            Parameter::Pan(i) => pan_cc(*mix.pan.get(i)?),
            Parameter::SendLevel { slot, .. } => soundcraft_model::fader_db_to_pos(mix.sends.get(slot)?.as_ref()?.level_db) * 127.0,
            Parameter::SendPan { slot, .. } => pan_cc(mix.sends.get(slot)?.as_ref()?.pan),
            _ => return None,
        };
        let before = pos.round().clamp(0.0, 127.0) as u8;
        let after = change.relative_value(before);
        if before == after {
            return None;
        }
        after
    } else {
        change.event.data2
    };
    let db = if value == (soundcraft_model::fader_db_to_pos(0.0) * 127.0).round() as u8 {
        0.0
    } else {
        soundcraft_model::fader_pos_to_db(f32::from(value) / 127.0)
    };
    let pan = pan(value);
    Some(match parameter {
        Parameter::Volume if mix.volume_db != db => ("mix.volume", json!({"track":track,"db":db})),
        Parameter::Pan(i) if *mix.pan.get(i)? != pan => ("mix.pan", json!({"track":track,"index":i,"pan":pan})),
        Parameter::SendLevel { slot, .. } if mix.sends.get(slot)?.as_ref()?.level_db != db => {
            ("mix.send_level", json!({"track":track,"slot":slot,"db":db}))
        }
        Parameter::SendPan { slot, .. } if mix.sends.get(slot)?.as_ref()?.pan != pan => {
            ("mix.send_level", json!({"track":track,"slot":slot,"pan":pan}))
        }
        _ => return None,
    })
}
fn pan_cc(pan: f32) -> f32 {
    64.0 + pan * if pan < 0.0 { 64.0 } else { 63.0 }
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

    fn controller() -> (SoundApp, Arc<soundcraft_playback::midi::MidiFeed>, TrackId, u8) {
        let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
        let id = app.engine.session().track_by_name("Kick").unwrap().id;
        let inputs = MidiInputs::with_port_names(vec![], vec!["Keys".into()]);
        let feed = inputs.feed().clone();
        let port = feed.ports().assign("Keys");
        app.midi_inputs = Some(inputs);
        app.run("midi.learn.bind", json!({"target":{"track":id,"parameter":"volume"},"source":{"device":"Keys","channel":1,"cc":71}})).unwrap();
        (app, feed, id, port)
    }

    #[test]
    fn midi_learn_pad_edges_and_relative_steps_survive_a_stalled_frame() {
        let (mut app, feed, id, port) = controller();
        for (cc, parameter, mode) in [(22, json!("mute"), "toggle"), (23, json!("solo"), "toggle"), (112, json!({"pan":0}), "relative_64")] {
            app.run(
                "midi.learn.bind",
                json!({"target":{"track":id,"parameter":parameter}, "source":{"device":"Keys","channel":1,"cc":cc}, "mode":mode}),
            )
            .unwrap();
        }
        let pan_before = pan_cc(app.engine.session().track(id).unwrap().mixer.pan[0]).round();
        for cc in [22, 23] {
            for value in [127, 127, 0] {
                feed.receive(port, &[0xb0, cc, value]);
            }
        }
        for v in [65, 64, 67, 64, 63, 64] {
            feed.receive(port, &[0xb0, 112, v]);
        }
        app.service_midi_learn();
        let mix = &app.engine.session().track(id).unwrap().mixer;
        assert!(mix.mute && mix.solo);
        assert_eq!(pan_cc(mix.pan[0]).round(), pan_before + 3.0, "+1, +3 and -1 all apply");
        for cc in [22, 23] {
            feed.receive(port, &[0xb0, cc, 127]);
            feed.receive(port, &[0xb0, cc, 0]);
        }
        app.service_midi_learn();
        let mix = &app.engine.session().track(id).unwrap().mixer;
        assert!(!mix.mute && !mix.solo);
    }

    #[test]
    fn midi_learn_continuous_turns_across_real_frames_preserve_undo() {
        let (mut app, feed, id, port) = controller();
        app.run("midi.learn.bind", json!({"target":{"track":id,"parameter":{"pan":0}},"source":{"device":"Keys","channel":1,"cc":75}})).unwrap();
        let before = app.engine.undo_history().len();
        let original = app.engine.session().track(id).unwrap().mixer.volume_db;
        let ctx = egui::Context::default();
        let frame = |app: &mut SoundApp, ms| {
            app.advance_test_clock(std::time::Duration::from_millis(ms));
            let _ = ctx.run_logic(&egui::RawInput::default(), |ctx| app.logic(ctx));
        };
        for value in 10..60 {
            feed.receive(port, &[0xb0, 71, value]);
            feed.receive(port, &[0xb0, 75, value]);
            frame(&mut app, 20);
        }
        assert_eq!(app.engine.undo_history().len(), before + 1);
        frame(&mut app, 600);
        feed.receive(port, &[0xb0, 71, 110]);
        frame(&mut app, 20);
        assert_eq!(app.engine.undo_history().len(), before + 2);
        assert!(app.engine.undo());
        assert!(app.engine.undo());
        assert_eq!(app.engine.session().track(id).unwrap().mixer.volume_db, original);
    }

    #[test]
    fn midi_learn_writes_volume_pan_and_send_until_touch_release_or_latch_stop() {
        use soundcraft_model::AutoParam;
        for mode in ["touch", "latch", "touch_latch"] {
            let (mut app, feed, id, port) = controller();
            app.run("midi.learn.bind", json!({"target":{"track":id,"parameter":{"pan":0}},"source":{"device":"Keys","channel":1,"cc":75}})).unwrap();
            app.run("mix.new_bus", json!({"name":"FX"})).unwrap();
            let send = app.run("mix.send", json!({"track":id,"slot":0,"bus":"FX"})).unwrap();
            app.run(
                "midi.learn.bind",
                json!({"target":{"track":id,"parameter":{"send_level":{"slot":0,"bus":send["bus"]}}},"source":{"device":"Keys","channel":1,"cc":76}}),
            )
            .unwrap();
            app.run("mix.automation_mode", json!({"tracks":[id],"mode":mode})).unwrap();
            app.run("transport.play", json!({})).unwrap();
            let ctx = egui::Context::default();
            let frame = |app: &mut SoundApp, ms| {
                app.advance_test_clock(std::time::Duration::from_millis(ms));
                let _ = ctx.run_logic(&egui::RawInput::default(), |ctx| app.logic(ctx));
            };
            frame(&mut app, 20);
            let before = app.engine.undo_history().len();
            feed.receive(port, &[0xb0, 71, 80]);
            feed.receive(port, &[0xb0, 75, 127]);
            feed.receive(port, &[0xb0, 76, 60]);
            frame(&mut app, 50);
            frame(&mut app, 50);
            let points = |app: &SoundApp, param: AutoParam| app.engine.session().track(id).unwrap().lane(&param).unwrap().points.len();
            let volumes = points(&app, AutoParam::Volume);
            let pans = points(&app, AutoParam::Pan(0));
            let sends = points(&app, AutoParam::SendLevel(0));
            assert!(volumes > 0 && pans > 0 && sends > 0);
            frame(&mut app, 400);
            assert_eq!(points(&app, AutoParam::Volume) > volumes, mode != "touch");
            assert_eq!(points(&app, AutoParam::Pan(0)) > pans, mode != "touch");
            assert_eq!(points(&app, AutoParam::SendLevel(0)) > sends, mode != "touch");
            assert_eq!(app.engine.undo_history().len(), before + 1);
            app.run("transport.stop", json!({})).unwrap();
            frame(&mut app, 20);
            assert!(app.touched_pans.is_empty());
            assert!(app.engine.undo());
            assert!(app.engine.session().track(id).unwrap().lane(&AutoParam::Pan(0)).is_none_or(|l| l.points.is_empty()));
        }
    }

    #[test]
    fn midi_learn_idle_port_sleeps_and_stalled_knob_reaches_unity() {
        let (mut app, feed, id, port) = controller();
        assert!(!app.service_midi_learn(), "idle controller must not request fast polling");
        for _ in 0..400 {
            feed.receive(port, &[0xb0, 71, 10]);
        }
        feed.receive(port, &[0xb0, 71, 95]);
        app.service_midi_learn();
        assert_eq!(app.engine.session().track(id).unwrap().mixer.volume_db, 0.0);
    }

    #[test]
    fn midi_learn_ignores_parameter_sequences_and_cancels_deleted_target() {
        let (mut app, feed, id, port) = controller();
        let target = Target { track: id, parameter: Parameter::Pan(0) };
        app.start_midi_learn(target).unwrap();
        for cc in [0, 32, 99, 98, 6, 38, 100, 101, 96, 97, 21] {
            feed.receive(port, &[0xb0, cc, 42]);
        }
        app.service_midi_learn();
        assert!(midi_learn::bindings(app.engine.session()).any(|b| b.target == target && b.source.cc == 21));
        app.start_midi_learn(target).unwrap();
        app.run("track.delete", json!({"tracks":[id]})).unwrap();
        app.service_midi_learn();
        assert!(app.midi_learn_target.is_none());
        assert!(!midi_learn::configured(app.engine.session()));
        feed.receive(port, &[0xb0, 71, 24]);
        assert!(!app.service_midi_learn(), "an orphan does not capture controls");
        assert!(app.engine.undo());
        app.service_midi_learn();
        feed.receive(port, &[0xb0, 71, 127]);
        assert!(app.service_midi_learn());
        assert_eq!(app.engine.session().track(id).unwrap().mixer.volume_db, 12.0);
    }

    #[test]
    fn midi_learn_errors_and_escape_release_the_capture() {
        let (mut app, feed, id, port) = controller();
        let target = Target { track: id, parameter: Parameter::Pan(0) };
        app.start_midi_learn(target).unwrap();
        let invalid = feed.ports().assign(&"x".repeat(257));
        feed.receive(invalid, &[0xb0, 75, 12]);
        app.service_midi_learn();
        assert!(app.midi_learn_target.is_none());
        assert!(app.ui.status.contains("256"), "{}", app.ui.status);
        assert_eq!(midi_learn::bindings(app.engine.session()).count(), 1);
        app.start_midi_learn(target).unwrap();
        let ctx = egui::Context::default();
        let mut input = egui::RawInput::default();
        input.events.push(egui::Event::Key {
            key: egui::Key::Escape,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        });
        for raw in [egui::RawInput::default(), input] {
            let mut out = ctx.run_ui(raw, |ui| {
                app.logic(ui.ctx());
                app.ui(ui);
            });
            out.textures_delta.clear();
        }
        assert!(app.midi_learn_target.is_none());
        feed.receive(port, &[0xb0, 75, 10]);
        assert!(feed.control_changes().next().is_none());
    }

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
        let unknown_flag = "midi.learn.v1=not-json";
        app.engine.session_mut().edit.flags.insert(unknown_flag.into());
        app.run("session.save_as", json!({"path": path})).unwrap();
        app.run("session.new", json!({"name": "Empty"})).unwrap();
        assert_eq!(midi_learn::bindings(app.engine.session()).count(), 0);
        feed.receive(port, &[0xB0, 19, 0]);
        app.run("session.open", json!({"path": path})).unwrap();
        assert!(app.engine.session().edit.flags.contains(unknown_flag));
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
    #[test]
    fn midi_learn_knob_and_overlapping_mouse_drag_preserve_undo() {
        let (mut app, feed, id, port) = controller();
        let other = app
            .engine
            .session()
            .tracks
            .iter()
            .find(|t| t.id != id && !t.mixer.pan.is_empty() && t.kind != soundcraft_model::TrackKind::Master)
            .unwrap()
            .id;
        let initial_volume = app.engine.session().track(id).unwrap().mixer.volume_db;
        let initial_pan = app.engine.session().track(other).unwrap().mixer.pan[0];
        let before = app.engine.undo_history().len();
        let ctx = egui::Context::default();
        for v in 10..40u8 {
            feed.receive(port, &[0xb0, 71, v]);
            app.advance_test_clock(std::time::Duration::from_millis(16));
            app.pointer_down = true;
            let _ = ctx.run_logic(&egui::RawInput::default(), |ctx| app.logic(ctx));
            app.pointer_down = true;
            app.engine
                .execute_merged("mix.pan", &json!({"track": other.0, "pan": f32::from(v) / 100.0, "index": 0}), &format!("pan:{}:0", other.0))
                .unwrap();
        }
        let steps = app.engine.undo_history().len() - before;
        assert!((1..=2).contains(&steps), "{steps} undo steps");
        assert_ne!(app.engine.session().track(id).unwrap().mixer.volume_db, initial_volume);
        assert_ne!(app.engine.session().track(other).unwrap().mixer.pan[0], initial_pan);
        for _ in 0..steps {
            assert!(app.engine.undo());
        }
        assert_eq!(app.engine.session().track(id).unwrap().mixer.volume_db, initial_volume);
        assert_eq!(app.engine.session().track(other).unwrap().mixer.pan[0], initial_pan);
    }

    #[test]
    fn midi_learn_repeated_values_do_not_hold_a_touch_or_merge_a_switch() {
        let (mut app, feed, id, port) = controller();
        app.run("mix.new_bus", json!({"name":"FX"})).unwrap();
        let bus = app.run("mix.send", json!({"track":id,"slot":0,"bus":"FX"})).unwrap()["bus"].clone();
        for (cc, parameter) in [
            (75, json!({"pan":0})),
            (76, json!({"send_level":{"slot":0,"bus":bus}})),
            (77, json!({"send_pan":{"slot":0,"bus":bus}})),
            (22, json!("mute")),
        ] {
            app.run("midi.learn.bind", json!({"target":{"track":id,"parameter":parameter}, "source":{"device":"Keys","channel":1,"cc":cc}})).unwrap();
        }
        let ctx = egui::Context::default();
        let frame = |app: &mut SoundApp, ms| {
            app.advance_test_clock(std::time::Duration::from_millis(ms));
            let _ = ctx.run_logic(&egui::RawInput::default(), |ctx| app.logic(ctx));
        };
        for cc in [71, 75, 76, 77] {
            feed.receive(port, &[0xb0, cc, 32]);
        }
        frame(&mut app, 20);
        assert_eq!(app.midi_moves.len(), 4);
        let before_switch = app.engine.undo_history().len();
        feed.receive(port, &[0xb0, 22, 127]);
        frame(&mut app, 20);
        assert_eq!(app.engine.undo_history().len(), before_switch + 1);
        for _ in 0..6 {
            for cc in [71, 75, 76, 77] {
                feed.receive(port, &[0xb0, cc, 32]);
            }
            frame(&mut app, 100);
        }
        assert!(app.midi_moves.is_empty());
        assert_eq!(app.engine.undo_history().len(), before_switch + 1);
        assert!(app.engine.undo());
        let mix = &app.engine.session().track(id).unwrap().mixer;
        assert!(!mix.mute);
        assert_eq!(mix.pan[0], pan(32));
    }

    #[test]
    fn mouse_pan_touches_one_side_and_write_starts_before_any_input() {
        use soundcraft_model::AutoParam;
        for mode in ["touch", "latch", "touch_latch", "write"] {
            let (mut app, _, _, _) = controller();
            let id = app.engine.session().tracks.iter().find(|t| t.mixer.pan.len() == 2).unwrap().id;
            app.run("mix.automation_mode", json!({"tracks":[id],"mode":mode})).unwrap();
            app.run("transport.play", json!({})).unwrap();
            let ctx = egui::Context::default();
            let frame = |app: &mut SoundApp| {
                app.advance_test_clock(std::time::Duration::from_millis(50));
                let _ = ctx.run_logic(&egui::RawInput::default(), |ctx| app.logic(ctx));
            };
            let points = |app: &SoundApp, side| app.engine.session().track(id).unwrap().lane(&AutoParam::Pan(side)).map_or(0, |l| l.points.len());
            frame(&mut app);
            assert_eq!(points(&app, 0) > 0, mode == "write");
            assert_eq!(points(&app, 1) > 0, mode == "write");
            app.pointer_down = true;
            app.gesture = Some(crate::Gesture::Pan { track: id, index: 1 });
            frame(&mut app);
            let touched = points(&app, 1);
            assert!(touched > 0);
            app.pointer_down = false;
            frame(&mut app);
            assert_eq!(points(&app, 0) > 0, mode == "write");
            assert_eq!(points(&app, 1) > touched, mode != "touch");
        }
    }

    #[test]
    fn midi_learn_start_keeps_an_assigned_move_queued_before_it() {
        let (mut app, feed, id, port) = controller();
        let original = app.engine.session().track(id).unwrap().mixer.volume_db;
        feed.receive(port, &[0xb0, 71, 20]);
        app.start_midi_learn(Target { track: id, parameter: Parameter::Pan(0) }).unwrap();
        assert_ne!(app.engine.session().track(id).unwrap().mixer.volume_db, original);
        feed.receive(port, &[0xb0, 21, 30]);
        app.service_midi_learn();
        assert!(midi_learn::bindings(app.engine.session()).any(|b| b.target.parameter == Parameter::Pan(0) && b.source.cc == 21));
    }

    #[test]
    fn midi_learn_start_keeps_an_assigned_move_queued_after_its_drain() {
        let (mut app, feed, id, port) = controller();
        let original = app.engine.session().track(id).unwrap().mixer.volume_db;
        // What `start_midi_learn` does after its drain, with a move arriving in between.
        app.service_midi_learn();
        feed.receive(port, &[0xb0, 71, 20]);
        app.midi_learn_target = Some(Target { track: id, parameter: Parameter::Pan(0) });
        app.midi_route_revision = None;
        app.sync_midi_routes();
        feed.receive(port, &[0xb0, 21, 30]);
        app.service_midi_learn();
        assert_ne!(app.engine.session().track(id).unwrap().mixer.volume_db, original, "the assigned move reached the mixer");
        let bindings: Vec<_> = midi_learn::bindings(app.engine.session()).collect();
        assert!(bindings.iter().any(|b| b.target.parameter == Parameter::Pan(0) && b.source.cc == 21), "Learn took the next control");
        assert!(bindings.iter().any(|b| b.target.parameter == Parameter::Volume && b.source.cc == 71), "the volume keeps its control");
    }

    /// A send moved by MIDI and then retargeted away and back within the release interval does
    /// not resume Latch writing: the retarget released the MIDI touch with the latch.
    #[test]
    fn a_midi_moved_send_retargeted_away_and_back_stays_released() {
        use soundcraft_model::AutoParam;
        let (mut app, feed, id, port) = controller();
        app.run("mix.new_bus", json!({"name":"FX"})).unwrap();
        app.run("mix.new_bus", json!({"name":"Other"})).unwrap();
        let send = app.run("mix.send", json!({"track":id,"slot":0,"bus":"FX"})).unwrap();
        app.run(
            "midi.learn.bind",
            json!({"target":{"track":id,"parameter":{"send_level":{"slot":0,"bus":send["bus"]}}},"source":{"device":"Keys","channel":1,"cc":76}}),
        )
        .unwrap();
        app.run("mix.automation_mode", json!({"tracks":[id],"mode":"latch"})).unwrap();
        app.run("transport.play", json!({})).unwrap();
        let ctx = egui::Context::default();
        let frame = |app: &mut SoundApp| {
            app.advance_test_clock(std::time::Duration::from_millis(30));
            let _ = ctx.run_logic(&egui::RawInput::default(), |ctx| app.logic(ctx));
        };
        let points = |app: &SoundApp| app.engine.session().track(id).unwrap().lane(&AutoParam::SendLevel(0)).map_or(0, |l| l.points.len());
        frame(&mut app);
        feed.receive(port, &[0xb0, 76, 60]);
        frame(&mut app);
        frame(&mut app);
        assert!(points(&app) > 0, "the MIDI move latched the send");
        app.run("mix.send", json!({"track":id,"slot":0,"bus":"Other"})).unwrap();
        frame(&mut app);
        app.run("mix.send", json!({"track":id,"slot":0,"bus":"FX"})).unwrap();
        let returned = points(&app);
        for _ in 0..3 {
            frame(&mut app);
        }
        assert_eq!(points(&app), returned, "within the release interval, the latch stays released after the send returns");
        // Moved away and back by two commands of one frame, before an automation pass sees it.
        feed.receive(port, &[0xb0, 76, 70]);
        frame(&mut app);
        frame(&mut app);
        assert!(points(&app) > returned, "the new MIDI move latched the send again");
        app.run("mix.send", json!({"track":id,"slot":0,"bus":"Other"})).unwrap();
        app.run("mix.send", json!({"track":id,"slot":0,"bus":"FX"})).unwrap();
        let returned = points(&app);
        for _ in 0..3 {
            frame(&mut app);
        }
        assert_eq!(points(&app), returned, "released between the two commands");
    }

    #[test]
    fn midi_touch_pan_writes_its_last_point_at_stop() {
        use soundcraft_model::AutoParam;
        let (mut app, feed, id, port) = controller();
        app.run("midi.learn.bind", json!({"target":{"track":id,"parameter":{"pan":0}},"source":{"device":"Keys","channel":1,"cc":75}})).unwrap();
        app.run("mix.automation_mode", json!({"tracks":[id],"mode":"touch"})).unwrap();
        app.run("transport.play", json!({})).unwrap();
        let ctx = egui::Context::default();
        let frame = |app: &mut SoundApp, ms| {
            app.advance_test_clock(std::time::Duration::from_millis(ms));
            let _ = ctx.run_logic(&egui::RawInput::default(), |ctx| app.logic(ctx));
        };
        frame(&mut app, 20);
        feed.receive(port, &[0xb0, 75, 100]);
        frame(&mut app, 50);
        feed.receive(port, &[0xb0, 75, 110]);
        frame(&mut app, 10);
        let last = |app: &SoundApp| app.engine.session().track(id).unwrap().lane(&AutoParam::Pan(0)).unwrap().points.last().map(|p| p.value);
        assert_ne!(last(&app), Some(pan(110)), "the second move lands between automation samples");
        app.run("transport.stop", json!({})).unwrap();
        frame(&mut app, 10);
        assert_eq!(last(&app), Some(pan(110)), "Stop writes the pan's final value");
        assert!(app.touched_pans.is_empty());
    }
}
