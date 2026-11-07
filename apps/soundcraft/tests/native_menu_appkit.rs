//! The macOS menu bar end to end, on the main thread (`harness = false`): install it through the
//! real muda menu (`src/native_menu.rs`), read AppKit's items back, then choose items the two ways
//! AppKit can. Every item shows its command's shortcut as a key equivalent where it has ⌘ or ⌃;
//! a key equivalent (a real key-down through `-[NSMenu performKeyEquivalent:]`) must reach egui
//! as the input its key-down would have been, and a click (`-[NSMenu
//! performActionForItemAtIndex:]`) must run as a click, also when the click is Return inside the
//! open menu.

// The app's own module, built into this test.
#[cfg(target_os = "macos")]
#[path = "../src/native_menu.rs"]
#[allow(dead_code)]
mod native_menu;

#[cfg(not(target_os = "macos"))]
fn main() {}

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
                    assert!(!key.is_empty(), "{} shows no {shortcut:?}", child.path);
                    assert_eq!(item.keyEquivalentModifierMask(), mask(want), "{} {shortcut:?}", child.path);
                    *shown += 1;
                }
                None => assert!(key.is_empty(), "{} has key equivalent {key:?} for {shortcut:?}", child.path),
            }
        }
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
    assert_eq!((copy.keyEquivalent().to_string().as_str(), copy.keyEquivalentModifierMask()), ("c", NSEventModifierFlags::Command), "⌘C");
    let app_menu = submenu(&bar, "SoundCraft");
    let quit = app_menu.itemAtIndex(index(&app_menu, "Quit SoundCraft")).expect("Quit");
    assert_eq!(quit.keyEquivalent().to_string(), "q", "⌘Q");
    let hide = app_menu.itemAtIndex(index(&app_menu, "Hide SoundCraft")).expect("Hide");
    assert!(hide.keyEquivalent().to_string().is_empty(), "⌘H stays Heal Separation");

    // Make `event` AppKit's current event, as the run loop does before dispatching it.
    let dequeue = |event: &NSEvent| {
        nsapp.postEvent_atStart(event, true);
        let mode = NSString::from_str("kCFRunLoopDefaultMode");
        let got = nsapp.nextEventMatchingMask_untilDate_inMode_dequeue(NSEventMask::Any, None, &mode, true).expect("the posted event");
        assert_eq!(got.r#type(), event.r#type());
        got
    };
    let key_down = |chars: &str, code: u16, flags: NSEventModifierFlags, repeat: bool| {
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
    };
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
    // The menu refreshes its enabled items a few times a second (`SYNC_EVERY`).
    let synced = |app: &mut SoundApp, menu: &mut NativeMenu| {
        std::thread::sleep(std::time::Duration::from_millis(300));
        frame(app, menu);
    };
    app.engine.execute("edit.select", &json!({"tracks": [track], "start": 0, "end": 48_000})).expect("a selection");
    synced(&mut app, &mut menu);
    assert!(bar.performKeyEquivalent(&dequeue(&key_down("c", 8, NSEventModifierFlags::Command, false))), "⌘C is Edit › Copy's key equivalent");
    let (events, _) = frame(&mut app, &mut menu);
    assert_eq!(events, [egui::Event::Copy], "⌘C is a Copy event");
    app.engine.execute("edit.copy", &json!({})).expect("clips on the clipboard");
    synced(&mut app, &mut menu);
    assert!(bar.performKeyEquivalent(&dequeue(&key_down("v", 9, NSEventModifierFlags::Command, false))), "⌘V is Edit › Paste's key equivalent");
    let (events, commands) = frame(&mut app, &mut menu);
    assert!(events.is_empty(), "the paste comes from the clipboard next frame: {events:?}");
    assert!(commands.contains(&egui::ViewportCommand::RequestPaste), "⌘V asks the window for the clipboard: {commands:?}");

    println!("native_menu_appkit: ok");
}
