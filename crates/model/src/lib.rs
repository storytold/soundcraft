//! Compatibility facade for SoundCraft's shared session document crate.
//!
//! Engine and UI adapters depend on this name, while the wire schema, JSON reader/writer,
//! validation, and bundle format live in `craft-scraft-format`.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub use craft_scraft_format::*;

mod midi_input;

pub use midi_input::MidiInput;

/// Start of the track's hardware input. Named paths need a complete, matching
/// range on the current device. Legacy numbered routes keep their wrap behaviour.
pub fn hardware_input_start(session: &Session, track: &Track, device_channels: usize) -> Option<usize> {
    match &track.mixer.input {
        Route::Input(name) => {
            let path = session.inputs.iter().find(|path| path.name.eq_ignore_ascii_case(name))?;
            let first = usize::from(path.first_channel);
            (path.format == track.format && first + path.format.channels() <= device_channels).then_some(first)
        }
        Route::Hardware(name) => {
            Some(name.trim_start_matches("In ").split(['-', ' ']).next().and_then(|n| n.parse::<usize>().ok()).map_or(0, |n| n.saturating_sub(1)))
        }
        _ => Some(0),
    }
}
