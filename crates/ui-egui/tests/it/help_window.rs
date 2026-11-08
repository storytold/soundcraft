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

/// Accept the supported question-mark spellings on macOS, Windows and Linux without toggling Help closed.
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

/// A saved binding replaces both default spellings without taking over an existing command.
#[test]
fn rebound_help_shortcut_replaces_the_default() {
    for (os, cmd) in OSES {
        let (ctx, mut app) = rig(os);
        app.ui.shortcut_overrides.insert("help.open".into(), "Cmd+Shift+A".into());
        press(&ctx, &mut app, Key::Questionmark, cmd);
        press(&ctx, &mut app, Key::Slash, cmd | Modifiers::SHIFT);
        assert!(!app.ui.show_help);
        press(&ctx, &mut app, Key::A, cmd | Modifiers::SHIFT);
        assert!(app.ui.show_help);
        let (ctx, mut app) = rig(os);
        app.ui.shortcut_overrides.insert("help.open".into(), "Cmd+Shift+N".into());
        press(&ctx, &mut app, Key::N, cmd | Modifiers::SHIFT);
        assert!(!app.ui.show_help, "an existing command cannot become the Help shortcut");
        app.dialogs.open = None;
        press(&ctx, &mut app, Key::Questionmark, cmd);
        assert!(app.ui.show_help, "a conflicting saved override falls back to the default");
    }
}

/// Ensure the Help chord does not take over existing slash-key commands.
#[test]
fn neighbouring_slash_keys_keep_their_own_commands() {
    for (os, cmd) in OSES {
        let (ctx, mut app) = rig(os);
        let linked = app.engine.session().edit.link_timeline_edit;
        // Shift+/ is Link Timeline and Edit Selection, Cmd+/ and Cmd+Alt+/ write automation.
        press(&ctx, &mut app, Key::Slash, Modifiers::SHIFT);
        assert_ne!(app.engine.session().edit.link_timeline_edit, linked, "{os:?}");
        app.run("edit.select", json!({"tracks": ["Kick"], "start": 0, "end": 1000})).unwrap();
        press(&ctx, &mut app, Key::Slash, cmd);
        assert_eq!(app.engine.undo_label(), Some("Write to Current"));
        press(&ctx, &mut app, Key::Slash, cmd | Modifiers::ALT);
        assert_eq!(app.engine.undo_label(), Some("Write to All Enabled"));
        press(&ctx, &mut app, Key::Slash, cmd | Modifiers::ALT | Modifiers::SHIFT);
        assert_eq!(app.engine.undo_label(), Some("Write to All Enabled"));
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

/// Draw every catalog page under each navigation tab to catch renderer or navigation regressions.
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

/// A whole app frame on a 1600×1000 screen.
fn app_frame(ctx: &egui::Context, app: &mut SoundApp, events: Vec<Event>) -> egui::FullOutput {
    let raw = RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1600.0, 1000.0))), events, ..Default::default() };
    let mut output = ctx.run_ui(raw, |ui| {
        app.logic(ui.ctx());
        app.ui(ui);
    });
    output.textures_delta.clear();
    output
}

/// Point at `at` and press Cmd+? there.
fn help_key_at(ctx: &egui::Context, app: &mut SoundApp, at: egui::Pos2) {
    app_frame(ctx, app, vec![Event::PointerMoved(at)]);
    let presses = [true, false].map(|pressed| Event::Key {
        key: Key::Slash,
        physical_key: None,
        pressed,
        repeat: false,
        modifiers: MAC_CMD | Modifiers::SHIFT,
    });
    app_frame(ctx, app, [Event::ModifiersChanged(MAC_CMD | Modifiers::SHIFT)].into_iter().chain(presses).collect());
    app_frame(ctx, app, vec![Event::ModifiersChanged(Modifiers::NONE)]);
}

/// Open every mapped floating window and verify that context Help resolves its actual egui layer.
#[test]
fn cmd_question_mark_opens_the_page_of_the_window_under_the_pointer() {
    let (ctx, mut app) = rig(OperatingSystem::Mac);
    app_frame(&ctx, &mut app, vec![]);
    let mut checked = 0;
    for (title, id) in help::WINDOW_PAGES {
        app.ui.show_help = false;
        app.ui.help = Default::default();
        app.run(id, json!({"value": true})).unwrap_or_else(|e| panic!("{id}: {e}"));
        for _ in 0..3 {
            app_frame(&ctx, &mut app, vec![]);
        }
        let rect = ctx.memory(|m| m.area_rect(help::window_id(title)));
        let Some(rect) = rect else { panic!("{title} ({id}) did not open as a window titled {title:?}") };
        help_key_at(&ctx, &mut app, rect.center() + egui::vec2(0.0, 10.0));
        assert!(app.ui.show_help, "{title}: help did not open");
        assert_eq!(app.ui.help.page, Page::Command(id.into()), "{title}");
        // Close it again so the next window isn't hidden behind it.
        app.run(id, json!({"value": false})).unwrap_or_else(|e| panic!("{id}: {e}"));
        app.ui.show_help = false;
        for _ in 0..2 {
            app_frame(&ctx, &mut app, vec![]);
        }
        checked += 1;
    }
    assert_eq!(checked, help::WINDOW_PAGES.len());
}

#[test]
fn cmd_question_mark_over_the_main_window_or_help_itself() {
    let (ctx, mut app) = rig(OperatingSystem::Mac);
    for _ in 0..2 {
        app_frame(&ctx, &mut app, vec![]);
    }
    // Over the Edit window's tracks.
    let [x0, y0, x1, y1] = app.edit_layout.timeline;
    help_key_at(&ctx, &mut app, egui::pos2((x0 + x1) / 2.0, (y0 + y1) / 2.0));
    assert!(app.ui.show_help);
    assert_eq!(app.ui.help.page, Page::Command("window.edit".into()));
    // Over help itself it stays where it is.
    app.ui.help.go(Page::Guide("record_audio".into()));
    for _ in 0..2 {
        app_frame(&ctx, &mut app, vec![]);
    }
    let rect = ctx.memory(|m| m.area_rect(help::window_id("SoundCraft Help"))).unwrap();
    help_key_at(&ctx, &mut app, rect.center());
    assert_eq!(app.ui.help.page, Page::Guide("record_audio".into()));
    // In the Mix window it opens the Mix window's page.
    app.ui.show_help = false;
    app.run("window.mix", json!({})).unwrap();
    for _ in 0..2 {
        app_frame(&ctx, &mut app, vec![]);
    }
    help_key_at(&ctx, &mut app, egui::pos2(800.0, 600.0));
    assert_eq!(app.ui.help.page, Page::Command("window.mix".into()));
}

/// Context help follows the main viewport even when the saved selection is Mix.
#[test]
fn separate_mix_keeps_main_window_context_help_on_edit() {
    for window in [soundcraft_ui_egui::MainWindow::Edit, soundcraft_ui_egui::MainWindow::Mix] {
        let (ctx, mut app) = rig(OperatingSystem::Mac);
        app.ui.mix_separate = true;
        app.ui.window = window;
        for _ in 0..2 {
            app_frame(&ctx, &mut app, vec![]);
        }
        // The headless backend embeds the separate Mix viewport as a floating window.
        // Point outside it so the shortcut targets the main window's background.
        let at = egui::pos2(1590.0, 990.0);
        assert_eq!(ctx.layer_id_at(at).unwrap().order, egui::Order::Background);
        help_key_at(&ctx, &mut app, at);
        assert!(app.ui.show_help);
        assert_eq!(app.ui.help.page, Page::Command("window.edit".into()), "{window:?}");
    }
}

/// The embedded fallback uses a viewport-based window id rather than the changing session title.
#[test]
fn embedded_mix_viewport_help_shortcut_opens_mix_help() {
    let (ctx, mut app) = rig(OperatingSystem::Mac);
    app.ui.mix_separate = true;
    for _ in 0..3 {
        app_frame(&ctx, &mut app, vec![]);
    }
    let id = egui::Id::unique(egui::ViewportId::from_hash_of("soundcraft.mix"));
    let rect = ctx.memory(|m| m.area_rect(id)).unwrap();
    let at = rect.center();
    help_key_at(&ctx, &mut app, at);
    assert!(app.ui.show_help);
    assert_eq!(app.ui.help.page, Page::Command("window.mix".into()));
}

/// Deliver key events only to the native child callback, as eframe does for a focused Mix window.
#[test]
fn native_mix_viewport_help_shortcut_opens_mix_help() {
    // egui's renderer callback is thread-local and has no restore API.
    std::thread::spawn(check_native_mix_help).join().unwrap();
}

fn check_native_mix_help() {
    for (os, cmd) in OSES {
        for window in [soundcraft_ui_egui::MainWindow::Edit, soundcraft_ui_egui::MainWindow::Mix] {
            let (ctx, mut app) = rig(os);
            ctx.set_embed_viewports(false);
            let events = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
            let child_events = events.clone();
            egui::Context::set_immediate_viewport_renderer(move |ctx, mut viewport| {
                let raw = RawInput {
                    viewport_id: viewport.ids.this,
                    viewports: [
                        (egui::ViewportId::ROOT, egui::ViewportInfo::default()),
                        (viewport.ids.this, egui::ViewportInfo { parent: Some(viewport.ids.parent), focused: Some(true), ..Default::default() }),
                    ]
                    .into_iter()
                    .collect(),
                    screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1100.0, 720.0))),
                    events: std::mem::take(&mut *child_events.borrow_mut()),
                    ..Default::default()
                };
                ctx.run_ui(raw, |ui| (viewport.viewport_ui_cb)(ui)).textures_delta.clear();
            });
            app.ui.mix_separate = true;
            app.ui.window = window;
            app_frame(&ctx, &mut app, vec![]);
            for (key, mods) in [
                (Key::Slash, cmd | Modifiers::SHIFT),
                (Key::Questionmark, cmd | Modifiers::SHIFT),
                (Key::Questionmark, cmd),
                (Key::A, cmd | Modifiers::SHIFT),
            ] {
                if key == Key::A {
                    app.ui.shortcut_overrides.insert("help.open".into(), "Cmd+Shift+A".into());
                }
                app.ui.show_help = false;
                app.ui.help.page = Page::Welcome;
                *events.borrow_mut() = vec![
                    Event::PointerMoved(egui::pos2(600.0, 400.0)),
                    Event::ModifiersChanged(mods),
                    Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers: mods },
                    Event::Key { key, physical_key: None, pressed: false, repeat: false, modifiers: mods },
                ];
                let output = app_frame(&ctx, &mut app, vec![]);
                assert!(app.ui.show_help, "{os:?} {window:?} {key:?}: child shortcut did not open Help");
                assert_eq!(app.ui.help.page, Page::Command("window.mix".into()));
                let root_focused = output
                    .viewport_output
                    .get(&egui::ViewportId::ROOT)
                    .is_some_and(|v| v.commands.iter().any(|command| matches!(command, egui::ViewportCommand::Focus)));
                assert!(root_focused, "{os:?} {window:?} {key:?}: Help must be brought in front of native Mix");
            }
        }
    }
}

#[test]
fn guide_pages_draw_on_every_platform_and_their_links_work() {
    for os in [OperatingSystem::Mac, OperatingSystem::Windows, OperatingSystem::Nix] {
        let (ctx, mut app) = rig(os);
        app.ui.show_help = true;
        for page in help::pages().into_iter().filter(|p| matches!(p, Page::Guide(_))) {
            app.ui.help.go(page.clone());
            for _ in 0..2 {
                app_frame(&ctx, &mut app, vec![]);
            }
            assert_eq!(app.ui.help.page, page, "{os:?}");
        }
    }
}

#[test]
fn cmd_question_mark_over_an_audiosuite_window_opens_its_process() {
    let (ctx, mut app) = rig(OperatingSystem::Mac);
    menus::invoke_menu(&mut app, "audiosuite.process", "AudioSuite > Dynamics > Dyn3 Compressor/Limiter");
    assert_eq!(app.ui.audiosuite.as_deref(), Some("compressor"));
    for _ in 0..3 {
        app_frame(&ctx, &mut app, vec![]);
    }
    let rect = ctx.memory(|m| m.layer_ids().filter(|l| l.order == egui::Order::Middle).find_map(|l| m.area_rect(l.id))).unwrap();
    help_key_at(&ctx, &mut app, rect.center() + egui::vec2(0.0, 10.0));
    assert_eq!(app.ui.help.page, Page::Process("compressor".into()));
}

/// With the app's real fonts every character on every help page has a glyph: the test harness
/// panics on one that would draw as a box. On a small screen every page also fits inside it:
/// nothing on a page may push the help window past the edge.
#[test]
fn every_help_page_draws_with_real_glyphs_and_fits_a_small_screen() {
    use egui_kittest::kittest::Queryable;
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
        assert!(h.query_by_label("Page not found").is_none(), "{page:?} did not render");
        let window = h.ctx.memory(|m| m.area_rect(egui::Id::unique(Some("SoundCraft Help")))).unwrap();
        assert!(window.max.x <= 760.0 && window.min.x >= 0.0 && window.max.y <= 700.0 && window.min.y >= 0.0, "{page:?} exceeds {window:?}");
    }
}

#[test]
fn help_opens_below_the_menu_bar_and_escape_over_it_closes_it() {
    let (ctx, mut app) = rig(OperatingSystem::Nix);
    app.ui.show_help = true;
    for _ in 0..3 {
        app_frame(&ctx, &mut app, vec![]);
    }
    let rect = ctx.memory(|m| m.area_rect(help::window_id("SoundCraft Help"))).unwrap();
    assert!(rect.min.y >= 24.0, "help's title bar covers the menu bar: {rect:?}");
    assert!(rect.max.x <= 1600.0 && rect.max.y <= 1000.0, "{rect:?}");
    // Escape away from help leaves it open.
    app_frame(&ctx, &mut app, vec![Event::PointerMoved(egui::pos2(1590.0, 990.0))]);
    let esc = |pressed| Event::Key { key: Key::Escape, physical_key: None, pressed, repeat: false, modifiers: Modifiers::NONE };
    app_frame(&ctx, &mut app, vec![esc(true), esc(false)]);
    assert!(app.ui.show_help);
    // Escape while typing in the search box only leaves the box.
    app.ui.help.tab = Tab::Search;
    app.ui.help.focus_search = true;
    app_frame(&ctx, &mut app, vec![Event::PointerMoved(rect.center())]);
    app_frame(&ctx, &mut app, vec![Event::Text("solo".into())]);
    assert_eq!(app.ui.help.query, "solo", "the search box should have the keyboard");
    app_frame(&ctx, &mut app, vec![esc(true), esc(false)]);
    app_frame(&ctx, &mut app, vec![]);
    assert!(app.ui.show_help, "Escape in the search box closed help");
    // Over help it closes it.
    app_frame(&ctx, &mut app, vec![Event::PointerMoved(rect.center())]);
    app_frame(&ctx, &mut app, vec![esc(true), esc(false)]);
    app_frame(&ctx, &mut app, vec![]);
    assert!(!app.ui.show_help, "Escape over help should close it");
}

/// A dialog opened from a Help command owns Escape, even while the pointer remains over Help.
#[test]
fn escape_cancels_a_dialog_before_closing_help() {
    let (ctx, mut app) = rig(OperatingSystem::Nix);
    app.ui.show_help = true;
    app.ui.help.go(Page::Command("session.new".into()));
    for _ in 0..3 {
        app_frame(&ctx, &mut app, vec![]);
    }
    let help_id = help::window_id("SoundCraft Help");
    let rect = ctx.memory(|m| m.area_rect(help_id)).unwrap();
    let at = rect.min + egui::vec2(24.0, rect.height() - 24.0);
    let session_name = app.engine.session().name.clone();
    // This is the same dispatcher used by the page's Do it now button.
    menus::invoke_menu(&mut app, "session.new", "File > New Session...");
    for _ in 0..3 {
        app_frame(&ctx, &mut app, vec![Event::PointerMoved(at)]);
    }
    assert_eq!(app.dialogs.open_name(), Some("new_session"));
    assert_eq!(ctx.layer_id_at(at).unwrap().id, help_id);
    assert!(!ctx.text_edit_focused(), "the test must exercise Help's Escape handler outside a text field");
    let esc = |pressed| Event::Key { key: Key::Escape, physical_key: None, pressed, repeat: false, modifiers: Modifiers::NONE };
    app_frame(&ctx, &mut app, vec![esc(true), esc(false)]);
    assert_eq!(app.dialogs.open_name(), None, "Escape should cancel the dialog");
    assert!(app.ui.show_help, "Escape intended for the dialog closed Help");
    assert_eq!(app.engine.session().name, session_name, "canceling New Session changed the session");
    // Once the dialog is gone, Escape over Help still closes Help normally.
    app_frame(&ctx, &mut app, vec![]);
    app_frame(&ctx, &mut app, vec![esc(true), esc(false)]);
    assert!(!app.ui.show_help);
}

/// Opening Help takes keyboard focus even with the pointer outside the window.
#[test]
fn keyboard_focus_navigates_and_closes_help_away_from_the_pointer() {
    let (ctx, mut app) = rig(OperatingSystem::Nix);
    let page = Page::Command("session.save".into());
    app.ui.help.go(page.clone());
    app.run("help.open", json!({})).unwrap();
    for _ in 0..3 {
        app_frame(&ctx, &mut app, vec![Event::PointerMoved(egui::pos2(0.0, 0.0))]);
    }
    let press = |app: &mut SoundApp, key, modifiers| {
        let events = [true, false].map(|pressed| Event::Key { key, physical_key: None, pressed, repeat: false, modifiers });
        app_frame(&ctx, app, events.to_vec());
    };
    press(&mut app, Key::ArrowLeft, Modifiers::ALT);
    assert_eq!(app.ui.help.page, Page::Welcome);
    press(&mut app, Key::ArrowRight, Modifiers::ALT);
    assert_eq!(app.ui.help.page, page);
    press(&mut app, Key::Escape, Modifiers::NONE);
    assert!(!app.ui.show_help);
}

/// Opening a configuration from an older catalog recovers to Welcome.
#[test]
fn stale_saved_help_pages_recover_to_welcome() {
    let (ctx, mut app) = rig(OperatingSystem::Nix);
    app_frame(&ctx, &mut app, vec![]);
    app_frame(&ctx, &mut app, vec![]);
    for kind in ["command", "category", "guide", "process"] {
        app.ui.help = serde_json::from_value(json!({"page": {"kind":kind, "id":"removed"}, "tab":"index"})).unwrap();
        app.run("help.open", json!({})).unwrap();
        app_frame(&ctx, &mut app, vec![]);
        assert_eq!(app.ui.help.page, Page::Welcome);
        assert_eq!(app.ui.help.tab, Tab::Index);
    }
}
