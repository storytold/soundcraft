//! Every user-facing engine command must be reachable from the GUI: through a menu, a shortcut,
//! or a widget that names it in `src/`. Commands that only work from the control channel / CLI /
//! MCP left users stuck (#282: `midi.new_clip` had no UI path, so an empty instrument track could
//! never get a MIDI clip).

use soundcraft_engine::{catalog, cmd::command_specs};
use soundcraft_ui_egui::menus::{self, MenuNode};
use std::collections::HashSet;
use std::path::Path;

/// Known gaps: commands with no menu, shortcut or literal reference in the UI source today.
/// Give each one a UI path and remove it from this list; don't add to it.
const KNOWN_UNREACHED: &[&str] = &[
    "edit.paste_clip_effects",
    "edit.paste_clip_gain",
    "event.midi_note",
    "file.bounce_stems",
    "markers.next",
    "markers.previous",
    "mix.clear_mutes",
    "mix.clear_solos",
    "mix.insert_move",
    "mix.phase_invert",
    "mix.send_surround_pan",
    "mix.solo_safe",
    "options.post_roll",
    "options.pre_roll",
    "setup.reset_edit_state",
    "track.load_preset",
    "track.pin",
    "track.trim_automation_write",
    "track.vca",
    "transport.pause",
    "transport.status",
    "video.offset",
    "view.sub_counter",
];

fn ui_source() -> String {
    fn walk(dir: &Path, out: &mut String) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let p = entry.unwrap().path();
            if p.is_dir() {
                walk(&p, out);
            } else if p.extension().is_some_and(|e| e == "rs") {
                out.push_str(&std::fs::read_to_string(&p).unwrap());
            }
        }
    }
    let mut s = String::new();
    walk(&Path::new(env!("CARGO_MANIFEST_DIR")).join("src"), &mut s);
    s
}

/// Command ids behind the leaves of the menu bar actually built by the app. A spec's own `menu`
/// field is not enough: the bar is built from the catalog, so an off-catalog path never shows.
fn menu_commands() -> HashSet<String> {
    fn walk(nodes: &[MenuNode], extra: &[(&str, &str)], out: &mut HashSet<String>) {
        for n in nodes {
            if n.children.is_empty() {
                out.extend(catalog::implemented_by(&n.path, extra));
            }
            walk(&n.children, extra, out);
        }
    }
    let mut out = HashSet::new();
    walk(&menus::tree(), &menus::ui_aliases(), &mut out);
    out
}

#[test]
fn every_command_has_a_ui_path() {
    let src = ui_source();
    let in_menus = menu_commands();
    let mut unreached = Vec::new();
    let mut stale = Vec::new();
    for c in command_specs() {
        // Queries and view-only commands are for agents / the UI's own reads.
        if !c.journal || c.view {
            continue;
        }
        let reached = in_menus.contains(c.id) || c.shortcut.is_some() || src.contains(&format!("\"{}\"", c.id));
        let known = KNOWN_UNREACHED.contains(&c.id);
        if !reached && !known {
            unreached.push(c.id);
        } else if reached && known {
            stale.push(c.id);
        }
    }
    assert!(unreached.is_empty(), "commands with no menu, shortcut or UI reference: {unreached:?}");
    assert!(stale.is_empty(), "now reachable, remove from KNOWN_UNREACHED: {stale:?}");
}

#[test]
fn playback_engine_menu_opens_the_window_instead_of_running_the_command() {
    let extra = menus::ui_aliases();
    assert_eq!(catalog::implemented_by("Setup > Playback Engine...", &extra).as_deref(), Some("window.playback_engine"));
    assert_eq!(catalog::implemented_by("Setup > Hardware...", &extra).as_deref(), Some("window.playback_engine"));
}

#[test]
fn new_midi_clip_is_in_the_clip_menu() {
    let extra = menus::ui_aliases();
    assert_eq!(catalog::implemented_by("Clip > New MIDI Clip", &extra).as_deref(), Some("midi.new_clip"));
    assert!(menu_commands().contains("midi.new_clip"));
    let tree = menus::tree();
    let clip = tree.iter().find(|n| n.label == "Clip").unwrap();
    assert_eq!(clip.children[0].label, "New MIDI Clip", "creates rather than edits, so it comes first");
}

#[test]
fn remove_sync_point_follows_identify_and_is_greyed_out_without_a_point() {
    let tree = menus::tree();
    let clip = tree.iter().find(|n| n.label == "Clip").unwrap();
    let labels: Vec<&str> = clip.children.iter().map(|n| n.label.as_str()).collect();
    let at = labels.iter().position(|l| *l == "Identify Sync Point").unwrap();
    assert_eq!(labels.get(at + 1), Some(&"Remove Sync Point"));
    let path = &clip.children[at + 1].path;
    let id = menus::command_for_path(path, &menus::ui_aliases());
    assert_eq!(id.as_deref(), Some("clip.remove_sync_point"));

    let mut app = soundcraft_ui_egui::SoundApp::new(soundcraft_engine::demo::demo_engine(), None, soundcraft_ui_egui::Services::default());
    app.engine.execute("edit.select", &serde_json::json!({"tracks": ["Kick"], "start": {"seconds": 12.0}})).unwrap();
    assert!(!menus::item_state(&app, path, id.as_deref()).0, "the selected Kick clip has no sync point yet");
    app.engine.execute("clip.identify_sync_point", &serde_json::json!({})).unwrap();
    assert!(menus::item_state(&app, path, id.as_deref()).0);
}
