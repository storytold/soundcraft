//! MIDI input devices: opening them, following hotplug, and turning what is played during a
//! record pass into MIDI clips.

use super::{Range, Samples, SoundApp, json};
use soundcraft_engine::io::MidiTakeEvent;
use soundcraft_playback::midi::MidiInputs;

impl SoundApp {
    /// Whether a MIDI input port is open.
    pub(crate) fn midi_connected(&self) -> bool {
        self.midi_inputs.as_ref().is_some_and(|m| m.feed().connected() > 0)
    }

    /// Hand the MIDI inputs to a (new) audio engine.
    pub(crate) fn attach_midi(&mut self, player: &soundcraft_playback::Player) {
        if let Some(m) = &self.midi_inputs {
            player.set_midi(std::sync::Arc::clone(m.feed()));
        }
        // A pass the old engine never confirmed is not this one's to answer: it starts idle.
        self.midi_pass_unconfirmed = None;
    }

    /// Whether the audio engine still owes the confirmation of an earlier record pass
    /// ([`Self::await_midi_capture`] gave up on it). It takes the MIDI that arrived since its
    /// last callback in one go, with the state of the pass it knows about: a new pass started
    /// and stopped before it runs again would have its notes written at the old pass's
    /// position. So a pass that records MIDI waits for it.
    pub(crate) fn midi_pass_pending(&mut self) -> bool {
        let pending = match (self.midi_pass_unconfirmed, &self.midi_inputs) {
            (Some(pass), Some(m)) => !m.feed().captured(pass),
            _ => false,
        };
        if !pending {
            self.midi_pass_unconfirmed = None;
        }
        pending
    }

    /// A record pass starts: whatever was played before is not part of it.
    pub(crate) fn begin_midi_take(&mut self) {
        if let Some(m) = &self.midi_inputs {
            m.feed().take_recorded(&mut self.midi_take);
            m.feed().take_recorded_lost();
        }
        self.midi_take.clear();
        self.midi_take_lost = 0;
        self.midi_take_inputs = soundcraft_engine::io::midi_record_inputs(&self.engine);
    }

    /// A record pass is about to be stopped: its number on the MIDI feed, when there is MIDI to
    /// wait for. The audio engine is told to stop recording with it
    /// (`Player::end_recording`) and answers for that pass alone ([`Self::await_midi_capture`]).
    ///
    /// That is decided by what the pass set out to record, not by the devices connected now: a
    /// pass whose last device was unplugged before Stop still has its notes on the way to the
    /// take, and the engine still has to say it is done with it before another pass starts.
    pub(crate) fn end_midi_pass(&self) -> Option<u64> {
        let feed = self.midi_inputs.as_ref()?.feed();
        midi_pass_waits(self.player.is_some(), self.midi_take_inputs.len()).then(|| feed.end_pass())
    }

    /// After the audio engine was told to stop recording: wait until it has written down the MIDI
    /// played up to the stop of `pass`, so that a note pressed just before Stop is in the clip.
    /// That takes one audio callback (milliseconds). An audio device that has stopped calling
    /// back never answers; the wait gives up after a tenth of a second and the take is built
    /// from what is there. False when it gave up: the take may lack its last notes. The answer
    /// to an earlier pass that comes in late does not count for this one.
    pub(crate) fn await_midi_capture(&self, pass: Option<u64>) -> bool {
        let (Some(pass), Some(m)) = (pass, &self.midi_inputs) else { return true };
        #[cfg(not(target_arch = "wasm32"))]
        {
            let start = std::time::Instant::now();
            while !m.feed().captured(pass) && start.elapsed() < std::time::Duration::from_millis(100) {
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            m.feed().captured(pass)
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = (pass, m);
            true
        }
    }

    /// Turn the MIDI played during the pass into clips on the MIDI and instrument tracks that
    /// were record-enabled when it started. `with_audio`: audio takes of this pass already made
    /// its undo step. `confirmed`: the audio engine said it had written down everything played
    /// up to the stop ([`Self::await_midi_capture`]). `stopped`: where a loop pass stopped
    /// inside the loop; keys still down end there. Returns how many clips, and what to tell the
    /// user when the take is not complete.
    pub(crate) fn finish_midi_take(&mut self, range: Range, with_audio: bool, confirmed: bool, stopped: Option<Samples>) -> (usize, Option<String>) {
        let Some(m) = &self.midi_inputs else { return (0, None) };
        let feed = m.feed();
        feed.take_recorded(&mut self.midi_take);
        // Events the audio thread could not hand over (this thread fell behind).
        let uncollected = self.midi_take_lost.saturating_add(u64::from(feed.take_recorded_lost()));
        self.midi_take_lost = 0;
        let mut events: Vec<MidiTakeEvent> = self
            .midi_take
            .drain(..)
            .map(|e| MidiTakeEvent {
                at: e.at,
                device: e.port,
                status: if e.is_lost() {
                    MidiTakeEvent::LOST
                } else if e.is_wrap() {
                    MidiTakeEvent::WRAP
                } else {
                    e.status
                },
                data1: e.data1,
                data2: e.data2,
            })
            .collect();
        if let Some(at) = stopped {
            events.push(MidiTakeEvent { at, device: u8::MAX, status: MidiTakeEvent::END, data1: 0, data2: 0 });
        }
        let devices: Vec<String> = (0..soundcraft_mix::MAX_MIDI_PORTS)
            .map(|i| u8::try_from(i).ok().and_then(|i| feed.ports().name(i)).unwrap_or_default().to_string())
            .collect();
        let inputs = std::mem::take(&mut self.midi_take_inputs);
        let take = soundcraft_engine::io::add_midi_recording_from(&mut self.engine, range, &events, &devices, &inputs, with_audio);
        (take.clips.len(), midi_take_problem(take.gaps, uncollected, confirmed))
    }

    /// Open the MIDI inputs once, hand them to the audio engine, and keep following devices that
    /// are plugged in or removed. Collects what is played while recording.
    pub(crate) fn service_midi(&mut self, now: f64) {
        let Some(p) = &self.player else { return };
        match &mut self.midi_inputs {
            None => {
                let m = MidiInputs::open(self.ui.midi_inputs_off.clone());
                p.set_midi(std::sync::Arc::clone(m.feed()));
                self.midi_inputs = Some(m);
                self.midi_scan_at = now;
            }
            Some(m) => {
                if (now - self.midi_scan_at).abs() >= 2.0 {
                    self.midi_scan_at = now;
                    m.rescan();
                }
                if self.engine.transport.recording {
                    m.feed().take_recorded(&mut self.midi_take);
                    self.midi_take_lost = self.midi_take_lost.saturating_add(u64::from(m.feed().take_recorded_lost()));
                    // A take longer than the engine keeps: say so instead of growing without end.
                    let most = soundcraft_engine::io::MAX_MIDI_TAKE_EVENTS;
                    if self.midi_take.len() > most {
                        self.midi_take_lost = self.midi_take_lost.saturating_add((self.midi_take.len() - most) as u64);
                        self.midi_take.truncate(most);
                    }
                }
            }
        }
    }

    /// Use or ignore a MIDI input port. Remembered in the UI prefs.
    pub fn set_midi_input_enabled(&mut self, name: &str, on: bool) {
        if let Some(m) = &mut self.midi_inputs {
            m.set_enabled(name, on);
            self.ui.midi_inputs_off = m.disabled().to_vec();
        }
        // The session's own record of the devices in use follows the switch.
        let _ = self.run("setup.midi_input_devices", json!({ if on { "enable" } else { "disable" }: [name] }));
    }
}

/// Whether the end of a record pass waits for the audio engine's answer: there is an engine
/// (without one nothing takes MIDI in), and `tracks` MIDI or instrument tracks were
/// record-enabled when the pass started. How many devices are connected at Stop does not count.
fn midi_pass_waits(engine: bool, tracks: usize) -> bool {
    engine && tracks > 0
}

/// Where a record pass stopped, from the audio engine's position (`None`: no engine) and the
/// UI's copy of it, which is a frame behind. A pass that plays on in a line ended at the later
/// of the two. A loop pass did not: just after the loop went round the UI's copy is still near
/// the end of the loop while the engine is at its start, so only the engine's position counts.
pub(crate) fn pass_stop(engine: Option<Samples>, ui: Samples, looped: bool) -> Samples {
    match engine {
        Some(at) if looped => at,
        Some(at) => at.max(ui),
        None => ui,
    }
}

/// What to tell the user about a MIDI take that is not complete: input was dropped at `gaps`
/// places, `uncollected` recorded events never reached this thread, or the audio engine did not
/// confirm the end of the pass (`confirmed` false). `None` for a complete take.
fn midi_take_problem(gaps: usize, uncollected: u64, confirmed: bool) -> Option<String> {
    let mut problems = Vec::new();
    if gaps > 0 || uncollected > 0 {
        log::warn!("MIDI recording: input was dropped at {gaps} place(s); {uncollected} recorded event(s) were not collected");
        problems.push(crate::localization::text("MIDI input was dropped during the take: it is incomplete, and notes held at that point end there"));
    }
    if !confirmed {
        log::warn!("MIDI recording: the audio engine did not confirm the end of the pass; the last notes may be missing");
        problems.push(crate::localization::text(
            "The audio engine did not answer when the pass ended: the last MIDI notes played may be missing from the take",
        ));
    }
    (!problems.is_empty()).then(|| problems.join("; "))
}

#[cfg(test)]
mod tests {
    use super::{midi_pass_waits, midi_take_problem, pass_stop};

    #[test]
    fn a_loop_pass_stopped_just_after_the_loop_went_round_ends_where_the_engine_is() {
        // The loop is 0..96 000. The engine has gone round and stands at 1 000; the UI still
        // shows 95 000 from the frame before.
        assert_eq!(pass_stop(Some(1_000), 95_000, true), 1_000, "not the stale position near the loop end");
        assert_eq!(pass_stop(Some(50_000), 49_000, true), 50_000);
        // A pass in a line ends at the later position; without an engine the UI's is all there is.
        assert_eq!(pass_stop(Some(1_000), 95_000, false), 95_000);
        assert_eq!(pass_stop(Some(96_000), 95_000, false), 96_000);
        assert_eq!((pass_stop(None, 7, true), pass_stop(None, 7, false)), (7, 7));
    }

    #[test]
    fn a_pass_waits_for_the_engine_whatever_is_connected_when_it_stops() {
        // The last device was unplugged before Stop: the pass still recorded MIDI tracks.
        assert!(midi_pass_waits(true, 1));
        assert!(!midi_pass_waits(true, 0), "no MIDI track was recorded");
        assert!(!midi_pass_waits(false, 2), "no engine takes MIDI in");
    }

    #[test]
    fn a_take_whose_end_was_not_confirmed_is_reported_as_incomplete() {
        assert_eq!(midi_take_problem(0, 0, true), None, "a complete take has nothing to report");
        // The audio engine never answered: no loss was counted, but the last notes may be missing.
        let unconfirmed = midi_take_problem(0, 0, false).unwrap();
        assert!(unconfirmed.contains("did not answer") && unconfirmed.contains("may be missing"), "{unconfirmed}");
        let dropped = midi_take_problem(1, 0, true).unwrap();
        assert!(dropped.contains("was dropped") && !dropped.contains("did not answer"), "{dropped}");
        assert!(midi_take_problem(0, 3, true).is_some());
        let both = midi_take_problem(2, 0, false).unwrap();
        assert!(both.contains("was dropped") && both.contains("did not answer"), "{both}");
    }
}
