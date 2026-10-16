//! Track presets, kept in a Track Presets folder: Save Track Preset asks only for a name, and
//! Recall Track Preset lists the presets in the folder.

use crate::SoundApp;
use crate::dialogs::Dialog;
use serde_json::json;
use soundcraft_model::TrackId;
use std::path::PathBuf;

/// The Track Presets folder, beside the plugin presets' folders (desktop only).
fn dir(app: &SoundApp) -> Option<PathBuf> {
    app.preset_dir.as_ref().map(|d| d.join("Track Presets"))
}

/// Opens the Save Track Preset dialog for `track`, named after it. False without a folder (the web
/// build), where the plain path prompt opens instead.
pub(crate) fn open_save(app: &mut SoundApp, track: TrackId) -> bool {
    let Some(dir) = dir(app) else { return false };
    let name = app.engine.session().track(track).map(|t| t.name.clone()).unwrap_or_default();
    app.dialogs.open = Some(Dialog::SaveTrackPreset { track, name, dir });
    true
}

/// Opens Recall Track Preset's path prompt for a preset file, starting in the Track Presets folder.
pub(crate) fn open_recall(app: &mut SoundApp) {
    let path = dir(app).map(|d| format!("{}/", d.display())).unwrap_or_default();
    app.dialogs.open =
        Some(Dialog::PathPrompt {
            cmd: "track.load_preset".into(), title: "Track — Recall Track Preset from File".into(), path, key: "path".into()
        });
}

/// The Track menu's items after Save Track Preset...: the Recall Track Preset submenu and a path
/// prompt for a preset outside the folder, both for the selected tracks.
pub(crate) fn track_menu(app: &mut SoundApp, ui: &mut egui::Ui) {
    let tracks: Vec<u64> = app.engine.session().edit.selected_tracks.iter().map(|t| t.0).collect();
    ui.add_enabled_ui(!tracks.is_empty(), |ui| {
        recall_menu(app, ui, &tracks);
        if ui.button("Recall Track Preset from File...").clicked() {
            open_recall(app);
            ui.close();
        }
    });
}

/// A track name's menu items: Save Track Preset... for that track (desktop only), and the Recall
/// Track Preset submenu for `tracks`.
pub(crate) fn name_menu(app: &mut SoundApp, ui: &mut egui::Ui, id: TrackId, tracks: &[u64]) {
    if dir(app).is_some() && ui.button("Save Track Preset...").clicked() {
        open_save(app, id);
    }
    recall_menu(app, ui, tracks);
}

/// The Recall Track Preset submenu: the presets in the folder, recalled onto `tracks`.
fn recall_menu(app: &mut SoundApp, ui: &mut egui::Ui, tracks: &[u64]) {
    ui.menu_button("Recall Track Preset", |ui| {
        let Some(dir) = dir(app) else {
            ui.add_enabled(false, egui::Button::new("Track presets need the desktop app"));
            return;
        };
        let names = crate::preset_names(&dir);
        if names.is_empty() {
            ui.add_enabled(false, egui::Button::new("No Track Presets"));
            return;
        }
        for n in names {
            if ui.button(&n).clicked() {
                let path = dir.join(format!("{n}.json")).display().to_string();
                let _ = app.run("track.load_preset", json!({"path": path, "tracks": tracks}));
                ui.close();
            }
        }
    });
}
