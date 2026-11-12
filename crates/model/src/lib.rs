//! Compatibility facade for SoundCraft's shared session document crate.
//!
//! Engine and UI adapters depend on this name, while the wire schema, JSON reader/writer,
//! validation, and bundle format live in `craft-scraft-format`.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

mod instrument;
mod live;
mod midi_input;

pub use craft_scraft_format::*;
pub use instrument::{INSTRUMENT_AUTO_SLOT, TrackPlugins, auto_param_label, parse_auto_param};
pub use live::{LivePlugins, LiveSlot};
pub use midi_input::MidiInput;
