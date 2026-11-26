//! MIDI input devices: opening them, following hotplug, and turning what is played during a
//! record pass into MIDI clips.

use super::{Range, Samples, SoundApp, json};
use soundcraft_engine::io::{MidiPass, MidiRecordStyle, MidiTakeEvent};
use soundcraft_playback::midi::MidiInputs;

impl SoundApp {
    /// Whether a MIDI input port is open.
    pub(crate) fn midi_connected(&self) -> bool {
        self.midi_inputs.as_ref().is_some_and(|m| m.feed().connected() > 0)
    }

    /// Hand the MIDI inputs to a (new) audio engine.
    pub(crate) fn attach_midi(&self, player: &soundcraft_playback::Player) {
        if let Some(m) = &self.midi_inputs {
            // A pass the old engine never confirmed is not this one's to answer: it starts idle.
            m.feed().forgive_passes();
            player.set_midi(std::sync::Arc::clone(m.feed()));
        }
    }

    /// Whether the audio engine still owes the confirmation of an earlier record pass
    /// ([`Self::await_midi_capture`] gave up on it): a pass that records MIDI waits for it
    /// (`MidiFeed::pass_owed` says why).
    pub(crate) fn midi_pass_pending(&self) -> bool {
        self.midi_inputs.as_ref().is_some_and(|m| m.feed().pass_owed())
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
        self.midi_take_merge = vec![self.engine.session().edit.midi_merge];
        self.midi_take_counted = 0;
        // From here on what is played belongs to the pass (punching in while playing: also what
        // the audio engine takes in before it hears of the pass).
        if let Some(m) = &self.midi_inputs {
            m.feed().begin_pass();
        }
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

    /// Where the audio engine's playhead stood when it confirmed the end of `pass`
    /// ([`Self::await_midi_capture`]): the stop position that goes with the take it wrote.
    /// `None` when it did not confirm, or there was no MIDI to wait for.
    pub(crate) fn midi_pass_stopped_at(&self, pass: Option<u64>) -> Option<Samples> {
        self.midi_inputs.as_ref()?.feed().stopped_at(pass?)
    }

    /// Where the audio engine's playhead stood when the pass last begun during playback began
    /// (`MidiFeed::pass_started_at`): the punch-in point as the take counts it. `None` when the
    /// audio engine did not say, or there is no MIDI input.
    pub(crate) fn midi_pass_started_at(&self) -> Option<Samples> {
        self.midi_inputs.as_ref()?.feed().pass_started_at()
    }

    /// Turn the MIDI played during the pass into clips on the MIDI and instrument tracks that
    /// were record-enabled when it started, and on those only: the same tracks the end of the
    /// pass waited for ([`Self::end_midi_pass`]). `with_audio`: audio takes of this pass already
    /// made its undo step. `confirmed`: the audio engine said it had written down everything
    /// played up to the stop ([`Self::await_midi_capture`]). `pass`: whether it was a loop pass,
    /// and where that stopped inside the loop; keys still down end there. Returns how many
    /// clips, and what to tell the user when the take is not complete. `loop_record`: the pass
    /// ran in Loop Record mode, which makes a take of every round played in; otherwise MIDI Merge
    /// decides, round by round, whether what was played is merged or replaces the range.
    /// `punched_in`: where a pass punched in while a loop played started; its first round covers
    /// only from there.
    pub(crate) fn finish_midi_take(
        &mut self,
        range: Range,
        with_audio: bool,
        confirmed: bool,
        (pass, loop_record, punched_in): (MidiPass, bool, Option<Samples>),
    ) -> (usize, Option<String>) {
        let Some(m) = &self.midi_inputs else { return (0, None) };
        let feed = std::sync::Arc::clone(m.feed());
        feed.take_recorded(&mut self.midi_take);
        // The round under way ends at the stop, with MIDI Merge as it is now (it may have been
        // switched after the last frame looked); the rounds that went by since then keep the
        // state it had at that frame.
        self.follow_midi_merge();
        let merge = std::mem::take(&mut self.midi_take_merge);
        // Events the audio thread could not hand over (this thread fell behind).
        let uncollected = self.midi_take_lost.saturating_add(u64::from(feed.take_recorded_lost()));
        self.midi_take_lost = 0;
        let events: Vec<MidiTakeEvent> = self
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
        let devices: Vec<String> = (0..soundcraft_mix::MAX_MIDI_PORTS)
            .map(|i| u8::try_from(i).ok().and_then(|i| feed.ports().name(i)).unwrap_or_default().to_string())
            .collect();
        let pass = pass_with_known_stop(pass, uncollected);
        let inputs = std::mem::take(&mut self.midi_take_inputs);
        let style = MidiRecordStyle { loop_record, merge, release: self.ui.midi_merge_release, same_pass: with_audio, punched_in };
        let take = soundcraft_engine::io::add_midi_recording_with(&mut self.engine, range, pass, &events, &devices, &inputs, &style);
        (take.clips.len() + take.merged.len(), midi_take_problem(take.gaps, uncollected, confirmed))
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
                    self.follow_midi_merge();
                }
            }
        }
    }

    /// MIDI Merge can be switched while recording (Reference Guide p.813): the round under way
    /// takes the state it has now ([`note_merge`]), counting the rounds the take entries collected
    /// since the last look went through.
    fn follow_midi_merge(&mut self) {
        let counted = self.midi_take_counted.min(self.midi_take.len());
        let wraps = self.midi_take[counted..].iter().filter(|e| e.is_wrap()).count();
        self.midi_take_counted = self.midi_take.len();
        let on = self.engine.session().edit.midi_merge;
        note_merge(&mut self.midi_take_merge, wraps, on);
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

/// MIDI Merge in the rounds of a pass (`rounds`, one entry per round so far): `wraps` more rounds
/// began since the last look, and it is `on` now. A round that went by between two looks keeps
/// the state of the round before it; the round under way takes `on`.
fn note_merge(rounds: &mut Vec<bool>, wraps: usize, on: bool) {
    let last = rounds.last().copied().unwrap_or(on);
    rounds.extend(std::iter::repeat_n(last, wraps));
    match rounds.last_mut() {
        Some(r) => *r = on,
        None => rounds.push(on),
    }
}

/// Whether the end of a record pass waits for the audio engine's answer: there is an engine
/// (without one nothing takes MIDI in), and `tracks` MIDI or instrument tracks were
/// record-enabled when the pass started. How many devices are connected at Stop does not count.
fn midi_pass_waits(engine: bool, tracks: usize) -> bool {
    engine && tracks > 0
}

/// `pass` without the stop position of a loop pass when `uncollected` recorded entries never
/// reached the take. One of them may be the mark of the loop going round: the stop position
/// would then be read in the wrong round, and a key held to the stop would end early. Without
/// it the keys still down are held to the end of the loop.
fn pass_with_known_stop(pass: MidiPass, uncollected: u64) -> MidiPass {
    match pass {
        MidiPass::Loop { marked, .. } if uncollected > 0 => MidiPass::Loop { stopped: None, marked },
        pass => pass,
    }
}

/// Where the MIDI of a record pass goes and how the pass ran over that range. `looped` is the
/// loop the pass recorded over: the loop of loop recording, or the loop that was playing when
/// recording was punched in (the playhead goes on round it, so the take is a loop take though
/// the audio is laid out in a line). `stopped` is where the audio engine said a loop pass
/// stopped. Otherwise the pass ran in a line, over the punch range or from where it started to
/// where it stopped.
pub(crate) fn midi_take_span(
    looped: Option<Range>,
    punch: Option<Range>,
    started: Samples,
    stopped_at: Samples,
    stopped: Option<Samples>,
) -> (Range, MidiPass) {
    match looped {
        Some(l) => (l, MidiPass::Loop { stopped, marked: true }),
        None => (punch.unwrap_or(Range::new(started, stopped_at)), MidiPass::Linear),
    }
}

/// Where a record pass that plays on in a line stopped, from the audio engine's position
/// (`None`: no engine) and the UI's copy of it, which is a frame behind: the later of the two.
/// (A loop pass gets its stop position from the audio engine's confirmation instead:
/// `SoundApp::midi_pass_stopped_at`.)
pub(crate) fn pass_stop(engine: Option<Samples>, ui: Samples) -> Samples {
    engine.map_or(ui, |at| at.max(ui))
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
    use super::{MidiPass, Range, midi_pass_waits, midi_take_problem, midi_take_span, note_merge, pass_stop, pass_with_known_stop};

    #[test]
    fn the_round_under_way_takes_midi_merge_as_it_is_now() {
        // The pass started with MIDI Merge off.
        let mut rounds = vec![false];
        note_merge(&mut rounds, 0, true);
        assert_eq!(rounds, [true], "switched on during the first round: that round merges");
        note_merge(&mut rounds, 2, false);
        assert_eq!(rounds, [true, true, false], "a round that went by unseen keeps the state before it");
        note_merge(&mut rounds, 0, false);
        assert_eq!(rounds, [true, true, false]);
    }

    #[test]
    fn midi_merge_switched_after_the_last_frame_of_a_pass_applies_to_its_last_round() {
        use soundcraft_engine::io::{MidiTakeEvent, add_midi_recording};
        use soundcraft_model::ClipContent;
        use soundcraft_playback::midi::{MidiInputs, RecordedMidi};
        let mut app = super::SoundApp::new(soundcraft_engine::Engine::default(), None, crate::Services::default());
        app.midi_inputs = Some(MidiInputs::with_port_names(Vec::new(), vec!["Keys".into()]));
        let t = app.engine.execute("track.new", &serde_json::json!({"name": "Keys", "kind": "instrument"})).unwrap()["tracks"][0].as_u64().unwrap();
        app.engine.execute("track.input", &serde_json::json!({"tracks": [t], "input": "none"})).unwrap();
        app.engine.execute("mix.record_arm", &serde_json::json!({"tracks": [t], "value": true})).unwrap();
        let range = Range::new(0, 48_000);
        let note = |at, status, pitch| MidiTakeEvent { at, device: 0, status, data1: pitch, data2: 100 };
        add_midi_recording(&mut app.engine, range, &[note(0, 0x90, 50), note(12_000, 0x80, 50)], &[], false);
        // The pass starts with MIDI Merge off; it is switched on after the last frame that looked,
        // and the pass stops before the next one.
        app.engine.session_mut().edit.midi_merge = false;
        app.begin_midi_take();
        app.midi_take.extend([
            RecordedMidi { at: 24_000, port: 0, status: 0x90, data1: 62, data2: 100 },
            RecordedMidi { at: 36_000, port: 0, status: 0x80, data1: 62, data2: 0 },
        ]);
        app.engine.session_mut().edit.midi_merge = true;
        app.finish_midi_take(range, false, true, (MidiPass::Linear, false, None));
        let track = app.engine.session().track(soundcraft_model::TrackId(t)).unwrap();
        let pitches: Vec<Vec<u8>> = track
            .clips()
            .iter()
            .map(|c| match &c.content {
                ClipContent::Midi { sequence } => sequence.notes.iter().map(|n| n.pitch).collect(),
                _ => Vec::new(),
            })
            .collect();
        assert_eq!(pitches, [vec![50, 62]], "merged into the clip there, as the button said at Stop");
    }

    #[test]
    fn the_stop_of_a_loop_pass_is_not_used_when_entries_of_the_take_went_missing() {
        let pass = MidiPass::Loop { stopped: Some(24_000), marked: true };
        assert_eq!(pass_with_known_stop(pass, 0), pass, "a whole take keeps its stop");
        // One entry never arrived: it may be the mark of a round, so the round of the stop is
        // not known.
        assert_eq!(pass_with_known_stop(pass, 1), MidiPass::Loop { stopped: None, marked: true });
        assert_eq!(pass_with_known_stop(MidiPass::Linear, 3), MidiPass::Linear);
    }

    #[test]
    fn a_pass_punched_in_while_a_loop_plays_is_a_loop_take_over_that_loop() {
        // The loop 0..96 000 plays; Record at 72 000, Stop at 24 000 in the next round. In a
        // line from 72 000 to 24 000 nothing played would be inside the range.
        let l = Range::new(0, 96_000);
        assert_eq!(midi_take_span(Some(l), None, 72_000, 24_000, Some(24_000)), (l, MidiPass::Loop { stopped: Some(24_000), marked: true }));
        // The audio engine did not say where it stopped: still that loop.
        assert_eq!(midi_take_span(Some(l), None, 72_000, 24_000, None), (l, MidiPass::Loop { stopped: None, marked: true }));
        // No loop: the punch range, or from the start of the pass to its stop.
        let punch = Range::new(10, 20);
        assert_eq!(midi_take_span(None, Some(punch), 0, 50, None), (punch, MidiPass::Linear));
        assert_eq!(midi_take_span(None, None, 5, 50, Some(7)), (Range::new(5, 50), MidiPass::Linear));
    }

    #[test]
    fn a_pass_in_a_line_ends_at_the_later_of_the_engines_position_and_the_uis() {
        assert_eq!(pass_stop(Some(1_000), 95_000), 95_000);
        assert_eq!(pass_stop(Some(96_000), 95_000), 96_000);
        assert_eq!(pass_stop(None, 7), 7, "without an engine the UI's is all there is");
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
