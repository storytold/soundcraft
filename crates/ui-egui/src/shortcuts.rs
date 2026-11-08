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

/// The key equivalent the macOS menu bar gives a command's shortcut: chords with ⌘ or ⌃ that
/// [`parse`] reads, so the bar shows what [`handle`] dispatches. AppKit takes a key equivalent's
/// key before any text field sees it, so bare keys and ⌥- or ⇧-only chords, which type text,
/// get none and stay egui's.
pub fn menu_key_equivalent(shortcut: &str) -> Option<(Modifiers, Key)> {
    parse(shortcut).filter(|(m, _)| m.command || m.ctrl)
}

/// What egui gets for a key-down that the macOS menu bar took as an item's key equivalent.
#[derive(Debug, PartialEq)]
pub enum KeyEquivalentInput {
    /// Events for this frame's input.
    Events(Vec<egui::Event>),
    /// ⌘V: the window reads the clipboard into an `Event::Paste` (`ViewportCommand::RequestPaste`).
    Paste,
}

/// The input egui-winit would have made from the key-down that a menu key equivalent took
/// (AppKit runs key equivalents before the window sees the key), so shortcuts and text fields
/// behave as they do without the menu bar: ⌘X and ⌘C are Cut and Copy and ⌘V a paste, as
/// egui-winit makes them; any other chord, ⌃ included (⌘⌃X is Cut Clip Gain), is its press and
/// its release, as egui-winit sends no release for a press it never saw, and a key egui still
/// holds makes the next press a repeat, which [`handle`] ignores.
pub fn key_equivalent_input(key: Key, modifiers: Modifiers) -> KeyEquivalentInput {
    // egui's `command_only` lets Control through: only the bare ⌘ chord is a clipboard command.
    if modifiers.command_only() && !modifiers.ctrl {
        match key {
            Key::X => return KeyEquivalentInput::Events(vec![egui::Event::Cut]),
            Key::C => return KeyEquivalentInput::Events(vec![egui::Event::Copy]),
            Key::V => return KeyEquivalentInput::Paste,
            _ => {}
        }
    }
    let event = |pressed| egui::Event::Key { key, physical_key: Some(key), pressed, repeat: false, modifiers };
    KeyEquivalentInput::Events(vec![event(true), event(false)])
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

/// Dispatch global keyboard shortcuts once no text field or dialog wants input, ignoring non-scripted auto-repeats
/// while still running a host press alongside a scripted one.
pub fn handle(app: &mut SoundApp, ctx: &egui::Context) {
    // Cmd+= and Cmd+0 are Mix/Edit and Quantize to Grid, not egui's built-in interface zoom.
    ctx.options_mut(|o| o.zoom_with_keyboard = false);
    if ctx.egui_wants_keyboard_input() || app.dialogs.open.is_some() || app.ui.show_preferences {
        return;
    }
    // A scripted press is identified by its exact position in this pass's events, not by matching
    // key/modifier values. A fresh host press with the same chord must still run its own action in
    // addition to the scripted one, while a host auto-repeat stays ignored: egui rewrites an
    // event's `repeat` flag from the physical key state, which says nothing about whether that
    // particular event was injected. Every non-scripted repeat is ignored as before.
    let scripted = app.scripted_key_press_indices();
    let events: Vec<(Key, Modifiers)> = ctx.input(|i| {
        i.events
            .iter()
            .enumerate()
            .filter_map(|(index, e)| match e {
                egui::Event::Key { key, pressed: true, repeat, modifiers, .. } => {
                    if scripted.contains(&index) {
                        Some((*key, *modifiers))
                    } else {
                        (!*repeat).then_some((*key, *modifiers))
                    }
                }
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
        if let Some(id) = command_for(key, mods, mac) {
            if crate::menus::invoke_shortcut_dialog(app, id) {
                continue;
            }
            let _ = app.run(id, json!({}));
        } else if let Some(c) =
            crate::menus::UI_COMMANDS.iter().find(|c| c.3.and_then(parse).is_some_and(|(m, k)| k == key && mods_match(m, mods, mac)))
        {
            let _ = app.run(c.0, json!({}));
        }
    }
    // Tab moves the insertion point here, so egui must not also hand focus to the first widget: that would block every shortcut.
    ctx.memory_mut(|m| m.move_focus(egui::FocusDirection::None));
}

fn command_for(key: Key, mods: Modifiers, mac: bool) -> Option<&'static str> {
    soundcraft_engine::command_specs()
        .iter()
        .find(|c| c.shortcut.and_then(parse).is_some_and(|(m, k)| k == key && mods_match(m, mods, mac)))
        .map(|c| c.id)
}

/// Keys with context-dependent meaning. Returns true when handled.
fn fixed(app: &mut SoundApp, key: Key, m: Modifiers, mac: bool) -> bool {
    let plain = !m.command && !m.alt && !m.ctrl && !m.shift;
    match key {
        Key::Space if !m.command && !m.alt => {
            let id = if m.shift { "transport.half_speed" } else { "transport.toggle" };
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
        Key::Equals if m.command && !(mac && m.ctrl) => {
            let _ = app.run("window.toggle_mix_edit", json!({}));
            true
        }
        Key::Enter if plain => {
            let _ = app.run("markers.add", json!({}));
            true
        }
        // Shift+=/- step the Grid value and Alt+Shift+=/- the Nudge value; macOS sends Alt+Shift+= as =, not +.
        Key::Plus | Key::Equals | Key::Minus if m.shift && !m.command && !m.ctrl && app.ui.window == MainWindow::Edit => {
            let step = if key == Key::Minus { -1 } else { 1 };
            let _ = app.run(if m.alt { "view.nudge" } else { "view.grid" }, json!({"step": step}));
            true
        }
        Key::F12 if plain => {
            let _ = app.run("transport.record", json!({}));
            true
        }
        Key::F1 | Key::F2 | Key::F3 | Key::F4 if plain => {
            edit_mode_key(app, key);
            true
        }
        Key::Num1 | Key::Num2 | Key::Num3 | Key::Num4 if m.alt && !m.command && !m.ctrl && !m.shift => {
            let f = match key {
                Key::Num1 => Key::F1,
                Key::Num2 => Key::F2,
                Key::Num3 => Key::F3,
                _ => Key::F4,
            };
            edit_mode_key(app, f);
            true
        }
        Key::Backtick if plain => {
            cycle_edit_mode(app);
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
        _ => fixed_more(app, key, m, mac),
    }
}

/// The rest of [`fixed`]'s keys.
fn fixed_more(app: &mut SoundApp, key: Key, m: Modifiers, mac: bool) -> bool {
    let plain = !m.command && !m.alt && !m.ctrl && !m.shift;
    match key {
        // `=` is what a bare press of the unlabeled +/- key reports (it has no shift held, so it
        // can never be `Key::Plus`); `Key::Plus` is kept for a keyboard that does send it (e.g. a
        // numpad +). `!m.alt` leaves Alt+= to `window.event_list`'s registered shortcut.
        Key::Plus | Key::Equals | Key::Minus if !m.command && !m.alt && app.ui.window == MainWindow::Edit => {
            let dir = if key == Key::Minus { -1 } else { 1 };
            let _ = app.run("edit.nudge", json!({"direction": dir}));
            true
        }
        // Alt+Tab goes back (off macOS the desktop takes Alt+Tab, so Ctrl+Tab does) and Shift extends the selection.
        // Cmd+Alt+Tab is Tab to Transient, and the Mac's Control+Tab is Pro Tools' Go to Next Clip.
        Key::Tab if !(m.command && (mac || m.alt)) && !(mac && m.ctrl) => {
            let _ = app.run("edit.tab", json!({"back": m.alt || (!mac && m.ctrl), "extend": m.shift}));
            true
        }
        // The registry's key families "Cmd+Alt+[ / ]", "Cmd+Alt+0-5" and "Cmd+Ctrl+1-5", which `parse` can't read.
        Key::OpenBracket | Key::CloseBracket if mods_match(Modifiers::COMMAND | Modifiers::ALT, m, mac) => {
            let factor = if key == Key::CloseBracket { 2.0 } else { 0.5 };
            let _ = app.run("view.waveform_zoom", json!({"factor": factor}));
            true
        }
        Key::Num0 | Key::Num1 | Key::Num2 | Key::Num3 | Key::Num4 | Key::Num5 if mods_match(Modifiers::COMMAND | Modifiers::ALT, m, mac) => {
            let _ = app.run("clip.rating", json!({"rating": digit(key)}));
            true
        }
        Key::Num1 | Key::Num2 | Key::Num3 | Key::Num4 | Key::Num5 if mods_match(Modifiers::COMMAND | Modifiers::CTRL, m, mac) => {
            let _ = app.run("view.zoom_preset", json!({"preset": digit(key)}));
            true
        }
        // Pro Tools' Control+Up/Down track height, on Ctrl off macOS too (its Start key isn't available there).
        Key::ArrowUp | Key::ArrowDown if m.ctrl && (m.alt || !m.shift) => {
            let s = app.engine.session();
            let tracks = crate::edit_window::height_targets(s, m, &s.edit.selected_tracks);
            let id = if key == Key::ArrowUp { "track.height_up" } else { "track.height_down" };
            let _ = app.run(id, if tracks.is_empty() { json!({}) } else { json!({"tracks": tracks}) });
            true
        }
        // Extend Edit Down is Control+Shift+;, which macOS sends as Control+:.
        Key::Semicolon | Key::Colon if mac && m.ctrl && m.shift && !m.alt && !m.command => {
            let _ = app.run("edit.extend_selection_down", json!({}));
            true
        }
        // Ctrl+Shift+Up/Down nudge clip gain off macOS too (the registry's Ctrl is the Mac Control key).
        Key::ArrowUp | Key::ArrowDown if !mac && m.ctrl && m.shift && !m.alt => {
            let id = if key == Key::ArrowUp { "clip.gain_nudge_up" } else { "clip.gain_nudge_down" };
            let _ = app.run(id, json!({}));
            true
        }
        // With an edit selection (a selected clip makes one), Left/Right centre the Edit window on its start/end.
        // Over the piano roll the arrows move its notes instead.
        Key::ArrowLeft | Key::ArrowRight if plain && app.ui.window == MainWindow::Edit && !piano_roll_has_pointer(app) => {
            centre_on_selection(app, key == Key::ArrowRight)
        }
        // Commands Keyboard Focus: single-key editing (Edit window only).
        k if !m.command
            && !m.alt
            && !m.ctrl
            && app.ui.window == MainWindow::Edit
            && app.engine.session().edit.keyboard_focus == "commands"
            && commands_focus(app, k, m.shift) =>
        {
            true
        }
        Key::R | Key::T if plain => {
            // Zoom out / in on the timeline (Commands Focus style).
            let id = if key == Key::T { "view.zoom_in" } else { "view.zoom_out" };
            let _ = app.run(id, json!({}));
            true
        }
        _ => false,
    }
}

/// F1–F4 (and Alt+1–4) choose Shuffle, Slip, Spot and Grid; F4 in Grid switches between Absolute and
/// Relative unless Edit/Tool Mode Keyboard Lock is on. Holding F4 with F1–F3, in either order, adds Snap to Grid.
fn edit_mode_key(app: &mut SoundApp, pressed: Key) {
    let ctx = app.docking_context.clone();
    let held = |k: Key| ctx.as_ref().is_some_and(|c| c.input(|i| i.key_down(k)));
    let (key, snap) = match [Key::F1, Key::F2, Key::F3].into_iter().find(|k| *k != pressed && held(*k)) {
        Some(k) if pressed == Key::F4 => (k, true),
        _ => (pressed, pressed != Key::F4 && held(Key::F4)),
    };
    let ed = &app.engine.session().edit;
    let keep_grid = ed.edit_mode.is_grid() && ed.flag("options.edit_tool_mode_keyboard_lock");
    // F4 pressed first in an F4+F1–F3 chord switched the Grid mode on its own; put that back.
    let flipped = egui::Id::unique("edit_mode_key_grid_before_f4");
    let before =
        (ed.edit_mode.is_grid() && !keep_grid).then(|| if ed.flag(soundcraft_engine::edit::RELATIVE_GRID) { "relative_grid" } else { "grid" });
    let restore = match &ctx {
        Some(c) if pressed == Key::F4 => {
            c.data_mut(|d| d.insert_temp(flipped, before));
            None
        }
        Some(c) if snap => c.data_mut(|d| d.remove_temp::<Option<&'static str>>(flipped)).flatten(),
        _ => None,
    };
    let mode = match key {
        Key::F1 => "shuffle",
        Key::F2 => "slip",
        Key::F3 => "spot",
        _ if keep_grid => return,
        _ => grid_mode(app),
    };
    if let Some(grid) = restore {
        let _ = app.run("edit.mode", json!({"mode": grid}));
    }
    let _ = app.run("edit.mode", json!({"mode": mode, "snap_to_grid": snap}));
}

/// What choosing Grid picks: in Grid the other Grid mode, from another mode the one used last.
pub(crate) fn grid_mode(app: &SoundApp) -> &'static str {
    use soundcraft_model::EditMode;
    let ed = &app.engine.session().edit;
    match ed.edit_mode {
        EditMode::Grid => "relative_grid",
        EditMode::GridRelative => "grid",
        _ if ed.flag(soundcraft_engine::edit::RELATIVE_GRID) => "relative_grid",
        _ => "grid",
    }
}

/// ` steps through the Edit modes in F-key order, past a locked Shuffle.
fn cycle_edit_mode(app: &mut SoundApp) {
    use soundcraft_model::EditMode;
    let ed = &app.engine.session().edit;
    let mode = match ed.edit_mode {
        EditMode::Shuffle => "slip",
        EditMode::Slip => "spot",
        EditMode::Spot => grid_mode(app),
        _ if ed.flag(soundcraft_engine::edit::SHUFFLE_LOCK) => "slip",
        _ => "shuffle",
    };
    let _ = app.run("edit.mode", json!({"mode": mode}));
}

/// Single-key commands. Returns true when `key` was handled.
fn commands_focus(app: &mut SoundApp, key: Key, shift: bool) -> bool {
    let id = match key {
        Key::Z if shift => "edit.redo",
        Key::P if shift => "edit.extend_selection_up",
        Key::Semicolon | Key::Colon if shift => "edit.extend_selection_down",
        _ if shift => return false,
        Key::A => "edit.trim_start_to_insertion",
        Key::S => "edit.trim_end_to_insertion",
        Key::D => "edit.fade_to_start",
        Key::G => "edit.fade_to_end",
        Key::F => "edit.fades_create",
        Key::Comma | Key::Period | Key::M | Key::Slash => {
            let direction = if matches!(key, Key::Period | Key::Slash) { 1 } else { -1 };
            let _ = app.run("edit.nudge", json!({"direction": direction, "next": matches!(key, Key::M | Key::Slash)}));
            return true;
        }
        Key::Num0 => "edit.timeline_to_selection",
        Key::O => "edit.selection_to_timeline",
        Key::OpenBracket => "transport.play_selection",
        Key::CloseBracket => "edit.play_timeline",
        Key::B => "edit.separate",
        Key::X => "edit.cut",
        Key::C => "edit.copy",
        Key::V => "edit.paste",
        Key::Z => "edit.undo",
        Key::N => "options.insertion_follows_playback",
        Key::Num1 | Key::Num2 | Key::Num3 | Key::Num4 | Key::Num5 => {
            let _ = app.run("view.zoom_preset", json!({"preset": digit(key)}));
            return true;
        }
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
        Key::E => {
            crate::edit_window::zoom_toggle(app);
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

/// Scroll the Edit window so the edit selection's start (or `end`) is in its middle. Returns false, leaving
/// the key unhandled, when there is only an insertion point.
fn centre_on_selection(app: &mut SoundApp, end: bool) -> bool {
    let s = app.engine.session();
    let sel = s.edit.selection;
    if sel.start >= sel.end {
        return false;
    }
    let [x0, _, x1, _] = app.edit_layout.timeline;
    let half = f64::from((x1 - x0).max(0.0)) / 2.0 * s.edit.zoom.samples_per_px.max(0.01);
    let at = if end { sel.end } else { sel.start };
    let _ = app.run("view.scroll", json!({"to": at.saturating_sub(soundcraft_time::to_samples(half)).max(0)}));
    true
}

/// Whether the pointer is over the MIDI editor, whose piano roll moves the selected notes with the arrow keys.
fn piano_roll_has_pointer(app: &SoundApp) -> bool {
    app.ui.show_midi_editor
        && app.docking_context.as_ref().is_some_and(|c| {
            let body = c.data(|d| d.get_temp::<egui::Rect>(egui::Id::unique("soundcraft-midi-body")));
            c.pointer_hover_pos().zip(body).is_some_and(|(p, r)| r.contains(p))
        })
}

fn digit(key: Key) -> u8 {
    key.name().parse().unwrap_or(0)
}

/// Shortcut text for display. The registry stores macOS-style strings ("Cmd+N"); off macOS the Cmd
/// modifier is the Ctrl key. Mac-only Control chords ("Cmd+Ctrl+S") have no key there, so they show nothing.
pub fn shortcut_label(s: &str, mac: bool) -> String {
    let has = |m: &str| s.split('+').any(|p| p == m);
    if mac || (!has("Cmd") && !has("Ctrl")) || PC_CTRL_CHORDS.contains(&s) {
        return s.to_string();
    }
    if has("Ctrl") {
        return String::new();
    }
    s.split('+').map(|p| if p == "Cmd" { "Ctrl" } else { p }).collect::<Vec<_>>().join("+")
}

/// Mac Control chords that `fixed` also maps to Ctrl off macOS.
const PC_CTRL_CHORDS: [&str; 4] = ["Ctrl+Up", "Ctrl+Down", "Ctrl+Shift+Up", "Ctrl+Shift+Down"];

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
        assert_eq!(shortcut_label("Ctrl+Up", false), "Ctrl+Up");
        assert_eq!(shortcut_label("Ctrl+Shift+Down", false), "Ctrl+Shift+Down");
        assert_eq!(shortcut_label("Ctrl+Shift+P", false), "");
        assert_eq!(shortcut_label("Ctrl+Alt+L", false), "");
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
    use super::{command_for, mods_match, parse, search_chord};
    use egui::{Key, Modifiers};

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

    #[test]
    fn the_menu_bar_takes_only_command_and_control_chords() {
        use super::menu_key_equivalent;
        assert_eq!(menu_key_equivalent("Cmd+C"), Some((Modifiers::COMMAND, Key::C)));
        assert_eq!(menu_key_equivalent("Cmd+Shift+Z"), Some((Modifiers::COMMAND | Modifiers::SHIFT, Key::Z)));
        assert_eq!(menu_key_equivalent("Cmd+Ctrl+S"), Some((Modifiers::COMMAND | Modifiers::CTRL, Key::S)));
        assert_eq!(menu_key_equivalent("Ctrl+Up"), Some((Modifiers::CTRL, Key::ArrowUp)));
        // These type text in a text field, so AppKit must never take them.
        for typed in ["Shift+R", "Alt+R", "Alt+Shift+3", "A", "Space", "Shift+Space", "Enter"] {
            assert_eq!(menu_key_equivalent(typed), None, "{typed}");
        }
        // Key families the dispatcher reads in `fixed`, not through `parse`.
        for family in ["Cmd+Alt+[ / ]", "Cmd+Ctrl+1-5", "Cmd+Alt+0-5", ""] {
            assert_eq!(menu_key_equivalent(family), None, "{family}");
        }
    }

    #[test]
    fn a_key_equivalent_is_the_input_egui_winit_makes_of_its_key_down() {
        use super::{KeyEquivalentInput, key_equivalent_input};
        let key = |key, pressed| egui::Event::Key { key, physical_key: Some(key), pressed, repeat: false, modifiers: MAC_CMD };
        assert_eq!(key_equivalent_input(Key::S, MAC_CMD), KeyEquivalentInput::Events(vec![key(Key::S, true), key(Key::S, false)]));
        assert_eq!(key_equivalent_input(Key::C, MAC_CMD), KeyEquivalentInput::Events(vec![egui::Event::Copy]));
        assert_eq!(key_equivalent_input(Key::X, MAC_CMD), KeyEquivalentInput::Events(vec![egui::Event::Cut]));
        assert_eq!(key_equivalent_input(Key::V, MAC_CMD), KeyEquivalentInput::Paste);
        // With Shift or Option held egui-winit sends the key itself (Cmd+Alt+V is Paste Special).
        let alt = Modifiers { alt: true, ..MAC_CMD };
        let alt_v = |pressed| egui::Event::Key { key: Key::V, physical_key: Some(Key::V), pressed, repeat: false, modifiers: alt };
        assert_eq!(key_equivalent_input(Key::V, alt), KeyEquivalentInput::Events(vec![alt_v(true), alt_v(false)]));
        // ⌘⌃X is no Cut: it goes to egui as its press and release, which run Cut Clip Gain.
        let ctrl = Modifiers { ctrl: true, ..MAC_CMD };
        let ctrl_x = |pressed| egui::Event::Key { key: Key::X, physical_key: Some(Key::X), pressed, repeat: false, modifiers: ctrl };
        assert_eq!(key_equivalent_input(Key::X, ctrl), KeyEquivalentInput::Events(vec![ctrl_x(true), ctrl_x(false)]));
        assert_eq!(command_for(Key::X, ctrl, true), Some("edit.cut_clip_gain"));
    }

    /// A key equivalent pressed twice runs its shortcut twice: egui must see each press released,
    /// or it takes the second press for an auto-repeat.
    #[test]
    fn a_key_equivalent_runs_its_shortcut_once_per_press() {
        use super::{KeyEquivalentInput, key_equivalent_input};
        let ctx = egui::Context::default();
        let mut app = crate::SoundApp::new(soundcraft_engine::demo::demo_engine(), None, crate::Services::default());
        let mut frame = |events: Vec<egui::Event>| {
            let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1400.0, 900.0));
            let mut raw = egui::RawInput { screen_rect: Some(rect), events, ..Default::default() };
            app.raw_input_hook(&ctx, &mut raw);
            let mut out = ctx.run_ui(raw, |ui| {
                app.logic(ui.ctx());
                app.ui(ui);
            });
            out.textures_delta.clear();
            app.ui.window
        };
        for _ in 0..4 {
            frame(Vec::new());
        }
        let KeyEquivalentInput::Events(toggle) = key_equivalent_input(Key::Equals, MAC_CMD) else { panic!("Cmd+= is a key press") };
        let start = frame(Vec::new());
        let once = frame(toggle.clone());
        assert_ne!(once, start, "Cmd+= switches between the Mix and Edit windows");
        assert_eq!(frame(toggle), start, "the second Cmd+= switches back");
    }

    #[test]
    fn shift_cmd_k_prompts_for_a_folder_to_export_clips() {
        assert_eq!(command_for(Key::K, Modifiers { shift: true, ..MAC_CMD }, true), Some("file.export_clips"));
        assert_eq!(command_for(Key::K, Modifiers { shift: true, ..PC_CTRL }, false), Some("file.export_clips"));
        assert!(crate::dialogs::takes_path("file.export_clips"));
    }
}
