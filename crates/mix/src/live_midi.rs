//! Live MIDI input: events from MIDI devices, played on the instruments of record-enabled and
//! input-monitored MIDI and instrument tracks (MIDI thru).
//!
//! The audio host fills [`crate::MixEngine::midi`] before each block. Notes reach the instrument
//! at their sample offset; the sustain pedal (CC 64) holds note-offs back until it is released.
//! Other controllers, pitch bend and aftertouch are not played (instruments take notes only);
//! the host still records them.

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
                _ => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
