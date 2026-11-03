use super::{MeterDisplay, OutputTrouble, Player, SoundApp, Trouble};

impl SoundApp {
    /// Whether the meters are live: while playing, and while armed or input-monitored tracks are heard.
    pub(crate) fn meters_live(&self) -> bool {
        self.engine.transport.playing
            || (self.recorder.is_some() && self.engine.session().tracks.iter().any(|t| t.mixer.input_monitor || t.mixer.record_arm))
    }

    /// Reopens the audio devices with `cfg` (Setup › Playback Engine › Apply), keeping the
    /// transport position, and keeps it as `ui.audio`. Plugin instances and their editors are
    /// recreated with the new player. Refused while recording, which would lose the take; `ui.audio`
    /// then stays the configuration of the open device.
    pub fn apply_audio_config(&mut self, cfg: soundcraft_playback::AudioConfig) -> Result<(), String> {
        if self.engine.transport.recording {
            return Err("stop recording before changing the audio device".into());
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
                self.recorder = Some(r);
            }
            Err(e) => {
                self.recorder_failed = true;
                self.ui.status = format!("No audio input: {e}");
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
                "the audio engine hit an internal error (see the log)".to_string(),
                "Use Reconnect audio device in Setup › Playback Engine, or restart SoundCraft.",
            ),
            OutputTrouble::Gone => {
                (format!("“{name}” is unavailable"), "Check the audio device, then use Reconnect audio device in Setup › Playback Engine.")
            }
            OutputTrouble::Stalled => (format!("“{name}” stopped asking for audio"), "Use Reconnect audio device in Setup › Playback Engine."),
        };
        let (recording, playing) = (self.engine.transport.recording, self.engine.transport.playing);
        if recording || playing {
            self.stop_play();
        }
        self.ui.status = if recording {
            format!("Recording stopped: {cause}. The take is kept up to there. {then}")
        } else if playing {
            format!("Playback stopped: {cause}. {then}")
        } else {
            format!("Audio output: {cause}. {then}")
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
                self.ui.status = format!("The disk is too slow: this take has lost {lost} ms of input so far.");
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
                (what, "Reconnect it, then use Reconnect audio device in Setup › Playback Engine.")
            }
            Trouble::Disk(what) => (what, "Check the disk, then record again."),
        };
        self.ui.status = if recording {
            format!("Recording stopped: {what}. The take is kept up to there. {then}")
        } else {
            format!("Audio input: {what}. {then}")
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
        self.player = Some(Player::with_config(self.engine.session_arc(), &self.ui.audio));
    }
}

/// Fold one reading into a strip's meter. `peak` is the stereo pair (a mono strip repeats its one
/// channel), `peaks` every channel of the strip: channels from the third on read from `peaks`.
pub(super) fn feed_meter(d: &mut MeterDisplay, peak: [f32; 2], peaks: &[f32], gr: f32, dt: f32) {
    // Instant attack, ~26 dB/s release, 2 s peak hold.
    let fall = 10f32.powf(-26.0 * dt / 20.0);
    for c in 0..d.level.len() {
        let v = if c < 2 { peak.get(c) } else { peaks.get(c) }.copied().filter(|v| v.is_finite()).unwrap_or(0.0);
        let lvl = d.level.get(c).copied().unwrap_or(0.0);
        let nv = if v >= lvl { v } else { (lvl * fall).max(v) };
        if let Some(x) = d.level.get_mut(c) {
            *x = if nv < 1e-5 { 0.0 } else { nv };
        }
        // Only a reading refreshes the shared hold timer: a silent or unused channel matching its
        // zero hold must not keep the other channels' holds up.
        if v > 0.0 && v >= d.hold.get(c).copied().unwrap_or(0.0) {
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

#[cfg(test)]
mod tests {
    use super::{MeterDisplay, feed_meter};

    #[test]
    fn surround_channels_get_levels_holds_and_clip_like_the_stereo_pair() {
        let mut d = MeterDisplay::default();
        // A 5.1 strip: L R C LFE Ls Rs; the surround right channel clips.
        let peaks = [0.5, 0.4, 0.3, 0.2, 0.1, 1.0];
        feed_meter(&mut d, [0.5, 0.4], &peaks, 0.0, 0.05);
        assert_eq!(d.level.get(..6), Some(&peaks[..]));
        assert_eq!(d.hold.get(..6), Some(&peaks[..]));
        assert!(d.clip, "a clip on any channel lights the clip light");
        assert!(d.level.get(6..).is_some_and(|rest| rest.iter().all(|v| *v == 0.0)));
        // Silence: every channel falls back smoothly, and its peak hold stays up for now.
        feed_meter(&mut d, [0.0, 0.0], &[], 0.0, 0.05);
        let c = d.level.get(2).copied().unwrap_or_default();
        assert!(c > 0.0 && c < 0.3, "C falls, not drops: {c}");
        assert_eq!(d.hold.get(2).copied(), Some(0.3));
    }

    #[test]
    fn peak_holds_expire_after_two_seconds_of_silence_on_a_strip_with_unused_channels() {
        let mut d = MeterDisplay::default();
        // A stereo strip: every channel past the pair is unused and reads 0.
        feed_meter(&mut d, [0.5, 0.4], &[0.5, 0.4], 0.0, 0.05);
        for _ in 0..50 {
            feed_meter(&mut d, [0.0, 0.0], &[], 0.0, 0.05);
        }
        let l = d.hold.first().copied().unwrap_or_default();
        assert!(l < 0.5, "the L hold drops back to the falling level: {l}");
        assert_eq!(d.hold, d.level);
    }

    #[test]
    fn a_misbehaving_plugin_cannot_stick_a_surround_meter() {
        let mut d = MeterDisplay::default();
        feed_meter(&mut d, [0.0, 0.0], &[0.0, 0.0, f32::NAN, f32::INFINITY], 0.0, 0.05);
        assert!(d.level.iter().all(|v| *v == 0.0) && !d.clip);
    }
}
