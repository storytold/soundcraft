//! A live plugin instance the desktop app already has loaded.
//!
//! The engine consults this before it creates a temporary plugin to parse text or probe a value.
//! Playback implements it from the instances it is rendering. The trait stays here so neither
//! crate has to depend on the other.

use crate::TrackId;

/// Reads one live plugin instance. `slot` is an insert index, or [`usize::MAX`] for the instrument.
///
/// A method returns `None` when that slot has no live instance, or the instance cannot answer.
/// Callers compare [`LivePlugins::plugin_id`] with the insert's plugin id and ignore the other
/// answers when they differ, so a slot that still holds the previous plugin is not reused.
pub trait LivePlugins: Send {
    /// Plugin id of the live instance in this slot (`None` when nothing is loaded there).
    fn plugin_id(&mut self, track: TrackId, slot: usize) -> Option<String>;
    /// The plugin's own text for a value in SoundCraft units.
    fn value_text(&mut self, track: TrackId, slot: usize, id: &str, value: f32) -> Option<String>;
    /// Parses the plugin's own text into a value in SoundCraft units.
    fn value_from_text(&mut self, track: TrackId, slot: usize, id: &str, text: &str) -> Option<f32>;
    /// The parameter's current value on the live instance.
    fn value(&mut self, track: TrackId, slot: usize, id: &str) -> Option<f32>;
}
