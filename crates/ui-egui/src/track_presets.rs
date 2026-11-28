//! Track presets, kept in a Track Presets folder: Save Track Preset asks only for a name, and
//! Recall Track Preset lists the presets in the folder.

use crate::SoundApp;
use crate::dialogs::Dialog;
use crate::theme::Tokens;
use serde_json::json;
use soundcraft_model::TrackId;
use std::path::{Path, PathBuf};

/// The Track Presets folder, beside the plugin presets' folders (desktop only).
pub(crate) fn dir(app: &SoundApp) -> Option<PathBuf> {
    app.preset_dir.as_ref().map(|d| d.join("Track Presets"))
}

/// Opens the Save Track Preset dialog for `track`, named after it. False without a folder (the web
/// build), where the plain path prompt opens instead.
pub(crate) fn open_save(app: &mut SoundApp, track: TrackId) -> bool {
    let Some(dir) = dir(app) else { return false };
    let name = app.engine.session().track(track).map(|t| t.name.clone()).unwrap_or_default();
    app.dialogs.set(Dialog::SaveTrackPreset { track, name, dir, confirm: false, error: None });
    true
}

/// The Save Track Preset dialog's contents: the name, or the question whether to replace the preset
/// of that name. True when Save or Replace was pressed (or Enter). Cancel in the question goes back
/// to the name.
pub(crate) fn save_form(ui: &mut egui::Ui, name: &mut String, dir: &Path, confirm: &mut bool, error: &mut Option<String>, enter: bool) -> bool {
    let t = Tokens::current();
    if !*confirm {
        let before = name.clone();
        crate::dialogs::name_field(ui, name);
        // An error is about the name it was saved under.
        if *name != before {
            *error = None;
        }
        ui.label(egui::RichText::new(crate::localized_format!("Saved in {folder}", folder = dir.display())).small().color(t.text_dim));
        if let Some(e) = error {
            ui.label(egui::RichText::new(e.as_str()).small().color(t.rec));
        }
        return crate::dialogs::buttons_enabled(ui, "Save", enter, !name.trim().is_empty());
    }
    ui.label(crate::localized_format!("“{}” already exists. Do you want to replace it?", name.trim()));
    ui.separator();
    let mut replace = enter;
    ui.horizontal(|ui| {
        if ui.button(crate::localization::text("Cancel")).clicked() {
            *confirm = false;
            replace = false;
        } else if ui.add(egui::Button::new(egui::RichText::new(crate::localization::text("Replace")).strong()).fill(t.accent)).clicked() {
            replace = true;
        }
    });
    replace
}

/// Saves `track` as the preset `name` in `dir`. `Ok(false)` without saving when a preset of that
/// name exists and `replace` is false.
pub(crate) fn save(app: &mut SoundApp, track: TrackId, name: &str, dir: &Path, replace: bool) -> Result<bool, String> {
    let file = dir.join(format!("{}.json", soundcraft_engine::io::sanitize_name(name.trim())));
    if !replace && file.exists() {
        return Ok(false);
    }
    // `track.save_preset` writes the file but doesn't create its folder.
    std::fs::create_dir_all(dir).map_err(|e| crate::localized_format!("Couldn't create {folder}: {error}", folder = dir.display(), error = e))?;
    app.run("track.save_preset", json!({"path": file.to_string_lossy(), "track": track.0})).map_err(|e| crate::localization::error_text(&e))?;
    Ok(true)
}

/// Opens Recall Track Preset's path prompt for a preset file, starting in the Track Presets folder.
pub(crate) fn open_recall(app: &mut SoundApp) {
    let path = dir(app).map(|d| format!("{}/", d.display())).unwrap_or_default();
    let title = "Track — Recall Track Preset from File".into();
    app.dialogs.set(Dialog::PathPrompt { cmd: "track.load_preset".into(), title, path, key: "path".into() });
}

/// The Track menu's items after Save Track Preset...: the Recall Track Preset submenu and a path
/// prompt for a preset outside the folder, both for the selected tracks.
pub(crate) fn track_menu(app: &mut SoundApp, ui: &mut egui::Ui) {
    let tracks: Vec<u64> = app.engine.session().edit.selected_tracks.iter().map(|t| t.0).collect();
    ui.add_enabled_ui(!tracks.is_empty(), |ui| {
        recall_menu(app, ui, &tracks);
        if ui.button(crate::localization::text("Recall Track Preset from File...")).clicked() {
            open_recall(app);
            ui.close();
        }
    });
}

/// A track name's menu items: Save Track Preset... for that track (desktop only), and the Recall
/// Track Preset submenu for `tracks`.
pub(crate) fn name_menu(app: &mut SoundApp, ui: &mut egui::Ui, id: TrackId, tracks: &[u64]) {
    if dir(app).is_some() && ui.button(crate::localization::text("Save Track Preset...")).clicked() {
        open_save(app, id);
        ui.close();
    }
    recall_menu(app, ui, tracks);
}

/// The Recall Track Preset submenu: the presets in the folder, recalled onto `tracks`. A folder that
/// can't be read says why in the menu, a preset that can't be recalled in the status line
/// (`SoundApp::run`).
fn recall_menu(app: &mut SoundApp, ui: &mut egui::Ui, tracks: &[u64]) {
    if let Some(path) = presets_menu(ui, "Recall Track Preset", dir(app).as_deref()) {
        let _ = app.run("track.load_preset", json!({"path": path, "tracks": tracks}));
    }
}

/// What was picked in an insert or send slot's track preset items.
pub(crate) enum SlotPick {
    Save,
    /// The preset file to recall.
    Recall(String),
}

/// An insert or send slot's menu items, as in Pro Tools: Save Track Preset... for the clicked track
/// (desktop only), and Recall Inserts (or Recall Sends). For menus that can't lend `SoundApp` while
/// they draw; `slot_pick` acts on the pick.
pub(crate) fn slot_items(ui: &mut egui::Ui, dir: Option<&Path>, sends: bool) -> Option<SlotPick> {
    if dir.is_some() && ui.button(crate::localization::text("Save Track Preset...")).clicked() {
        ui.close();
        return Some(SlotPick::Save);
    }
    presets_menu(ui, if sends { "Recall Sends" } else { "Recall Inserts" }, dir).map(SlotPick::Recall)
}

/// Acts on `slot_items`' pick: Save Track Preset for `track`, or a recall whose inserts or sends land
/// on `track` starting at `slot`.
pub(crate) fn slot_pick(app: &mut SoundApp, track: TrackId, slot: usize, sends: bool, pick: SlotPick) {
    match pick {
        SlotPick::Save => {
            open_save(app, track);
        }
        SlotPick::Recall(path) => {
            let only = if sends { "sends" } else { "inserts" };
            let _ = app.run("track.load_preset", json!({"path": path, "tracks": [track.0], "only": only, "from": slot}));
        }
    }
}

/// `slot_items` and `slot_pick` together, for a menu that has `SoundApp` at hand.
pub(crate) fn slot_menu(app: &mut SoundApp, ui: &mut egui::Ui, track: TrackId, slot: usize, sends: bool) {
    if let Some(pick) = slot_items(ui, dir(app).as_deref(), sends) {
        slot_pick(app, track, slot, sends, pick);
    }
}

/// The presets in `dir` as a submenu; the path of the one picked.
fn presets_menu(ui: &mut egui::Ui, label: &str, dir: Option<&Path>) -> Option<String> {
    ui.menu_button(crate::localization::text(label), |ui| {
        let Some(dir) = dir else {
            ui.add_enabled(false, egui::Button::new(crate::localization::text("Track presets need the desktop app")));
            return None;
        };
        let names = match crate::listed_presets(ui, dir) {
            Ok(names) => names,
            Err(e) => {
                ui.add_enabled(false, egui::Button::new(&*e));
                return None;
            }
        };
        if names.is_empty() {
            ui.add_enabled(false, egui::Button::new(crate::localization::text("No Track Presets")));
            return None;
        }
        let mut picked = None;
        for n in names.iter() {
            if ui.button(n).clicked() {
                ui.close();
                picked = Some(dir.join(format!("{n}.json")).to_string_lossy().into_owned());
            }
        }
        picked
    })
    .inner
    .flatten()
}

/// The status line after a recall that made buses for its sends (`created_buses`) or left sends
/// out (`skipped_sends`). None when every send found its bus.
pub(crate) fn recall_note(result: &serde_json::Value) -> Option<String> {
    let list = |key: &str| -> Vec<String> {
        result.get(key).and_then(serde_json::Value::as_array).into_iter().flatten().filter_map(|x| x.as_str().map(str::to_string)).collect()
    };
    let mut notes: Vec<String> = list("created_buses").iter().map(|bus| crate::localized_format!("Created bus {bus}", bus = bus)).collect();
    notes.extend(
        list("skipped_sends")
            .iter()
            .map(|slot| crate::localized_format!("Send {slot} left out: the preset doesn't say which bus it sends to", slot = slot.to_uppercase())),
    );
    (!notes.is_empty()).then(|| notes.join(" · "))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    #[test]
    fn the_recall_note_names_made_buses_then_left_out_sends() {
        let note = super::recall_note(&json!({"created_buses": ["Room"], "skipped_sends": ["a", "c"]}));
        assert_eq!(
            note.as_deref(),
            Some(
                "Created bus Room · Send A left out: the preset doesn't say which bus it sends to · Send C left out: the preset doesn't say which bus it sends to"
            )
        );
        assert_eq!(super::recall_note(&json!({"created_buses": [], "skipped_sends": []})), None);
    }
}
