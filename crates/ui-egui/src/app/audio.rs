use super::{MeterDisplay, OutputTrouble, Player, SoundApp, Trouble};

impl crate::UnavailableOutput {
    fn recovered(&self, valid_reports: u64) -> bool {
        self.stalled_at_report.is_some_and(|at| at != valid_reports)
    }
}

impl SoundApp {
    /// Whether the meters are live: while playing, and while armed or input-monitored tracks are heard.
    pub(crate) fn meters_live(&self) -> bool {
        self.engine.transport.playing
            || (self.recorder.is_some() && self.engine.session().tracks.iter().any(|t| t.mixer.input_monitor || t.mixer.record_arm))
            || (self.midi_connected() && self.engine.session().tracks.iter().any(|t| t.kind.is_midi() && soundcraft_mix::midi_thru(t)))
    }

    /// Reopens the audio devices with `cfg` (Setup › Playback Engine › Apply), keeping the
    /// transport position, and keeps it as `ui.audio`. Plugin instances and their editors are
    /// recreated with the new player. Refused while recording, which would lose the take; `ui.audio`
    /// then stays the configuration of the open device.
    pub fn apply_audio_config(&mut self, cfg: soundcraft_playback::AudioConfig) -> Result<(), String> {
        if self.engine.transport.recording {
            return Err(crate::localization::text("stop recording before changing the audio device").into());
        }
        let changed_output = self.ui.audio.system != cfg.system || self.ui.audio.output != cfg.output;
        self.ui.audio = cfg;
        let at = self.position();
        if let Some(p) = &self.player {
            p.stop();
            // The old player's plugin instances go away with it; their editor windows must be
            // closed here, on the main thread, or they would stay on screen orphaned.
            p.close_editors();
        }
        // The input reopens on the next record or monitor with the new choice.
        self.recorder = None;
        self.recorder_failed = false;
        let player = soundcraft_playback::Player::with_config(self.engine.session_arc(), &self.ui.audio);
        self.applied_output_status(changed_output, player.silent);
        self.attach_midi(&player);
        player.locate(at);
        self.player = Some(player);
        self.applied_mtc_output = None;
        self.service_mtc_output();
        self.applied_midi_sync = None;
        self.service_midi_sync();
        // The new engine's MIDI output starts with no ports.
        self.applied_midi_output = None;
        Ok(())
    }

    /// Open the input device once (for recording and input monitoring) and feed the player.
    pub(super) fn ensure_input(&mut self) {
        if self.recorder.is_some() || self.recorder_failed || self.player.is_none() {
            return;
        }
        match soundcraft_playback::record::Recorder::open_with(&self.ui.audio) {
            Ok(r) => {
                if let Some(p) = &self.player {
                    p.set_input(std::sync::Arc::clone(&r.monitor));
                }
                self.recorder = Some(r);
            }
            Err(e) => {
                self.recorder_failed = true;
                self.ui.status = crate::localized_format!("No audio input: {e}", e = e);
            }
        }
    }

    /// Stop the transport, keeping a take in progress, when the audio output fails.
    pub(super) fn watch_output(&mut self) {
        let safe_to_switch = !self.transport_active_for_reconnect();
        if self.player.as_mut().is_some_and(|p| p.service_output_recovery(safe_to_switch)) {
            self.clear_recovered_output();
        }
        let Some(p) = &self.player else { return };
        let Some(trouble) = p.trouble() else {
            if self.unavailable_output.as_ref().is_some_and(|o| o.recovered(p.reported_output_timing_count())) {
                self.clear_recovered_output();
            }
            return;
        };
        let name = p.device_name.clone();
        let valid_reports = p.reported_output_timing_count();
        self.report_output_trouble(trouble, name, valid_reports);
    }

    fn applied_output_status(&mut self, changed: bool, silent: bool) {
        if changed {
            self.clear_recovered_output();
        }
        self.reconnected_output_status(self.ui.audio.output.clone(), self.ui.audio.system.clone(), silent);
    }

    fn reconnected_output_status(&mut self, name: Option<String>, system: Option<String>, silent: bool) {
        if !silent {
            self.clear_recovered_output();
        } else if let Some(fault) = &mut self.unavailable_output {
            // A new silent clock cannot clear the original warning with timing from an unrelated stream.
            fault.stalled_at_report = None;
        } else if let Some(name) = name {
            self.unavailable_output = Some(crate::UnavailableOutput { name, system, stalled_at_report: None, idle_status: None });
        }
    }

    fn recovery_config(&self) -> soundcraft_playback::AudioConfig {
        let mut cfg = self.ui.audio.clone();
        let host = self.unavailable_output.as_ref().and_then(|o| o.system.as_deref()).or_else(|| self.player.as_ref().and_then(|p| p.device_system));
        if let Some(host) = host {
            cfg.system = Some(host.to_owned());
        }
        cfg
    }

    fn clear_recovered_output(&mut self) {
        if let Some(fault) = self.unavailable_output.take()
            && fault.idle_status.as_ref() == Some(&self.ui.status)
        {
            self.ui.status.clear();
        }
    }

    fn report_output_trouble(&mut self, trouble: OutputTrouble, name: String, valid_reports: u64) {
        let (cause, then) = match trouble {
            OutputTrouble::Bug => (
                crate::localization::text("the audio engine hit an internal error (see the log)").to_string(),
                crate::localization::text("Use Reconnect audio device in Setup › Playback Engine, or restart SoundCraft."),
            ),
            OutputTrouble::Gone => (
                format!("“{}” {}", name, crate::localization::text("is unavailable")),
                crate::localization::text("Check the audio device, then use Reconnect audio device in Setup › Playback Engine."),
            ),
            OutputTrouble::Stalled => (
                format!("“{}” {}", name, crate::localization::text("stopped asking for audio")),
                crate::localization::text("Use Reconnect audio device in Setup › Playback Engine."),
            ),
        };
        let (recording, playing) = (self.engine.transport.recording, self.engine.transport.playing);
        if recording || playing {
            self.stop_play();
        }
        self.ui.status = if recording {
            crate::localized_format!("Recording stopped: {cause} The take is kept up to there. {then}", cause = cause, then = then)
        } else if playing {
            crate::localized_format!("Playback stopped: {cause} {then}", cause = cause, then = then)
        } else {
            crate::localized_format!("Audio output: {cause} {then}", cause = cause, then = then)
        };
        if matches!(trouble, OutputTrouble::Gone | OutputTrouble::Stalled) {
            self.unavailable_output = Some(crate::UnavailableOutput {
                name,
                system: self.player.as_ref().and_then(|p| p.device_system).map(str::to_owned),
                stalled_at_report: (trouble == OutputTrouble::Stalled).then_some(valid_reports),
                idle_status: (!recording && !playing).then(|| self.ui.status.clone()),
            });
        }
    }

    /// Stop recording, keeping the take, when the input device or the disk fails, and say as soon
    /// as a slow disk loses input.
    pub(super) fn watch_input(&mut self) {
        let recording = self.engine.transport.recording;
        let Some(r) = &mut self.recorder else { return };
        let Some(trouble) = r.trouble() else {
            let lost = r.lost_ms();
            if recording && lost > self.lost_shown {
                self.lost_shown = lost;
                self.ui.status = crate::localized_format!("The disk is too slow: this take has lost {lost} ms of input so far.", lost = lost);
            }
            return;
        };
        if recording {
            self.stop_play();
        }
        let (what, then) = match trouble {
            Trouble::Device(what) => {
                self.recorder = None;
                self.recorder_failed = true;
                (what, crate::localization::text("Reconnect it, then use Reconnect audio device in Setup › Playback Engine."))
            }
            Trouble::Disk(what) => (what, crate::localization::text("Check the disk, then record again.")),
        };
        self.ui.status = if recording {
            crate::localized_format!("Recording stopped: {what} The take is kept up to there. {then}", what = what, then = then)
        } else {
            crate::localized_format!("Audio input: {what} {then}", what = what, then = then)
        };
    }

    fn recovery_output(&self) -> Option<String> {
        self.unavailable_output.as_ref().map(|o| o.name.clone()).or_else(|| {
            let p = self.player.as_ref()?;
            p.output_timing_issue().map(|_| p.device_name.clone())
        })
    }

    fn transport_active_for_reconnect(&self) -> bool {
        // The UI polls device health before it dispatches transport commands in this frame.
        self.is_playing()
            || self.engine.transport.recording
            || !self.engine.transport_requests.is_empty()
            || self.player.as_ref().is_some_and(Player::is_playing)
            || (self.mtc_online && self.midi_inputs.as_ref().is_some_and(|inputs| inputs.feed().mtc_transport_active()))
    }

    pub(crate) fn timing_reconnect_allowed(&self) -> bool {
        !self.transport_active_for_reconnect() || self.recovery_output().is_none()
    }

    pub(super) fn show_output_warning(&mut self, ui: &mut egui::Ui) {
        let timing = self.player.as_ref().and_then(Player::output_timing_issue);
        let untimed = timing.is_none() && self.midi_output_untimed();
        let Some(message) = output_warning(self.unavailable_output.as_ref().map(|o| o.name.as_str()), timing, untimed) else { return };
        egui::Panel::top("output_timing_warning").show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.colored_label(ui.visuals().warn_fg_color, message);
                if ui.button(crate::localization::text("Playback Engine")).clicked() {
                    self.ui.show_playback_engine = true;
                }
            });
        });
    }

    /// Reopen the audio devices, keeping a take in progress.
    pub fn reconnect_audio(&mut self) {
        let required = self.recovery_output();
        let config = self.recovery_config();
        if required.is_some() && self.transport_active_for_reconnect() {
            return;
        }
        if self.is_playing() || self.engine.transport.recording {
            self.stop_play();
        }
        let at = self.position();
        self.capture_plugin_states();
        if let Some(p) = self.player.take() {
            // Its plugin editors go with it; closed here, on the main thread (see `apply_audio_config`).
            p.close_editors();
        }
        self.recorder = None;
        self.recorder_failed = false;
        let session = self.engine.session_arc();
        let player = match required {
            Some(name) => {
                let player = Player::reconnect_output(session, &config, &name);
                self.reconnected_output_status(Some(name), config.system, player.silent);
                player
            }
            None => Player::with_config(session, &self.ui.audio),
        };
        self.attach_midi(&player);
        player.locate(at);
        self.player = Some(player);
        self.applied_mtc_output = None;
        self.service_mtc_output();
        self.applied_midi_sync = None;
        self.service_midi_sync();
        // The new engine's MIDI output starts with no ports.
        self.applied_midi_output = None;
    }
}

/// The output warning bar's text. `untimed`: the tracks' MIDI output goes uncompensated because
/// the audio device reports no output timing (`midi_output_untimed`).
fn output_warning(unavailable: Option<&str>, issue: Option<soundcraft_playback::OutputTimingIssue>, untimed: bool) -> Option<String> {
    use soundcraft_playback::OutputTimingIssue;
    if let Some(name) = unavailable {
        return Some(crate::localized_format!("Audio output “{name}” is unavailable. Reconnect it in Playback Engine when stopped.", name = name));
    }
    if issue.is_none() && untimed {
        return Some(format!(
            "{} {}",
            crate::localization::text(crate::midi_sync::UNTIMED),
            crate::localization::text("Check the audio device, then use Reconnect audio device in Setup › Playback Engine.")
        ));
    }
    match issue? {
        OutputTimingIssue::Retained { lead_us } => Some(crate::localized_format!(
            "The audio driver stopped reporting output timing. Compensated MIDI outputs use the last measured lead ({ms} ms).",
            ms = format!("{:.2}", lead_us as f64 / 1000.)
        )),
        OutputTimingIssue::Unknown => Some(format!(
            "{} {}",
            crate::localization::text("Audio output timing is unavailable. Compensated MIDI outputs need a valid measurement."),
            crate::localization::text("Check the audio device, then use Reconnect audio device in Setup › Playback Engine.")
        )),
    }
}

pub(super) fn feed_meter(d: &mut MeterDisplay, peak: [f32; 2], gr: f32, dt: f32) {
    // Instant attack, ~26 dB/s release, 2 s peak hold.
    let fall = 10f32.powf(-26.0 * dt / 20.0);
    for c in 0..2 {
        let v = peak.get(c).copied().unwrap_or(0.0);
        let lvl = d.level.get(c).copied().unwrap_or(0.0);
        let nv = if v >= lvl { v } else { (lvl * fall).max(v) };
        if let Some(x) = d.level.get_mut(c) {
            *x = if nv < 1e-5 { 0.0 } else { nv };
        }
        if v >= d.hold.get(c).copied().unwrap_or(0.0) {
            if let Some(h) = d.hold.get_mut(c) {
                *h = v;
            }
            d.hold_age = 0.0;
        }
        if v >= 1.0 {
            d.clip = true;
        }
    }
    d.hold_age += dt;
    if d.hold_age > 2.0 {
        d.hold = d.level;
    }
    d.gr = if gr > d.gr { gr } else { d.gr * fall };
}

/// Whether a track needs the audio input opened: an audio track (or an aux) that is
/// record-enabled or input-monitored. A MIDI or instrument track listens to MIDI devices, so
/// arming one does not open an audio device or complain that there is none.
pub(super) fn wants_audio_input(s: &soundcraft_model::Session) -> bool {
    // An inactive track keeps its record and monitor buttons but takes no input.
    s.tracks.iter().any(|t| !t.kind.is_midi() && !t.inactive && (t.mixer.input_monitor || t.mixer.record_arm))
}

#[cfg(test)]
mod audio_input_tests {
    use super::wants_audio_input;
    use soundcraft_model::{ChannelFormat, Session, TrackKind};

    #[test]
    fn only_audio_tracks_ask_for_the_audio_input() {
        let mut s = Session::default();
        let synth = s.add_track(TrackKind::Instrument, ChannelFormat::Stereo, None);
        let midi = s.add_track(TrackKind::Midi, ChannelFormat::Stereo, None);
        let vox = s.add_track(TrackKind::Audio, ChannelFormat::Mono, None);
        assert!(!wants_audio_input(&s));
        s.track_mut(synth).unwrap().mixer.record_arm = true;
        s.track_mut(midi).unwrap().mixer.input_monitor = true;
        assert!(!wants_audio_input(&s), "armed and monitored MIDI tracks listen to MIDI devices");
        s.track_mut(vox).unwrap().mixer.input_monitor = true;
        assert!(wants_audio_input(&s));
        s.track_mut(vox).unwrap().mixer.input_monitor = false;
        s.track_mut(vox).unwrap().mixer.record_arm = true;
        assert!(wants_audio_input(&s));
    }

    #[test]
    fn an_inactive_audio_track_does_not_ask_for_the_audio_input() {
        let mut s = Session::default();
        let vox = s.add_track(TrackKind::Audio, ChannelFormat::Mono, None);
        let track = s.track_mut(vox).unwrap();
        track.mixer.record_arm = true;
        track.mixer.input_monitor = true;
        track.inactive = true;
        assert!(!wants_audio_input(&s), "an inactive track keeps its buttons but records and monitors nothing");
    }
}

#[cfg(test)]
mod timing_warning_tests {
    use super::*;

    #[test]
    fn online_locate_preserves_the_cursor_and_first_quarter_defers_stream_switch() {
        let mut app = SoundApp::new(crate::Engine::default(), None, crate::Services::default());
        app.engine.session_mut().timecode_start = 0;
        app.midi_inputs = Some(soundcraft_playback::midi::MidiInputs::with_port_names(vec![], vec!["Timecode".into()]));
        app.run("setup.mtc", serde_json::json!({"input":"Timecode","online":true})).unwrap();
        app.service_mtc_chase();
        let feed = app.midi_inputs.as_ref().unwrap().feed().clone();
        let port = feed.ports().assign("Timecode");
        feed.receive(port, &[0xf0, 0x7f, 0x7f, 1, 1, 0x60, 0, 5, 0, 0xf7]);
        app.service_mtc_chase();
        app.handle_transport(0.);
        let at = app.engine.session().sample_rate.samples(5.);
        assert_eq!(app.position(), at);
        assert!(app.mtc_online && !app.transport_active_for_reconnect());

        feed.receive(port, &[0xf1, 0]);
        assert!(app.transport_active_for_reconnect(), "defer during acquisition, even before the UI sees Roll");
        assert!(!app.is_playing());
        feed.receive(port, &[0xfc]);
        assert!(!app.transport_active_for_reconnect());
        assert!(app.mtc_online);
        assert_eq!(app.position(), at);
    }

    #[test]
    fn pending_transport_commands_defer_output_recovery_until_the_host_handles_them() {
        use soundcraft_engine::TransportRequest;
        let mut app = SoundApp::new(crate::Engine::default(), None, crate::Services::default());
        app.engine.session_mut().edit.selection = crate::Range::point(240_000);
        for request in [TransportRequest::Play, TransportRequest::PlayFrom(96_000), TransportRequest::TogglePlay, TransportRequest::Record] {
            app.engine.transport_requests.push(request);
            assert!(app.transport_active_for_reconnect(), "a queued transport operation must complete before the stream is replaced");
            assert_eq!(app.engine.transport_requests.len(), 1);
            assert_eq!(app.position(), 240_000);
            app.engine.transport_requests.clear();
        }
        assert!(!app.transport_active_for_reconnect());
    }

    #[test]
    fn applied_selection_retargets_a_failed_open_including_equal_names_on_other_hosts() {
        let mut app = SoundApp::new(crate::Engine::default(), None, crate::Services::default());
        app.report_output_trouble(OutputTrouble::Gone, "A".into(), 0);
        for (name, host) in [("B", "WASAPI"), ("B", "ASIO")] {
            app.ui.audio.output = Some(name.into());
            app.ui.audio.system = Some(host.into());
            app.applied_output_status(true, true);
            let fault = app.unavailable_output.as_ref().unwrap();
            assert_eq!(fault.name, name);
            assert_eq!(fault.system.as_deref(), Some(host));
            assert!(!app.ui.status.contains("“A”"));
        }
        app.ui.audio.output = None;
        app.applied_output_status(true, true);
        assert!(app.recovery_output().is_none(), "retry the current default");
    }

    #[test]
    fn manual_retry_and_apply_clear_only_the_matching_idle_warning() {
        let mut app = SoundApp::new(crate::Engine::default(), None, crate::Services::default());
        for apply in [false, true] {
            app.report_output_trouble(OutputTrouble::Gone, "A".into(), 0);
            app.reconnected_output_status(Some("A".into()), None, true);
            assert_eq!(app.unavailable_output.as_ref().unwrap().idle_status.as_ref(), Some(&app.ui.status));
            if apply {
                app.applied_output_status(false, false);
            } else {
                app.reconnected_output_status(Some("A".into()), None, false);
            }
            assert!(app.unavailable_output.is_none() && app.ui.status.is_empty());
        }
        app.report_output_trouble(OutputTrouble::Gone, "A".into(), 0);
        app.ui.status = "Saved".into();
        app.reconnected_output_status(Some("A".into()), None, false);
        assert_eq!(app.ui.status, "Saved");
    }

    #[test]
    fn reconnect_uses_the_original_open_host_after_falling_back() {
        let mut app = SoundApp::new(crate::Engine::default(), None, crate::Services::default());
        app.ui.audio.system = Some("initially unavailable host".into());
        app.ui.audio.input = Some("microphone".into());
        app.report_output_trouble(OutputTrouble::Gone, "USB".into(), 0);
        app.unavailable_output.as_mut().unwrap().system = Some("WASAPI".into());
        let retry = app.recovery_config();
        assert_eq!(retry.system.as_deref(), Some("WASAPI"));
        assert_eq!(retry.input, app.ui.audio.input);
        assert_eq!(app.ui.audio.system.as_deref(), Some("initially unavailable host"));
    }

    #[test]
    fn recovered_output_dismisses_only_the_current_idle_warning() {
        let mut app = SoundApp::new(crate::Engine::default(), None, crate::Services::default());
        for trouble in [OutputTrouble::Gone, OutputTrouble::Stalled] {
            app.report_output_trouble(trouble, "Model 12".into(), 0);
            assert!(app.ui.status.contains("Reconnect"));
            app.clear_recovered_output();
            assert!(app.ui.status.is_empty(), "recovered output still shows: {}", app.ui.status);
            assert!(app.unavailable_output.is_none());
            app.report_output_trouble(trouble, "Model 12".into(), 1);
            app.ui.status = "Saved session".into();
            app.clear_recovered_output();
            assert_eq!(app.ui.status, "Saved session");
        }
    }

    #[test]
    fn recovered_output_preserves_the_interrupted_take_status() {
        let mut app = SoundApp::new(crate::Engine::default(), None, crate::Services::default());
        app.engine.transport.recording = true;
        app.report_output_trouble(OutputTrouble::Gone, "Model 12".into(), 0);
        let interrupted = app.ui.status.clone();
        assert!(interrupted.contains("The take is kept"));
        app.clear_recovered_output();
        assert_eq!(app.ui.status, interrupted);
        assert!(app.unavailable_output.is_none());
    }

    #[test]
    fn storage_loss_warning_stays_visible_after_other_status_updates() {
        let mut app = SoundApp::new(crate::Engine::default(), None, crate::Services::default());
        app.report_output_trouble(OutputTrouble::Gone, "Previous USB output".into(), 4);
        assert!(app.ui.status.contains("Previous USB output"));
        app.ui.status = "Saved".into();
        let ctx = egui::Context::default();
        crate::framework::install_headless(&ctx);
        let mut found = false;
        app.set_system_fonts(false);
        for _ in 0..4 {
            let mut raw =
                egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1300., 800.))), ..Default::default() };
            app.raw_input_hook(&ctx, &mut raw);
            let mut output = ctx.run_ui(raw, |ui| {
                app.logic(ui.ctx());
                app.ui(ui);
            });
            output.textures_delta.clear();
            found |= output.shapes.iter().any(|s| matches!(&s.shape, egui::Shape::Text(t) if t.galley.text().contains("Previous USB output")));
        }
        assert!(found, "The visible device warning disappeared after an unrelated status update");
    }

    #[test]
    fn cold_and_cached_timing_have_distinct_warnings_and_recovered_timing_clears_them() {
        use soundcraft_playback::OutputTimingIssue;
        let unknown = output_warning(None, Some(OutputTimingIssue::Unknown), false).unwrap();
        assert!(unknown.contains("need a valid measurement"));
        assert!(unknown.contains("Reconnect audio device in Setup › Playback Engine"), "{unknown}");
        for (lead_us, ms) in [(0, "0.00 ms"), (75_430, "75.43 ms")] {
            let cached = output_warning(None, Some(OutputTimingIssue::Retained { lead_us }), false).unwrap();
            assert!(cached.contains("last measured lead") && cached.contains(ms));
        }
        assert!(output_warning(None, None, false).is_none());
        assert!(output_warning(Some("Disconnected"), None, false).unwrap().contains("Disconnected"));
        // The tracks' output went uncompensated: the bar says so and how to get timing back.
        let untimed = output_warning(None, None, true).unwrap();
        assert!(untimed.contains("not delay-compensated") && untimed.contains("Reconnect audio device"), "{untimed}");
    }

    #[test]
    fn callback_stall_clears_only_after_a_fresh_measurement_and_a_gone_stream_needs_reopen() {
        let mut app = SoundApp::new(crate::Engine::default(), None, crate::Services::default());
        app.report_output_trouble(OutputTrouble::Stalled, "USB".into(), 7);
        let stalled = app.unavailable_output.as_ref().unwrap();
        assert!(!stalled.recovered(7));
        assert!(stalled.recovered(8));
        app.report_output_trouble(OutputTrouble::Gone, "USB".into(), 8);
        assert!(!app.unavailable_output.as_ref().unwrap().recovered(9));
    }

    #[test]
    fn timing_recovery_during_record_or_play_leaves_the_take_and_routing_untouched() {
        let mut app = SoundApp::new(crate::Engine::default(), None, crate::Services::default());
        app.ui.audio.output = Some("Output A".into());
        app.ui.audio.input = Some("Input B".into());
        app.engine.session_mut().edit.selection = crate::Range::point(66_150);
        app.unavailable_output = Some(crate::UnavailableOutput { name: "Output A".into(), system: None, stalled_at_report: None, idle_status: None });
        let before = app.engine.session_arc();
        for (playing, recording) in [(true, false), (true, true), (false, true)] {
            app.engine.transport.playing = playing;
            app.engine.transport.recording = recording;
            assert!(!app.timing_reconnect_allowed());
            app.reconnect_audio();
            assert_eq!((app.engine.transport.playing, app.engine.transport.recording), (playing, recording));
            assert!(std::sync::Arc::ptr_eq(&before, &app.engine.session_arc()));
            assert!(app.player.is_none());
            assert_eq!(app.ui.audio.input.as_deref(), Some("Input B"));
            assert_eq!(app.ui.audio.output.as_deref(), Some("Output A"));
        }
        app.engine.transport.playing = false;
        app.engine.transport.recording = false;
        assert!(app.timing_reconnect_allowed());
        assert_eq!(app.position(), 66_150);
    }
}
