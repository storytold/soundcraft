//! Help › SoundCraft Help: the Help menu, Cmd+? (Ctrl+Shift+/ off macOS) and the help window.

use egui::os::OperatingSystem;
use egui::{Event, Key, Modifiers, RawInput};
use serde_json::json;
use soundcraft_ui_egui::help::{self, Page, Tab};
use soundcraft_ui_egui::{Services, SoundApp, menus, shortcuts};

const MAC_CMD: Modifiers = Modifiers { mac_cmd: true, command: true, ..Modifiers::NONE };
const PC_CTRL: Modifiers = Modifiers { ctrl: true, command: true, ..Modifiers::NONE };
const OSES: [(OperatingSystem, Modifiers); 3] =
    [(OperatingSystem::Mac, MAC_CMD), (OperatingSystem::Windows, PC_CTRL), (OperatingSystem::Nix, PC_CTRL)];

fn press(ctx: &egui::Context, app: &mut SoundApp, key: Key, modifiers: Modifiers) {
    let presses = [true, false].map(|pressed| Event::Key { key, physical_key: None, pressed, repeat: false, modifiers });
    let events = [Event::ModifiersChanged(modifiers)].into_iter().chain(presses).collect();
    ctx.run_ui(RawInput { events, ..Default::default() }, |ui| shortcuts::handle(app, ui.ctx())).textures_delta.clear();
}

fn rig(os: OperatingSystem) -> (egui::Context, SoundApp) {
    let ctx = egui::Context::default();
    ctx.set_os(os);
    (ctx, SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default()))
}

#[test]
fn cmd_question_mark_opens_help_on_every_platform() {
    for (os, cmd) in OSES {
        // Some keyboards report Shift+/ as `?`, others as Shift plus `/`; `?` alone counts on layouts where it isn't Shift+/.
        for (key, mods) in [(Key::Questionmark, cmd | Modifiers::SHIFT), (Key::Slash, cmd | Modifiers::SHIFT), (Key::Questionmark, cmd)] {
            let (ctx, mut app) = rig(os);
            press(&ctx, &mut app, key, mods);
            assert!(app.ui.show_help, "{os:?} {key:?} {mods:?}");
            // Pressing it again keeps help open rather than closing it.
            press(&ctx, &mut app, key, mods);
            assert!(app.ui.show_help, "{os:?} {key:?} {mods:?} again");
        }
    }
}

#[test]
fn neighbouring_slash_keys_keep_their_own_commands() {
    for (os, cmd) in OSES {
        let (ctx, mut app) = rig(os);
        let linked = app.engine.session().edit.link_timeline_edit;
        // Shift+/ is Link Timeline and Edit Selection, Cmd+/ and Cmd+Alt+/ write automation.
        press(&ctx, &mut app, Key::Slash, Modifiers::SHIFT);
        assert_ne!(app.engine.session().edit.link_timeline_edit, linked, "{os:?}");
        press(&ctx, &mut app, Key::Slash, cmd);
        press(&ctx, &mut app, Key::Slash, cmd | Modifiers::ALT);
        press(&ctx, &mut app, Key::Slash, cmd | Modifiers::ALT | Modifiers::SHIFT);
        assert!(!app.ui.show_help, "{os:?}");
    }
}

#[test]
fn help_is_the_last_menu_and_every_item_runs_something() {
    let tree = menus::tree();
    let last = tree.last().unwrap();
    assert_eq!(last.label, "Help");
    let labels: Vec<&str> = last.children.iter().map(|n| n.label.as_str()).collect();
    assert_eq!(labels, ["SoundCraft Help", "Search Help...", "Help Index", "Keyboard Shortcuts", "Report a Problem..."]);
    let aliases = menus::ui_aliases();
    for n in &last.children {
        assert!(menus::command_for_path(&n.path, &aliases).is_some(), "{} does nothing", n.path);
    }
}

#[test]
fn help_menu_items_open_help_where_they_say() {
    let (_, mut app) = rig(OperatingSystem::Mac);
    menus::invoke_menu(&mut app, "help.index", "Help > Help Index");
    assert!(app.ui.show_help);
    assert_eq!(app.ui.help.tab, Tab::Index);
    menus::invoke_menu(&mut app, "help.search", "Help > Search Help...");
    assert_eq!(app.ui.help.tab, Tab::Search);
    assert!(app.ui.help.focus_search);
    menus::invoke_menu(&mut app, "window.shortcuts", "Help > Keyboard Shortcuts");
    assert!(app.ui.show_shortcuts);
    menus::invoke_menu(&mut app, "window.shortcuts", "Help > Keyboard Shortcuts");
    assert!(app.ui.show_shortcuts, "the menu opens the window, it never closes it");
    let r = app.run("help.report", json!({})).unwrap();
    assert_eq!(r["url"], help::REPORT_URL);
    assert!(app.ui.help.open_report);
    app.run("help.open", json!({"value": false})).unwrap();
    assert!(!app.ui.show_help);
}

#[test]
fn every_help_page_draws_without_closing_help() {
    let (ctx, mut app) = rig(OperatingSystem::Mac);
    app.ui.show_help = true;
    let pages = help::pages();
    assert!(pages.len() > 500, "{} pages", pages.len());
    for page in pages {
        for tab in [Tab::Contents, Tab::Index, Tab::Search] {
            app.ui.help.go(page.clone());
            app.ui.help.tab = tab;
            app.ui.help.query = "mix".into();
            ctx.run_ui(RawInput::default(), |ui| help::show(&mut app, ui.ctx())).textures_delta.clear();
            assert!(app.ui.show_help, "{page:?} closed help");
        }
    }
    assert!(matches!(app.ui.help.page, Page::Process(_) | Page::Command(_)));
}

/// With the app's real fonts every character on every help page has a glyph: the test harness
/// panics on one that would draw as a box. On a small screen every page also fits inside it:
/// nothing on a page may push the help window past the edge.
#[test]
fn every_help_page_draws_with_real_glyphs_and_fits_a_small_screen() {
    let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
    app.ui.show_help = true;
    let mut h = egui_kittest::Harness::builder().with_size(egui::vec2(760.0, 700.0)).build_ui_state(
        |ui, app: &mut SoundApp| {
            let ctx = ui.ctx().clone();
            app.logic(&ctx);
            app.ui(ui);
        },
        app,
    );
    h.run_steps(2);
    let tabs = [Tab::Contents, Tab::Index, Tab::Search];
    for (i, page) in help::pages().into_iter().enumerate() {
        let s = h.state_mut();
        s.ui.help.go(page);
        s.ui.help.tab = tabs[i % 3];
        s.ui.help.query = ["mix", "cmd+s", "loop record"][i % 3].into();
        h.step();
        h.step();
        assert!(h.state().ui.show_help);
        let page = &h.state().ui.help.page;
        let window = h.ctx.memory(|m| m.area_rect(egui::Id::unique(Some("SoundCraft Help")))).unwrap();
        assert!(window.max.x <= 760.0 && window.min.x >= 0.0, "{page:?} widens help to {window:?}");
    }
}
