//! A live plugin instance the desktop app already has loaded.
//!
//! The engine consults this before it creates a temporary plugin to parse text or probe a value.
//! Playback implements it from the instances it is rendering. The trait stays here so neither
//! crate has to depend on the other.

use crate::TrackId;

/// One slot's live instance, named by the plugin the caller expects there. `slot` is an insert
/// index, or [`usize::MAX`] for the instrument.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LiveSlot<'a> {
    pub track: TrackId,
    pub slot: usize,
    pub plugin: &'a str,
}

/// Reads one live plugin instance.
///
/// A method returns `None` when that slot has no live instance, the instance is not `plugin`
/// (the slot still holds the previous plugin), or the instance cannot answer. Each call checks
/// the plugin and answers under one lock, so a slot replaced between two calls cannot answer
/// for the plugin that was there before.
pub trait LivePlugins: Send {
    /// Plugin id of the live instance in this slot (`None` when nothing is loaded there).
    fn plugin_id(&mut self, track: TrackId, slot: usize) -> Option<String>;
    /// The plugin's own text for a value in SoundCraft units.
    fn value_text(&mut self, at: LiveSlot<'_>, id: &str, value: f32) -> Option<String>;
    /// Parses the plugin's own text into a value in SoundCraft units.
    fn value_from_text(&mut self, at: LiveSlot<'_>, id: &str, text: &str) -> Option<f32>;
    /// The parameter's current value on the live instance.
    fn value(&mut self, at: LiveSlot<'_>, id: &str) -> Option<f32>;
}
