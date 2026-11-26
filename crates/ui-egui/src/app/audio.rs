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
        // A device chosen here replaces any that went away.
        self.lost_audio = None;
        let player = soundcraft_playback::Player::with_config(self.engine.session_arc(), &self.ui.audio);
        self.attach_midi(&player);
        player.locate(at);
        self.player = Some(player);
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
                let name = r.device_name.clone();
                self.recorder = Some(r);
                self.input_opened(&name);
            }
            Err(e) => {
                self.recorder_failed = true;
                self.ui.status = crate::localized_format!("No audio input: {e}", e = e);
            }
        }
    }

    /// Stop the transport, keeping a take in progress, when the audio output fails.
    pub(super) fn watch_output(&mut self) {
        let Some(p) = &self.player else { return };
        let Some(trouble) = p.trouble() else { return };
        let name = p.device_name.clone();
        let lost = self.audio_lost();
        if trouble == OutputTrouble::Gone {
            lost.away(name.clone(), true);
        } else {
            lost.by_hand = true;
        }
        let (cause, then) = match trouble {
            OutputTrouble::Bug => (
                crate::localization::text("the audio engine hit an internal error (see the log)").to_string(),
                crate::localization::text("Use Reconnect audio device in Setup › Playback Engine, or restart SoundCraft."),
            ),
            OutputTrouble::Gone => (
                format!("“{}” {}", name, crate::localization::text("is unavailable")),
                crate::localization::text("SoundCraft reconnects it when it is back, or use Reconnect."),
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
        // Read after `trouble`, so it agrees with the fault reported (a fault is never cleared).
        let (gone, name) = (r.gone(), r.device_name.clone());
        if recording {
            self.stop_play();
        }
        let (what, then) = match trouble {
            Trouble::Device(what) => {
                self.recorder = None;
                self.recorder_failed = true;
                let lost = self.audio_lost();
                if gone {
                    lost.away(name, false);
                } else {
                    lost.by_hand = true;
                }
                (what, input_advice(gone))
            }
            Trouble::Disk(what) => (what, crate::localization::text("Check the disk, then record again.")),
        };
        self.ui.status = if recording {
            crate::localized_format!("Recording stopped: {what} The take is kept up to there. {then}", what = what, then = then)
        } else {
            crate::localized_format!("Audio input: {what} {then}", what = what, then = then)
        };
    }

    /// Reopen the audio devices with `cfg`, keeping a take in progress (see `reconnect_audio`).
    pub(super) fn reopen_audio(&mut self, cfg: &soundcraft_playback::AudioConfig) {
        if self.is_playing() || self.engine.transport.recording {
            self.stop_play();
        }
        if let Some(p) = &self.player {
            // Its plugin editors go with it; closed here, on the main thread (see `apply_audio_config`).
            p.close_editors();
        }
        self.recorder = None;
        self.recorder_failed = false;
        let player = Player::with_config(self.engine.session_arc(), cfg);
        self.attach_midi(&player);
        self.player = Some(player);
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

/// What to do about an input device that failed: one that went away is reopened once it is listed
/// again (see `watch_replug`); one that is still there but failed needs Reconnect.
fn input_advice(gone: bool) -> &'static str {
    if gone {
        crate::localization::text("SoundCraft reconnects it when it is back, or use Reconnect.")
    } else {
        crate::localization::text("Reconnect it, then use Reconnect audio device in Setup › Playback Engine.")
    }
}

#[cfg(test)]
mod audio_input_tests {
    use super::{input_advice, wants_audio_input};

    #[test]
    fn an_input_that_went_away_is_said_to_reconnect_when_it_is_back() {
        assert_eq!(input_advice(true), "SoundCraft reconnects it when it is back, or use Reconnect.");
        assert_eq!(input_advice(false), "Reconnect it, then use Reconnect audio device in Setup › Playback Engine.");
    }
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
