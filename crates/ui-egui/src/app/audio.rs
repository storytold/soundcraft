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
        let player = soundcraft_playback::Player::with_config(self.engine.session_arc(), &self.ui.audio);
        self.attach_midi(&player);
        player.locate(at);
        self.player = Some(player);
        self.settle_lost_refusal();
        // A stall of the device chosen now is a first one.
        self.stall = None;
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
        let Some(p) = &self.player else { return };
        let Some(trouble) = p.trouble() else { return };
        let name = p.device_name.clone();
        let (cause, then) = match trouble {
            OutputTrouble::Bug => (
                crate::localization::text("the audio engine hit an internal error (see the log)").to_string(),
                crate::localization::text("Use Reconnect audio device in Setup › Playback Engine, or restart SoundCraft."),
            ),
            OutputTrouble::Gone => (
                format!("“{}” {}.", name, crate::localization::text("is unavailable")),
                crate::localization::text("Check the audio device, then use Reconnect audio device in Setup › Playback Engine."),
            ),
            OutputTrouble::Stalled => {
                let said = stalled_text(&name, self.stall, std::time::Instant::now());
                self.stall = Some(Stall::Reported);
                said
            }
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

    /// Reopen the audio devices, keeping a take in progress.
    pub fn reconnect_audio(&mut self) {
        if self.is_playing() || self.engine.transport.recording {
            self.stop_play();
        }
        if let Some(p) = &self.player {
            // Its plugin editors go with it; closed here, on the main thread (see `apply_audio_config`).
            p.close_editors();
        }
        self.recorder = None;
        self.recorder_failed = false;
        let player = Player::with_config(self.engine.session_arc(), &self.ui.audio);
        self.attach_midi(&player);
        self.player = Some(player);
        self.settle_lost_refusal();
        // Reconnecting an output that stopped asking for audio: if it stops again soon, the
        // status says Reconnect is not enough (see `stalled_text`).
        self.stall = reconnected(self.stall, std::time::Instant::now());
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

/// An output that stopped asking for audio, until a device choice: reported, then reopened by
/// the latest Reconnect at the instant held.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Stall {
    Reported,
    Reconnected(std::time::Instant),
}

/// The stall state after a Reconnect at `now`: no stall stays none; a stall, reported or
/// already reconnected, is reconnected at `now`, so "soon after Reconnect" counts from the latest.
fn reconnected(stall: Option<Stall>, now: std::time::Instant) -> Option<Stall> {
    stall.map(|_| Stall::Reconnected(now))
}

/// How soon after a Reconnect a stall counts as the same one coming back. Seen on Windows after
/// sleep and wake (Model 12, WASAPI): the reopened output stopped again within 2 s, twice; only
/// unplugging and replugging the interface brought it back, and a fresh SoundCraft stalled on it
/// too, so it was the driver's state, which Reconnect cannot reset.
const STALLED_AGAIN: std::time::Duration = std::time::Duration::from_secs(60);

/// What to say about the output `name` that stopped asking for audio at `now`, after `stall`:
/// first, use Reconnect; when it stops again soon after a Reconnect, that the interface itself
/// needs unplugging and plugging back in (or switching off and on) before Reconnect. Wording
/// only: Reconnect is offered either way. All audio systems.
fn stalled_text(name: &str, stall: Option<Stall>, now: std::time::Instant) -> (String, &'static str) {
    let again = matches!(stall, Some(Stall::Reconnected(at)) if now.saturating_duration_since(at) < STALLED_AGAIN);
    if again {
        (
            crate::localized_format!("“{name}” stopped asking for audio again after Reconnect.", name = name),
            crate::localization::text(
                "Unplug the interface and plug it back in (or switch it off and on), then use Reconnect audio device in Setup › Playback Engine.",
            ),
        )
    } else {
        (
            crate::localized_format!("“{name}” stopped asking for audio.", name = name),
            crate::localization::text("Use Reconnect audio device in Setup › Playback Engine."),
        )
    }
}

#[cfg(test)]
mod stall_tests {
    use super::{Stall, reconnected, stalled_text};
    use std::time::{Duration, Instant};

    /// Reconnect at 0 s, again at 55 s, stall at 65 s: the stall comes 10 s after the last
    /// Reconnect, so it is the same one coming back, not a new one.
    #[test]
    fn a_stall_soon_after_the_latest_of_two_reconnects_is_said_to_need_a_replug() {
        let name = "喇叭 (Model 12)";
        let t0 = Instant::now();
        assert_eq!(reconnected(None, t0), None);
        let first = reconnected(Some(Stall::Reported), t0);
        assert_eq!(first, Some(Stall::Reconnected(t0)));
        let second = reconnected(first, t0 + Duration::from_secs(55));
        assert_eq!(second, Some(Stall::Reconnected(t0 + Duration::from_secs(55))));
        let said = stalled_text(name, second, t0 + Duration::from_secs(65));
        assert_eq!(said.0, "“喇叭 (Model 12)” stopped asking for audio again after Reconnect.");
    }

    /// Seen on Windows after sleep and wake: a Model 12 that stopped asking for audio stopped
    /// again within 2 s of each Reconnect; only unplugging and replugging it brought it back.
    #[test]
    fn an_output_that_stalls_again_soon_after_reconnect_is_said_to_need_a_replug() {
        let name = "喇叭 (Model 12)";
        let t0 = Instant::now();
        let first = stalled_text(name, None, t0);
        assert_eq!(first.0, "“喇叭 (Model 12)” stopped asking for audio.");
        assert_eq!(first.1, "Use Reconnect audio device in Setup › Playback Engine.");
        // Stalled before, not reconnected since: Reconnect has not been tried yet.
        assert_eq!(stalled_text(name, Some(Stall::Reported), t0 + Duration::from_secs(5)), first);
        let again = stalled_text(name, Some(Stall::Reconnected(t0)), t0 + Duration::from_secs(5));
        assert_eq!(again.0, "“喇叭 (Model 12)” stopped asking for audio again after Reconnect.");
        assert_eq!(
            again.1,
            "Unplug the interface and plug it back in (or switch it off and on), then use Reconnect audio device in Setup › Playback Engine."
        );
        // Long after a Reconnect that worked, a stall is a new one.
        assert_eq!(stalled_text(name, Some(Stall::Reconnected(t0)), t0 + Duration::from_secs(600)), first);
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
