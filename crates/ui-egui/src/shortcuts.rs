//! Keyboard shortcuts. Most come from the command registry's `shortcut` strings; a few
//! context-sensitive keys (Space, Enter, F-keys, nudge, Tab) are handled here.

use crate::{MainWindow, SoundApp};
use egui::{Key, Modifiers};
use serde_json::json;

/// Parse "Cmd+Shift+X" style strings. Returns None for descriptive ones ("F1-F4", "Plus/Minus").
pub fn parse(s: &str) -> Option<(Modifiers, Key)> {
    let mut m = Modifiers::NONE;
    let mut key = None;
    for part in s.split('+') {
        match part.trim() {
            "Cmd" => m.command = true,
            "Shift" => m.shift = true,
            "Alt" => m.alt = true,
            "Ctrl" => m.ctrl = true,
            "" => {}
            k => {
                if key.is_some() {
                    return None;
                }
                key = Some(match k {
                    "Space" => Key::Space,
                    "Enter" => Key::Enter,
                    "Home" => Key::Home,
                    "End" => Key::End,
                    "Tab" => Key::Tab,
                    "[" => Key::OpenBracket,
                    "]" => Key::CloseBracket,
                    "," => Key::Comma,
                    "." => Key::Period,
                    "/" => Key::Slash,
                    "'" => Key::Quote,
                    "=" => Key::Equals,
                    other => Key::from_name(other)?,
                });
            }
        }
    }
    key.map(|k| (m, k))
}

/// Whether the held modifiers `got` fire a shortcut that asks for `want`.
fn mods_match(want: Modifiers, got: Modifiers, mac: bool) -> bool {
    // Off macOS egui reports Ctrl as both `ctrl` and `command`: Ctrl is the shortcut's Cmd, and a shortcut's
    // Ctrl (the Mac Control key) has no key of its own there, so Ctrl+X is Cut, not Cmd+Ctrl+X.
    let ctrl = if mac { want.ctrl == got.ctrl } else { !want.ctrl };
    want.command == got.command && want.shift == got.shift && want.alt == got.alt && ctrl
}

/// Cmd+Ctrl+S opens Search. Off macOS Ctrl+S is Save (Cmd+S), so Search is reached from the Window menu.
fn search_chord(m: Modifiers, mac: bool) -> bool {
    mac && m.command && m.ctrl
}

pub fn handle(app: &mut SoundApp, ctx: &egui::Context) {
    if ctx.egui_wants_keyboard_input() || app.dialogs.open.is_some() {
        return;
    }
    let events: Vec<(Key, Modifiers)> = ctx.input(|i| {
        i.events
            .iter()
            .filter_map(|e| match e {
                egui::Event::Key { key, pressed: true, repeat: false, modifiers, .. } => Some((*key, *modifiers)),
                _ => None,
            })
            .collect()
    });
    // Cmd and Ctrl are separate keys only on macOS (asked at runtime so the web build in a Mac browser agrees).
    let mac = ctx.os() == egui::os::OperatingSystem::Mac;
    for (key, mods) in events {
        if fixed(app, key, mods, mac) {
            continue;
        }
        let hit =
            soundcraft_engine::command_specs().iter().find(|c| c.shortcut.and_then(parse).is_some_and(|(m, k)| k == key && mods_match(m, mods, mac)));
        if let Some(c) = hit {
            let id = c.id;
            if crate::menus::invoke_shortcut_dialog(app, id) {
                continue;
            }
            let _ = app.run(id, json!({}));
        }
    }
}

/// Keys with context-dependent meaning. Returns true when handled.
fn fixed(app: &mut SoundApp, key: Key, m: Modifiers, mac: bool) -> bool {
    let plain = !m.command && !m.alt && !m.ctrl && !m.shift;
    match key {
        Key::Space if !m.command && !m.alt => {
            let id = if m.shift {
                "transport.half_speed"
            } else if app.is_playing() {
                "transport.pause"
            } else {
                "transport.play"
            };
            let _ = app.run(id, json!({}));
            true
        }
        Key::Space if m.command => {
            let _ = app.run("transport.record", json!({}));
            true
        }
        Key::S if search_chord(m, mac) => {
            let _ = app.run("window.search", json!({}));
            true
        }
        Key::Equals if m.command => {
            let _ = app.run("window.toggle_mix_edit", json!({}));
            true
        }
        Key::Enter if plain => {
            let _ = app.run("markers.add", json!({}));
            true
        }
        Key::F1 | Key::F2 | Key::F3 | Key::F4 if plain => {
            let mode = match key {
                Key::F1 => "shuffle",
                Key::F2 => "slip",
                Key::F3 => "spot",
                _ => "grid",
            };
            let _ = app.run("edit.mode", json!({"mode": mode}));
            true
        }
        Key::F5 | Key::F6 | Key::F7 | Key::F8 | Key::F9 | Key::F10 if plain => {
            let tool = match key {
                Key::F5 => "zoom",
                Key::F6 => "trim",
                Key::F7 => "selector",
                Key::F8 => "grabber",
                Key::F9 => "scrubber",
                _ => "pencil",
            };
            let _ = app.run("edit.tool", json!({"tool": tool}));
            true
        }
        Key::Plus | Key::Minus if !m.command && app.ui.window == MainWindow::Edit => {
            let dir = if key == Key::Plus { 1 } else { -1 };
            let _ = app.run("edit.nudge", json!({"direction": dir}));
            true
        }
        Key::Tab if plain || m.alt => {
            // Tab to next clip boundary (or transient when enabled).
            let s = app.engine.session();
            let at = s.edit.selection.start;
            let back = m.alt;
            if s.edit.tab_to_transient
                && !back
                && let Some(t) = s.edit.selected_tracks.first().copied()
            {
                let r = soundcraft_time::Range::new(at + 1, s.content_end());
                if let Some(n) = soundcraft_engine::io::transients_in(s, t, r, 0.5).into_iter().find(|x| *x > at + 64) {
                    let _ = app.run("transport.locate", json!({"at": n}));
                    return true;
                }
            }
            let mut edges: Vec<i64> = s
                .tracks
                .iter()
                .filter(|t| s.edit.selected_tracks.contains(&t.id))
                .flat_map(|t| t.clips().iter().flat_map(|c| [c.start, c.end()]))
                .collect();
            edges.sort_unstable();
            let next = if back { edges.iter().rev().find(|e| **e < at).copied() } else { edges.iter().find(|e| **e > at).copied() };
            if let Some(n) = next {
                let _ = app.run("transport.locate", json!({"at": n}));
            }
            true
        }
        Key::ArrowUp | Key::ArrowDown if m.ctrl && !m.shift => {
            let id = if key == Key::ArrowUp { "edit.extend_selection_up" } else { "edit.extend_selection_down" };
            let _ = app.run(id, json!({}));
            true
        }
        // Commands Keyboard Focus: single-key editing (Edit window only).
        k if plain && app.ui.window == MainWindow::Edit && app.engine.session().edit.keyboard_focus == "commands" && commands_focus(app, k) => true,
        Key::R | Key::T if plain => {
            // Zoom out / in on the timeline (Commands Focus style).
            let id = if key == Key::T { "view.zoom_in" } else { "view.zoom_out" };
            let _ = app.run(id, json!({}));
            true
        }
        _ => false,
    }
}

/// Single-key commands. Returns true when `key` was handled.
fn commands_focus(app: &mut SoundApp, key: Key) -> bool {
    let id = match key {
        Key::A => "edit.trim_start_to_insertion",
        Key::S => "edit.trim_end_to_insertion",
        Key::D => "edit.fade_to_start",
        Key::G => "edit.fade_to_end",
        Key::F => "edit.fades_create",
        Key::B => "edit.separate",
        Key::X => "edit.cut",
        Key::C => "edit.copy",
        Key::V => "edit.paste",
        Key::Z => "edit.undo",
        Key::P => {
            move_selection_track(app, -1);
            return true;
        }
        Key::Semicolon => {
            move_selection_track(app, 1);
            return true;
        }
        Key::L | Key::Quote => {
            tab_clip(app, key == Key::L);
            return true;
        }
        _ => return false,
    };
    let _ = app.run(id, json!({}));
    true
}

/// Move the edit selection to the track above/below (P / ;).
fn move_selection_track(app: &mut SoundApp, dir: i64) {
    let s = app.engine.session();
    let visible: Vec<u64> = s.tracks.iter().filter(|t| !t.hidden).map(|t| t.id.0).collect();
    let Some(cur) = s.edit.selected_tracks.first().map(|t| t.0) else { return };
    let Some(i) = visible.iter().position(|t| *t == cur) else { return };
    let j = (i as i64 + dir).clamp(0, visible.len() as i64 - 1) as usize;
    if let Some(t) = visible.get(j) {
        let sel = s.edit.selection;
        let _ = app.run("edit.select", json!({"tracks": [t], "start": sel.start, "end": sel.end, "exact": true}));
    }
}

/// Tab to the previous/next clip boundary on the selected tracks (L / ').
fn tab_clip(app: &mut SoundApp, back: bool) {
    let s = app.engine.session();
    let at = s.edit.selection.start;
    let mut edges: Vec<i64> =
        s.tracks.iter().filter(|t| s.edit.selected_tracks.contains(&t.id)).flat_map(|t| t.clips().iter().flat_map(|c| [c.start, c.end()])).collect();
    edges.sort_unstable();
    let next = if back { edges.iter().rev().find(|e| **e < at).copied() } else { edges.iter().find(|e| **e > at).copied() };
    if let Some(n) = next {
        let _ = app.run("transport.locate", json!({"at": n}));
    }
}

/// Shortcut text for display. The registry stores macOS-style strings ("Cmd+N"); off macOS the Cmd
/// modifier is the Ctrl key. Mac-only Control chords ("Cmd+Ctrl+S") have no key there, so they show nothing.
pub fn shortcut_label(s: &str, mac: bool) -> String {
    if mac || !s.split('+').any(|p| p == "Cmd") {
        return s.to_string();
    }
    if s.split('+').any(|p| p == "Ctrl") {
        return String::new();
    }
    s.split('+').map(|p| if p == "Cmd" { "Ctrl" } else { p }).collect::<Vec<_>>().join("+")
}

#[cfg(test)]
mod label_tests {
    use super::shortcut_label;

    #[test]
    fn cmd_becomes_ctrl_off_mac() {
        assert_eq!(shortcut_label("Cmd+N", false), "Ctrl+N");
        assert_eq!(shortcut_label("Cmd+Shift+W", false), "Ctrl+Shift+W");
        assert_eq!(shortcut_label("Cmd+Alt+B", false), "Ctrl+Alt+B");
        assert_eq!(shortcut_label("Cmd+,", false), "Ctrl+,");
        assert_eq!(shortcut_label("Shift+R", false), "Shift+R");
        assert_eq!(shortcut_label("Cmd+Ctrl+S", false), "");
        assert_eq!(shortcut_label("", false), "");
    }

    #[test]
    fn unchanged_on_mac() {
        for s in ["Cmd+N", "Cmd+Shift+W", "Cmd+Alt+B", "Shift+R", "Cmd+Ctrl+S"] {
            assert_eq!(shortcut_label(s, true), s);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{mods_match, parse, search_chord};
    use egui::Modifiers;

    /// Ctrl held on Windows or Linux, as egui reports it.
    const PC_CTRL: Modifiers = Modifiers { ctrl: true, command: true, ..Modifiers::NONE };
    /// Cmd held on macOS.
    const MAC_CMD: Modifiers = Modifiers { mac_cmd: true, command: true, ..Modifiers::NONE };

    fn want(s: &str) -> Modifiers {
        parse(s).map(|(m, _)| m).unwrap()
    }

    #[test]
    fn ctrl_is_cmd_off_macos() {
        assert!(mods_match(want("Cmd+S"), PC_CTRL, false));
        assert!(mods_match(want("Cmd+Shift+N"), Modifiers { shift: true, ..PC_CTRL }, false));
        assert!(mods_match(want("Cmd+X"), PC_CTRL, false));
        assert!(!mods_match(want("Cmd+Ctrl+X"), PC_CTRL, false));
        assert!(!mods_match(want("Cmd+S"), Modifiers::NONE, false));
        assert!(!search_chord(PC_CTRL, false));
    }

    #[test]
    fn cmd_and_ctrl_stay_apart_on_macos() {
        assert!(mods_match(want("Cmd+S"), MAC_CMD, true));
        assert!(!mods_match(want("Cmd+S"), Modifiers::CTRL, true));
        assert!(mods_match(want("Cmd+Ctrl+X"), Modifiers { ctrl: true, ..MAC_CMD }, true));
        assert!(!mods_match(want("Cmd+X"), Modifiers { ctrl: true, ..MAC_CMD }, true), "Cmd+Ctrl+X is not Cut");
        assert!(!search_chord(MAC_CMD, true));
        assert!(search_chord(Modifiers { ctrl: true, ..MAC_CMD }, true));
    }
}
