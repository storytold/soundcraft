//! macOS system menu bar. All actions use the same menu dispatch as the egui host.
use muda::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu};
use objc2::MainThreadMarker;
use objc2_app_kit::NSApplication;
use soundcraft_ui_egui::{SoundApp, menus};
use std::sync::mpsc::{Receiver, channel};

struct Entry {
    item: CheckMenuItem,
    command: Option<String>,
    path: String,
    enabled: bool,
    checked: bool,
}

pub struct NativeMenu {
    menu: Menu,
    entries: Vec<Entry>,
    events: Receiver<MenuEvent>,
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
        let mut native = Self { menu, entries: Vec::new(), events, last_sync: None };
        let app_menu = Submenu::new("SoundCraft", true);
        native.add_item(app, &app_menu, "About SoundCraft", "", Some("window.about".into()))?;
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
        MenuEvent::set_event_handler(Some(move |event| {
            if tx.send(event).is_ok() {
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
        // egui owns keyboard shortcuts, so a key press is dispatched only once.
        let item = CheckMenuItem::new(label, enabled, checked, None);
        parent.append(&item)?;
        self.entries.push(Entry { item, command, path: path.into(), enabled, checked });
        Ok(())
    }

    pub fn process(&mut self, app: &mut SoundApp) {
        while let Ok(event) = self.events.try_recv() {
            // A click changes state (and Cocoa flips the item's check mark): sync on the next pass.
            self.last_sync = None;
            if (event.id == "hide" || event.id == "hide_others")
                && let Some(mtm) = MainThreadMarker::new()
            {
                let ns = NSApplication::sharedApplication(mtm);
                if event.id == "hide" { ns.hide(None) } else { ns.hideOtherApplications(None) }
            } else if let Some(entry) = self.entries.iter().find(|e| e.item.id() == &event.id)
                && let Some(command) = &entry.command
                && menus::item_state(app, &entry.path, Some(command)).0
            {
                if entry.path.is_empty() && command.starts_with("window.") {
                    // The application menu's About / Session Info open their window (never close it).
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
