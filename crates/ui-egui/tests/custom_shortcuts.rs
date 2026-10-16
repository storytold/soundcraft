use egui::os::OperatingSystem;
use egui::{Key, Modifiers};
use serde_json::json;
use soundcraft_ui_egui::shortcuts::{CATEGORIES, bindable, chord_text, clashes, is_modifier, parse, shortcut_for};
use soundcraft_ui_egui::{Services, SoundApp, UiState, shortcut_editor};

const MAC_CMD: Modifiers = Modifiers { mac_cmd: true, command: true, ..Modifiers::NONE };
const PC_CTRL: Modifiers = Modifiers { ctrl: true, command: true, ..Modifiers::NONE };

fn app() -> SoundApp {
    SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default())
}

fn context(os: OperatingSystem) -> egui::Context {
    let ctx = egui::Context::default();
    ctx.set_os(os);
    ctx
}

fn send(ctx: &egui::Context, app: &mut SoundApp, events: Vec<egui::Event>) {
    ctx.run_ui(egui::RawInput { events, ..Default::default() }, |ui| soundcraft_ui_egui::shortcuts::handle(app, ui.ctx())).textures_delta.clear();
}

fn press(ctx: &egui::Context, app: &mut SoundApp, key: Key, modifiers: Modifiers) {
    send(ctx, app, [true, false].map(|pressed| egui::Event::Key { key, physical_key: None, pressed, repeat: false, modifiers }).to_vec());
}

fn set(app: &mut SoundApp, id: &str, shortcut: &str) -> serde_json::Value {
    app.run("ui.shortcuts", json!({"command": id, "shortcut": shortcut})).unwrap()
}

#[test]
fn chosen_shortcuts_beat_context_keys_and_free_the_old_ones() {
    let (mut app, ctx) = (app(), context(OperatingSystem::Windows));
    let markers = |app: &SoundApp| app.engine.session().markers.len();
    let before = markers(&app);
    set(&mut app, "window.big_counter", "F5");
    set(&mut app, "markers.add", "Alt+Shift+M");
    press(&ctx, &mut app, Key::F5, Modifiers::NONE);
    assert!(app.ui.show_big_counter);
    assert_ne!(app.engine.session().edit.tool, soundcraft_engine::model::Tool::Zoom);
    press(&ctx, &mut app, Key::Enter, Modifiers::NONE);
    assert_eq!(markers(&app), before);
    press(&ctx, &mut app, Key::M, Modifiers { alt: true, shift: true, ..Modifiers::NONE });
    assert_eq!(markers(&app), before + 1);
}

#[test]
fn a_captured_shortcut_replaces_the_built_in_one_until_reset() {
    for (os, cmd) in [(OperatingSystem::Mac, MAC_CMD), (OperatingSystem::Windows, PC_CTRL)] {
        let (mut app, ctx) = (app(), context(os));
        app.shortcut_editor.capturing = Some("window.big_counter");
        press(&ctx, &mut app, Key::Escape, Modifiers::NONE);
        assert!(app.shortcut_editor.capturing.is_none() && app.ui.shortcuts.is_empty());
        app.shortcut_editor.capturing = Some("window.big_counter");
        press(&ctx, &mut app, Key::ShiftLeft, Modifiers::SHIFT);
        press(&ctx, &mut app, Key::D, Modifiers { shift: true, ..cmd });
        assert_eq!((app.shortcut_editor.capturing, shortcut_for(&app.ui.shortcuts, "window.big_counter")), (None, Some("Cmd+Shift+D")), "{os:?}");
        press(&ctx, &mut app, Key::Num3, cmd);
        assert!(!app.ui.show_big_counter, "{os:?}");
        press(&ctx, &mut app, Key::D, Modifiers { shift: true, ..cmd });
        assert!(app.ui.show_big_counter, "{os:?}");
        app.run("ui.shortcuts", json!({"command": "window.big_counter", "reset": true})).unwrap();
        press(&ctx, &mut app, Key::Num3, cmd);
        assert!(!app.ui.show_big_counter && app.ui.shortcuts.is_empty(), "{os:?}");
    }
}

#[test]
fn capturing_names_keys_as_the_shortcut_handler_sees_them() {
    let (mut app, ctx) = (app(), context(OperatingSystem::Mac));
    app.shortcut_editor.capturing = Some("window.big_counter");
    press(&ctx, &mut app, Key::Questionmark, Modifiers::SHIFT);
    assert_eq!(app.shortcut_editor.clash, Some(("window.big_counter", "Shift+/".into())));
    assert_eq!(clashes(&app.ui.shortcuts, "Shift+/", "window.big_counter"), ["options.link_timeline_edit"]);
    app.shortcut_editor.capturing = Some("window.big_counter");
    send(&ctx, &mut app, vec![egui::Event::ModifiersChanged(Modifiers { shift: true, ..MAC_CMD }), egui::Event::Copy]);
    assert_eq!(shortcut_for(&app.ui.shortcuts, "window.big_counter"), Some("Cmd+Shift+C"));
}

#[test]
fn a_taken_key_waits_for_replace_and_reset_takes_it_back() {
    let (mut app, ctx) = (app(), context(OperatingSystem::Mac));
    app.shortcut_editor.capturing = Some("window.big_counter");
    press(&ctx, &mut app, Key::S, MAC_CMD);
    assert_eq!(app.shortcut_editor.clash, Some(("window.big_counter", "Cmd+S".into())));
    assert!(app.ui.shortcuts.is_empty());
    assert_eq!(set(&mut app, "window.big_counter", "Cmd+S")["taken_from"], json!(["session.save"]));
    assert_eq!(shortcut_for(&app.ui.shortcuts, "session.save"), None);
    press(&ctx, &mut app, Key::S, MAC_CMD);
    assert!(app.ui.show_big_counter);
    let r = app.run("ui.shortcuts", json!({"command": "session.save", "reset": true})).unwrap();
    assert_eq!((r["shortcut"].clone(), r["taken_from"].clone()), (json!("Cmd+S"), json!(["window.big_counter"])));
    assert_eq!(shortcut_for(&app.ui.shortcuts, "window.big_counter"), None);
    set(&mut app, "window.big_counter", "Cmd+3");
    assert!(app.ui.shortcuts.is_empty(), "built-in keys are not stored: {:?}", app.ui.shortcuts);
}

#[test]
fn ui_shortcuts_checks_its_input_and_is_saved_with_the_ui_prefs() {
    let mut app = app();
    for p in [
        json!({"command": "edit.mode", "shortcut": "F1"}),
        json!({"command": "no.such", "shortcut": "F1"}),
        json!({"command": "session.save", "shortcut": "Cmd+Banana"}),
        json!({"command": "session.save", "shortcut": "shift+cmd+s"}),
        json!({"command": "session.save"}),
    ] {
        assert!(app.run("ui.shortcuts", p.clone()).is_err(), "{p}");
    }
    assert!(app.ui.shortcuts.is_empty());
    set(&mut app, "session.save", "Shift+Cmd+F2");
    assert_eq!(app.ui.shortcuts.get("session.save").map(String::as_str), Some("Cmd+Shift+F2"));
    set(&mut app, "session.save", "");
    let list = app.run("ui.shortcuts", json!({})).unwrap();
    let save = list["shortcuts"].as_array().unwrap().iter().find(|r| r["id"] == "session.save").unwrap().clone();
    assert_eq!((save["shortcut"].clone(), save["default"].clone(), save["custom"].clone()), (json!(null), json!("Cmd+S"), json!(true)));
    let saved: UiState = serde_json::from_value(serde_json::to_value(&app.ui).unwrap()).unwrap();
    assert_eq!(saved.shortcuts, app.ui.shortcuts);
    let old: UiState = serde_json::from_value(json!({"show_big_counter": true})).unwrap();
    assert!(old.shortcuts.is_empty());
}

#[test]
fn every_listed_command_has_one_row_and_a_category() {
    let rows = bindable();
    assert_eq!(rows.iter().map(|b| b.id).collect::<std::collections::BTreeSet<_>>().len(), rows.len());
    for id in ["session.save", "edit.undo", "window.transport", "window.shortcuts", "transport.toggle", "transport.pause", "mix.mute"] {
        assert!(rows.iter().any(|b| b.id == id), "{id}");
    }
    for id in ["setup.keyboard_shortcuts", "engine.history", "ui.set", "view.mix_section", "audiosuite.process", "view.ruler"] {
        assert!(rows.iter().all(|b| b.id != id), "{id}");
    }
    let mut categories: Vec<&str> = rows.iter().map(|b| b.category).collect();
    categories.dedup();
    assert_eq!(categories, CATEGORIES);
    assert!(rows.iter().any(|b| b.id == "edit.cut_all_automation" && b.menu == "Edit › Cut Special"));
    assert!(rows.iter().filter(|b| b.editable).all(|b| b.default.is_none_or(|d| parse(d).is_some())));
    assert!(rows.iter().any(|b| b.id == "edit.mode" && !b.editable));
}

#[test]
fn key_presses_round_trip_through_shortcut_text() {
    for &key in Key::ALL.iter().filter(|k| !is_modifier(**k)) {
        for (mods, mac, want) in [
            (Modifiers::NONE, true, Modifiers::NONE),
            (Modifiers { alt: true, shift: true, ..MAC_CMD }, true, Modifiers { alt: true, shift: true, ..Modifiers::COMMAND }),
            (Modifiers { ctrl: true, ..MAC_CMD }, true, Modifiers { ctrl: true, ..Modifiers::COMMAND }),
            (Modifiers { shift: true, ..PC_CTRL }, false, Modifiers { shift: true, ..Modifiers::COMMAND }),
        ] {
            let text = chord_text(key, mods, mac);
            assert_eq!(parse(&text), Some((want, key)), "{key:?} → {text}");
        }
    }
}

#[test]
fn the_window_draws_in_every_state() {
    let mut app = app();
    app.ui.show_shortcuts = true;
    set(&mut app, "window.big_counter", "F12");
    let ctx = context(OperatingSystem::Mac);
    soundcraft_ui_egui::fonts::install(&ctx);
    ctx.run_ui(egui::RawInput::default(), |_| {}).textures_delta.clear();
    let frame = |app: &mut SoundApp| ctx.run_ui(egui::RawInput::default(), |ui| shortcut_editor::show(app, ui.ctx())).textures_delta.clear();
    frame(&mut app);
    app.shortcut_editor.capturing = Some("session.save");
    frame(&mut app);
    (app.shortcut_editor.capturing, app.shortcut_editor.clash) = (None, Some(("session.save", "Cmd+O".into())));
    frame(&mut app);
    (app.shortcut_editor.clash, app.shortcut_editor.query) = (None, "zzz-nothing".into());
    frame(&mut app);
    (app.shortcut_editor.query, app.shortcut_editor.category, app.shortcut_editor.changed_only) = (String::new(), Some("Transport"), true);
    frame(&mut app);
    (app.ui.show_shortcuts, app.shortcut_editor.capturing) = (false, Some("session.save"));
    frame(&mut app);
    assert_eq!(app.shortcut_editor.capturing, None);
}

#[test]
fn recording_tab_leaves_no_widget_focused() {
    let (mut app, ctx) = (app(), context(OperatingSystem::Mac));
    app.ui.show_shortcuts = true;
    let frame = |app: &mut SoundApp, events: Vec<egui::Event>| {
        ctx.run_ui(egui::RawInput { events, ..Default::default() }, |ui| {
            app.logic(ui.ctx());
            app.ui(ui);
        })
        .textures_delta
        .clear()
    };
    for _ in 0..3 {
        frame(&mut app, Vec::new());
    }
    for modifiers in [Modifiers::NONE, Modifiers::SHIFT] {
        app.shortcut_editor.capturing = Some("window.big_counter");
        let tab = [true, false].map(|pressed| egui::Event::Key { key: Key::Tab, physical_key: None, pressed, repeat: false, modifiers });
        frame(&mut app, tab.to_vec());
        assert_eq!(ctx.memory(|m| m.focused()), None, "{modifiers:?}");
    }
    assert_eq!(shortcut_for(&app.ui.shortcuts, "window.big_counter"), Some("Shift+Tab"));
}
