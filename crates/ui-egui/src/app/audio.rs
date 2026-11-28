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
    }
}

/// How long the meters keep the last readings while no audio block arrives. Longer than the
/// longest device buffer (8192 frames at 44.1 kHz, 186 ms) plus the slowest meter poll (50 ms), so
/// a large buffer never reads as silence between its callbacks; once it has passed, the device has
/// stopped calling back (switched off, unplugged, stalled) and the meters fall to silence.
pub(super) const METER_STALE_SECS: f64 = 0.5;

/// After a meter take that `fresh`ly brought new blocks or not, at `now` (uptime, seconds): note
/// when the last block came in `last_block`, and clear `snap`'s readings once none has come for
/// [`METER_STALE_SECS`]. Otherwise the last block's levels would be fed to the meters every frame
/// and never fall.
pub(super) fn age_meters(snap: &mut soundcraft_playback::MeterSnapshot, fresh: bool, now: f64, last_block: &mut f64) {
    if fresh {
        *last_block = now;
    } else if now - *last_block > METER_STALE_SECS {
        snap.clear_levels();
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
        // Only a signal holds: silence meeting a hold of 0 would restart the shared hold time
        // every frame and keep the other channel's hold up for good.
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

/// Whether a track needs the audio input opened: an audio track (or an aux) that is
/// record-enabled or input-monitored. A MIDI or instrument track listens to MIDI devices, so
/// arming one does not open an audio device or complain that there is none.
pub(super) fn wants_audio_input(s: &soundcraft_model::Session) -> bool {
    s.tracks.iter().any(|t| !t.kind.is_midi() && (t.mixer.input_monitor || t.mixer.record_arm))
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

#[cfg(test)]
mod meter_tests {
    use super::{METER_STALE_SECS, MeterDisplay, age_meters, feed_meter};
    use soundcraft_mix::StripMeter;
    use soundcraft_model::TrackId;
    use soundcraft_playback::MeterSnapshot;

    /// Runs the UI's meter loop for `secs` at 60 frames a second, polling at 20 Hz as the Edit
    /// window does; `blocks(prev, now)` says whether a block (with `peak`) arrived since the
    /// previous poll.
    fn run(secs: f64, peak: [f32; 2], blocks: impl Fn(f64, f64) -> bool) -> (MeterDisplay, f32) {
        let t = TrackId(1);
        let mut snap = MeterSnapshot::default();
        let mut d = MeterDisplay::default();
        let (mut last_block, mut last_poll, mut lowest) = (f64::NEG_INFINITY, f64::NEG_INFINITY, f32::INFINITY);
        let dt = 1.0 / 60.0;
        let mut now = 0.0;
        while now < secs {
            if now - last_poll >= 1.0 / 20.0 {
                let fresh = blocks(last_poll.max(-1e-9), now);
                if fresh {
                    let m = snap.tracks.entry(t).or_insert_with(StripMeter::default);
                    m.peak = peak;
                    m.peaks = peak.to_vec();
                }
                age_meters(&mut snap, fresh, now, &mut last_block);
                last_poll = now;
            }
            let m = snap.tracks.get(&t).map_or([0.0; 2], |m| m.peak);
            feed_meter(&mut d, m, 0.0, dt as f32);
            if now > 0.0 {
                lowest = lowest.min(d.level[0]);
            }
            now += dt;
        }
        (d, lowest)
    }

    #[test]
    fn meters_fall_to_silence_when_the_device_stops_calling_back() {
        // Blocks come for a second, then the device is switched off: no callback ever again.
        let (d, _) = run(6.0, [0.4; 2], |_, now| now < 1.0);
        assert!(d.level == [0.0; 2] && d.hold == [0.0; 2], "the meter still reads {d:?}");
    }

    /// A silent channel does not keep the other channel's peak hold up: 0.0 meets a hold of 0.0
    /// every frame, which must not restart the shared hold time.
    #[test]
    fn a_silent_channel_does_not_keep_the_other_channels_peak_hold() {
        let (d, _) = run(6.0, [0.4, 0.0], |_, now| now < 1.0);
        assert!(d.level == [0.0; 2] && d.hold == [0.0; 2], "the meter still reads {d:?}");
    }

    #[test]
    fn a_large_device_buffer_keeps_the_meters_up_between_callbacks() {
        // 8192 frames at 44.1 kHz: one block every 186 ms, several polls without one.
        let period = 8192.0 / 44_100.0;
        assert!(period + 1.0 / 20.0 < METER_STALE_SECS);
        let (d, lowest) = run(3.0, [0.4; 2], |prev, now| (now / period).floor() > (prev / period).floor());
        assert!(lowest >= 0.4 && d.level[0] >= 0.4, "the meter dropped to {lowest} between callbacks");
    }
}
