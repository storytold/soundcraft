//! Preferences window and persisted extra CLAP/VST3 search folders.
//! Folder changes take effect on restart, so editing preferences never loads plugin binaries.

use crate::SoundApp;
use egui::vec2;
use serde_json::{Value, json};

const MAX_FOLDERS: usize = 32;
const MAX_PATH_BYTES: usize = 4096;

#[derive(Debug, Clone, Default)]
pub struct Preferences {
    pub open: bool,
    folders: String,
    error: Option<String>,
}

pub fn command(app: &mut SoundApp, params: &Value) -> Result<Value, String> {
    let open = params.get("value").and_then(Value::as_bool).unwrap_or(true);
    if open && !app.preferences.open {
        app.preferences.folders = app.ui.plugin_folders.join("\n");
        app.preferences.error = None;
    }
    app.preferences.open = open;
    Ok(json!({"value": open}))
}

pub fn show(app: &mut SoundApp, ctx: &egui::Context) {
    let mut open = app.preferences.open;
    if !open {
        return;
    }
    egui::Window::new("Preferences").open(&mut open).default_size(vec2(470.0, 340.0)).show(ctx, |ui| {
        ui.horizontal_wrapped(|ui| {
            for (label, command) in
                [("Appearance", "window.ui_customization"), ("Playback Engine", "window.playback_engine"), ("Keyboard Shortcuts", "window.shortcuts")]
            {
                if ui.button(label).clicked() {
                    let _ = app.run(command, json!({"value": true}));
                }
            }
        });
        ui.separator();
        if cfg!(target_arch = "wasm32") {
            ui.label("Plugin folders are available in the desktop application.");
            return;
        }
        let label = ui.label(egui::RichText::new("Plugin folders").strong());
        ui.label("Additional CLAP and VST3 folders, one per line. Standard and environment-variable folders are always searched.");
        egui::ScrollArea::vertical().max_height(180.0).show(ui, |ui| {
            ui.add(
                egui::TextEdit::multiline(&mut app.preferences.folders)
                    .desired_rows(5)
                    .desired_width(f32::INFINITY)
                    .char_limit(MAX_FOLDERS * MAX_PATH_BYTES),
            )
            .labelled_by(label.id);
        });
        if let Some(pick) = &app.services.pick_folder
            && ui.button("Add folder…").clicked()
            && let Some(folder) = pick(app.ui.plugin_folders.last().map_or("", String::as_str))
        {
            if !app.preferences.folders.is_empty() && !app.preferences.folders.ends_with('\n') {
                app.preferences.folders.push('\n');
            }
            app.preferences.folders.push_str(&folder);
        }
        ui.label("Restart SoundCraft after applying folder changes.");
        if let Some(error) = &app.preferences.error {
            ui.colored_label(crate::theme::Tokens::current().rec, error);
        }
        ui.horizontal(|ui| {
            if ui.button("Apply").clicked() {
                match validate_folders(app.preferences.folders.lines()) {
                    Ok(folders) => {
                        // Changing the native code search path is a dialog-only action.
                        app.ui.plugin_folders = folders;
                        app.preferences.error = None;
                    }
                    Err(error) => app.preferences.error = Some(error),
                }
            }
            if ui.button("Revert").clicked() {
                app.preferences.folders = app.ui.plugin_folders.join("\n");
                app.preferences.error = None;
            }
        });
    });
    app.preferences.open = open;
}

/// Called once by the desktop host after loading preferences, before opening sessions.
pub fn load_plugin_folders(folders: &[String]) {
    match validate_folders(folders.iter().map(String::as_str)) {
        Ok(folders) => {
            for folder in folders {
                let path = std::path::Path::new(&folder);
                soundcraft_clap_host::register_search_dir(path);
                soundcraft_vst3_host::register_search_dir(path);
            }
        }
        Err(error) => log::warn!("ignoring invalid plugin folder preferences: {error}"),
    }
}

pub(crate) fn validate_folders<'a>(folders: impl Iterator<Item = &'a str>) -> Result<Vec<String>, String> {
    let mut result = Vec::new();
    for (index, folder) in folders.enumerate() {
        if index >= MAX_FOLDERS {
            return Err(format!("at most {MAX_FOLDERS} plugin folders are allowed"));
        }
        let folder = folder.trim();
        if folder.is_empty() {
            continue;
        }
        if result.len() >= MAX_FOLDERS || folder.len() > MAX_PATH_BYTES || folder.chars().any(char::is_control) {
            return Err(format!("use at most {MAX_FOLDERS} folders, each at most {MAX_PATH_BYTES} bytes and without control characters"));
        }
        if !std::path::Path::new(folder).is_absolute() {
            return Err("plugin folders must be absolute paths".into());
        }
        if !result.iter().any(|p| p == folder) {
            result.push(folder.to_string());
        }
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folder_drafts_are_normalized_and_bounded() {
        let path = std::env::temp_dir().join("soundcraft-plugins").to_string_lossy().into_owned();
        assert_eq!(
            validate_folders([format!("  {path}  "), String::new(), path.clone()].iter().map(String::as_str)).unwrap(),
            std::slice::from_ref(&path)
        );
        for invalid in [vec!["relative".to_string()], vec![format!("{path}\0")], vec![path.clone(); 33], vec![format!("{path}{}", "x".repeat(4096))]]
        {
            assert!(validate_folders(invalid.iter().map(String::as_str)).is_err());
        }
    }
}
