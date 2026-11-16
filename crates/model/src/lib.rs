//! Compatibility facade for SoundCraft's shared session document crate.
//!
//! Engine and UI adapters depend on this name, while the wire schema, JSON reader/writer,
//! validation, and bundle format live in `craft-scraft-format`.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub use craft_scraft_format::*;
