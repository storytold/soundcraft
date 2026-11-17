//! Live MIDI input: events from MIDI devices, played on the instruments of record-enabled and
//! input-monitored MIDI and instrument tracks (MIDI thru).
//!
//! The audio host fills [`crate::MixEngine::midi`] before each block. Notes reach the instrument
//! at their sample offset; the sustain pedal (CC 64) holds note-offs back until it is released.
//! Other controllers, pitch bend, program changes and aftertouch are forwarded to the instrument's
//! MIDI handler, preserving their channel and sample offset as well as being recorded.

use soundcraft_dsp::Plugin;
use soundcraft_model::{MidiInput, Track};
use std::sync::OnceLock;

/// Most input ports one engine tells apart; later ones still reach tracks that listen to all.
pub const MAX_MIDI_PORTS: usize = 32;

/// A MIDI channel message from a device, for the next block.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct LiveMidi {
    /// Sample offset within the block.
    pub offset: u32,
    /// Index into [`MidiPorts`]; `u8::MAX` when the port has none.
    pub port: u8,
    pub status: u8,
    pub data1: u8,
    pub data2: u8,
}

impl LiveMidi {
    /// MIDI channel 0..=15.
    pub fn channel(&self) -> u8 {
        self.status & 0x0F
    }
}

/// Names of the input ports, by the index events carry. Slots are written once and read without
/// locking, so the audio thread can look a name up.
#[derive(Debug)]
pub struct MidiPorts {
    names: Vec<OnceLock<String>>,
}

impl Default for MidiPorts {
    fn default() -> Self {
        MidiPorts { names: (0..MAX_MIDI_PORTS).map(|_| OnceLock::new()).collect() }
    }
}

impl MidiPorts {
    /// The index of `name`, giving it a free slot the first time; `u8::MAX` when all are taken.
    /// Not for the audio thread (allocates).
    pub fn assign(&self, name: &str) -> u8 {
        for (i, slot) in self.names.iter().enumerate() {
            if slot.get_or_init(|| name.to_string()) == name {
                return u8::try_from(i).unwrap_or(u8::MAX);
            }
        }
        u8::MAX
    }

    pub fn name(&self, port: u8) -> Option<&str> {
        self.names.get(usize::from(port)).and_then(OnceLock::get).map(String::as_str)
    }
}

/// Whether a track plays live MIDI input: record-enabled or input-monitored.
pub fn midi_thru(t: &Track) -> bool {
    !t.inactive && (t.mixer.record_arm || t.mixer.input_monitor)
}

/// 128 notes, one bit each.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct NoteSet([u64; 2]);

impl NoteSet {
    fn set(&mut self, note: u8, on: bool) {
        let Some(w) = self.0.get_mut(usize::from(note >> 6) & 1) else { return };
        let bit = 1u64 << (note & 63);
        if on {
            *w |= bit;
        } else {
            *w &= !bit;
        }
    }

    fn has(&self, note: u8) -> bool {
        self.0.get(usize::from(note >> 6) & 1).is_some_and(|w| w & (1u64 << (note & 63)) != 0)
    }

    fn is_empty(&self) -> bool {
        self.0 == [0, 0]
    }
}

/// Per-strip state of the live notes.
#[derive(Debug, Default)]
pub(crate) struct LiveNotes {
    /// Keys held down.
    down: NoteSet,
    /// Keys released while the sustain pedal was down.
    sustained: NoteSet,
    pedal: bool,
}

impl LiveNotes {
    /// Forget everything (the instrument was told `all_notes_off`).
    pub(crate) fn clear(&mut self) {
        *self = LiveNotes::default();
    }

    /// Whether a live note may still sound.
    pub(crate) fn active(&self) -> bool {
        !self.down.is_empty() || !self.sustained.is_empty()
    }

    /// Release what live input started, leaving notes from clips alone.
    pub(crate) fn release(&mut self, inst: &mut dyn Plugin) {
        for note in 0..128u8 {
            if self.down.has(note) || self.sustained.has(note) {
                inst.note_off(0, note);
            }
        }
        self.clear();
    }

    /// Play the block's events that `t` listens to.
    pub(crate) fn play(&mut self, t: &Track, inst: &mut dyn Plugin, events: &[LiveMidi], ports: Option<&MidiPorts>, frames: usize) {
        if events.is_empty() {
            return;
        }
        let input = MidiInput::of(&t.mixer.input);
        for e in events {
            if !input.accepts(ports.and_then(|p| p.name(e.port)), e.channel()) {
                continue;
            }
            let at = usize::try_from(e.offset).unwrap_or(0).min(frames.saturating_sub(1));
            let (d1, d2) = (e.data1 & 0x7F, e.data2 & 0x7F);
            match e.status & 0xF0 {
                0x90 if d2 > 0 => {
                    inst.note_on(at, d1, d2);
                    self.down.set(d1, true);
                    self.sustained.set(d1, false);
                }
                0x80 | 0x90 => {
                    if !self.down.has(d1) {
                        continue;
                    }
                    self.down.set(d1, false);
                    if self.pedal {
                        self.sustained.set(d1, true);
                    } else {
                        inst.note_off(at, d1);
                    }
                }
                // Sustain pedal.
                0xB0 if d1 == 64 => {
                    self.pedal = d2 >= 64;
                    if !self.pedal {
                        for note in 0..128u8 {
                            if self.sustained.has(note) {
                                inst.note_off(at, note);
                            }
                        }
                        self.sustained = NoteSet::default();
                    }
                }
                // All Sound Off, All Notes Off.
                0xB0 if d1 == 120 || d1 == 123 => self.release(inst),
                0xA0 | 0xB0 | 0xC0 | 0xD0 | 0xE0 => inst.midi(at, [e.status, d1, d2]),
                _ => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct ControllerEars(Vec<(usize, [u8; 3])>);

    impl Plugin for ControllerEars {
        fn info(&self) -> &'static soundcraft_dsp::PluginInfo {
            soundcraft_dsp::plugins()[0]
        }
        fn prepare(&mut self, _: f32, _: usize, _: usize) {}
        fn reset(&mut self) {}
        fn set_param(&mut self, _: &str, _: f32) -> bool {
            false
        }
        fn param(&self, _: &str) -> Option<f32> {
            None
        }
        fn process(&mut self, _: &mut [Vec<f32>], _: usize) {}
        fn midi(&mut self, offset: usize, message: [u8; 3]) {
            self.0.push((offset, message));
        }
    }

    #[test]
    fn live_controllers_reach_the_instrument_at_their_offsets_and_channels() {
        use soundcraft_model::{ChannelFormat, Session, TrackKind};
        let mut session = Session::default();
        let track = session.add_track(TrackKind::Instrument, ChannelFormat::Stereo, None);
        let messages = [[0xB0, 1, 127], [0xB0, 7, 0], [0xE0, 127, 127], [0xC0, 24, 0], [0xD9, 90, 0], [0xA9, 38, 100]];
        let events: Vec<_> =
            messages.iter().enumerate().map(|(i, &m)| LiveMidi { offset: (i * 4) as u32, port: 0, status: m[0], data1: m[1], data2: m[2] }).collect();
        let mut ears = ControllerEars::default();
        LiveNotes::default().play(session.track(track).unwrap(), &mut ears, &events, None, 64);
        let expected: Vec<_> = messages.iter().enumerate().map(|(i, &m)| (i * 4, m)).collect();
        assert_eq!(ears.0, expected, "live controller input was received but never delivered to the instrument");
    }

    #[test]
    fn controller_forwarding_obeys_the_tracks_midi_input_filter() {
        use soundcraft_model::{ChannelFormat, Session, TrackKind};
        let mut session = Session::default();
        let track = session.add_track(TrackKind::Instrument, ChannelFormat::Stereo, None);
        session.track_mut(track).unwrap().mixer.input = MidiInput { device: None, channel: Some(9) }.route();
        let events =
            [LiveMidi { offset: 5, port: 0, status: 0xB0, data1: 7, data2: 0 }, LiveMidi { offset: 7, port: 0, status: 0xB9, data1: 7, data2: 100 }];
        let mut ears = ControllerEars::default();
        LiveNotes::default().play(session.track(track).unwrap(), &mut ears, &events, None, 64);
        assert_eq!(ears.0, vec![(7, [0xB9, 7, 100])]);
    }

    #[test]
    fn ports_keep_their_index_and_run_out_gracefully() {
        let p = MidiPorts::default();
        assert_eq!((p.assign("Keys"), p.assign("Pad"), p.assign("Keys")), (0, 1, 0));
        assert_eq!((p.name(1), p.name(2), p.name(u8::MAX)), (Some("Pad"), None, None));
        for i in 2..MAX_MIDI_PORTS {
            p.assign(&format!("port {i}"));
        }
        assert_eq!(p.assign("one too many"), u8::MAX);
    }

    #[test]
    fn note_set_covers_all_128_notes() {
        let mut s = NoteSet::default();
        for n in [0u8, 63, 64, 127] {
            assert!(!s.has(n));
            s.set(n, true);
            assert!(s.has(n));
        }
        s.set(64, false);
        assert!(!s.has(64) && s.has(63) && !s.is_empty());
    }
}
