use crate::{SoundApp, TransportRequest, Value, json};
use soundcraft_playback::mtc_input::{ChaseEvent, Position};

impl SoundApp {
    pub(crate) fn mtc_command(&mut self, p: &Value) -> Result<Value, String> {
        let args = p.as_object().ok_or("setup.mtc expects an object")?;
        if args.is_empty() {
            return Ok(self.mtc_state());
        }
        if args.keys().any(|k| !matches!(k.as_str(), "input" | "online" | "freewheel" | "freewheel_frames" | "jam_sync")) {
            return Err("Unknown MTC reader setting".into());
        }
        let source = match args.get("input") {
            None => self.ui.mtc_input_port.clone(),
            Some(Value::Null) => None,
            Some(Value::String(s)) if !s.trim().is_empty() && s.len() <= 1024 => Some(s.clone()),
            Some(_) => return Err("input must be a MIDI port name or null".into()),
        };
        let online = match args.get("online") {
            Some(v) => v.as_bool().ok_or("online must be a boolean")?,
            None => self.mtc_online && source.is_some(),
        };
        if online && source.is_none() {
            return Err("Select an MTC input before going online".into());
        }
        if online && !soundcraft_playback::mtc::supported(self.engine.session().frame_rate) {
            return Err(soundcraft_playback::mtc::UNSUPPORTED_RATE.into());
        }
        if online && self.ui.gen_mtc {
            return Err("Disable MTC output before going online".into());
        }
        if online && self.engine.transport.recording {
            return Err("Stop recording before enabling MTC chase".into());
        }
        let mut session_settings = serde_json::Map::new();
        for key in ["freewheel", "freewheel_frames", "jam_sync"] {
            if let Some(value) = args.get(key) {
                session_settings.insert(key.into(), value.clone());
            }
        }
        if !session_settings.is_empty() {
            self.engine.execute("setup.session", &Value::Object(session_settings)).map_err(|e| e.to_string())?;
        }
        if self.ui.mtc_input_port != source || self.mtc_online != online {
            if let Some(m) = &self.midi_inputs {
                m.feed().set_mtc_source(None);
            }
            self.stop_mtc_transport();
            self.update_mtc_warning(None);
            self.ui.mtc_input_port = source;
            self.mtc_online = online;
            self.mtc_status = if online { "Waiting for timecode" } else { "Offline" };
        }
        Ok(self.mtc_state())
    }

    fn mtc_state(&self) -> Value {
        json!({"input": self.ui.mtc_input_port, "online": self.mtc_online, "status": self.mtc_status,"freewheel":self.engine.mtc_freewheel_mode(),"freewheel_frames":self.engine.mtc_freewheel_frames(),"jam_sync":self.engine.mtc_jam_sync()})
    }

    fn stop_mtc_transport(&mut self) {
        // The audio callback may have locked before the host has observed its first Roll.
        self.leave_chase_audio();
        if std::mem::take(&mut self.mtc_chasing) && self.is_playing() && !self.engine.transport.recording {
            self.stop_play();
        }
    }

    fn leave_chase_audio(&mut self) {
        if let Some(player) = &self.player {
            player.end_mtc_chase();
        }
    }

    fn update_mtc_warning(&mut self, warning: Option<String>) {
        if self.mtc_warning != warning {
            if self.mtc_warning.as_ref() == Some(&self.ui.status) {
                self.ui.status.clear();
            }
            if let Some(msg) = &warning {
                self.ui.status.clone_from(msg);
            }
            self.mtc_warning = warning;
        }
    }
    pub(crate) fn mtc_rate_warning(&self) -> Option<String> {
        let mismatch = self.midi_inputs.as_ref()?.feed().mtc_input_status().1?;
        Some(crate::localized_format!("Receiving {incoming} fps; session is {session} fps", incoming = mismatch.incoming, session = mismatch.session))
    }

    fn mtc_at(&self, p: Position) -> i64 {
        let s = self.engine.session();
        p.session_sample(s.frame_rate, s.sample_rate, s.timecode_start, s.edit.value("sync.external_offset", 0.) as i64)
    }

    pub(crate) fn service_mtc_chase(&mut self) {
        if !self.mtc_online {
            return;
        }
        let queued = self.engine.transport_requests.len();
        self.engine.transport_requests.retain(|r| !matches!(r, TransportRequest::Record));
        if self.engine.transport_requests.len() != queued {
            self.ui.status = crate::localization::text("Take MTC chase offline before recording").into();
        }
        // A local transport or document change returns control to the host.
        if self.engine.transport.recording || self.engine.transport_requests.iter().any(|r| !matches!(r, TransportRequest::AllNotesOff)) {
            self.leave_chase_audio();
            if let Some(m) = &self.midi_inputs {
                m.feed().set_mtc_source(None);
            }
            self.mtc_online = false;
            self.update_mtc_warning(None);
            self.mtc_chasing = false;
            self.mtc_status = "Offline";
            return;
        }
        let Some(inputs) = &self.midi_inputs else { return };
        let source = self.ui.mtc_input_port.as_deref().and_then(|name| {
            inputs.ports().iter().find(|p| p.name == name && p.open).and_then(|p| {
                let port = inputs.feed().ports().assign(&p.name);
                (inputs.feed().ports().name(port) == Some(name)).then_some(port)
            })
        });
        let s = self.engine.session();
        inputs.feed().set_mtc_options(
            s.frame_rate,
            if self.engine.mtc_freewheel_mode() == "none" { 0 } else { self.engine.mtc_freewheel_frames() },
            self.engine.mtc_jam_sync(),
            s.edit.value("sync.external_offset", 0.) as i64,
        );
        inputs.feed().set_mtc_source(source);
        if source.is_none() {
            if self.mtc_status != "Input unavailable" {
                self.stop_mtc_transport();
            }
            self.update_mtc_warning(None);
            self.mtc_status = "Input unavailable";
            return;
        }
        if self.mtc_status == "Input unavailable" {
            self.mtc_status = "Waiting for timecode";
        }
        let event = inputs.feed().take_mtc_with_lock();
        self.update_mtc_warning(self.mtc_rate_warning());
        match event {
            Some((ChaseEvent::Locate(p) | ChaseEvent::Roll(p, _), lock)) if self.mtc_at(p) < 0 => {
                if self.mtc_status != "Before session start" {
                    if let Some(player) = &self.player {
                        // As for a stop: only the roll this observation belongs to may be ended,
                        // not a newer one that began since (the callback halts its own chase of a
                        // position before the session start).
                        player.end_observed_mtc_chase(lock);
                        self.mtc_chasing = false;
                        self.engine.transport.playing = player.is_playing();
                    } else {
                        self.stop_mtc_transport();
                    }
                }
                self.mtc_status = "Before session start";
            }
            Some((ChaseEvent::Locate(p), lock)) => {
                if let Some(player) = &self.player {
                    // As for a stop: a newer roll may already own the callback, and only the roll
                    // this locate ended may be stopped (the callback halts that one itself).
                    player.end_observed_mtc_chase(lock);
                    self.engine.transport.playing = player.is_playing();
                } else if self.is_playing() {
                    self.stop_play();
                }
                self.mtc_chasing = false;
                let at = self.mtc_at(p);
                let _ = self.run("transport.locate", json!({"at": at}));
                self.mtc_status = "Located";
            }
            Some((ChaseEvent::Roll(p, age_us), _)) => {
                let sr = self.engine.session().sample_rate;
                let at = self.mtc_at(p).saturating_add(sr.samples(age_us as f64 / 1_000_000.0));
                if let Some(player) = &self.player {
                    if !player.is_playing() {
                        return;
                    }
                    self.engine.transport.playing = true;
                    self.engine.transport.position = player.position();
                } else if !self.is_playing() || !self.mtc_chasing {
                    self.engine.transport_requests.push(TransportRequest::PlayFrom(at));
                }
                self.mtc_chasing = true;
                self.mtc_status = "Locked";
            }
            Some((ChaseEvent::Stop, lock)) => {
                if let Some(player) = &self.player {
                    // A newer roll may already own the callback while the UI processes this stop.
                    player.end_observed_mtc_chase(lock);
                    self.mtc_chasing = false;
                    self.engine.transport.playing = player.is_playing();
                } else {
                    self.stop_mtc_transport();
                }
                self.mtc_status = "Waiting for timecode";
            }
            None => {}
        }
    }

    pub(crate) fn mtc_input_settings(&mut self, ui: &mut egui::Ui) {
        let mut selected = self.ui.mtc_input_port.clone();
        egui::ComboBox::from_label(crate::localization::text("MTC Reader Port"))
            .selected_text(selected.as_deref().unwrap_or(crate::localization::text("None")))
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut selected, None, crate::localization::text("None"));
                if let Some(m) = &self.midi_inputs {
                    for p in m.ports().iter().filter(|p| p.enabled) {
                        ui.selectable_value(&mut selected, Some(p.name.clone()), &p.name);
                    }
                }
            });
        let mut online = self.mtc_online;
        let changed = crate::widgets::checkbox(ui, &mut online, crate::localization::text("Online (chase MTC)")).changed();
        if selected != self.ui.mtc_input_port || changed {
            // An explicit Online attempt reaches validation, which explains a missing input.
            // Clearing the input of an online reader takes it offline.
            let online = online && (changed || selected.is_some());
            if let Err(e) = self.mtc_command(&json!({"input":selected,"online":online})) {
                self.ui.status = crate::localization::error_text(&e);
            }
        }
        ui.label(crate::localization::text(self.mtc_status));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use soundcraft_playback::midi::{MidiFeed, MidiInputs};

    fn app() -> SoundApp {
        let mut app = SoundApp::new(Default::default(), None, crate::Services::default());
        app.midi_inputs = Some(MidiInputs::with_port_names(vec![], vec!["Sender".into()]));
        let start = app.engine.session().sample_rate.samples(3600.0);
        app.engine.session_mut().timecode_start = start;
        app.run("setup.mtc", json!({"input":"Sender","online":true})).unwrap();
        app.service_mtc_chase();
        app
    }

    #[test]
    fn recovery_reports_the_document_name_and_the_external_offset_conflict() {
        let mut host = app();
        let dir = std::env::temp_dir().join(format!("soundcraft-mtc-conflict-recovery-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut s = host.engine.session().clone();
        s.edit.values.insert("sync.external_offset".into(), 42.);
        s.edit.values.insert("midi.mtc.external_offset_frames".into(), 100.);
        std::fs::write(dir.join("session.scraft"), s.to_json().unwrap()).unwrap();
        let leftover = soundcraft_engine::autosave::Leftover {
            dir: dir.clone(),
            name: "Take 12".into(),
            origin: None,
            saved: std::time::SystemTime::now(),
            recording: false,
        };
        host.recover(&leftover);
        std::fs::remove_file(dir.join("session.scraft")).unwrap();
        std::fs::remove_dir(dir).unwrap();
        assert!(host.ui.status.contains("Take 12") && host.ui.status.contains("conflicting timecode offsets"), "{}", host.ui.status);
    }
    #[test]
    fn offset_edit_repositions_a_stopped_reader_without_another_packet() {
        let mut host = app();
        let feed = host.midi_inputs.as_ref().unwrap().feed().clone();
        let sender = feed.ports().assign("Sender");
        feed.receive(sender, &[0xf0, 0x7f, 127, 1, 1, 0x61, 0, 10, 0, 0xf7]);
        host.service_mtc_chase();
        host.handle_transport(0.);
        host.run("setup.external_timecode_offset", json!({"timecode":"+00:00:01:00"})).unwrap();
        host.service_mtc_chase();
        host.handle_transport(0.);
        assert_eq!(host.position(), host.engine.session().sample_rate.samples(11.));
    }

    #[test]
    fn online_transition_gates_a_record_command_already_in_the_queue() {
        let mut host = app();
        host.mtc_command(&json!({"online":false})).unwrap();
        let id = host.run("track.new", json!({"kind":"midi","format":"mono"})).unwrap()["tracks"][0].as_u64().unwrap();
        host.run("mix.record_arm", json!({"tracks":[id],"value":true})).unwrap();
        host.run("transport.record", json!({})).unwrap();
        host.mtc_command(&json!({"online":true})).unwrap();
        host.handle_transport(0.);
        assert_eq!(host.ui.status, crate::localization::text("Take MTC chase offline before recording"));
        assert!(!host.engine.transport.recording);
        host.mtc_command(&json!({"online":false})).unwrap();
        host.run("transport.record", json!({})).unwrap();
        host.handle_transport(0.);
        assert!(host.midi_connected() && host.player.is_none());
        assert!(!host.engine.transport.recording, "a MIDI-only take needs a running audio callback");
        assert!(host.ui.status.contains("Playback Engine"));
    }

    #[test]
    fn online_service_reports_why_a_pending_record_was_refused() {
        let mut host = app();
        let reason = host.run("transport.record", json!({})).unwrap_err();
        host.ui.status.clear();
        host.engine.transport_requests.push(TransportRequest::Record);
        host.service_mtc_chase();
        assert_eq!(host.ui.status, reason);
        assert!(host.engine.transport_requests.is_empty());
        assert!(!host.engine.transport.recording);
    }

    #[test]
    fn before_start_external_code_preserves_position_and_reports_range_status() {
        let mut host = app();
        let feed = host.midi_inputs.as_ref().unwrap().feed().clone();
        let port = feed.ports().assign("Sender");
        host.engine.session_mut().edit.selection = crate::Range::point(48_000);
        host.engine.transport.position = 48_000;
        assert_eq!(host.position(), 48_000);
        feed.receive(port, &[0xf0, 0x7f, 127, 1, 1, 0x20, 59, 50, 0, 0xf7]);
        host.service_mtc_chase();
        host.handle_transport(0.0);
        assert_eq!(host.position(), 48_000);
        let status = host.run("setup.midi_sync_status", json!({})).unwrap();
        assert_eq!(status["mtc_chase"]["state"], "Before session start");
        for frame in [0, 2] {
            // Rolling 25 fps code, still ten seconds before the session.
            for (i, v) in [frame, 0, 2, 3, 11, 3, 0, 2].into_iter().enumerate() {
                feed.receive(port, &[0xf1, (i as u8) << 4 | v]);
            }
        }
        host.service_mtc_chase();
        assert!(host.engine.transport_requests.is_empty());
        assert_eq!(host.position(), 48_000);
        feed.receive(port, &[0xf0, 0x7f, 127, 1, 1, 0x21, 0, 0, 0, 0xf7]);
        host.service_mtc_chase();
        host.handle_transport(0.0);
        assert_eq!(host.position(), 0);
        assert_eq!(host.run("setup.midi_sync_status", json!({})).unwrap()["mtc_chase"]["state"], "Located");
        host.engine.session_mut().timecode_start = 0;
        feed.receive(port, &[0xf0, 0x7f, 127, 1, 1, 0x20, 59, 50, 0, 0xf7]);
        host.service_mtc_chase();
        host.handle_transport(0.0);
        assert_eq!(host.position(), host.engine.session().sample_rate.samples(3590.));
    }

    #[test]
    fn offset_sign_follows_session_setup_semantics_with_absolute_incoming_diagnostics() {
        let mut host = app();
        let feed = host.midi_inputs.as_ref().unwrap().feed().clone();
        let port = feed.ports().assign("Sender");
        for (offset, expected) in [("+00:00:00:00", None), ("+01:00:00:00", Some(480000)), ("-01:00:00:00", None)] {
            host.run("setup.external_timecode_offset", json!({"timecode":offset})).unwrap();
            feed.receive(port, &[0xf0, 0x7f, 127, 1, 1, 0x60, 0, 10, 0, 0xf7]);
            host.service_mtc_chase();
            host.handle_transport(0.);
            let status = host.run("setup.midi_sync_status", json!({})).unwrap();
            if let Some(pos) = expected {
                assert_eq!(host.position(), pos);
            } else {
                assert_eq!(status["mtc_chase"]["state"], "Before session start");
            }
        }
        let final_status = host.run("setup.midi_sync_status", json!({})).unwrap();
        assert_eq!(final_status["mtc_chase"]["incoming_timecode"], "00:00:10:00");
    }

    #[test]
    fn unsupported_session_rates_refuse_generator_and_reader_with_one_message() {
        let mut host = app();
        host.run("setup.mtc", json!({"online":false})).unwrap();
        host.engine.session_mut().frame_rate = soundcraft_time::FrameRate::Fps50;
        let gen_error = host.run("setup.mtc_output", json!({"enabled":true,"port":"Generator"})).unwrap_err();
        let online_error = host.run("options.transport_online", json!({"value":true})).unwrap_err();
        assert_eq!(gen_error, online_error);
        assert!(gen_error.contains("29.97 non-drop"));
    }

    #[test]
    fn ctrl_j_toggles_transport_online_and_the_button_blinks_while_waiting() {
        use egui_kittest::{
            Harness,
            kittest::{NodeT, Queryable},
        };
        let mut host = app();
        host.mtc_command(&json!({"online":false})).unwrap();
        host.ui.show_transport = true;
        let mut h = Harness::builder().with_context_setup(crate::framework::install_headless).with_size(egui::vec2(1400., 900.)).build_ui_state(
            |ui, app: &mut SoundApp| {
                app.logic(ui.ctx());
                app.ui(ui);
            },
            host,
        );
        h.ctx.set_os(egui::os::OperatingSystem::Windows);
        h.run_steps(3);
        let cmd = egui::Modifiers { ctrl: true, command: true, ..Default::default() };
        h.key_press_modifiers(cmd, egui::Key::J);
        h.run_steps(2);
        assert!(h.state().mtc_online);
        let lit = |h: &Harness<SoundApp>| {
            let node = h.get_by_label("Online");
            node.accesskit_node().is_selected() == Some(true) || node.accesskit_node().toggled() == Some(egui::accesskit::Toggled::True)
        };
        let mut phases = [false; 2];
        for _ in 0..75 {
            h.run_steps(1);
            phases[usize::from(lit(&h))] = true;
        }
        assert_eq!(phases, [true, true]);
        h.state_mut().mtc_status = "Locked";
        for _ in 0..35 {
            h.run_steps(1);
            assert!(lit(&h));
        }
        h.key_press_modifiers(cmd, egui::Key::J);
        h.run_steps(2);
        assert!(!h.state().mtc_online);
    }

    #[test]
    fn diagnostics_expose_reader_counters_before_any_timecode_arrives() {
        let mut host = app();
        let status = host.run("setup.midi_sync_status", json!({})).unwrap();
        for key in ["locks", "freewheel_entries", "losses", "hard_relocates", "input_jitter_p95_us"] {
            assert_eq!(status["mtc_chase"][key], 0, "{key}");
        }
    }

    #[test]
    fn chase_preferences_follow_the_document_when_switching_sessions() {
        let mut host = app();
        host.run("setup.mtc", json!({"freewheel_frames":12,"jam_sync":true})).unwrap();
        let saved = host.engine.session().clone();
        host.run("session.new", json!({})).unwrap();
        let state = host.run("setup.mtc", json!({})).unwrap();
        assert_eq!(state["freewheel_frames"], 8);
        assert_eq!(state["jam_sync"], false);
        assert_eq!(state["freewheel"], "frames");
        host.engine.replace_session(saved);
        assert_eq!(host.run("setup.mtc", json!({})).unwrap()["freewheel_frames"], 12);
        assert_eq!(host.run("setup.mtc", json!({})).unwrap()["jam_sync"], true);
        assert_eq!(host.run("setup.mtc", json!({})).unwrap()["freewheel"], "jam_sync");
    }

    #[test]
    fn counter_and_selection_report_before_start_without_moving() {
        let mut host = SoundApp::new(Default::default(), None, crate::Services::default());
        host.run("edit.select", json!({"start":100,"end":200})).unwrap();
        let selection = host.engine.session().edit.selection;
        for (cmd, args) in [("transport.locate", json!({"at":"00:00:20:00"})), ("edit.select", json!({"start":"00:00:20:00"}))] {
            assert!(host.run(cmd, args).is_err());
            assert!(host.ui.status.contains("01:00:00:00"), "{}", host.ui.status);
            assert_eq!(host.engine.session().edit.selection, selection);
        }
    }

    fn send_cycle(feed: &MidiFeed, port: u8, frame: u8) {
        // 25 fps, 01:00:01:ff.
        for (n, piece) in [frame & 15, frame >> 4, 1, 0, 0, 0, 1, 2].into_iter().enumerate() {
            feed.receive(port, &[0xf1, (n as u8) << 4 | piece]);
        }
    }

    #[test]
    fn mtc_chase_locates_from_session_start_and_follows_without_restarting() {
        let mut app = app();
        let feed = app.midi_inputs.as_ref().unwrap().feed().clone();
        let src = feed.ports().assign("Sender");
        let sr = app.engine.session().sample_rate;
        let locate = [0xf0, 0x7f, 0x7f, 1, 1, 0x21, 0, 1, 0, 0xf7];
        feed.receive(src, &locate);
        app.service_mtc_chase();
        app.handle_transport(0.0);
        assert_eq!(app.position(), sr.samples(1.0));
        assert!(!app.is_playing());
        send_cycle(&feed, src, 0);
        app.service_mtc_chase();
        assert!(app.engine.transport_requests.is_empty());
        send_cycle(&feed, src, 2);
        app.service_mtc_chase();
        assert!(matches!(app.engine.transport_requests.as_slice(), [TransportRequest::PlayFrom(at)] if *at == sr.samples(1.0)));
        app.handle_transport(0.0);
        assert!(app.is_playing());
        app.engine.transport.position = sr.samples(1.24);
        send_cycle(&feed, src, 4);
        app.service_mtc_chase();
        assert!(app.engine.transport_requests.is_empty());
        app.engine.transport.position = 0;
        send_cycle(&feed, src, 6);
        app.service_mtc_chase();
        assert!(app.engine.transport_requests.is_empty(), "one group does not cause a drift relocate");
        app.handle_transport(0.0);
        app.engine.session_mut().edit.insertion_follows_playback = true;
        feed.receive(src, &locate);
        app.service_mtc_chase();
        app.handle_transport(0.0);
        assert!(!app.is_playing());
        assert_eq!(app.position(), sr.samples(1.0));
    }

    #[test]
    fn first_audio_block_starts_at_the_confirmed_five_second_locate() {
        let mut app = app();
        let feed = app.midi_inputs.as_ref().unwrap().feed().clone();
        let src = feed.ports().assign("Sender");
        let onset = app.engine.session().sample_rate.samples(5.0);
        // A sender may label its first two groups 05:02 and 05:04 after locating to 05:00.
        feed.receive(src, &[0xf0, 0x7f, 0x7f, 1, 1, 0x61, 0, 5, 0, 0xf7]);
        app.service_mtc_chase();
        app.handle_transport(0.0);
        for frame in [2, 4] {
            for (i, nibble) in [frame, 0, 5, 0, 0, 0, 1, 6].into_iter().enumerate() {
                feed.receive(src, &[0xf1, (i as u8) << 4 | nibble]);
            }
            app.service_mtc_chase();
        }
        // A later PlayFrom skips the note-on at sample 240000; an earlier one can replay notes.
        assert!(
            matches!(app.engine.transport_requests.as_slice(), [TransportRequest::PlayFrom(at)] if *at == onset),
            "expected {onset}, got {:?}",
            app.engine.transport_requests
        );
    }

    #[test]
    fn mtc_chase_input_recovers_and_online_mode_refuses_recording() {
        let mut app = app();
        let feed = app.midi_inputs.as_ref().unwrap().feed().clone();
        let src = feed.ports().assign("Sender");
        send_cycle(&feed, src, 0);
        send_cycle(&feed, src, 2);
        app.service_mtc_chase();
        app.handle_transport(0.0);
        assert!(app.is_playing());
        let inputs = app.midi_inputs.as_mut().unwrap();
        inputs.set_port_names(vec![]);
        inputs.rescan();
        app.service_mtc_chase();
        assert!(!app.is_playing());
        assert_eq!(app.mtc_status, "Input unavailable");
        let inputs = app.midi_inputs.as_mut().unwrap();
        inputs.set_port_names(vec!["Sender".into()]);
        inputs.rescan();
        app.service_mtc_chase();
        assert_eq!(app.mtc_status, "Waiting for timecode");
        send_cycle(&feed, src, 4);
        app.service_mtc_chase();
        assert!(app.engine.transport_requests.is_empty());
        send_cycle(&feed, src, 6);
        app.engine.transport_requests.push(TransportRequest::Record);
        app.service_mtc_chase();
        assert!(app.mtc_online);
        assert!(!app.engine.transport_requests.iter().any(|r| matches!(r, TransportRequest::Record)));
        assert!(app.run("transport.record", json!({})).is_err());
        app.run("setup.mtc", json!({"online":false})).unwrap();
        app.ui.gen_mtc = true;
        assert!(app.run("setup.mtc", json!({"online":true})).is_err());
        assert_eq!(app.run("setup.mtc", json!({})).unwrap()["online"], false);
    }

    #[test]
    fn full_frame_and_rolling_labels_use_the_session_pulldown() {
        use soundcraft_time::FrameRate::*;
        for (rate, bits, nominal) in [(Fps29_97, 6, 30), (Fps23_976, 0, 24), (Fps30, 6, 30)] {
            let mut app = app();
            let sr = app.engine.session().sample_rate;
            app.engine.session_mut().frame_rate = rate;
            app.engine.session_mut().timecode_start = rate.samples_at_frame(nominal * 3600, sr);
            let feed = app.midi_inputs.as_ref().unwrap().feed().clone();
            let src = feed.ports().assign("Sender");
            feed.receive(src, &[0xf0, 127, 127, 1, 1, 1 | (bits << 4), 0, 10, 0, 0xf7]);
            app.service_mtc_chase();
            app.handle_transport(0.);
            assert_eq!(app.position(), rate.samples_at_frame(10 * nominal, sr));
            // A different rolling position forces a live lock instead of the stopped anchor.
            for frame in [10, 12] {
                for (i, n) in [frame, 0, 10, 0, 0, 0, 1, bits].into_iter().enumerate() {
                    feed.receive(src, &[0xf1, ((i as u8) << 4) | n]);
                }
            }
            app.service_mtc_chase();
            app.handle_transport(0.);
            let expected = rate.samples_at_frame(nominal * 10, sr) + (13.75 * rate.samples_per_frame(sr)).round() as i64;
            assert!(app.position().abs_diff(expected) < sr.samples(0.005) as u64);
        }
    }

    #[test]
    fn online_command_and_transport_share_validation_and_manual_controls_leave_chase() {
        let mut app = app();
        assert_eq!(app.run("options.transport_online", json!({})).unwrap()["value"], false);
        assert_eq!(app.mtc_state()["online"], false);
        app.run("options.transport_online", json!({"value":true})).unwrap();
        assert!(app.mtc_online);
        app.engine.transport_requests.push(TransportRequest::HalfSpeed);
        app.service_mtc_chase();
        assert!(!app.mtc_online);
        app.run("setup.mtc", json!({"input":null})).unwrap();
        assert!(app.run("options.transport_online", json!({"value":true})).is_err());
    }
}
