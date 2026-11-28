use super::{MeterDisplay, OutputTrouble, Player, SoundApp, Trouble};

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
        self.ui.audio = cfg;
        // The new player starts stopped: so does the transport, as with Reconnect.
        if self.is_playing() {
            self.stop_play();
        }
        self.reopen_audio();
        Ok(())
    }

    /// Open the input device once (for recording and input monitoring) and feed the player. When a
    /// shared ASIO capture fails and takes the output down with it, reopen the output so playback
    /// goes on.
    pub(super) fn ensure_input(&mut self) {
        if self.recorder.is_some() || self.recorder_failed {
            return;
        }
        let Some(player) = &self.player else { return };
        // After an idle ASIO recovery, wait for the output to reopen before attaching the input.
        if player.awaiting_asio_output() {
            return;
        }
        match player.open_input(&self.ui.audio) {
            Ok(r) => {
                player.set_input(std::sync::Arc::clone(&r.monitor));
                self.recorder = Some(r);
            }
            Err(e) => {
                if e.rebuild_output {
                    let at = self.position();
                    self.capture_plugin_states();
                    if self.is_playing() {
                        self.stop_play();
                    }
                    if let Some(player) = self.player.take() {
                        player.close_editors();
                        // The old ASIO callback must be dropped before reopening the device.
                    }
                    let restored = Player::with_config(self.engine.session_arc(), &self.ui.audio);
                    self.attach_midi(&restored);
                    restored.locate(at);
                    self.player = Some(restored);
                }
                self.recorder_failed = true;
                self.ui.status = crate::localized_format!("No audio input: {e}", e = e.message);
            }
        }
    }

    /// Stop the transport, keeping a take in progress, when the audio output fails. An output that
    /// asked for its stream to be rebuilt is reopened on the next frame unless it asked again too
    /// soon (`reopen_after_reset`), and an idle ASIO output whose clock ran too slow is rebuilt
    /// (`Player::recover_idle_asio`), with the status line saying what happened.
    pub(super) fn watch_output(&mut self) {
        if let Some(cause) = self.reopen_pending.take() {
            self.reconnect_audio();
            let silent = self.player.as_ref().is_none_or(|p| p.silent);
            self.ui.status = reset_reopen_status(&cause, silent);
            return;
        }
        if let Some(p) = &mut self.player {
            let outcome = p.recover_idle_asio(
                self.engine.transport.playing,
                self.engine.transport.recording,
                !self.engine.transport_requests.is_empty(),
                || {
                    self.recorder = None;
                    self.recorder_failed = false;
                },
            );
            // Without this the status line keeps the last advice (to reconnect by hand) after the
            // device came back on its own, and says nothing while it is away.
            if let Some(status) = idle_recovery_status(outcome, &p.device_name) {
                self.ui.status = status;
            }
        }
        let Some(p) = &self.player else { return };
        // While the device is away there is no output stream to run the transport: a Play (or a
        // record) started meanwhile stops at once, and the idle recovery keeps trying.
        let (recording, playing) = (self.engine.transport.recording, self.engine.transport.playing);
        if p.awaiting_asio_output() && (recording || playing) {
            let cause = format!("“{}” {}", p.device_name, crate::localization::text("is unavailable"));
            self.stop_play();
            self.ui.status = output_status(recording, playing, &cause, crate::localization::text("SoundCraft reopens it when it is back."));
            return;
        }
        let Some(trouble) = p.trouble() else { return };
        let name = p.device_name.clone();
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
            OutputTrouble::ClockTooSlow => (
                format!("“{}” {}", name, crate::localization::text("stopped providing audio at its configured rate")),
                crate::localization::text("Check the audio device, then use Reconnect audio device in Setup › Playback Engine."),
            ),
            OutputTrouble::Reset => {
                let cause = format!("“{}” {}", name, crate::localization::text("asked to be reopened"));
                let (recording, playing) = (self.engine.transport.recording, self.engine.transport.playing);
                let then = self.reopen_after_reset(&cause);
                self.ui.status = output_status(recording, playing, &cause, then);
                return;
            }
        };
        let (recording, playing) = (self.engine.transport.recording, self.engine.transport.playing);
        if recording || playing {
            self.stop_play();
        }
        if trouble == OutputTrouble::ClockTooSlow {
            // The input shares the failed driver: it reopens with Reconnect, not on its own.
            self.recorder = None;
            self.recorder_failed = true;
        }
        self.ui.status = output_status(recording, playing, &cause, then);
    }

    /// The device asked for its stream to be rebuilt (cpal's `StreamInvalidated` on any audio
    /// system, for example an ASIO driver's reset request after its buffer size changed in the
    /// driver's own settings). Stop the transport, keeping a take in progress up to here, and
    /// reopen the device on the next frame, as Reconnect audio
    /// device does, so that the input of the same device, reporting its reset in this frame, joins
    /// the one reopen. Says what happens next. A device that asks again within [`AUTO_REOPEN_GAP`]
    /// of the last automatic reopen is left to the user, so a driver that keeps resetting cannot
    /// keep the app reopening it.
    fn reopen_after_reset(&mut self, cause: &str) -> &'static str {
        if self.is_playing() || self.engine.transport.recording {
            self.stop_play();
        }
        // The input of a resetting device is gone too; it opens again with the reopened output.
        self.recorder = None;
        self.recorder_failed = true;
        if self.reopen_pending.is_some() {
            return crate::localization::text("SoundCraft reopens it in a moment.");
        }
        let now = std::time::Instant::now();
        if may_reopen(self.auto_reopened, now) {
            self.auto_reopened = Some(now);
            self.reopen_pending = Some(cause.to_string());
            crate::localization::text("SoundCraft reopens it in a moment.")
        } else {
            crate::localization::text("Use Reconnect audio device in Setup › Playback Engine.")
        }
    }

    /// Stop recording, keeping the take, when the input device or the disk fails, and say as soon
    /// as a slow disk loses input. An input that asked for its stream to be rebuilt is reopened
    /// with the output (`reopen_after_reset`).
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
        // Stop first in every case: stopping finishes the take with the recorder still open.
        let then = match &trouble {
            Trouble::Reset(what) => self.reopen_after_reset(what),
            Trouble::Device(_) => {
                if recording {
                    self.stop_play();
                }
                self.recorder = None;
                self.recorder_failed = true;
                crate::localization::text("Reconnect it, then use Reconnect audio device in Setup › Playback Engine.")
            }
            Trouble::Disk(_) => {
                if recording {
                    self.stop_play();
                }
                crate::localization::text("Check the disk, then record again.")
            }
        };
        let (Trouble::Reset(what) | Trouble::Device(what) | Trouble::Disk(what)) = trouble;
        self.ui.status = input_status(recording, &what, then);
    }

    /// Reopen the audio devices, keeping a take in progress.
    pub fn reconnect_audio(&mut self) {
        // A reopen pending after a device reset is done now.
        self.reopen_pending = None;
        if self.is_playing() || self.engine.transport.recording {
            self.stop_play();
        }
        self.reopen_audio();
    }

    /// Open the player again with `ui.audio`, keeping the transport position. The input reopens
    /// on the next record or monitor.
    fn reopen_audio(&mut self) {
        let at = self.position();
        // Keep what open plugin editors changed: the plugins go with the old player.
        self.capture_plugin_states();
        // The device opened here replaces one a device reset would have reopened.
        self.reopen_pending = None;
        self.recorder = None;
        self.recorder_failed = false;
        // Drop the old player, with both its streams, before the device opens again: an ASIO
        // driver is loaded once per process, and dropping the old stream after the new one opened
        // stops the new one too (or reuses a disconnected driver instance). Its plugin editors go
        // with it and are closed here, on the main thread, or they would stay on screen orphaned.
        if let Some(p) = self.player.take() {
            p.stop();
            p.close_editors();
            drop(p);
        }
        let player = Player::with_config(self.engine.session_arc(), &self.ui.audio);
        self.attach_midi(&player);
        player.locate(at);
        self.player = Some(player);
    }
}

/// The shortest time between two automatic reopens after a device reset (see
/// `SoundApp::reopen_after_reset`).
const AUTO_REOPEN_GAP: std::time::Duration = std::time::Duration::from_secs(10);

/// Whether a device reset at `now` may be answered by reopening the device automatically, given
/// the last automatic reopen `last`.
fn may_reopen(last: Option<std::time::Instant>, now: std::time::Instant) -> bool {
    last.is_none_or(|t| now.saturating_duration_since(t) >= AUTO_REOPEN_GAP)
}

/// The status line for an output problem `cause`, with what to do about it (`then`), by what
/// the transport was doing.
fn output_status(recording: bool, playing: bool, cause: &str, then: &str) -> String {
    if recording {
        crate::localized_format!("Recording stopped: {cause}. The take is kept up to there. {then}", cause = cause, then = then)
    } else if playing {
        crate::localized_format!("Playback stopped: {cause}. {then}", cause = cause, then = then)
    } else {
        crate::localized_format!("Audio output: {cause}. {then}", cause = cause, then = then)
    }
}

/// The status line after the automatic reopen that followed a device reset (`cause`). A reopen
/// that could not start an output stream falls back to the silent clock, which has no recovery of
/// its own: the user must reconnect.
fn reset_reopen_status(cause: &str, silent: bool) -> String {
    let then = if silent {
        crate::localization::text("Use Reconnect audio device in Setup › Playback Engine.")
    } else {
        crate::localization::text("SoundCraft reopened it.")
    };
    output_status(false, false, cause, then)
}

/// The status line for an idle ASIO recovery attempt on device `name`, if it did anything.
fn idle_recovery_status(outcome: soundcraft_playback::IdleRecovery, name: &str) -> Option<String> {
    use soundcraft_playback::IdleRecovery;
    let (cause, then) = match outcome {
        IdleRecovery::None => return None,
        IdleRecovery::Reopened => (crate::localization::text("is available again"), crate::localization::text("SoundCraft reopened it.")),
        IdleRecovery::Waiting => (crate::localization::text("is unavailable"), crate::localization::text("SoundCraft reopens it when it is back.")),
    };
    Some(output_status(false, false, &format!("“{name}” {cause}"), then))
}

/// The status line for an input problem `what`, with what to do about it (`then`).
fn input_status(recording: bool, what: &str, then: &str) -> String {
    if recording {
        crate::localized_format!("Recording stopped: {what}. The take is kept up to there. {then}", what = what, then = then)
    } else {
        crate::localized_format!("Audio input: {what}. {then}", what = what, then = then)
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
    s.tracks.iter().any(|t| !t.kind.is_midi() && (t.mixer.input_monitor || t.mixer.record_arm))
}

#[cfg(test)]
mod reset_tests {
    use super::{AUTO_REOPEN_GAP, idle_recovery_status, input_status, may_reopen, output_status, reset_reopen_status};
    use soundcraft_playback::IdleRecovery;
    use std::time::{Duration, Instant};

    #[test]
    fn a_reset_reopen_that_found_no_output_says_to_reconnect() {
        // The reopen falls back to the silent clock when no output stream starts; the status line
        // said "SoundCraft reopened it." while nothing played, and nothing later corrected it.
        let cause = "“Model Mixer ASIO” asked to be reopened";
        assert_eq!(reset_reopen_status(cause, false), "Audio output: “Model Mixer ASIO” asked to be reopened. SoundCraft reopened it.");
        assert_eq!(
            reset_reopen_status(cause, true),
            "Audio output: “Model Mixer ASIO” asked to be reopened. Use Reconnect audio device in Setup › Playback Engine."
        );
    }

    #[test]
    fn an_idle_recovery_says_whether_the_device_is_back() {
        // On a Model 12 put in USB STORAGE mode or unplugged and plugged back, the status line kept
        // "use Reconnect audio device" after SoundCraft had reopened the device on its own.
        assert_eq!(idle_recovery_status(IdleRecovery::None, "Model Mixer ASIO"), None, "a quiet frame keeps the status");
        assert_eq!(
            idle_recovery_status(IdleRecovery::Waiting, "Model Mixer ASIO").as_deref(),
            Some("Audio output: “Model Mixer ASIO” is unavailable. SoundCraft reopens it when it is back.")
        );
        assert_eq!(
            idle_recovery_status(IdleRecovery::Reopened, "Model Mixer ASIO").as_deref(),
            Some("Audio output: “Model Mixer ASIO” is available again. SoundCraft reopened it.")
        );
    }

    #[test]
    fn a_reset_is_reopened_automatically_unless_one_was_just_reopened() {
        let now = Instant::now();
        assert!(may_reopen(None, now), "the first reset reopens the device");
        assert!(!may_reopen(Some(now), now), "a device that resets again at once is left to the user");
        assert!(!may_reopen(Some(now), now + AUTO_REOPEN_GAP - Duration::from_millis(1)));
        assert!(may_reopen(Some(now), now + AUTO_REOPEN_GAP), "a later reset reopens it again");
    }

    #[test]
    fn the_cause_and_the_advice_are_separate_sentences() {
        let (cause, then) = ("“UFX” asked to be reopened", "SoundCraft reopened it.");
        assert_eq!(output_status(false, true, cause, then), "Playback stopped: “UFX” asked to be reopened. SoundCraft reopened it.");
        assert_eq!(output_status(false, false, cause, then), "Audio output: “UFX” asked to be reopened. SoundCraft reopened it.");
        assert_eq!(
            output_status(true, true, cause, then),
            "Recording stopped: “UFX” asked to be reopened. The take is kept up to there. SoundCraft reopened it."
        );
        assert_eq!(input_status(false, "“Mic In” is unavailable", "Reconnect it."), "Audio input: “Mic In” is unavailable. Reconnect it.");
        assert_eq!(
            input_status(true, "“Mic In” is unavailable", "Reconnect it."),
            "Recording stopped: “Mic In” is unavailable. The take is kept up to there. Reconnect it."
        );
    }
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
}
