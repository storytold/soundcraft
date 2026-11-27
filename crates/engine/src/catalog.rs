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

// Name migration fallbacks run after explicit UI mappings so opening a dialog
// keeps its original command and enablement rather than becoming a direct edit.
const LEGACY_LABEL_ALIASES: &[(&str, &str)] = &[
    // Historical catalog paths remain valid when their display names change.
    ("Clip > Elastic Properties", "clip.elastic_properties"),
    ("Options > QuickPunch", "options.quickpunch"),
    ("Options > TrackPunch", "options.trackpunch"),
    ("View > Transport > Ableton Link", "view.transport_ableton_link"),
    ("Event > Beat Detective", "event.beat_detective"),
    ("File > Export > Sibelius...", "file.export_sibelius"),
    ("File > Send To... > Sibelius", "file.send_to_sibelius"),
];

/// Catalog leaves naming features we do not have. They keep the incumbent's place — and with it
/// the parity floor — under a descriptive label, but they are never interactive: the UI draws them
/// disabled with a "Not available yet." hover until the behaviour exists, as Preferences does for
/// settings without a backend. Naming the place does not claim the protocol, plug-in format or
/// processor behind it.
pub const PLACEHOLDERS: &[&str] = &[
    "View > Transport > Ableton Link",
    "Track > ARA Plugins > Melodyne",
    "Clip > ARA Plugins > Melodyne",
    "View > Mix Window Views > HEAT",
    "Options > Activate HEAT",
];

/// Whether `path` is a catalog leaf kept as a placeholder for a feature we do not have.
pub fn is_placeholder(path: &str) -> bool {
    PLACEHOLDERS.contains(&path)
}

/// RFC-0052 hand marks for catalog leaves the engine registry doesn't implement:
/// `(path, headless, reach, why)`. A path ending in ` > ` marks every leaf under it; the longest
/// matching path wins.
///
/// `headless` is `no` or `partial`; `reach` is `window` (needs a desktop window, dialog or pointer),
/// `os` (needs an OS or online service the engine doesn't wrap) or `engine-gap` (could be an engine
/// command, isn't one yet). `yes` is never written here: a leaf is `yes` only when the engine registry
/// that `soundcraft-cli` and the MCP server load implements it. UI-layer ids (`parity_with`'s
/// `extra`) don't count. An unmarked missing leaf is `unknown`. [`headless_errors`] rejects marks
/// that match nothing or only hide a `yes`.
pub const HEADLESS: &[(&str, &str, &str, &str)] = &[
    ("File > Open > Project...", "no", "os", "online project service"),
    ("File > Open > Sketch...", "no", "engine-gap", "sketch files could be opened by a command"),
    ("File > Dashboard...", "no", "window", "the start dialog"),
    ("File > Start Collaboration...", "no", "os", "online collaboration service"),
    ("File > Import > ", "no", "engine-gap", "the import could be a command"),
    ("File > Export > ", "no", "engine-gap", "the export could be a command"),
    ("File > Sign In", "no", "os", "online account service"),
    ("Edit > Automation > ", "no", "engine-gap", "automation copies could be commands"),
    ("View > ", "no", "window", "shows or hides part of a desktop window"),
    ("Track > Change Track Width > Show / Hide...", "no", "window", "a desktop dialog"),
    ("Track > ARA Plugins > ", "no", "engine-gap", "no plug-in extension host in the engine yet"),
    ("Clip > ARA Plugins > ", "no", "engine-gap", "no plug-in extension host in the engine yet"),
    ("AudioSuite > ", "no", "engine-gap", "offline processing over the crates/dsp plug-ins could be commands"),
    ("Options > Activate HEAT", "no", "engine-gap", "the processor could be an engine option"),
    ("Setup > Machine Track Arming Profiles...", "no", "os", "external machine control hardware"),
    ("Window > ", "no", "window", "opens or arranges a desktop window"),
    ("Window > Collaboration > ", "no", "os", "online collaboration service"),
    ("Window > Splice", "no", "os", "online sample service"),
    ("Window > SoundFlow", "no", "os", "third-party automation service"),
];

/// Whether a `HEADLESS` mark path covers a catalog leaf.
fn mark_covers(mark: &str, leaf: &str) -> bool {
    mark == leaf || (mark.ends_with(" > ") && leaf.starts_with(mark))
}

/// A leaf's RFC-0052 `headless:` value, `reach:` and reason: `yes` when the engine implements it,
/// else the longest `marks` path that covers it, else `unknown`.
pub fn headless_of<'a>(leaf: &str, in_engine: bool, marks: &[(&'a str, &'a str, &'a str, &'a str)]) -> (&'a str, Option<&'a str>, &'a str) {
    if in_engine {
        return ("yes", None, "");
    }
    marks.iter().filter(|m| mark_covers(m.0, leaf)).max_by_key(|m| m.0.len()).map_or(("unknown", None, ""), |m| (m.1, Some(m.2), m.3))
}

/// Problems with `marks` against `leaves` (`(path, in_engine)`): bad values, a mark that covers no
/// leaf, and a mark whose leaves the engine all implements (it would only hide a `yes`).
pub fn headless_errors(marks: &[(&str, &str, &str, &str)], leaves: &[(&str, bool)]) -> Vec<String> {
    let mut errs = Vec::new();
    for (i, (path, value, reach, _)) in marks.iter().enumerate() {
        if !matches!(*value, "no" | "partial") {
            errs.push(format!("headless mark `{path}`: `{value}` is not `no` or `partial` (`yes` comes from the registry)"));
        }
        if !matches!(*reach, "window" | "os" | "engine-gap") {
            errs.push(format!("headless mark `{path}`: reach `{reach}` is not window, os or engine-gap"));
        }
        if marks[..i].iter().any(|m| m.0 == *path) {
            errs.push(format!("headless mark `{path}` is written twice"));
        }
        let covered: Vec<bool> = leaves.iter().filter(|l| mark_covers(path, l.0)).map(|l| l.1).collect();
        if covered.is_empty() {
            errs.push(format!("headless mark `{path}` matches no catalog leaf"));
        } else if covered.iter().all(|live| *live) {
            errs.push(format!("headless mark `{path}` is stale: the engine implements every leaf it covers"));
        }
    }
    errs
}

/// The RFC-0052 summary line for a [`parity_with`] report.
pub fn headless_summary_line(p: &Value) -> String {
    let h = &p["headless"];
    let n = |v: &Value| v.as_u64().unwrap_or(0);
    format!(
        "**Headless ([RFC-0052](https://github.com/storytold/craft-contrib/blob/main/rfcs/0052-headless-reach.md)): {} yes, {} no (window {}, os {}, engine-gap {}), {} partial, {} unknown.**",
        n(&h["yes"]),
        n(&h["no"]["window"]) + n(&h["no"]["os"]) + n(&h["no"]["engine-gap"]),
        n(&h["no"]["window"]),
        n(&h["no"]["os"]),
        n(&h["no"]["engine-gap"]),
        n(&h["partial"]),
        n(&h["unknown"]),
    )
}

pub fn catalog() -> Vec<&'static str> {
    MENUS.lines().map(str::trim).filter(|l| !l.is_empty()).collect()
}

/// Command id implementing a catalog entry, if any.
pub fn implemented_by(entry: &str, extra: &[(&str, &str)]) -> Option<String> {
    if let Some((_, id)) = ALIASES
        .iter()
        .chain(crate::catalog_more::ALIASES_MORE.iter())
        .chain(extra.iter())
        .chain(LEGACY_LABEL_ALIASES.iter())
        .find(|(p, _)| *p == entry)
    {
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
    let mut headless: Vec<(&str, Option<&str>)> = Vec::new();
    let mut headless_missing = Vec::new();
    for e in &cat {
        let top = e.split(" > ").next().unwrap_or("").to_string();
        let slot = per_menu.entry(top).or_default();
        slot.1 += 1;
        let in_engine = implemented_by(e, &[]).is_some();
        let (value, reach, why) = headless_of(e, in_engine, HEADLESS);
        headless.push((value, reach));
        if value != "yes" {
            headless_missing.push(json!({"path": e, "headless": value, "reach": reach, "why": why}));
        }
        if in_engine || (!extra.is_empty() && implemented_by(e, extra).is_some()) {
            done += 1;
            slot.0 += 1;
        } else {
            missing.push(*e);
        }
    }
    let count = |v: &str| headless.iter().filter(|h| h.0 == v).count();
    let no = |r: &str| headless.iter().filter(|h| h.0 == "no" && h.1 == Some(r)).count();
    let pct = if cat.is_empty() { 0.0 } else { done as f64 * 100.0 / cat.len() as f64 };
    json!({
        "total": cat.len(),
        "implemented": done,
        "percent": (pct * 10.0).round() / 10.0,
        "per_menu": per_menu.iter().map(|(k, (d, t))| json!({"menu": k, "implemented": d, "total": t})).collect::<Vec<_>>(),
        "missing": missing,
        "headless": {
            "yes": count("yes"),
            "no": {"window": no("window"), "os": no("os"), "engine-gap": no("engine-gap")},
            "partial": count("partial"),
            "unknown": count("unknown"),
        },
        "headless_missing": headless_missing,
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
        for (path, id) in ALIASES.iter().chain(crate::catalog_more::ALIASES_MORE.iter()).chain(LEGACY_LABEL_ALIASES.iter()) {
            assert!(crate::find_command(id).is_some(), "{path} → unknown {id}");
            assert!(catalog().contains(path), "alias for unknown catalog entry {path}");
        }
    }

    #[test]
    fn headless_marks_are_current() {
        let leaves: Vec<(&str, bool)> = catalog().into_iter().map(|e| (e, implemented_by(e, &[]).is_some())).collect();
        let errs = headless_errors(HEADLESS, &leaves);
        assert!(errs.is_empty(), "{errs:?}");
    }

    #[test]
    fn headless_yes_is_the_engine_only() {
        let p = parity_json();
        let h = &p["headless"];
        assert_eq!(h["yes"], p["implemented"], "engine-only parity: every implemented leaf is yes");
        // AudioSuite has no engine command: every leaf is marked, none reads as reachable.
        let audiosuite: Vec<&Value> = p["headless_missing"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|m| m["path"].as_str().is_some_and(|x| x.starts_with("AudioSuite > ")))
            .collect();
        assert!(!audiosuite.is_empty());
        assert!(audiosuite.iter().all(|m| m["headless"] == "no" && m["reach"] == "engine-gap"));
        // A UI-layer alias makes a leaf implemented for the desktop report, not headless.
        let ui = [("Window > Mix", "window.mix")];
        let with_ui = parity_with(&ui);
        assert!(with_ui["implemented"].as_u64() > p["implemented"].as_u64());
        assert_eq!(with_ui["headless"], p["headless"]);
        assert!(headless_summary_line(&p).contains(&format!("{} yes", h["yes"])));
    }

    #[test]
    fn headless_marks_pick_the_longest_path_and_refuse_bad_ones() {
        let marks = [("W > ", "no", "window", "a"), ("W > C > ", "no", "os", "b"), ("X", "partial", "engine-gap", "c")];
        assert_eq!(headless_of("W > C > M", false, &marks), ("no", Some("os"), "b"));
        assert_eq!(headless_of("W > Mix", false, &marks), ("no", Some("window"), "a"));
        assert_eq!(headless_of("W > Mix", true, &marks).0, "yes");
        assert_eq!(headless_of("Y", false, &marks).0, "unknown");
        assert_eq!(headless_of("WX", false, &marks).0, "unknown", "a prefix mark needs the separator");
        let leaves = [("W > Mix", false), ("X", true)];
        let bad = [("W > ", "yes", "window", ""), ("W > ", "no", "desk", ""), ("X", "no", "os", ""), ("Z", "no", "os", "")];
        let errs = headless_errors(&bad, &leaves);
        for want in ["is not `no` or `partial`", "reach `desk`", "written twice", "`X` is stale", "`Z` matches no catalog leaf"] {
            assert!(errs.iter().any(|e| e.contains(want)), "{want}: {errs:?}");
        }
    }

    #[test]
    fn parity_floor() {
        let p = parity_json();
        let pct = p["percent"].as_f64().unwrap_or(0.0);
        let done = p["implemented"].as_u64().unwrap_or(0);
        // The floor only ever rises.
        assert!(pct >= 25.0, "engine parity regressed: {pct}%");
        assert!(done >= 396, "engine parity regressed: {done} menu items");
    }

    /// A placeholder must keep its catalog place (so the floor holds and the menu still lists it)
    /// and must not read as the incumbent's own name: the label says what it is, not who makes it.
    #[test]
    fn placeholders_keep_their_place_with_honest_labels() {
        let entries = catalog();
        for path in PLACEHOLDERS {
            assert!(entries.contains(path), "placeholder for an unknown catalog entry: {path}");
            assert!(is_placeholder(path), "{path}");
            let leaf = path.rsplit_once(" > ").map_or(*path, |(_, leaf)| leaf);
            let label = crate::feature_names::display_label(leaf);
            assert_ne!(label, leaf, "{path} still shows the incumbent's name");
            for retired in ["ableton", "melodyne", "heat"] {
                assert!(!label.to_lowercase().contains(retired), "{path}: {label}");
            }
        }
        assert!(!is_placeholder("Options > Loop Record"), "an implemented item is not a placeholder");
        assert!(!is_placeholder("View > Transport > MIDI Controls"));
    }

    #[test]
    fn renamed_labels_keep_legacy_catalog_dispatch() {
        let ui = [("Event > Beat Detective", "window.beat_detective")];
        assert_eq!(implemented_by("Event > Beat Detective", &ui).as_deref(), Some("window.beat_detective"));
        for (path, id, label) in [
            ("Options > QuickPunch", "options.quickpunch", "Punch Recording"),
            ("Options > TrackPunch", "options.trackpunch", "TrackPunch"),
            ("View > Transport > Ableton Link", "view.transport_ableton_link", "Tempo Sync"),
            ("Event > Beat Detective", "event.beat_detective", "Beat Analysis"),
            ("File > Export > Sibelius...", "file.export_sibelius", "MusicXML..."),
            ("File > Send To... > Sibelius", "file.send_to_sibelius", "MusicXML"),
        ] {
            assert_eq!(implemented_by(path, &[]).as_deref(), Some(id));
            assert_eq!(crate::find_command(id).unwrap().label, label);
        }
    }
}
