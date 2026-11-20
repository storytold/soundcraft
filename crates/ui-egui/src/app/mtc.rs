use crate::{SoundApp, TransportRequest, Value, json};
use soundcraft_playback::mtc_input::{ChaseEvent, Position};

impl SoundApp {
    pub(crate) fn mtc_command(&mut self, p: &Value) -> Result<Value, String> {
        let args = p.as_object().ok_or("setup.mtc expects an object")?;
        if args.is_empty() {
            return Ok(self.mtc_state());
        }
        if args.keys().any(|k| !matches!(k.as_str(), "input" | "online")) {
            return Err("setup.mtc accepts input and online".into());
        }
        let source = match args.get("input") {
            None => self.ui.mtc_input_port.clone(),
            Some(Value::Null) => None,
            Some(Value::String(s)) if !s.trim().is_empty() => Some(s.clone()),
            Some(_) => return Err("input must be a MIDI port name or null".into()),
        };
        let online = match args.get("online") {
            Some(v) => v.as_bool().ok_or("online must be a boolean")?,
            None => self.mtc_online && source.is_some(),
        };
        if online && source.is_none() {
            return Err("Select an MTC input before going online".into());
        }
        if online && self.ui.mtc_output_port.is_some() {
            return Err("Disable MTC output before going online".into());
        }
        if online && self.engine.transport.recording {
            return Err("Stop recording before enabling MTC chase".into());
        }
        if self.ui.mtc_input_port != source || self.mtc_online != online {
            if let Some(m) = &self.midi_inputs {
                m.feed().set_mtc_source(None);
            }
            self.stop_mtc_transport();
            self.ui.mtc_input_port = source;
            self.mtc_online = online;
            self.mtc_status = if online { "Waiting for timecode" } else { "Offline" };
        }
        Ok(self.mtc_state())
    }

    fn mtc_state(&self) -> Value {
        json!({"input": self.ui.mtc_input_port, "online": self.mtc_online, "status": self.mtc_status})
    }

    fn stop_mtc_transport(&mut self) {
        self.set_mtc_playback_rate(1.0);
        if std::mem::take(&mut self.mtc_chasing) && self.is_playing() && !self.engine.transport.recording {
            self.stop_play();
        }
    }

    fn set_mtc_playback_rate(&mut self, rate: f32) {
        if self.mtc_playback_rate != rate {
            self.mtc_playback_rate = rate;
            if let Some(player) = &self.player {
                player.set_speed(rate);
            }
        }
    }

    fn mtc_at(&self, p: Position) -> i64 {
        let session = self.engine.session();
        p.samples(session.sample_rate).saturating_sub(session.timecode_start)
    }

    pub(crate) fn service_mtc_chase(&mut self) {
        if !self.mtc_online {
            return;
        }
        // A local stop, record or document change takes the transport back offline.
        if self.engine.transport.recording
            || self.engine.transport_requests.iter().any(|r| {
                matches!(
                    r,
                    TransportRequest::Record
                        | TransportRequest::ResetSession
                        | TransportRequest::Pause
                        | TransportRequest::Stop
                        | TransportRequest::TogglePlay
                )
            })
        {
            if let Some(m) = &self.midi_inputs {
                m.feed().set_mtc_source(None);
            }
            self.set_mtc_playback_rate(1.0);
            self.mtc_online = false;
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
        inputs.feed().set_mtc_source(source);
        if source.is_none() {
            self.stop_mtc_transport();
            self.mtc_status = "Input unavailable";
            return;
        }
        if self.mtc_status == "Input unavailable" {
            self.mtc_status = "Waiting for timecode";
        }
        match inputs.feed().take_mtc() {
            Some(ChaseEvent::Locate(p)) => {
                self.set_mtc_playback_rate(1.0);
                if self.is_playing() {
                    self.stop_play();
                }
                self.mtc_chasing = false;
                let at = self.mtc_at(p).max(0);
                let _ = self.run("transport.locate", json!({"at": at}));
                self.mtc_status = "Located";
            }
            Some(ChaseEvent::Roll(p, age_us)) => {
                let sr = self.engine.session().sample_rate;
                let at = self.mtc_at(p).saturating_add(sr.samples(age_us as f64 / 1_000_000.0));
                if at < 0 {
                    self.stop_mtc_transport();
                    self.mtc_status = "Before session start";
                    return;
                }
                if !self.is_playing() || !self.mtc_chasing {
                    self.engine.transport_requests.push(TransportRequest::PlayFrom(at));
                } else {
                    // Compare against the audio device's current playhead, not the previous UI repaint.
                    let playing_at = self.player.as_ref().map_or(self.engine.transport.position, |player| player.presented_position());
                    let error = at.saturating_sub(playing_at);
                    let three_frames = p.drift_tolerance(sr);
                    if error.unsigned_abs() > (4 * three_frames) as u64 {
                        self.set_mtc_playback_rate(1.0);
                        self.engine.transport_requests.push(TransportRequest::Locate(at));
                    } else {
                        // Preserve attacks at a located start while making up acquisition latency.
                        // Increase catch-up by at most 0.2 percentage points per complete MTC group.
                        // Taper immediately as the phase error shrinks; an ahead player still slows by at most 2%.
                        let trim =
                            if error.unsigned_abs() <= (three_frames / 48) as u64 { 0.0 } else { (error as f64 / sr.as_f64()).clamp(-0.02, 0.04) };
                        let rate = (1.0 + trim as f32).min(self.mtc_playback_rate.max(1.0) + 0.002);
                        self.set_mtc_playback_rate(rate);
                    }
                }
                self.mtc_chasing = true;
                self.mtc_status = "Locked";
            }
            Some(ChaseEvent::Stop) => {
                self.stop_mtc_transport();
                self.mtc_status = "Waiting for timecode";
            }
            None => {}
        }
    }

    pub(crate) fn mtc_input_settings(&mut self, ui: &mut egui::Ui) {
        let mut selected = self.ui.mtc_input_port.clone();
        craft_ui::ComboBox::from_label(crate::localization::text("MTC input"))
            .selected_text(selected.as_deref().unwrap_or(crate::localization::text("None")))
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut selected, None, crate::localization::text("None"));
                if let Some(m) = &self.midi_inputs {
                    for p in m.ports() {
                        ui.selectable_value(&mut selected, Some(p.name.clone()), &p.name);
                    }
                }
            });
        let mut online = self.mtc_online;
        let changed = crate::widgets::checkbox(ui, &mut online, crate::localization::text("Online (chase MTC)")).changed();
        if selected != self.ui.mtc_input_port || changed {
            let online = online && selected.is_some();
            if let Err(e) = self.mtc_command(&json!({"input":selected,"online":online})) {
                self.ui.status = e;
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
        assert!(matches!(app.engine.transport_requests.as_slice(), [TransportRequest::Locate(_)]));
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
        // The Model 12 starts its first two 30 fps groups at 05:02 and 05:04.
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
    fn located_roll_preserves_the_first_note_interval_and_converges_to_timecode() {
        let mut app = app();
        let feed = app.midi_inputs.as_ref().unwrap().feed().clone();
        let src = feed.ports().assign("Sender");
        let sr = app.engine.session().sample_rate;
        let first = sr.samples(5.0);
        let second = sr.samples(5.5);
        feed.receive(src, &[0xf0, 0x7f, 0x7f, 1, 1, 0x61, 0, 5, 0, 0xf7]);
        app.service_mtc_chase();
        app.handle_transport(0.0);
        let mut elapsed = 0.0;
        let mut second_onset = None;
        let mut seeks = 0;
        for offset in (2..=364).step_by(2) {
            let before = app.position();
            if app.is_playing() {
                app.handle_transport(2.0 / 30.0);
                elapsed += 2.0 / 30.0;
            }
            let seconds = 5 + offset / 30;
            let f = offset % 30;
            for (i, nibble) in [f & 15, f >> 4, seconds & 15, seconds >> 4, 0, 0, 1, 6].into_iter().enumerate() {
                feed.receive(src, &[0xf1, (i as u8) << 4 | nibble as u8]);
            }
            app.service_mtc_chase();
            seeks += app.engine.transport_requests.iter().filter(|r| matches!(r, TransportRequest::Locate(_))).count();
            app.handle_transport(0.0);
            let after = app.position();
            if offset == 4 {
                assert!(app.is_playing());
                assert_eq!(after, first);
            }
            if before < second && after >= second {
                second_onset = Some(elapsed - (after - second) as f64 / (after - before) as f64 * (2.0 / 30.0));
            }
        }
        let interval = second_onset.expect("second note reached");
        assert!((interval - 0.5).abs() < 0.020, "first interval was {interval:.3} s");
        assert_eq!(seeks, 0, "continuous timecode must not skip or replay audio");
        let external = first + sr.samples(366.0 / 30.0);
        assert!(app.position().abs_diff(external) <= sr.samples(1.0 / 30.0) as u64);
    }

    #[test]
    fn located_chase_catches_a_queued_output_without_squeezing_the_first_interval() {
        let mut app = app();
        let feed = app.midi_inputs.as_ref().unwrap().feed().clone();
        let port = feed.ports().assign("Sender");
        let sr = app.engine.session().sample_rate;
        let start = sr.samples(5.0);
        feed.receive(port, &[0xf0, 0x7f, 0x7f, 1, 1, 0x61, 0, 5, 0, 0xf7]);
        app.service_mtc_chase();
        app.handle_transport(0.0);
        let cycle = 2.0 / 30.0;
        // The master's first label is two frames past the locate. Two full groups confirm
        // motion. Model another 80 ms before the output starts advancing at the endpoint.
        let mut queued: f64 = 0.080;
        let mut output_seconds = 0.0;
        let mut first_interval = None;
        for frame in (2..=358).step_by(2) {
            let before = app.position();
            if app.is_playing() {
                let wait = queued.min(cycle);
                queued -= wait;
                let active = cycle - wait;
                app.handle_transport(active as f32);
                let after = app.position();
                let second = start + sr.samples(0.5);
                if before < second && second <= after {
                    first_interval = Some(output_seconds + active * (second - before) as f64 / (after - before) as f64);
                }
                output_seconds += active;
            }
            let sec = 5 + frame / 30;
            let f = frame % 30;
            for (i, nibble) in [f & 15, f >> 4, sec & 15, sec >> 4, 0, 0, 1, 6].into_iter().enumerate() {
                feed.receive(port, &[0xf1, (i as u8) << 4 | nibble as u8]);
            }
            app.service_mtc_chase();
            if frame == 268 {
                assert!(app.position().abs_diff(sr.samples(14.0)) < sr.samples(1.0 / 30.0) as u64);
            }
            assert!(app.mtc_playback_rate <= 1.04, "excessive acquisition speed");
            if frame == 4 {
                assert!(matches!(app.engine.transport_requests.as_slice(), [TransportRequest::PlayFrom(at)] if *at == start));
            } else {
                assert!(app.engine.transport_requests.is_empty(), "unexpected transport jump at frame {frame}");
            }
            app.handle_transport(0.0);
        }
        // At the last group's receipt its compensated label is exactly 17.000 seconds.
        let lag = (sr.samples(17.0) - app.position()) as f64 / sr.as_f64();
        let interval = first_interval.unwrap();
        assert!(lag.abs() <= 1.0 / 60.0, "late by {lag:.4} s at 17 s; first interval {interval:.4} s");
        assert!((interval - 0.5).abs() < 0.010, "first interval {interval:.4} s");
    }

    #[test]
    fn chase_trim_slows_an_ahead_player_and_resets_on_stop() {
        let mut app = app();
        let feed = app.midi_inputs.as_ref().unwrap().feed().clone();
        let src = feed.ports().assign("Sender");
        send_cycle(&feed, src, 0);
        send_cycle(&feed, src, 2);
        app.service_mtc_chase();
        app.handle_transport(0.0);
        let sr = app.engine.session().sample_rate;
        app.engine.transport.position = sr.samples(1.40);
        send_cycle(&feed, src, 4);
        app.service_mtc_chase();
        assert_eq!(app.mtc_playback_rate, 0.98);
        assert!(app.engine.transport_requests.is_empty());
        feed.receive(src, &[0xfc]);
        app.service_mtc_chase();
        assert_eq!(app.mtc_playback_rate, 1.0);
        assert!(!app.is_playing());
    }

    #[test]
    fn mtc_chase_input_loss_recovers_and_record_takes_priority() {
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
        assert!(!app.mtc_online);
        assert!(matches!(app.engine.transport_requests.as_slice(), [TransportRequest::Record]));
        app.ui.mtc_output_port = Some("Sender".into());
        assert!(app.run("setup.mtc", json!({"online":true})).is_err());
        assert_eq!(app.run("setup.mtc", json!({})).unwrap()["online"], false);
    }
}
