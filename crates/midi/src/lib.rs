//! SoundCraft MIDI.
//!
//! - [`Note`], [`CtrlEvent`] and [`Sequence`]: the MIDI content of a region / track, in ticks at
//!   [`PPQ`] (960) ticks per quarter note.
//! - [`read_smf`] / [`write_smf`]: Standard MIDI File import (format 0/1, format 2 best-effort) and
//!   export (format 1, 960 PPQ). The reader treats its input as hostile: every length is checked,
//!   variable-length quantities are bounded and event counts are capped.
//! - [`ops`]: MIDI editing operations (quantize, transpose, velocity, duration, split).
//! - [`note_name`], [`gm_drum_name`], [`gm_program_name`], [`cc_name`]: display names.
//! - [`PencilShape`] / [`ShapeStroke`]: what a Pencil stroke draws, as values over ticks.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

mod ctrl;
mod names;
pub mod ops;
mod rng;
mod sequence;
mod shape;
mod smf;

pub use ctrl::cc_name;
pub use names::{gm_drum_name, gm_program_name, note_name};
pub use rng::XorShift;
pub use sequence::{CtrlEvent, CtrlKind, Note, Sequence};
pub use shape::{MAX_POINTS, PencilShape, ShapeStroke};
pub use smf::{MAX_EVENTS, MAX_TRACKS, Smf, SmfTrack, read_smf, write_smf};

/// Ticks per quarter note used throughout the model.
pub const PPQ: u32 = 960;

/// Errors from reading or writing MIDI data.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum MidiError {
    #[error("not a Standard MIDI File (missing MThd header)")]
    NotSmf,
    #[error("unexpected end of data at byte {0}")]
    Truncated(usize),
    #[error("variable-length quantity longer than 4 bytes at byte {0}")]
    VlqTooLong(usize),
    #[error("chunk length {len} at byte {offset} exceeds the remaining data")]
    BadChunkLength { offset: usize, len: u64 },
    #[error("invalid MIDI data at byte {offset}: {reason}")]
    Invalid { offset: usize, reason: &'static str },
    #[error("unsupported MIDI file: {0}")]
    Unsupported(&'static str),
    #[error("too many events (limit {0})")]
    TooManyEvents(usize),
    #[error("too many tracks (limit {0})")]
    TooManyTracks(usize),
    #[error("cannot write MIDI file: {0}")]
    Write(String),
}
