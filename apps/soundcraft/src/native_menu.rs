//! macOS system menu bar. All actions use the same menu dispatch as the egui host.
//!
//! Items show their command's shortcut as a key equivalent, but **keys stay with egui**: AppKit
//! runs an item's key equivalent before the window sees the key, which would skip the shortcut
//! rules in `shortcuts::handle` (text fields keep their keys, dialogs take none, Commands Focus).
//! So a key equivalent goes back to egui as the input its key-down would have been
//! ([`NativeMenu::raw_input`]), and only a click runs the item like an in-window menu click.
use muda::accelerator::Accelerator;
use muda::{CheckMenuItem, Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem, Submenu};
use objc2::MainThreadMarker;
use objc2_app_kit::{NSApplication, NSEventModifierFlags, NSEventType};
use soundcraft_ui_egui::shortcuts::{self, KeyEquivalentInput};
use soundcraft_ui_egui::{SoundApp, menus};
use std::sync::mpsc::{Receiver, channel};

struct Entry {
    item: CheckMenuItem,
    command: Option<String>,
    path: String,
    enabled: bool,
    checked: bool,
    /// The key of the item's key equivalent, if it has one.
    key: Option<egui::Key>,
}

/// An item AppKit chose, read from its current event while it calls the handler: a key-down
/// (`Some` modifiers held, and whether it is an auto-repeat) or a click.
struct Chosen {
    id: MenuId,
    key_down: Option<(egui::Modifiers, bool)>,
}

pub struct NativeMenu {
    menu: Menu,
    entries: Vec<Entry>,
    events: Receiver<Chosen>,
    /// Items chosen by a click, run by `process`.
    clicks: Vec<MenuId>,
    /// Key equivalents not yet handed to egui by `raw_input`.
    keys: Vec<(egui::Key, egui::Modifiers)>,
    /// When `sync` last walked the entries; see `SYNC_EVERY`.
    last_sync: Option<std::time::Instant>,
}

/// `sync` walks every menu leaf (~500), so it runs a few times a second rather than every frame,
/// and right after a menu click.
const SYNC_EVERY: std::time::Duration = std::time::Duration::from_millis(250);

impl NativeMenu {
    /// Called on the UI thread after eframe has created NSApplication.
    pub fn new(app: &SoundApp, ctx: &egui::Context) -> muda::Result<Self> {
        let menu = Menu::new();
        let (tx, events) = channel();
        let mut native = Self { menu, entries: Vec::new(), events, clicks: Vec::new(), keys: Vec::new(), last_sync: None };
        let app_menu = Submenu::new("SoundCraft", true);
        native.add_item(app, &app_menu, "About SoundCraft", "", Some("window.about".into()))?;
        native.add_item(app, &app_menu, "Preferences...", "", Some("setup.preferences".into()))?;
        native.add_item(app, &app_menu, "Session Info", "", Some("window.session_info".into()))?;
        native.add_item(app, &app_menu, "Session Audio Health", "", Some("window.audio_health".into()))?;
        app_menu.append(&PredefinedMenuItem::separator())?;
        app_menu.append(&PredefinedMenuItem::services(None))?;
        app_menu.append(&PredefinedMenuItem::separator())?;
        // muda's own Hide items take ⌘H and ⌥⌘H, which are Heal Separation and Copy to Send.
        app_menu.append(&MenuItem::with_id("hide", "Hide SoundCraft", true, None))?;
        app_menu.append(&MenuItem::with_id("hide_others", "Hide Others", true, None))?;
        app_menu.append(&PredefinedMenuItem::show_all(None))?;
        app_menu.append(&PredefinedMenuItem::separator())?;
        native.add_item(app, &app_menu, "Quit SoundCraft", "", Some("app.quit".into()))?;
        native.menu.append(&app_menu)?;
        let aliases = menus::ui_aliases();
        for root in menus::tree() {
            let submenu = native.add_branch(app, &root, &aliases)?;
            native.menu.append(&submenu)?;
        }
        native.menu.init_for_nsapp();
        let ctx = ctx.clone();
        MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
            // AppKit calls this on the main thread from the event that chose the item: a key
            // equivalent is a key-down, a click a mouse-up (or Return inside the open menu).
            let current = MainThreadMarker::new().and_then(|mtm| NSApplication::sharedApplication(mtm).currentEvent());
            let key_down = current.filter(|e| e.r#type() == NSEventType::KeyDown).map(|e| {
                let flags = e.modifierFlags();
                let cmd = flags.contains(NSEventModifierFlags::Command);
                let modifiers = egui::Modifiers {
                    alt: flags.contains(NSEventModifierFlags::Option),
                    ctrl: flags.contains(NSEventModifierFlags::Control),
                    shift: flags.contains(NSEventModifierFlags::Shift),
                    mac_cmd: cmd,
                    command: cmd,
                };
                (modifiers, e.isARepeat())
            });
            if tx.send(Chosen { id: event.id, key_down }).is_ok() {
                ctx.request_repaint();
            }
        }));
        Ok(native)
    }

    fn add_branch(&mut self, app: &SoundApp, node: &menus::MenuNode, aliases: &[(&str, &str)]) -> muda::Result<Submenu> {
        let submenu = Submenu::new(&node.label, true);
        for child in &node.children {
            if child.children.is_empty() {
                self.add_item(app, &submenu, &child.label, &child.path, menus::command_for_path(&child.path, aliases))?;
            } else {
                submenu.append(&self.add_branch(app, child, aliases)?)?;
            }
        }
        Ok(submenu)
    }

    fn add_item(&mut self, app: &SoundApp, parent: &Submenu, label: &str, path: &str, command: Option<String>) -> muda::Result<()> {
        let (enabled, checked) = menus::item_state(app, path, command.as_deref());
        // The shortcut the in-window menus show, where it can be a key equivalent.
        let shortcut = command.as_deref().and_then(menus::shortcut_for);
        let (key, accelerator) = shortcut.and_then(|s| Some((shortcuts::menu_key_equivalent(s)?.1, s.parse::<Accelerator>().ok()?))).unzip();
        let item = CheckMenuItem::new(label, enabled, checked, accelerator);
        parent.append(&item)?;
        self.entries.push(Entry { item, command, path: path.into(), enabled, checked, key });
        Ok(())
    }

    /// Sort what AppKit chose since the last call into key equivalents and clicks.
    fn drain(&mut self) {
        while let Ok(chosen) = self.events.try_recv() {
            // A choice changes state (and Cocoa flips the item's check mark): sync on the next pass.
            self.last_sync = None;
            let key = self.entries.iter().find(|e| e.item.id() == &chosen.id).and_then(|e| e.key);
            match (chosen.key_down, key) {
                // Every key equivalent has ⌘ or ⌃; a key-down without them is Return or Space in
                // the open menu, which is a click. `shortcuts::handle` ignores auto-repeats.
                (Some((modifiers, repeat)), Some(key)) if modifiers.command || modifiers.ctrl => {
                    if !repeat {
                        self.keys.push((key, modifiers));
                    }
                }
                _ => self.clicks.push(chosen.id),
            }
        }
    }

    /// Before egui sees this frame's input: each key equivalent becomes the input its key-down
    /// would have been without the menu bar (see `shortcuts::key_equivalent_input`).
    pub fn raw_input(&mut self, ctx: &egui::Context, raw: &mut egui::RawInput) {
        self.drain();
        for (key, modifiers) in self.keys.drain(..) {
            match shortcuts::key_equivalent_input(key, modifiers) {
                KeyEquivalentInput::Events(events) => raw.events.extend(events),
                // The window reads the clipboard and sends the paste with the next frame's input.
                KeyEquivalentInput::Paste => ctx.send_viewport_cmd_to(egui::ViewportId::ROOT, egui::ViewportCommand::RequestPaste),
            }
        }
    }

    /// Run the items chosen by a click, like in-window menu clicks.
    pub fn process(&mut self, app: &mut SoundApp) {
        self.drain();
        for id in std::mem::take(&mut self.clicks) {
            if (id == "hide" || id == "hide_others")
                && let Some(mtm) = MainThreadMarker::new()
            {
                let ns = NSApplication::sharedApplication(mtm);
                if id == "hide" { ns.hide(None) } else { ns.hideOtherApplications(None) }
            } else if let Some(entry) = self.entries.iter().find(|e| e.item.id() == &id)
                && let Some(command) = &entry.command
                && menus::item_state(app, &entry.path, Some(command)).0
            {
                if entry.path.is_empty() && (command.starts_with("window.") || command == "setup.preferences") {
                    // Application-menu window entries open their window rather than toggling it.
                    let _ = app.run(command, serde_json::json!({"value": true}));
                } else {
                    menus::invoke_menu(app, command, &entry.path);
                }
            }
        }
    }

    /// Refresh after engine/control changes; avoid touching Cocoa items with unchanged state.
    pub fn sync(&mut self, app: &SoundApp) {
        let now = std::time::Instant::now();
        if self.last_sync.is_some_and(|t| now.duration_since(t) < SYNC_EVERY) {
            return;
        }
        self.last_sync = Some(now);
        for entry in &mut self.entries {
            let (enabled, checked) = menus::item_state(app, &entry.path, entry.command.as_deref());
            if let Some(label) = menus::live_label(app, entry.command.as_deref())
                && entry.item.text() != label
            {
                entry.item.set_text(label);
            }
            if enabled != entry.enabled {
                entry.item.set_enabled(enabled);
                entry.enabled = enabled;
            }
            // Cocoa toggles check items on click, including ordinary command items.
            if checked != entry.checked || entry.item.is_checked() != checked {
                entry.item.set_checked(checked);
                entry.checked = checked;
            }
        }
    }
}

impl Drop for NativeMenu {
    fn drop(&mut self) {
        self.menu.remove_for_nsapp();
        MenuEvent::set_event_handler(None::<fn(MenuEvent)>);
    }
}
