//! MIDI input devices: opening them, following hotplug, and turning what is played during a
//! record pass into MIDI clips.

use super::{Range, SoundApp, json};
use soundcraft_engine::io::MidiTakeEvent;
use soundcraft_playback::midi::MidiInputs;

impl SoundApp {
    /// Whether a MIDI input port is open.
    pub(crate) fn midi_connected(&self) -> bool {
        self.midi_inputs.as_ref().is_some_and(|m| m.feed().connected() > 0)
    }

    /// Hand the MIDI inputs to a (new) audio engine.
    pub(crate) fn attach_midi(&self, player: &soundcraft_playback::Player) {
        if let Some(m) = &self.midi_inputs {
            player.set_midi(std::sync::Arc::clone(m.feed()));
        }
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

    /// Before a record pass is stopped: how many passes the audio thread has finished capturing
    /// so far, when there is MIDI to wait for ([`Self::await_midi_capture`]).
    pub(crate) fn midi_pass_mark(&self) -> Option<u64> {
        let feed = self.midi_inputs.as_ref()?.feed();
        (self.player.is_some() && feed.connected() > 0 && !self.midi_take_inputs.is_empty()).then(|| feed.captures_done())
    }

    /// After the audio engine was told to stop recording: wait until it has written down the MIDI
    /// played up to the stop, so that a note pressed just before Stop is in the clip. That takes
    /// one audio callback (milliseconds). An audio device that has stopped calling back never
    /// answers; the wait gives up after a tenth of a second and the take is built from what is
    /// there.
    pub(crate) fn await_midi_capture(&self, mark: Option<u64>) {
        let (Some(mark), Some(m)) = (mark, &self.midi_inputs) else { return };
        #[cfg(not(target_arch = "wasm32"))]
        {
            let start = std::time::Instant::now();
            while m.feed().captures_done() == mark && start.elapsed() < std::time::Duration::from_millis(100) {
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            if m.feed().captures_done() == mark {
                log::warn!("MIDI recording: the audio engine did not confirm the end of the pass; the last notes may be missing");
            }
        }
        #[cfg(target_arch = "wasm32")]
        let _ = (mark, m);
    }

    /// Turn the MIDI played during the pass into clips on the record-enabled MIDI and instrument
    /// tracks. `with_audio`: audio takes of this pass already made its undo step. Returns how
    /// many clips, and what to tell the user when input was dropped during the pass.
    pub(crate) fn finish_midi_take(&mut self, range: Range, with_audio: bool) -> (usize, Option<String>) {
        let Some(m) = &self.midi_inputs else { return (0, None) };
        let feed = m.feed();
        feed.take_recorded(&mut self.midi_take);
        // Events the audio thread could not hand over (this thread fell behind).
        let uncollected = self.midi_take_lost.saturating_add(u64::from(feed.take_recorded_lost()));
        self.midi_take_lost = 0;
        let events: Vec<MidiTakeEvent> = self
            .midi_take
            .drain(..)
            .map(|e| MidiTakeEvent {
                at: e.at,
                device: e.port,
                status: if e.is_lost() { MidiTakeEvent::LOST } else { e.status },
                data1: e.data1,
                data2: e.data2,
            })
            .collect();
        let devices: Vec<String> = (0..soundcraft_mix::MAX_MIDI_PORTS)
            .map(|i| u8::try_from(i).ok().and_then(|i| feed.ports().name(i)).unwrap_or_default().to_string())
            .collect();
        let inputs = std::mem::take(&mut self.midi_take_inputs);
        let take = soundcraft_engine::io::add_midi_recording_from(&mut self.engine, range, &events, &devices, &inputs, with_audio);
        let problem = (take.gaps > 0 || uncollected > 0).then(|| {
            log::warn!("MIDI recording: input was dropped at {} place(s); {uncollected} recorded event(s) were not collected", take.gaps);
            crate::localization::text("MIDI input was dropped during the take: it is incomplete, and notes held at that point end there").to_string()
        });
        (take.clips.len(), problem)
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
