//! The macOS menu bar end to end, on the main thread (`harness = false`): install it through the
//! real muda menu (`src/native_menu.rs`), read AppKit's items back, then choose items the two ways
//! AppKit can. Every item shows its command's shortcut as a key equivalent where it has ⌘ or ⌃;
//! a key equivalent (a real key-down through `-[NSMenu performKeyEquivalent:]`) must reach egui
//! as the input its key-down would have been, and a click (`-[NSMenu
//! performActionForItemAtIndex:]`) must run as a click, also when the click is Return inside the
//! open menu. In a focused text field, ⌘C, ⌘X, ⌘V and ⌘Z edit the text, and the session's own
//! Undo doesn't run.

// The app's own module, built into this test.
#[cfg(target_os = "macos")]
#[path = "../src/native_menu.rs"]
#[allow(dead_code)]
mod native_menu;

/// The menu bar is macOS's: elsewhere there is nothing to check.
#[cfg(not(target_os = "macos"))]
fn main() {}

/// AppKit needs the main thread, so this test runs without libtest's harness (`harness = false`):
/// it installs the menu bar on a `SoundApp` and checks its items and their key equivalents.
#[cfg(target_os = "macos")]
fn main() {
    use native_menu::NativeMenu;
    use objc2::MainThreadMarker;
    use objc2::rc::Retained;
    use objc2_app_kit::{NSApplication, NSEvent, NSEventMask, NSEventModifierFlags, NSEventSubtype, NSEventType, NSMenu};
    use objc2_foundation::{NSPoint, NSString};
    use serde_json::json;
    use soundcraft_ui_egui::{SoundApp, menus, shortcuts};

    // Without a harness nothing answers cargo-nextest's `--list`: list the one test as libtest's
    // terse format would, and nothing for `--ignored`.
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--list") {
        if !args.iter().any(|a| a == "--ignored") {
            println!("native_menu_appkit: test");
        }
        return;
    }

    let mtm = MainThreadMarker::new().expect("harness = false runs on the main thread");
    let nsapp = NSApplication::sharedApplication(mtm);
    nsapp.finishLaunching();

    let ctx = egui::Context::default();
    let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, soundcraft_ui_egui::Services::default());
    let mut menu = NativeMenu::new(&app, &ctx).expect("the menu bar installed");
    let bar = nsapp.mainMenu().expect("a main menu");

    // Every leaf shows the shortcut the in-window menus show, wherever it can be a key equivalent,
    // and nothing elsewhere. The tree is built in order, after the application menu.
    /// The AppKit modifier flags of a shortcut's modifiers.
    fn mask((m, _): (egui::Modifiers, egui::Key)) -> NSEventModifierFlags {
        let mut flags = NSEventModifierFlags::empty();
        for (on, flag) in [
            (m.command, NSEventModifierFlags::Command),
            (m.ctrl, NSEventModifierFlags::Control),
            (m.alt, NSEventModifierFlags::Option),
            (m.shift, NSEventModifierFlags::Shift),
        ] {
            if on {
                flags |= flag;
            }
        }
        flags
    }
    /// The character AppKit shows and matches for `key`: the key's own (lower case) for letters,
    /// digits and punctuation, AppKit's function-key characters for the others.
    fn equivalent(key: egui::Key) -> String {
        use egui::Key;
        let f = |c: u32| char::from_u32(c).map(String::from).unwrap_or_default();
        match key {
            Key::ArrowUp => f(0xF700),
            Key::ArrowDown => f(0xF701),
            Key::ArrowLeft => f(0xF702),
            Key::ArrowRight => f(0xF703),
            Key::Home => f(0xF729),
            Key::End => f(0xF72B),
            Key::PageUp => f(0xF72C),
            Key::PageDown => f(0xF72D),
            Key::Delete => f(0xF728),
            Key::Backspace => f(0x08),
            Key::Enter => "\r".into(),
            Key::Tab => "\t".into(),
            Key::Escape => f(0x1B),
            Key::Space => " ".into(),
            Key::Colon => ":".into(),
            Key::Comma => ",".into(),
            Key::Minus => "-".into(),
            Key::Period => ".".into(),
            Key::Plus => "+".into(),
            Key::Equals => "=".into(),
            Key::Semicolon => ";".into(),
            Key::Backslash => "\\".into(),
            Key::Slash => "/".into(),
            Key::OpenBracket => "[".into(),
            Key::CloseBracket => "]".into(),
            Key::Backtick => "`".into(),
            Key::Quote => "'".into(),
            k if k.name().chars().count() == 1 => k.name().to_lowercase(),
            k => match k.name().strip_prefix('F').and_then(|n| n.parse::<u32>().ok()) {
                Some(n) => f(0xF704 + n - 1),
                None => panic!("no key equivalent character known for {k:?}"),
            },
        }
    }
    /// Check `menu` against the in-window menu `node`, submenus included: each item's key
    /// equivalent is its command's shortcut, or none; `shown` counts the items that have one.
    fn check(menu: &NSMenu, node: &menus::MenuNode, aliases: &[(&str, &str)], shown: &mut usize) {
        // AppKit adds its own items to the end of the Edit menu (Start Dictation, Emoji & Symbols).
        assert!(menu.numberOfItems() >= node.children.len() as isize, "{}", node.label);
        for (i, child) in node.children.iter().enumerate() {
            let item = menu.itemAtIndex(i as isize).expect("an item per menu node");
            if !child.children.is_empty() {
                check(&item.submenu().expect("a submenu"), child, aliases, shown);
                continue;
            }
            let shortcut = menus::command_for_path(&child.path, aliases).as_deref().and_then(menus::shortcut_for);
            let key = item.keyEquivalent().to_string();
            match shortcut.and_then(shortcuts::menu_key_equivalent) {
                Some(want) => {
                    assert_eq!(key, equivalent(want.1), "{} {shortcut:?}", child.path);
                    assert_eq!(item.keyEquivalentModifierMask(), mask(want), "{} {shortcut:?}", child.path);
                    *shown += 1;
                }
                None => assert!(key.is_empty(), "{} has key equivalent {key:?} for {shortcut:?}", child.path),
            }
        }
    }
    /// Make `event` AppKit's current event, as the run loop does before dispatching it.
    fn dequeue(nsapp: &NSApplication, event: &NSEvent) -> Retained<NSEvent> {
        nsapp.postEvent_atStart(event, true);
        let mode = NSString::from_str("kCFRunLoopDefaultMode");
        let got = nsapp.nextEventMatchingMask_untilDate_inMode_dequeue(NSEventMask::Any, None, &mode, true).expect("the posted event");
        assert_eq!(got.r#type(), event.r#type());
        got
    }
    /// A key-down of `chars` (virtual key `code`) with `flags`.
    fn key_down(chars: &str, code: u16, flags: NSEventModifierFlags, repeat: bool) -> Retained<NSEvent> {
        let s = NSString::from_str(chars);
        NSEvent::keyEventWithType_location_modifierFlags_timestamp_windowNumber_context_characters_charactersIgnoringModifiers_isARepeat_keyCode(
            NSEventType::KeyDown,
            NSPoint::new(0.0, 0.0),
            flags,
            0.0,
            0,
            None,
            &s,
            &s,
            repeat,
            code,
        )
        .expect("a key event")
    }

    /// In a focused text field the menu's keys edit the text, as they do without the menu bar:
    /// ⌘C copies the selection, ⌘X cuts it, ⌘V asks for the clipboard and ⌘Z undoes the cut in
    /// the field, while the session's own Undo stays where it was (shortcuts stand aside while a
    /// text field has the keyboard).
    fn text_field_keys(nsapp: &NSApplication, ctx: &egui::Context, app: &mut SoundApp, menu: &mut NativeMenu, edit: &NSMenu) {
        let items = || (0..edit.numberOfItems()).filter_map(|i| edit.itemAtIndex(i));
        let item = |title: &str| items().find(|it| it.title().to_string() == title).unwrap_or_else(|| panic!("no Edit › {title}"));
        let (copy, cut, paste) = (item("Copy"), item("Cut"), item("Paste"));
        let bar = nsapp.mainMenu().expect("a main menu");
        let press = |chars: &str, code: u16| bar.performKeyEquivalent(&dequeue(nsapp, &key_down(chars, code, NSEventModifierFlags::Command, false)));
        let field = egui::Id::unique("native_menu_appkit text field");
        // One frame as `main.rs` runs it, with a focused text field over the app.
        let typing = |app: &mut SoundApp, menu: &mut NativeMenu, text: &mut String| {
            let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1400.0, 900.0));
            let mut raw = egui::RawInput { screen_rect: Some(rect), ..Default::default() };
            menu.raw_input(ctx, &mut raw);
            app.raw_input_hook(ctx, &mut raw);
            let mut out = ctx.run_ui(raw, |ui| {
                menu.process(app);
                app.logic(ui.ctx());
                app.ui(ui);
                egui::Area::new(egui::Id::unique("native_menu_appkit text field area")).show(ui.ctx(), |ui| {
                    ui.add(egui::TextEdit::singleline(text).id(field));
                });
                menu.sync(app);
            });
            out.textures_delta.clear();
            let commands = out.viewport_output.remove(&egui::ViewportId::ROOT).map(|v| v.commands).unwrap_or_default();
            (out.platform_output.commands, commands)
        };
        // Frames, each with the field (one without it would take its focus), until the menu has
        // refreshed `item` to `enabled`, for at most five seconds.
        let settle = |app: &mut SoundApp, menu: &mut NativeMenu, text: &mut String, item: &objc2_app_kit::NSMenuItem, enabled: bool| {
            let until = std::time::Instant::now() + std::time::Duration::from_secs(5);
            typing(app, menu, text);
            while item.isEnabled() != enabled {
                assert!(std::time::Instant::now() < until, "{} never became {}", item.title(), if enabled { "enabled" } else { "disabled" });
                std::thread::sleep(std::time::Duration::from_millis(20));
                typing(app, menu, text);
            }
        };

        // Nothing selected in the session, so Edit › Copy and Cut are disabled for the session and
        // only the focused field enables them; an edit, so Edit › Undo would undo something. Undo's
        // title follows the last edit ("Undo …"), so find it by its key equivalent.
        app.engine.execute("track.new", &json!({"name": "Undo me", "format": "mono"})).expect("a new track");
        app.engine.execute("edit.select_none", &json!({})).expect("nothing selected");
        assert!(soundcraft_engine::cmd::has_range(&app.engine).is_err(), "no session selection to copy");
        let session_undo = Some(app.engine.undo_label().expect("the session has something to undo").to_owned());
        let undo = items()
            .find(|it| it.keyEquivalent().to_string() == "z" && it.keyEquivalentModifierMask() == NSEventModifierFlags::Command)
            .expect("Edit › Undo is ⌘Z");
        let tracks = app.engine.session().tracks.len();

        let mut text = String::from("hello world");
        typing(app, menu, &mut text);
        // AppKit takes a disabled item's key equivalent too, and the window never sees the key:
        // without the focused field's own Edit items, ⌘C would be lost.
        settle(app, menu, &mut text, &copy, false);
        assert!(press("c", 8), "AppKit takes a disabled item's key equivalent");
        ctx.memory_mut(|m| m.request_focus(field));
        for _ in 0..2 {
            typing(app, menu, &mut text);
        }
        let focused = || ctx.memory(|m| m.has_focus(field));
        assert!(focused(), "the field has the keyboard");
        assert!(copy.isEnabled(), "a focus change re-syncs the menu at once");
        assert_eq!(undo.title().to_string(), "Undo", "Undo is the field's, not the session's last edit");
        settle(app, menu, &mut text, &copy, true);
        assert!(cut.isEnabled() && paste.isEnabled(), "the field enables Cut and Paste too");
        // Select "world", as a drag over it would.
        let mut state = egui::text_edit::TextEditState::load(ctx, field).expect("the field has been shown");
        state.cursor.set_char_range(Some(egui::text::CCursorRange::two(egui::text::CCursor::new(6), egui::text::CCursor::new(11))));
        state.store(ctx, field);

        settle(app, menu, &mut text, &copy, true);
        assert!(focused(), "the field still has the keyboard");
        assert!(press("c", 8), "⌘C in the field");
        let (output, _) = typing(app, menu, &mut text);
        assert!(output.contains(&egui::OutputCommand::CopyText("world".into())), "⌘C copies the field's selection: {output:?}");

        settle(app, menu, &mut text, &cut, true);
        assert!(focused(), "the field still has the keyboard");
        assert!(press("x", 7), "⌘X in the field");
        let (output, _) = typing(app, menu, &mut text);
        assert_eq!(text, "hello ", "⌘X cut the field's selection");
        assert!(output.contains(&egui::OutputCommand::CopyText("world".into())), "the cut text goes to the clipboard: {output:?}");

        settle(app, menu, &mut text, &paste, true);
        assert!(focused(), "the field still has the keyboard");
        assert!(press("v", 9), "⌘V in the field");
        let (_, commands) = typing(app, menu, &mut text);
        assert!(commands.contains(&egui::ViewportCommand::RequestPaste), "⌘V asks the window for the clipboard: {commands:?}");

        settle(app, menu, &mut text, &undo, true);
        assert!(focused(), "the field still has the keyboard");
        assert!(press("z", 6), "⌘Z is Edit › Undo's key equivalent");
        typing(app, menu, &mut text);
        assert_eq!(text, "hello world", "⌘Z undid the cut in the field");
        assert_eq!(app.engine.undo_label().map(str::to_owned), session_undo, "the session's Undo didn't run");
        assert_eq!(app.engine.session().tracks.len(), tracks, "the new track is still there");

        // A click (Return in the open menu) on Cut and then Undo edits the field too.
        let select_world = || {
            let mut state = egui::text_edit::TextEditState::load(ctx, field).expect("the field has been shown");
            state.cursor.set_char_range(Some(egui::text::CCursorRange::two(egui::text::CCursor::new(6), egui::text::CCursor::new(11))));
            state.store(ctx, field);
        };
        let click = |title: &str| {
            let at = (0..edit.numberOfItems()).find(|&i| edit.itemAtIndex(i).is_some_and(|it| it.title().to_string() == title)).expect(title);
            dequeue(nsapp, &key_down("\r", 36, NSEventModifierFlags::empty(), false));
            edit.performActionForItemAtIndex(at);
        };
        select_world();
        typing(app, menu, &mut text);
        click("Cut");
        typing(app, menu, &mut text);
        assert_eq!(text, "hello ", "a click on Cut cut the field's selection");
        click("Undo");
        typing(app, menu, &mut text);
        assert_eq!(text, "hello world", "a click on Undo undid the cut in the field");
        assert_eq!(app.engine.session().tracks.len(), tracks, "the session's Undo didn't run");

        // With no field focused, the same ⌘Z is the session's Undo: the new track goes.
        ctx.memory_mut(|m| m.surrender_focus(field));
        typing(app, menu, &mut text);
        assert!(!focused(), "the field gave up the keyboard");
        settle(app, menu, &mut text, &copy, false);
        settle(app, menu, &mut text, &undo, true);
        assert!(press("z", 6), "⌘Z with no field focused");
        typing(app, menu, &mut text);
        assert_eq!(text, "hello world", "the field is untouched");
        assert_eq!(app.engine.session().tracks.len(), tracks - 1, "⌘Z undid the new track");
    }

    let aliases = menus::ui_aliases();
    let mut shown = 0;
    for (i, root) in menus::tree().iter().enumerate() {
        let item = bar.itemAtIndex(i as isize + 1).expect("a menu per root");
        check(&item.submenu().expect("a submenu"), root, &aliases, &mut shown);
    }
    assert!(shown > 50, "only {shown} items show a shortcut");

    let submenu = |menu: &NSMenu, title: &str| -> Retained<NSMenu> {
        (0..menu.numberOfItems())
            .filter_map(|i| menu.itemAtIndex(i))
            .find(|it| it.title().to_string() == title)
            .and_then(|it| it.submenu())
            .unwrap_or_else(|| panic!("no {title} submenu"))
    };
    let index = |menu: &NSMenu, title: &str| -> isize {
        (0..menu.numberOfItems())
            .find(|&i| menu.itemAtIndex(i).is_some_and(|it| it.title().to_string() == title))
            .unwrap_or_else(|| panic!("no {title}"))
    };
    let edit = submenu(&bar, "Edit");
    let copy = edit.itemAtIndex(index(&edit, "Copy")).expect("Edit › Copy");
    let cut = edit.itemAtIndex(index(&edit, "Cut")).expect("Edit › Cut");
    let paste = edit.itemAtIndex(index(&edit, "Paste")).expect("Edit › Paste");
    assert_eq!((copy.keyEquivalent().to_string().as_str(), copy.keyEquivalentModifierMask()), ("c", NSEventModifierFlags::Command), "⌘C");
    let app_menu = submenu(&bar, "SoundCraft");
    let quit = app_menu.itemAtIndex(index(&app_menu, "Quit SoundCraft")).expect("Quit");
    assert_eq!(quit.keyEquivalent().to_string(), "q", "⌘Q");
    let hide = app_menu.itemAtIndex(index(&app_menu, "Hide SoundCraft")).expect("Hide");
    assert!(hide.keyEquivalent().to_string().is_empty(), "⌘H stays Heal Separation");

    let dequeue = |event: &NSEvent| dequeue(&nsapp, event);
    let other = || {
        NSEvent::otherEventWithType_location_modifierFlags_timestamp_windowNumber_context_subtype_data1_data2(
            NSEventType::ApplicationDefined,
            NSPoint::new(0.0, 0.0),
            NSEventModifierFlags::empty(),
            0.0,
            0,
            None,
            NSEventSubtype::ApplicationActivated.0,
            0,
            0,
        )
        .expect("an application-defined event")
    };
    // One frame as the desktop app runs it (`main.rs`): the menu's input first, then the app's.
    let frame = |app: &mut SoundApp, menu: &mut NativeMenu| {
        let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1400.0, 900.0));
        let mut raw = egui::RawInput { screen_rect: Some(rect), ..Default::default() };
        menu.raw_input(&ctx, &mut raw);
        let events = raw.events.clone();
        app.raw_input_hook(&ctx, &mut raw);
        let mut out = ctx.run_ui(raw, |ui| {
            menu.process(app);
            app.logic(ui.ctx());
            app.ui(ui);
            menu.sync(app);
        });
        out.textures_delta.clear();
        let commands = out.viewport_output.remove(&egui::ViewportId::ROOT).map(|v| v.commands).unwrap_or_default();
        (events, commands)
    };
    for _ in 0..4 {
        frame(&mut app, &mut menu);
    }

    // A key equivalent: ⌘1 (Window › Transport) reaches egui as the ⌘1 press and its release, and
    // the shortcut toggles the Transport window once (a click as well would toggle it back).
    let transport = app.ui.show_transport;
    let cmd_1 = |repeat| key_down("1", 18, NSEventModifierFlags::Command, repeat);
    assert!(bar.performKeyEquivalent(&dequeue(&cmd_1(false))), "⌘1 is Window › Transport's key equivalent");
    let (events, _) = frame(&mut app, &mut menu);
    let keys: Vec<_> = events
        .iter()
        .filter_map(|e| if let egui::Event::Key { key, pressed, modifiers, .. } = e { Some((*key, *pressed, modifiers.mac_cmd)) } else { None })
        .collect();
    assert_eq!(keys, [(egui::Key::Num1, true, true), (egui::Key::Num1, false, true)], "the key press and its release: {events:?}");
    assert_eq!(app.ui.show_transport, !transport, "⌘1 toggled Transport once");

    // Pressed again it runs again: egui saw the first press released.
    assert!(bar.performKeyEquivalent(&dequeue(&cmd_1(false))));
    frame(&mut app, &mut menu);
    assert_eq!(app.ui.show_transport, transport, "the second ⌘1 toggled Transport back");

    // An auto-repeat of a held key equivalent is dropped, as egui's own key repeats are.
    assert!(bar.performKeyEquivalent(&dequeue(&cmd_1(true))));
    let (events, _) = frame(&mut app, &mut menu);
    assert!(events.is_empty(), "a repeat adds nothing: {events:?}");
    assert_eq!(app.ui.show_transport, transport);

    // A click: Window › Transport chosen with the mouse runs the command.
    let window = submenu(&bar, "Window");
    let at = index(&window, "Transport");
    dequeue(&other());
    window.performActionForItemAtIndex(at);
    let (events, _) = frame(&mut app, &mut menu);
    assert!(events.is_empty(), "a click is no key press: {events:?}");
    assert_eq!(app.ui.show_transport, !transport, "the click toggled Transport");

    // Return inside the open menu is a key-down without ⌘: still a click.
    dequeue(&key_down("\r", 36, NSEventModifierFlags::empty(), false));
    window.performActionForItemAtIndex(at);
    let (events, _) = frame(&mut app, &mut menu);
    assert!(events.is_empty(), "Return in the menu is a click: {events:?}");
    assert_eq!(app.ui.show_transport, transport, "Return on Transport toggled it");

    // ⌘C and ⌘V are egui-winit's clipboard events, so a text field still copies and pastes while
    // Edit › Copy and Paste are enabled and AppKit takes the keys.
    let track = app.engine.session().tracks.first().map(|t| t.id.0).expect("a demo track");
    // The menu refreshes its enabled items a few times a second (`SYNC_EVERY`): run frames until
    // `item` is enabled, for at most five seconds.
    let synced = |app: &mut SoundApp, menu: &mut NativeMenu, item: &objc2_app_kit::NSMenuItem| {
        let until = std::time::Instant::now() + std::time::Duration::from_secs(5);
        frame(app, menu);
        while !item.isEnabled() {
            assert!(std::time::Instant::now() < until, "{} never became enabled", item.title());
            std::thread::sleep(std::time::Duration::from_millis(20));
            frame(app, menu);
        }
    };

    // A disabled item's key equivalent queues nothing: with nothing selected, Copy is disabled and
    // ⌘C doesn't run it.
    assert!(!copy.isEnabled(), "nothing selected yet");
    bar.performKeyEquivalent(&dequeue(&key_down("c", 8, NSEventModifierFlags::Command, false)));
    let (events, _) = frame(&mut app, &mut menu);
    assert!(events.is_empty(), "a disabled item's key equivalent adds nothing: {events:?}");

    app.engine.execute("edit.select", &json!({"tracks": [track], "start": 0, "end": 48_000})).expect("a selection");
    synced(&mut app, &mut menu, &copy);
    assert!(bar.performKeyEquivalent(&dequeue(&key_down("c", 8, NSEventModifierFlags::Command, false))), "⌘C is Edit › Copy's key equivalent");
    let (events, _) = frame(&mut app, &mut menu);
    assert_eq!(events, [egui::Event::Copy], "⌘C is a Copy event");
    app.engine.execute("edit.copy", &json!({})).expect("clips on the clipboard");
    synced(&mut app, &mut menu, &paste);
    assert!(bar.performKeyEquivalent(&dequeue(&key_down("v", 9, NSEventModifierFlags::Command, false))), "⌘V is Edit › Paste's key equivalent");
    let (events, commands) = frame(&mut app, &mut menu);
    assert!(events.is_empty(), "the paste comes from the clipboard next frame: {events:?}");
    assert!(commands.contains(&egui::ViewportCommand::RequestPaste), "⌘V asks the window for the clipboard: {commands:?}");

    // ⌘X is egui-winit's Cut event, as ⌘C is Copy.
    synced(&mut app, &mut menu, &cut);
    assert!(bar.performKeyEquivalent(&dequeue(&key_down("x", 7, NSEventModifierFlags::Command, false))), "⌘X is Edit › Cut's key equivalent");
    let (events, _) = frame(&mut app, &mut menu);
    assert_eq!(events, [egui::Event::Cut], "⌘X is a Cut event");

    text_field_keys(&nsapp, &ctx, &mut app, &mut menu, &edit);

    println!("native_menu_appkit: ok");
}
