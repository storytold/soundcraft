use super::*;
use crate::{Services, SoundApp, UiState};
use serde_json::json;
use std::collections::BTreeSet;
use std::path::Path;

/// Strings shown through `tr` from a variable rather than a literal (option ids shown to the user,
/// enum labels from lower crates). Literal `tr("…")`/`trf("…")` calls are found by scanning the sources.
const INDIRECT: &[&str] = &[
    // Tempo / Time / MIDI Operations option lists.
    "constant",
    "linear",
    "parabolic",
    "s-curve",
    "scale",
    "stretch",
    "insert",
    "cut",
    "change meter",
    "move song start",
    "quantize",
    "transpose",
    "change velocity",
    "flatten performance",
    "restore performance",
    // Track views.
    "waveform",
    "blocks",
    "clip_gain",
    "playlists",
    "notes",
    "regions",
    // Automation-enable checkboxes.
    "send level",
    "send pan",
    "send mute",
    "plugin",
    // Grid / nudge values.
    "1 bar",
    "1/2",
    "1/4",
    "1/8",
    "1/16",
    "1/32",
    "1/64",
    "1/8t",
    "1/16t",
    "1/4.",
    "1 msec",
    "10 msec",
    "100 msec",
    "1 second",
    "1 frame",
    "100 samples",
    // Edit-mode cells.
    "SHUFFLE",
    "SPOT",
    "SLIP",
    "GRID",
    // Dialog suffix and the Inserts/Sends section titles.
    "INSERTS A-E",
    "INSERTS F-J",
    "SENDS A-E",
    "SENDS F-J",
    // Table headers and the About tabs.
    "Name",
    "Location",
    "Type",
    "Start (ticks)",
    "Note",
    "Vel",
    "Company",
    "Model",
    "Version",
    "Commits",
    "% of all commits",
    "Lines +/−",
    "About",
    "Contributors",
    "Models",
    // Dock tabs and status.
    "<factory default>",
    "movie offline",
];

/// Every `tr("…")` / `trf("…")` literal in `src`.
fn literals(src: &str, out: &mut BTreeSet<String>) {
    for call in ["tr(", "trf("] {
        let mut rest = src;
        while let Some(i) = rest.find(call) {
            let before = rest[..i].chars().next_back();
            rest = &rest[i + call.len()..];
            if before.is_some_and(|c| c.is_alphanumeric() || c == '_') {
                continue;
            }
            // The literal may sit on the next line (`tr(\n    "…"`); anything else is not a literal.
            let Some(lit) = rest.trim_start().strip_prefix('"') else { continue };
            rest = lit;
            let mut s = String::new();
            let mut chars = rest.chars();
            while let Some(c) = chars.next() {
                match c {
                    '"' => break,
                    '\\' => match chars.next() {
                        Some('n') => s.push('\n'),
                        Some(other) => s.push(other),
                        None => break,
                    },
                    c => s.push(c),
                }
            }
            out.insert(s);
        }
    }
}

fn scan(dir: &Path, out: &mut BTreeSet<String>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            if p.file_name().is_some_and(|n| n != "i18n") {
                scan(&p, out);
            }
        } else if p.extension().is_some_and(|x| x == "rs")
            && let Ok(src) = std::fs::read_to_string(&p)
        {
            // Test modules don't show anything.
            let src = src.split("\n#[cfg(test)]").next().unwrap_or("");
            literals(src, out);
        }
    }
}

fn menu_labels(nodes: &[crate::menus::MenuNode], out: &mut BTreeSet<String>) {
    for n in nodes {
        out.insert(n.label.clone());
        menu_labels(&n.children, out);
    }
}

/// Every English string the interface can show through `tr`.
fn all_keys() -> BTreeSet<String> {
    let mut keys = BTreeSet::new();
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    scan(&root.join("src"), &mut keys);
    scan(&root.join("../../apps/soundcraft/src"), &mut keys);
    menu_labels(&crate::menus::tree(), &mut keys);
    for c in soundcraft_engine::command_specs() {
        keys.insert(c.label.to_string());
        keys.extend(c.menu.iter().map(|m| m.to_string()));
    }
    for c in crate::menus::UI_COMMANDS {
        keys.insert(c.1.to_string());
    }
    for p in soundcraft_dsp::plugins() {
        keys.extend(p.params.iter().map(|x| x.name.to_string()));
    }
    use soundcraft_model::{AutomationMode, ChannelFormat, FadeShape, TrackHeight, TrackKind};
    keys.extend(TrackKind::ALL.iter().map(|k| k.label().to_string()));
    keys.extend(TrackHeight::ALL.iter().map(|k| k.label().to_string()));
    keys.extend(ChannelFormat::ALL.iter().map(|k| k.label().to_string()));
    keys.extend(FadeShape::ALL.iter().map(|k| k.label().to_string()));
    keys.extend(AutomationMode::ALL.iter().map(|k| k.label().to_string()));
    keys.extend(soundcraft_model::Tool::ALL.iter().map(|k| k.label().to_string()));
    keys.extend(soundcraft_time::TimeFormat::ALL.iter().map(|k| k.label().to_string()));
    keys.extend(soundcraft_time::NoteValue::ALL.iter().map(|k| k.label().to_string()));
    keys.extend(crate::theme::ThemeMode::ALL.iter().map(|k| k.label().to_string()));
    keys.extend(INDIRECT.iter().map(|k| k.to_string()));
    // Strings with no letters (symbols, numbers) read the same everywhere.
    keys.retain(|k| k.chars().any(char::is_alphabetic));
    keys
}

#[test]
fn every_visible_string_has_a_french_and_a_spanish_translation() {
    let mut rows = BTreeSet::new();
    for r in strings::STRINGS {
        assert!(rows.insert(r.0), "duplicate row `{}`", r.0);
    }
    let missing: Vec<String> = all_keys().into_iter().filter(|k| !rows.contains(k.as_str())).collect();
    assert!(missing.is_empty(), "{} strings have no translation row:\n{}", missing.len(), missing.join("\n"));
    let empty: Vec<&str> = strings::STRINGS.iter().filter(|r| r.1.is_empty() || r.2.is_empty()).map(|r| r.0).collect();
    assert!(empty.is_empty(), "rows missing French or Spanish:\n{}", empty.join("\n"));
}

#[test]
fn translations_keep_their_placeholders() {
    for (en, fr, es) in strings::STRINGS {
        let n = en.matches("{}").count();
        assert_eq!(fr.matches("{}").count(), n, "fr: {en} → {fr}");
        assert_eq!(es.matches("{}").count(), n, "es: {en} → {es}");
    }
}

#[test]
fn tr_follows_the_language_and_falls_back_to_english() {
    set(Language::Fr);
    assert_eq!(tr("File"), "Fichier");
    assert_eq!(tr("no such string"), "no such string");
    set(Language::Es);
    assert_eq!(tr("File"), "Archivo");
    assert_eq!(trf("{} tracks", &[&3]), "3 pistas");
    set(Language::En);
    assert_eq!(tr("File"), "File");
    assert_eq!(fill("{} of {} {{x}}", &[&1, &"two"]), "1 of two {x}");
    assert_eq!(fill("{} {}", &[&1]), "1 ");
}

#[test]
fn language_command_sets_reports_and_persists() {
    let mut app = SoundApp::new(soundcraft_engine::Engine::default(), None, Services::default());
    assert_eq!(app.run("ui.language", json!({})), Ok(json!({"lang": "en"})));
    assert_eq!(app.run("ui.language", json!({"lang": "fr"})), Ok(json!({"lang": "fr"})));
    assert_eq!(current(), Language::Fr);
    assert_eq!(app.run("ui.language", json!({"lang": "Español"})), Ok(json!({"lang": "es"})));
    assert!(app.run("ui.language", json!({"lang": "tlh"})).is_err());
    assert_eq!(app.ui.language, Language::Es);
    let back: UiState = serde_json::from_value(serde_json::to_value(&app.ui).unwrap_or_default()).unwrap_or_default();
    assert_eq!(back.language, Language::Es);
    // Prefs saved before the setting existed load as English.
    let old: UiState = serde_json::from_value(json!({"show_tracks_list": true})).unwrap_or_default();
    assert_eq!(old.language, Language::En);
    set(Language::En);
}

#[test]
fn the_menu_bar_draws_in_french_and_still_dispatches_english_paths() {
    let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
    let _ = app.run("ui.language", json!({"lang": "fr"}));
    let ctx = egui::Context::default();
    for _ in 0..2 {
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| app.ui(ui));
        out.textures_delta.clear();
    }
    assert_eq!(crate::menus::command_for_path("Window > Mix", &crate::menus::ui_aliases()).as_deref(), Some("window.mix"));
    assert_eq!(tr("Window"), "Fenêtre");
    set(Language::En);
}
