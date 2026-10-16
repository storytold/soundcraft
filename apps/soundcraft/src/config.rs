//! Per-user settings and automation profile directories.

use std::ffi::OsString;
use std::path::PathBuf;

#[derive(Clone, Copy)]
enum Platform {
    Mac,
    Windows,
    Other,
}

/// SoundCraft's settings folder. A nonempty `$SOUNDCRAFT_CONFIG_DIR` overrides the platform default.
/// The override is the complete directory; no application-name suffix is appended. An explicitly
/// empty value disables the settings directory instead of falling back to a personal profile.
pub fn config_dir() -> Option<PathBuf> {
    let platform = if cfg!(target_os = "macos") {
        Platform::Mac
    } else if cfg!(windows) {
        Platform::Windows
    } else {
        Platform::Other
    };
    config_dir_with(platform, |name| std::env::var_os(name))
}

fn config_dir_with(platform: Platform, env: impl Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    if let Some(dir) = env("SOUNDCRAFT_CONFIG_DIR") {
        return (!dir.is_empty()).then(|| PathBuf::from(dir));
    }
    match platform {
        Platform::Mac => env("HOME").map(|h| PathBuf::from(h).join("Library/Application Support/SoundCraft")),
        Platform::Windows => env("APPDATA").map(|a| PathBuf::from(a).join("SoundCraft")),
        Platform::Other => {
            env("XDG_CONFIG_HOME").map(PathBuf::from).or_else(|| env("HOME").map(|h| PathBuf::from(h).join(".config"))).map(|c| c.join("soundcraft"))
        }
    }
}

/// Read bounded settings, recovering unreadable docking metadata independently of other settings.
pub fn load_ui(path: &std::path::Path) -> Result<Option<soundcraft_ui_egui::UiState>, String> {
    read_ui(path, true)
}

fn read_ui(path: &std::path::Path, recover_docking: bool) -> Result<Option<soundcraft_ui_egui::UiState>, String> {
    use std::io::Read as _;
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("Settings could not be read: {error}")),
    };
    let mut bytes = Vec::new();
    file.take(MAX_UI_BYTES + 1).read_to_end(&mut bytes).map_err(|error| format!("Settings could not be read: {error}"))?;
    if bytes.len() as u64 > MAX_UI_BYTES {
        return Err("Settings file exceeds the 4 MiB limit".into());
    }
    let mut saved: serde_json::Value = serde_json::from_slice(&bytes).map_err(|error| format!("Settings could not be restored: {error}"))?;
    if recover_docking && let Some(fields) = saved.as_object_mut() {
        for (field, mix) in [("docking", false), ("mix_docking", true)] {
            let unreadable =
                fields.get(field).is_some_and(|value| match serde_json::from_value::<soundcraft_ui_egui::docking::Workspace>(value.clone()) {
                    Ok(workspace) => if mix { workspace.validate_mix() } else { workspace.validate() }.is_err(),
                    Err(_) => true,
                });
            if unreadable {
                fields.remove(field);
            }
        }
    }
    let state: soundcraft_ui_egui::UiState = serde_json::from_value(saved).map_err(|error| format!("Settings could not be restored: {error}"))?;
    state.docking.validate()?;
    state.mix_docking.validate_mix()?;
    Ok(Some(state))
}

const MAX_UI_BYTES: u64 = 4 * 1024 * 1024;

/// Write settings atomically. Preserve an unreadable or malformed existing file for recovery.
pub fn save_ui(path: &std::path::Path, ui: &soundcraft_ui_egui::UiState) -> Result<(), String> {
    use std::io::Write as _;
    // Loading can recover a future layout for this session, but saving must preserve its source.
    read_ui(path, false)?;
    ui.docking.validate()?;
    ui.mix_docking.validate_mix()?;
    let mut saved = ui.clone();
    saved.plugin_windows.clear();
    saved.status.clear();
    let bytes = serde_json::to_vec_pretty(&saved).map_err(|error| error.to_string())?;
    if bytes.len() as u64 > MAX_UI_BYTES {
        return Err("Settings exceed the 4 MiB limit".into());
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let nonce = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_err(|error| error.to_string())?.as_nanos();
    let temporary = path.with_extension(format!("json.{}.{nonce}.tmp", std::process::id()));
    let mut file =
        std::fs::OpenOptions::new().write(true).create_new(true).open(&temporary).map_err(|error| format!("Settings could not be saved: {error}"))?;
    let written = (|| -> std::io::Result<()> {
        file.write_all(&bytes)?;
        file.sync_all()?;
        std::fs::rename(&temporary, path)
    })();
    if written.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    written.map_err(|error| format!("Settings could not be saved: {error}"))
}

#[cfg(test)]
mod config_tests {
    use super::*;

    #[test]
    fn plugin_folders_survive_saving_and_loading_preferences() {
        let directory = std::env::temp_dir().join(format!("soundcraft-plugin-prefs-{}", std::process::id()));
        let path = directory.join("ui.json");
        let ui = soundcraft_ui_egui::UiState { plugin_folders: vec![directory.join("plugins").to_string_lossy().into_owned()], ..Default::default() };
        save_ui(&path, &ui).unwrap();
        assert_eq!(load_ui(&path).unwrap().unwrap().plugin_folders, ui.plugin_folders);
        std::fs::remove_file(path).unwrap();
        std::fs::remove_dir(directory).unwrap();
    }

    #[test]
    fn native_file_reload_preserves_edit_mix_and_hidden_floating_panels() {
        use soundcraft_ui_egui::{MainWindow, Services, SoundApp, docking::Panel};
        let directory = std::env::temp_dir().join(format!("soundcraft-dock-prefs-{}", std::process::id()));
        let path = directory.join("ui.json");
        let _ = std::fs::remove_dir_all(&directory);
        let mut app = SoundApp::new(soundcraft_engine::Engine::default(), None, Services::default());
        app.run("ui.dock", serde_json::json!({"operation":"float","panel":"tracks","rect":[80,90,260,400]})).unwrap();
        app.run("ui.dock", serde_json::json!({"operation":"close","panel":"tracks"})).unwrap();
        app.ui.theme = soundcraft_ui_egui::theme::ThemeMode::Light;
        let edit = serde_json::to_value(&app.ui.docking).unwrap();
        app.ui.window = MainWindow::Mix;
        app.run("ui.dock", serde_json::json!({"operation":"open","panel":"tracks"})).unwrap();
        app.run("ui.dock", serde_json::json!({"operation":"float","panel":"tracks","rect":[140,150,280,420]})).unwrap();
        save_ui(&path, &app.ui).unwrap();
        app.ui = load_ui(&path).unwrap().unwrap();
        assert_eq!(app.ui.theme, soundcraft_ui_egui::theme::ThemeMode::Light);
        assert_eq!(serde_json::to_value(&app.ui.docking).unwrap(), edit);
        assert_eq!(app.ui.mix_docking.layout.floating.iter().find(|g| g.panels.contains(&Panel::Tracks)).unwrap().rect, [140.0, 150.0, 280.0, 420.0]);
        app.ui.window = MainWindow::Edit;
        app.run("ui.dock", serde_json::json!({"operation":"open","panel":"tracks"})).unwrap();
        assert_eq!(app.ui.docking.layout.floating.iter().find(|g| g.panels.contains(&Panel::Tracks)).unwrap().rect, [80.0, 90.0, 260.0, 400.0]);
        let mut invalid_mix = app.ui.clone();
        invalid_mix.mix_docking = soundcraft_ui_egui::docking::Workspace::default();
        let invalid_bytes = serde_json::to_vec(&invalid_mix).unwrap();
        std::fs::write(&path, &invalid_bytes).unwrap();
        let recovered = load_ui(&path).unwrap().unwrap();
        assert_eq!(recovered.theme, app.ui.theme);
        assert_eq!(serde_json::to_value(recovered.docking).unwrap(), serde_json::to_value(&app.ui.docking).unwrap());
        recovered.mix_docking.validate_mix().unwrap();
        assert!(save_ui(&path, &app.ui).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), invalid_bytes);
        std::fs::write(&path, b"damaged settings").unwrap();
        assert!(load_ui(&path).is_err());
        assert!(save_ui(&path, &app.ui).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"damaged settings");
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn future_edit_and_mix_layouts_preserve_other_preferences_and_the_original_file() {
        use soundcraft_ui_egui::{Services, SoundApp};
        let directory = std::env::temp_dir().join(format!("soundcraft-future-dock-prefs-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("ui.json");
        let mut app = SoundApp::new(soundcraft_engine::Engine::default(), None, Services::default());
        app.run("ui.dock", serde_json::json!({"operation":"float","panel":"tracks","rect":[80,90,260,400]})).unwrap();
        app.ui.theme = soundcraft_ui_egui::theme::ThemeMode::Light;
        app.ui.narrow_mix = true;
        for field in ["docking", "mix_docking"] {
            for invalid in [serde_json::json!({"layout":{"root":{"FutureLayout":{}}}}), serde_json::json!(false)] {
                let mut saved = serde_json::to_value(&app.ui).unwrap();
                saved[field] = invalid;
                assert!(serde_json::from_value::<soundcraft_ui_egui::UiState>(saved.clone()).is_err());
                let bytes = serde_json::to_vec(&saved).unwrap();
                std::fs::write(&path, &bytes).unwrap();
                let recovered = load_ui(&path).unwrap().unwrap();
                assert_eq!(recovered.theme, app.ui.theme);
                assert!(recovered.narrow_mix);
                recovered.docking.validate().unwrap();
                recovered.mix_docking.validate_mix().unwrap();
                let other = if field == "docking" { "mix_docking" } else { "docking" };
                assert_eq!(serde_json::to_value(&recovered).unwrap()[other], saved[other]);
                assert!(save_ui(&path, &recovered).is_err());
                assert_eq!(std::fs::read(&path).unwrap(), bytes);
            }
        }
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn oversized_settings_are_rejected_and_preserved() {
        let directory = std::env::temp_dir().join(format!("soundcraft-bounded-prefs-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("ui.json");
        let bytes = vec![b' '; MAX_UI_BYTES as usize + 1];
        std::fs::write(&path, &bytes).unwrap();
        assert!(load_ui(&path).is_err());
        assert!(save_ui(&path, &soundcraft_ui_egui::UiState::default()).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        std::fs::remove_dir_all(directory).unwrap();
    }

    fn resolve(platform: Platform, vars: &[(&str, &str)]) -> Option<PathBuf> {
        config_dir_with(platform, |name| vars.iter().find_map(|(key, value)| (*key == name).then(|| OsString::from(value))))
    }

    #[test]
    fn config_override_wins_without_reading_user_profile() {
        for platform in [Platform::Mac, Platform::Windows, Platform::Other] {
            let actual = config_dir_with(platform, |name| {
                assert_eq!(name, "SOUNDCRAFT_CONFIG_DIR");
                Some(OsString::from("isolated/settings"))
            });
            assert_eq!(actual, Some(PathBuf::from("isolated/settings")));
        }
    }

    #[test]
    fn config_empty_override_disables_profile_without_reading_defaults() {
        for platform in [Platform::Mac, Platform::Windows, Platform::Other] {
            assert_eq!(
                config_dir_with(platform, |name| {
                    assert_eq!(name, "SOUNDCRAFT_CONFIG_DIR");
                    Some(OsString::new())
                }),
                None
            );
        }
    }

    #[test]
    fn config_unset_override_keeps_platform_defaults() {
        let vars = [("HOME", "/user"), ("APPDATA", "/roaming"), ("XDG_CONFIG_HOME", "/xdg")];
        assert_eq!(resolve(Platform::Mac, &vars), Some(PathBuf::from("/user/Library/Application Support/SoundCraft")));
        assert_eq!(resolve(Platform::Windows, &vars), Some(PathBuf::from("/roaming/SoundCraft")));
        assert_eq!(resolve(Platform::Other, &vars), Some(PathBuf::from("/xdg/soundcraft")));
        assert_eq!(resolve(Platform::Other, &[("HOME", "/user")]), Some(PathBuf::from("/user/.config/soundcraft")));
        for platform in [Platform::Mac, Platform::Windows, Platform::Other] {
            assert_eq!(resolve(platform, &[]), None);
        }
    }

    #[cfg(unix)]
    #[test]
    fn config_override_preserves_non_unicode_paths() {
        use std::os::unix::ffi::OsStringExt as _;
        let value = OsString::from_vec(b"isolated/\xff".to_vec());
        assert_eq!(config_dir_with(Platform::Mac, |_| Some(value.clone())), Some(PathBuf::from(value)));
    }
}
