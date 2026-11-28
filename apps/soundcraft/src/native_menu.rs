//! macOS system menu bar. All actions use the same menu dispatch as the egui host.
use muda::{CheckMenuItem, Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem, Submenu};
use objc2::MainThreadMarker;
use objc2_app_kit::NSApplication;
use soundcraft_ui_egui::menus::dynamic::{self, Action, Contents, Entry};
use soundcraft_ui_egui::{SoundApp, menus};
use std::sync::mpsc::{Receiver, channel};

struct Leaf {
    item: CheckMenuItem,
    command: Option<String>,
    path: String,
    enabled: bool,
    checked: bool,
}

/// One of `menus::dynamic::SUBMENUS` (Open Recent, Instrument): its Cocoa submenu, rebuilt
/// whenever what it should show changes.
struct Dynamic {
    /// Its index in `SUBMENUS`, and in what `dynamic::submenus` returns.
    index: usize,
    submenu: Submenu,
    /// What it shows now; `None` until first filled, and after a click.
    shown: Option<Contents>,
    actions: Vec<(MenuId, Action)>,
}

pub struct NativeMenu {
    menu: Menu,
    entries: Vec<Leaf>,
    dynamic: Vec<Dynamic>,
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
        let mut native = Self { menu, entries: Vec::new(), dynamic: Vec::new(), events, last_sync: None };
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
            // After its entry, leaf or submenu (`File > Open` is a submenu), as the egui menu bar
            // places it. Filled on the first `sync`.
            for (index, d) in dynamic::SUBMENUS.iter().enumerate().filter(|(_, d)| d.after == child.path) {
                let dynamic = Submenu::new(soundcraft_ui_egui::localization::text(d.label), false);
                submenu.append(&dynamic)?;
                self.dynamic.push(Dynamic { index, submenu: dynamic, shown: None, actions: Vec::new() });
            }
        }
        Ok(submenu)
    }

    fn add_item(&mut self, app: &SoundApp, parent: &Submenu, label: &str, path: &str, command: Option<String>) -> muda::Result<()> {
        let (enabled, checked) = menus::item_state(app, path, command.as_deref());
        // egui owns keyboard shortcuts, so a key press is dispatched only once.
        let item = CheckMenuItem::new(label, enabled, checked, None);
        parent.append(&item)?;
        self.entries.push(Leaf { item, command, path: path.into(), enabled, checked });
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
            } else if let Some(action) = self.dynamic.iter().flat_map(|d| &d.actions).find(|(id, _)| id == &event.id).map(|(_, a)| a.clone()) {
                // Cocoa flips a clicked item's check mark: rebuild from the app's state.
                for d in &mut self.dynamic {
                    d.shown = None;
                }
                dynamic::run(app, &action);
            } else if let Some(entry) = self.entries.iter().find(|e| e.item.id() == &event.id)
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
    pub fn sync(&mut self, app: &mut SoundApp) {
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
        let wanted = dynamic::submenus(app);
        for d in &mut self.dynamic {
            let Some(want) = wanted.get(d.index) else { continue };
            if d.shown.as_ref() != Some(want)
                && let Err(e) = d.rebuild(want)
            {
                log::warn!("menu bar: {e}");
            }
        }
    }
}

impl Dynamic {
    fn rebuild(&mut self, want: &Contents) -> muda::Result<()> {
        while self.submenu.remove_at(0).is_some() {}
        self.actions.clear();
        // Mark it shown first: a failure part way is logged once, not retried every sync.
        self.shown = Some(want.clone());
        self.submenu.set_text(literal(&want.title));
        self.submenu.set_enabled(want.enabled);
        append(&self.submenu, &want.entries, &mut self.actions)
    }
}

/// `text` as muda shows it: muda takes `&` as a mnemonic marker and drops it, so a session or
/// vendor called "R&B" would read "RB"; `&&` shows one `&`.
fn literal(text: &str) -> String {
    text.replace('&', "&&")
}

/// Adds `entries` to `parent`, noting the action of each item that has one.
fn append(parent: &Submenu, entries: &[Entry], actions: &mut Vec<(MenuId, Action)>) -> muda::Result<()> {
    for e in entries {
        match e {
            Entry::Item { label, enabled, checked, action } => {
                let item = CheckMenuItem::new(literal(label), *enabled, *checked, None);
                parent.append(&item)?;
                if let Some(a) = action {
                    actions.push((item.id().clone(), a.clone()));
                }
            }
            Entry::Submenu { label, entries } => {
                let sub = Submenu::new(literal(label), true);
                append(&sub, entries, actions)?;
                parent.append(&sub)?;
            }
            Entry::Separator => parent.append(&PredefinedMenuItem::separator())?,
        }
    }
    Ok(())
}

impl Drop for NativeMenu {
    fn drop(&mut self) {
        self.menu.remove_for_nsapp();
        MenuEvent::set_event_handler(None::<fn(MenuEvent)>);
    }
}
