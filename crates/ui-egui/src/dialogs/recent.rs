//! File › Open Recent: the sessions last opened or saved, newest first.
//!
//! The list lives in [`crate::UiState::recent_sessions`], so it is saved with the rest of the UI
//! state in `ui.json`. A desktop host saves that only on a clean exit, and a list kept only until
//! then is lost by exactly the crash that sends someone looking for their last session, so a
//! change raises [`crate::SoundApp::recent_changed`] for the host to save it straight away.
//!
//! Entries are recorded from the engine's session path whenever it changes (open, save as), not
//! from the commands themselves, so every way a session gets a path (menu, control channel, MCP,
//! a file dropped on the window) is covered by one rule. A file that has gone missing is shown
//! greyed out instead of failing on click; whether each one exists is checked at most every few
//! seconds while the menu is open, because a session on a network share costs a round trip.

use crate::SoundApp;
use serde_json::json;
use std::ffi::OsStr;
use std::path::Path;

/// How many sessions the menu keeps.
pub const MAX_RECENT: usize = 10;
/// Seconds an existence check stays fresh while the menu is open.
const EXISTS_TTL: f64 = 3.0;

/// The same file? On Windows paths are case-insensitive and take either separator.
fn same(a: &str, b: &str) -> bool {
    if cfg!(windows) {
        let norm = |s: &str| s.replace('/', "\\").to_lowercase();
        norm(a) == norm(b)
    } else {
        a == b
    }
}

/// Moves `path` to the front of `list` (adding it if new) and keeps at most [`MAX_RECENT`].
/// Returns whether the list changed.
pub fn note(list: &mut Vec<String>, path: &str) -> bool {
    if path.is_empty() || list.first().is_some_and(|p| p == path) {
        return false;
    }
    list.retain(|p| !same(p, path));
    list.insert(0, path.to_string());
    list.truncate(MAX_RECENT);
    true
}

/// "name — folder" for a session path.
fn label(path: &str) -> String {
    let p = Path::new(path);
    let name = p.file_stem().and_then(OsStr::to_str).unwrap_or(path);
    match p.parent().and_then(Path::file_name).and_then(OsStr::to_str) {
        Some(folder) => format!("{name}  —  {folder}"),
        None => name.to_string(),
    }
}

impl SoundApp {
    /// Records the session path if it changed since the last frame (see the module docs).
    pub(crate) fn note_recent_session(&mut self) {
        if self.engine.path == self.recent_seen {
            return;
        }
        self.recent_seen.clone_from(&self.engine.path);
        if let Some(p) = self.recent_seen.clone()
            && note(&mut self.ui.recent_sessions, &p)
        {
            self.recent_changed = true;
        }
    }

    /// For the desktop host: whether the recent list changed since the last call (save it now).
    pub fn take_recent_changed(&mut self) -> bool {
        std::mem::take(&mut self.recent_changed)
    }

    /// Whether each recent session still exists, re-checked at most every [`EXISTS_TTL`] seconds.
    fn recent_exists(&mut self, now: f64) -> Vec<bool> {
        let stale = self.recent_checked.as_ref().is_none_or(|(at, list, _)| now - at > EXISTS_TTL || *list != self.ui.recent_sessions);
        if stale {
            let exists = self.ui.recent_sessions.iter().map(|p| Path::new(p).is_file()).collect();
            self.recent_checked = Some((now, self.ui.recent_sessions.clone(), exists));
        }
        self.recent_checked.as_ref().map(|(_, _, e)| e.clone()).unwrap_or_default()
    }
}

/// The File › Open Recent submenu.
pub fn menu(app: &mut SoundApp, ui: &mut egui::Ui) {
    ui.menu_button("Open Recent", |ui| {
        if app.ui.recent_sessions.is_empty() {
            ui.add_enabled(false, egui::Button::new("No Recent Sessions"));
            return;
        }
        let now = ui.ctx().input(|i| i.time);
        let exists = app.recent_exists(now);
        let mut open = None;
        for (i, path) in app.ui.recent_sessions.iter().enumerate() {
            let here = exists.get(i).copied().unwrap_or(true);
            let text = if here { label(path) } else { format!("{}  (missing)", label(path)) };
            let r = ui.add_enabled(here, egui::Button::new(text)).on_hover_text(path.as_str()).on_disabled_hover_text(format!("{path}\nnot found"));
            if r.clicked() {
                open = Some(path.clone());
            }
        }
        ui.separator();
        if ui.button("Clear Recent").clicked() {
            app.ui.recent_sessions.clear();
            app.recent_changed = true;
            ui.close();
        }
        if let Some(path) = open {
            ui.close();
            // Through the dialog layer, so unsaved changes are asked about first.
            if let Err(e) = crate::dialogs::run_or_ask(app, "session.open", json!({"path": path})) {
                log::warn!("{path}: {e}");
                app.ui.status = format!("Could not open {path}: {e}");
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn newest_first_without_duplicates_and_capped() {
        let mut l = Vec::new();
        assert!(note(&mut l, "a.scraft"));
        assert!(note(&mut l, "b.scraft"));
        assert!(!note(&mut l, "b.scraft"), "already at the front");
        assert!(note(&mut l, "a.scraft"));
        assert_eq!(l, ["a.scraft", "b.scraft"]);
        for i in 0..20 {
            note(&mut l, &format!("{i}.scraft"));
        }
        assert_eq!(l.len(), MAX_RECENT);
        assert_eq!(l.first().map(String::as_str), Some("19.scraft"));
        assert!(!note(&mut l, ""), "an empty path is never recorded");
    }

    #[test]
    fn windows_paths_match_regardless_of_case_and_separator() {
        let mut l = vec![r"C:\Songs\Mix.scraft".to_string()];
        note(&mut l, "c:/songs/mix.scraft");
        assert_eq!(l.len(), if cfg!(windows) { 1 } else { 2 });
    }

    #[test]
    fn label_names_the_session_and_its_folder() {
        assert_eq!(label("/x/on the mob/on the mob.scraft"), "on the mob  —  on the mob");
        assert_eq!(label("solo.scraft"), "solo");
    }

    #[test]
    fn older_ui_json_without_the_list_still_loads() {
        let ui: crate::UiState = serde_json::from_str("{}").unwrap();
        assert!(ui.recent_sessions.is_empty());
    }
}
