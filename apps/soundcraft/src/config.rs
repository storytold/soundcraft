//! Per-user settings and automation profile directories.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// Portable mode (#268): this file beside the executable keeps settings, autosaves, presets and
/// logs in [`PORTABLE_DATA_DIR`] next to it instead of the user profile.
const PORTABLE_MARKER: &str = "portable.txt";
const PORTABLE_DATA_DIR: &str = "SoundCraftData";

#[derive(Clone, Copy)]
enum Platform {
    Mac,
    Windows,
    Other,
}

/// SoundCraft's settings folder. A nonempty `$SOUNDCRAFT_CONFIG_DIR` overrides the platform default.
/// The override is the complete directory; no application-name suffix is appended. An explicitly
/// empty value disables the settings directory instead of falling back to a personal profile.
/// Otherwise `portable.txt` beside the executable selects `<exe dir>/SoundCraftData`, when that
/// folder can be written (a read-only medium falls back to the platform default).
pub fn config_dir() -> Option<PathBuf> {
    static DIR: std::sync::OnceLock<Option<PathBuf>> = std::sync::OnceLock::new();
    DIR.get_or_init(|| {
        let exe = std::env::current_exe().ok();
        resolve(exe.as_deref().and_then(Path::parent))
    })
    .clone()
}

fn resolve(exe_dir: Option<&Path>) -> Option<PathBuf> {
    let platform = if cfg!(target_os = "macos") {
        Platform::Mac
    } else if cfg!(windows) {
        Platform::Windows
    } else {
        Platform::Other
    };
    config_dir_with(platform, |name| std::env::var_os(name), exe_dir)
}

fn config_dir_with(platform: Platform, env: impl Fn(&str) -> Option<OsString>, exe_dir: Option<&Path>) -> Option<PathBuf> {
    if let Some(dir) = env("SOUNDCRAFT_CONFIG_DIR") {
        return (!dir.is_empty()).then(|| PathBuf::from(dir));
    }
    if let Some(exe_dir) = exe_dir.filter(|d| d.join(PORTABLE_MARKER).is_file()) {
        let data = exe_dir.join(PORTABLE_DATA_DIR);
        match writable(&data) {
            Ok(()) => return Some(data),
            Err(e) => log::warn!("portable mode: {} can't be written ({e}); using the user profile instead", data.display()),
        }
    }
    match platform {
        Platform::Mac => env("HOME").map(|h| PathBuf::from(h).join("Library/Application Support/SoundCraft")),
        Platform::Windows => env("APPDATA").map(|a| PathBuf::from(a).join("SoundCraft")),
        Platform::Other => {
            env("XDG_CONFIG_HOME").map(PathBuf::from).or_else(|| env("HOME").map(|h| PathBuf::from(h).join(".config"))).map(|c| c.join("soundcraft"))
        }
    }
}

/// Create `dir` if needed and prove a file can be written in it.
fn writable(dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let probe = dir.join(format!(".write-test-{}", std::process::id()));
    std::fs::write(&probe, b"")?;
    std::fs::remove_file(probe)
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

/// Read the engine-owned preference document (`interface.language`, …), bounded and `None` when
/// this profile has none yet.
pub fn load_preferences(path: &Path) -> Result<Option<Vec<u8>>, String> {
    use std::io::Read as _;
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("Preferences could not be read: {error}")),
    };
    let mut bytes = Vec::new();
    file.take(soundcraft_engine::prefs::MAX_BYTES + 1).read_to_end(&mut bytes).map_err(|error| format!("Preferences could not be read: {error}"))?;
    if bytes.len() as u64 > soundcraft_engine::prefs::MAX_BYTES {
        return Err(format!("Preferences exceed the {} KiB limit", soundcraft_engine::prefs::MAX_BYTES / 1024));
    }
    Ok(Some(bytes))
}

/// Write the preference document atomically (temporary file + rename), so a crash mid-write
/// leaves the previous file in place.
pub fn save_preferences(path: &Path, json: &str) -> Result<(), String> {
    use std::io::Write as _;
    let bytes = json.as_bytes();
    if bytes.len() as u64 > soundcraft_engine::prefs::MAX_BYTES {
        return Err(format!("Preferences exceed the {} KiB limit", soundcraft_engine::prefs::MAX_BYTES / 1024));
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| format!("Preferences could not be saved: {error}"))?;
    }
    let nonce = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_err(|error| error.to_string())?.as_nanos();
    let temporary = path.with_extension(format!("json.{}.{nonce}.tmp", std::process::id()));
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
        .map_err(|error| format!("Preferences could not be saved: {error}"))?;
    let written = (|| -> std::io::Result<()> {
        file.write_all(bytes)?;
        file.sync_all()?;
        std::fs::rename(&temporary, path)
    })();
    if written.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    written.map_err(|error| format!("Preferences could not be saved: {error}"))
}

/// The interface language older builds saved in `ui.json`, for the one-time migration into
/// `preferences.json`. Anything else in that file is ignored (and still loaded as UI state).
pub fn legacy_language(path: &Path) -> Option<String> {
    use std::io::Read as _;
    let mut bytes = Vec::new();
    std::fs::File::open(path).ok()?.take(MAX_UI_BYTES + 1).read_to_end(&mut bytes).ok()?;
    if bytes.len() as u64 > MAX_UI_BYTES {
        return None;
    }
    let saved: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    let language = saved.get("language")?.as_str()?.trim();
    // Only a value the preference accepts is worth migrating; anything else stays behind.
    soundcraft_engine::prefs::valid_language(language).then(|| language.to_owned())
}

/// A one-time migration: when there is no preference document yet, adopt the language `ui.json`
/// keeps from the version that stored the setting there. Returns whether it wrote the file.
pub fn migrate_language(engine: &mut soundcraft_engine::Engine, ui_path: &Path, preferences_path: &Path) -> Result<bool, String> {
    if preferences_path.is_file() {
        return Ok(false);
    }
    let Some(language) = legacy_language(ui_path) else { return Ok(false) };
    if language.eq_ignore_ascii_case("auto") {
        return Ok(false);
    }
    // Store the registry's canonical lowercase code: an old `zh-Hans` becomes `zh-hans`.
    let language = soundcraft_ui_egui::i18n::Lang::from_code(&language).map_or(language, |lang| lang.code().to_owned());
    engine
        .execute("prefs.set", &serde_json::json!({"path": soundcraft_engine::prefs::LANGUAGE, "value": language}))
        .map_err(|error| format!("Preferences could not be migrated: {error}"))?;
    save_preferences(preferences_path, &engine.prefs_json().map_err(|error| error.to_string())?)?;
    Ok(true)
}

#[cfg(test)]
mod config_tests {
    use super::*;

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
        config_dir_with(platform, |name| vars.iter().find_map(|(key, value)| (*key == name).then(|| OsString::from(value))), None)
    }

    #[test]
    fn config_override_wins_without_reading_user_profile() {
        for platform in [Platform::Mac, Platform::Windows, Platform::Other] {
            let actual = config_dir_with(
                platform,
                |name| {
                    assert_eq!(name, "SOUNDCRAFT_CONFIG_DIR");
                    Some(OsString::from("isolated/settings"))
                },
                None,
            );
            assert_eq!(actual, Some(PathBuf::from("isolated/settings")));
        }
    }

    #[test]
    fn config_empty_override_disables_profile_without_reading_defaults() {
        for platform in [Platform::Mac, Platform::Windows, Platform::Other] {
            assert_eq!(
                config_dir_with(
                    platform,
                    |name| {
                        assert_eq!(name, "SOUNDCRAFT_CONFIG_DIR");
                        Some(OsString::new())
                    },
                    None
                ),
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

    #[test]
    fn portable_txt_beside_the_exe_keeps_settings_next_to_it() {
        let exe = std::env::temp_dir().join(format!("soundcraft-portable-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&exe);
        std::fs::create_dir_all(&exe).unwrap();
        let vars = [("APPDATA", "/roaming")];
        assert_eq!(resolve_with_exe(&vars, &exe), Some(PathBuf::from("/roaming/SoundCraft")), "no marker: the profile");
        std::fs::write(exe.join("portable.txt"), b"").unwrap();
        assert_eq!(resolve_with_exe(&vars, &exe), Some(exe.join("SoundCraftData")));
        assert!(exe.join("SoundCraftData").is_dir());
        // The explicit override still wins.
        assert_eq!(resolve_with_exe(&[("SOUNDCRAFT_CONFIG_DIR", "/custom")], &exe), Some(PathBuf::from("/custom")));
        // A data folder that can't be made (a file is in the way) falls back to the profile.
        std::fs::remove_dir_all(exe.join("SoundCraftData")).unwrap();
        std::fs::write(exe.join("SoundCraftData"), b"").unwrap();
        assert_eq!(resolve_with_exe(&vars, &exe), Some(PathBuf::from("/roaming/SoundCraft")));
        std::fs::remove_dir_all(exe).unwrap();
    }

    fn resolve_with_exe(vars: &[(&str, &str)], exe: &Path) -> Option<PathBuf> {
        config_dir_with(Platform::Windows, |name| vars.iter().find_map(|(key, value)| (*key == name).then(|| OsString::from(value))), Some(exe))
    }

    #[cfg(unix)]
    #[test]
    fn config_override_preserves_non_unicode_paths() {
        use std::os::unix::ffi::OsStringExt as _;
        let value = OsString::from_vec(b"isolated/\xff".to_vec());
        assert_eq!(config_dir_with(Platform::Mac, |_| Some(value.clone()), None), Some(PathBuf::from(value)));
    }

    #[test]
    fn preferences_round_trip_atomically_and_are_size_capped() {
        let directory = std::env::temp_dir().join(format!("soundcraft-preferences-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&directory);
        let path = directory.join("preferences.json");
        assert_eq!(load_preferences(&path).unwrap(), None, "a fresh profile has no file");
        let document = r#"{"interface":{"language":"zh-hans"}}"#;
        save_preferences(&path, document).unwrap();
        assert_eq!(load_preferences(&path).unwrap(), Some(document.as_bytes().to_vec()), "a missing parent directory is created");
        let oversized = " ".repeat(soundcraft_engine::prefs::MAX_BYTES as usize + 1);
        assert!(save_preferences(&path, &oversized).is_err());
        std::fs::write(&path, &oversized).unwrap();
        assert!(load_preferences(&path).is_err());
        assert_eq!(std::fs::read(&path).unwrap().len(), oversized.len(), "the oversized file is preserved");
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn the_legacy_ui_json_language_migrates_once_into_the_preferences_file() {
        let directory = std::env::temp_dir().join(format!("soundcraft-language-migration-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).unwrap();
        let ui = directory.join("ui.json");
        let preferences = directory.join("preferences.json");
        let mut engine = soundcraft_engine::Engine::default();
        assert!(!migrate_language(&mut engine, &ui, &preferences).unwrap(), "nothing to migrate without ui.json");
        assert_eq!(legacy_language(&ui), None);
        std::fs::write(&ui, br#"{"theme":"Light","language":"zh-Hans"}"#).unwrap();
        assert_eq!(legacy_language(&ui).as_deref(), Some("zh-Hans"), "the old file's spelling is read as-is");
        assert!(migrate_language(&mut engine, &ui, &preferences).unwrap());
        assert_eq!(engine.prefs().interface.language, "zh-hans", "the preference stores the registry's canonical code");
        assert!(load_preferences(&preferences).unwrap().is_some());
        // Once migrated, `ui.json` no longer matters.
        std::fs::write(&ui, br#"{"language":"en"}"#).unwrap();
        assert!(!migrate_language(&mut engine, &ui, &preferences).unwrap());
        // `auto` is the default, so there is nothing worth writing.
        std::fs::remove_file(&preferences).unwrap();
        std::fs::write(&ui, br#"{"language":"auto"}"#).unwrap();
        assert!(!migrate_language(&mut engine, &ui, &preferences).unwrap());
        std::fs::write(&ui, b"damaged settings").unwrap();
        assert_eq!(legacy_language(&ui), None);
        // A value the preference would reject is left behind rather than failing every start.
        std::fs::write(&ui, br#"{"language":"zh Hans"}"#).unwrap();
        assert_eq!(legacy_language(&ui), None);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn an_old_ui_json_with_a_language_field_still_loads_unchanged() {
        // Removing `UiState::language` must not strand a profile: unknown fields are ignored and
        // loading never rewrites the file (the migration reads it separately).
        let directory = std::env::temp_dir().join(format!("soundcraft-legacy-ui-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("ui.json");
        let document = br#"{"theme":"light","language":"zh-Hans","show_transport":true}"#;
        std::fs::write(&path, document).unwrap();
        let ui = load_ui(&path).unwrap().expect("an old file still loads");
        assert!(matches!(ui.theme, soundcraft_ui_egui::theme::ThemeMode::Light));
        assert!(ui.show_transport);
        assert_eq!(std::fs::read(&path).unwrap(), document, "loading does not rewrite the file");
        std::fs::remove_dir_all(directory).unwrap();
    }
}
