//! macOS system menu bar. All actions use the same menu dispatch as the egui host.
use muda::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu};
use objc2::MainThreadMarker;
use objc2_app_kit::NSApplication;
use soundcraft_l10n::Language;
use soundcraft_ui_egui::{SoundApp, menus};
use std::sync::mpsc::{Receiver, channel};

struct Entry {
    item: CheckMenuItem,
    command: Option<String>,
    /// The catalog path, or the language id for `ui.language` items.
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
    /// The language the menu was built in: a switch relabels the whole bar in place (`main.rs`).
    pub labelled_in: Language,
}

/// `sync` walks every menu leaf (~500), so it runs a few times a second rather than every frame,
/// and right after a menu click.
const SYNC_EVERY: std::time::Duration = std::time::Duration::from_millis(250);

/// The translated text for an English source label.
fn label(en: &str) -> String {
    soundcraft_l10n::td(en).into_owned()
}

impl NativeMenu {
    /// Called on the UI thread after eframe has created NSApplication.
    pub fn new(app: &SoundApp, ctx: &egui::Context) -> muda::Result<Self> {
        let menu = Menu::new();
        let (tx, events) = channel();
        let mut native = Self { menu, entries: Vec::new(), events, last_sync: None, labelled_in: soundcraft_l10n::current() };
        native.populate(app)?;
        native.menu.init_for_nsapp();
        let ctx = ctx.clone();
        MenuEvent::set_event_handler(Some(move |event| {
            if tx.send(event).is_ok() {
                ctx.request_repaint();
            }
        }));
        Ok(native)
    }

    /// Rebuilds every item in the current language, in place: the same `Menu` stays installed as
    /// the system bar, and the event channel and global handler keep running. Building a second
    /// `NativeMenu` instead would register a new handler and then drop the old one, whose `Drop`
    /// unsets it — after one switch, no click would dispatch again.
    pub fn relabel(&mut self, app: &SoundApp) -> muda::Result<()> {
        self.entries.clear();
        while self.menu.remove_at(0).is_some() {}
        self.last_sync = None;
        self.labelled_in = soundcraft_l10n::current();
        self.populate(app)
    }

    /// The whole bar: the application submenu, then one submenu per menu-tree root.
    fn populate(&mut self, app: &SoundApp) -> muda::Result<()> {
        let app_menu = Submenu::new("SoundCraft", true);
        self.add_item(app, &app_menu, "About SoundCraft", "", Some("window.about".into()))?;
        self.add_item(app, &app_menu, "Session Info", "", Some("window.session_info".into()))?;
        self.add_item(app, &app_menu, "Session Audio Health", "", Some("window.audio_health".into()))?;
        app_menu.append(&PredefinedMenuItem::separator())?;
        self.add_language_menu(app, &app_menu)?;
        app_menu.append(&PredefinedMenuItem::separator())?;
        app_menu.append(&PredefinedMenuItem::services(None))?;
        app_menu.append(&PredefinedMenuItem::separator())?;
        // muda's own Hide items take ⌘H and ⌥⌘H, which are Heal Separation and Copy to Send.
        app_menu.append(&MenuItem::with_id("hide", soundcraft_l10n::t("Hide SoundCraft"), true, None))?;
        app_menu.append(&MenuItem::with_id("hide_others", soundcraft_l10n::t("Hide Others"), true, None))?;
        app_menu.append(&PredefinedMenuItem::show_all(None))?;
        app_menu.append(&PredefinedMenuItem::separator())?;
        self.add_item(app, &app_menu, "Quit SoundCraft", "", Some("app.quit".into()))?;
        self.menu.append(&app_menu)?;
        let aliases = menus::ui_aliases();
        for root in menus::tree() {
            let submenu = self.add_branch(app, &root, &aliases)?;
            self.menu.append(&submenu)?;
        }
        Ok(())
    }

    /// SoundCraft › Language: one item per supported language, checked for the current one.
    /// Items dispatch `ui.language` and carry the language id in `path`.
    fn add_language_menu(&mut self, app: &SoundApp, parent: &Submenu) -> muda::Result<()> {
        let submenu = Submenu::new(soundcraft_l10n::t("Language"), true);
        let current = soundcraft_l10n::current();
        for lang in Language::ALL {
            let name = lang.native_name();
            let item = CheckMenuItem::new(name, true, current == lang, None);
            submenu.append(&item)?;
            self.entries.push(Entry { item, command: Some("ui.language".into()), path: lang.id().into(), enabled: true, checked: current == lang });
        }
        let _ = app;
        parent.append(&submenu)
    }

    /// One catalog subtree, translated at display time; leaves go through `add_item`.
    fn add_branch(&mut self, app: &SoundApp, node: &menus::MenuNode, aliases: &[(&str, &str)]) -> muda::Result<Submenu> {
        let submenu = Submenu::new(label(&node.label), true);
        for child in &node.children {
            if child.children.is_empty() {
                self.add_item(app, &submenu, &child.label, &child.path, menus::command_for_path(&child.path, aliases))?;
            } else {
                submenu.append(&self.add_branch(app, child, aliases)?)?;
            }
        }
        Ok(submenu)
    }

    /// One leaf: a checked menu item whose label is the translation of the English source.
    fn add_item(&mut self, app: &SoundApp, parent: &Submenu, en_label: &str, path: &str, command: Option<String>) -> muda::Result<()> {
        let (enabled, checked) = menus::item_state(app, path, command.as_deref());
        // egui owns keyboard shortcuts, so a key press is dispatched only once.
        let item = CheckMenuItem::new(label(en_label), enabled, checked, None);
        parent.append(&item)?;
        self.entries.push(Entry { item, command, path: path.into(), enabled, checked });
        Ok(())
    }

    /// Dispatches menu clicks: Hide items locally, Language items through `ui.language`, the
    /// rest through the shared menu dispatch.
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
            {
                if command == "ui.language" {
                    let _ = app.run(command, serde_json::json!({"language": entry.path}));
                } else if menus::item_state(app, &entry.path, Some(command)).0 {
                    if entry.path.is_empty() && command.starts_with("window.") {
                        // The application menu's About / Session Info open their window (never close it).
                        let _ = app.run(command, serde_json::json!({"value": true}));
                    } else {
                        menus::invoke_menu(app, command, &entry.path);
                    }
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
