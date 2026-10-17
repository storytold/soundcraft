//! The incumbent's menu tree (feature names only, observed black-box) and the parity measure.
//!
//! A catalog entry counts as implemented when a registered command has the same menu path and
//! label, or when [`ALIASES`] maps the entry to a command id (for UI-layer commands, the UI crate
//! registers extra ids through [`parity_with`]).

use crate::cmd::command_specs;
use serde_json::{Value, json};

/// One leaf per line: `Menu > Submenu > Item`.
pub const MENUS: &str = include_str!("../catalog/menus.txt");

/// Catalog path → command id, for entries whose label or menu differs from ours.
pub const ALIASES: &[(&str, &str)] = &[
    ("File > Open > Session...", "session.open"),
    ("Edit > Can't Undo", "edit.undo"),
    ("Edit > Can't Redo", "edit.redo"),
    ("Edit > Selection > Play Edit", "transport.play_selection"),
    ("Edit > Selection > Change Timeline to Match Edit", "edit.selection_to_timeline"),
    ("Edit > Selection > Change Edit to Match Timeline", "edit.timeline_to_selection"),
    ("Edit > Copy Selection to... > New Playlist", "edit.copy_to_new_playlist"),
    ("Edit > Paste Special > Repeat to Fill Selection", "edit.paste_repeat_to_fill"),
    ("Edit > Trim > Start to Fill Selection", "edit.trim_to_fill_selection"),
    ("Edit > Trim > End to Fill Selection", "edit.trim_to_fill_selection"),
    ("Edit > Trim > To File Start", "edit.trim_to_file_boundaries"),
    ("Edit > Trim > To File End", "edit.trim_to_file_boundaries"),
    ("Edit > Automation > Glide to Current", "automation.write_range"),
    ("Edit > Automation > Trim to Current", "automation.write_to_current"),
    ("Track > Bypass Inserts > All", "track.bypass_inserts"),
    ("Track > Mute Sends > All", "track.mute_sends"),
    ("Clip > Rating > none", "clip.rating"),
    ("Clip > Rating > 1", "clip.rating"),
    ("Clip > Rating > 2", "clip.rating"),
    ("Clip > Rating > 3", "clip.rating"),
    ("Clip > Rating > 4", "clip.rating"),
    ("Clip > Rating > 5", "clip.rating"),
    ("Clip > Clip Gain > Bypass", "clip.gain_bypass"),
    ("Clip > Clip Gain > Render", "clip.gain_render"),
    ("Event > Time Operations > Change Meter...", "event.meter"),
    ("Event > Tempo Operations > Constant...", "event.tempo_constant"),
    ("Event > Tempo Operations > Scale...", "event.tempo_scale"),
    ("Options > Solo Mode > SIP (Solo In Place)", "options.solo_mode"),
    ("Options > Solo Mode > AFL (After Fader Listen)", "options.solo_mode"),
    ("Options > Solo Mode > PFL (Pre Fader Listen)", "options.solo_mode"),
    ("Options > Solo Mode > X-OR (Cancels Previous Solo)", "options.solo_mode"),
    ("Options > Solo Mode > Latch", "options.solo_mode"),
    ("Options > Solo Mode > Momentary", "options.solo_mode"),
    ("Options > Edit Window Scrolling > No Scrolling", "options.scrolling"),
    ("Options > Edit Window Scrolling > After Playback", "options.scrolling"),
    ("Options > Edit Window Scrolling > Page", "options.scrolling"),
    ("Options > Edit Window Scrolling > Continuous", "options.scrolling"),
    ("Options > Edit Window Scrolling > Center Playhead", "options.scrolling"),
    ("View > Main Counter > Bars|Beats", "view.main_counter"),
    ("View > Main Counter > Min:Secs", "view.main_counter"),
    ("View > Main Counter > Timecode", "view.main_counter"),
    ("View > Main Counter > Feet+Frames", "view.main_counter"),
    ("View > Main Counter > Samples", "view.main_counter"),
    ("View > Rulers > Bars|Beats", "view.ruler"),
    ("View > Rulers > Min:Secs", "view.ruler"),
    ("View > Rulers > Timecode", "view.ruler"),
    ("View > Rulers > Feet+Frames", "view.ruler"),
    ("View > Rulers > Samples", "view.ruler"),
    ("View > Rulers > Meter", "view.ruler"),
    ("View > Rulers > Markers", "view.ruler"),
    ("View > Rulers > Chord Symbols", "view.ruler"),
];

pub fn catalog() -> Vec<&'static str> {
    MENUS.lines().map(str::trim).filter(|l| !l.is_empty()).collect()
}

/// Command id implementing a catalog entry, if any.
pub fn implemented_by(entry: &str, extra: &[(&str, &str)]) -> Option<String> {
    if let Some((_, id)) = ALIASES.iter().chain(crate::catalog_more::ALIASES_MORE.iter()).chain(extra.iter()).find(|(p, _)| *p == entry) {
        return Some((*id).to_string());
    }
    let parts: Vec<&str> = entry.split(" > ").collect();
    let (label, menu) = parts.split_last()?;
    command_specs().iter().find(|c| c.label == *label && c.menu == menu).map(|c| c.id.to_string())
}

/// Parity over the catalog, with extra UI-layer mappings.
pub fn parity_with(extra: &[(&str, &str)]) -> Value {
    let cat = catalog();
    let mut done = 0usize;
    let mut missing = Vec::new();
    let mut per_menu: std::collections::BTreeMap<String, (usize, usize)> = Default::default();
    for e in &cat {
        let top = e.split(" > ").next().unwrap_or("").to_string();
        let slot = per_menu.entry(top).or_default();
        slot.1 += 1;
        if implemented_by(e, extra).is_some() {
            done += 1;
            slot.0 += 1;
        } else {
            missing.push(*e);
        }
    }
    let pct = if cat.is_empty() { 0.0 } else { done as f64 * 100.0 / cat.len() as f64 };
    json!({
        "total": cat.len(),
        "implemented": done,
        "percent": (pct * 10.0).round() / 10.0,
        "per_menu": per_menu.iter().map(|(k, (d, t))| json!({"menu": k, "implemented": d, "total": t})).collect::<Vec<_>>(),
        "missing": missing,
    })
}

pub fn parity_json() -> Value {
    parity_with(&[])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aliases_point_at_real_commands() {
        for (path, id) in ALIASES.iter().chain(crate::catalog_more::ALIASES_MORE.iter()) {
            assert!(crate::find_command(id).is_some(), "{path} → unknown {id}");
            assert!(catalog().contains(path), "alias for unknown catalog entry {path}");
        }
    }

    #[test]
    fn parity_floor() {
        let p = parity_json();
        let pct = p["percent"].as_f64().unwrap_or(0.0);
        let done = p["implemented"].as_u64().unwrap_or(0);
        // The floor only ever rises.
        assert!(pct >= 78.0, "engine parity regressed: {pct}%");
        assert!(done >= 401, "engine parity regressed: {done} menu items");
    }
}
