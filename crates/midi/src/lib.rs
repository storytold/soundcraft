//! SoundCraft MIDI.
//!
//! - [`Note`], [`CtrlEvent`] and [`Sequence`]: the MIDI content of a region / track, in ticks at
//!   [`PPQ`] (960) ticks per quarter note; [`read_smf`] / [`write_smf`]: Standard MIDI File import
//!   and export. These come from `craft-midi` in craft-libs, shared with the other Craft apps, and
//!   are re-exported so the rest of SoundCraft names them from here.
//! - [`ops`]: MIDI editing operations (quantize, transpose, velocity, duration, split).
//! - [`note_name`], [`gm_drum_name`], [`gm_program_name`]: display names.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

mod names;
pub mod ops;
mod rng;

pub use craft_midi::{CtrlEvent, CtrlKind, MAX_EVENTS, MAX_TRACKS, MidiError, Note, PPQ, Sequence, Smf, SmfTrack, read_smf, write_smf};
pub use names::{gm_drum_name, gm_program_name, note_name};
pub use rng::XorShift;
