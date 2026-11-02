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
        let (menu, entries) = build(app)?;
        menu.init_for_nsapp();
        let (tx, events) = channel();
        let ctx = ctx.clone();
        MenuEvent::set_event_handler(Some(move |event| {
            if tx.send(event).is_ok() {
                ctx.request_repaint();
            }
        }));
        Ok(Self { menu, entries, events, last_sync: None, labelled_in: soundcraft_l10n::current() })
    }

    /// Rebuilds the whole bar in the current language. The replacement is built unattached and
    /// installed only once complete: on an error nothing has changed yet, so the running bar,
    /// its entries and `labelled_in` stand and `App::logic` can retry. The event channel and
    /// the global handler are never touched — a handler registered by a second `NativeMenu`
    /// would be unset again by the first one's `Drop`, leaving the bar dead after one switch.
    pub fn relabel(&mut self, app: &SoundApp) -> muda::Result<()> {
        let (menu, entries) = build(app)?;
        // Installed before the assignment drops the old menu: a `Menu` whose bar is still
        // installed clears the app menu in its own `Drop`, and this way AppKit swaps in one step.
        menu.init_for_nsapp();
        self.menu = menu;
        self.entries = entries;
        self.last_sync = None;
        self.labelled_in = soundcraft_l10n::current();
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

/// The whole bar, built unattached: the application submenu, then one submenu per menu-tree
/// root. Nothing is installed until the caller has a complete menu in hand.
fn build(app: &SoundApp) -> muda::Result<(Menu, Vec<Entry>)> {
    let menu = Menu::new();
    let mut entries = Vec::new();
    let app_menu = Submenu::new("SoundCraft", true);
    add_item(app, &app_menu, &mut entries, "About SoundCraft", "", Some("window.about".into()))?;
    add_item(app, &app_menu, &mut entries, "Session Info", "", Some("window.session_info".into()))?;
    add_item(app, &app_menu, &mut entries, "Session Audio Health", "", Some("window.audio_health".into()))?;
    app_menu.append(&PredefinedMenuItem::separator())?;
    add_language_menu(&mut entries, &app_menu)?;
    app_menu.append(&PredefinedMenuItem::separator())?;
    app_menu.append(&PredefinedMenuItem::services(None))?;
    app_menu.append(&PredefinedMenuItem::separator())?;
    // muda's own Hide items take ⌘H and ⌥⌘H, which are Heal Separation and Copy to Send.
    app_menu.append(&MenuItem::with_id("hide", soundcraft_l10n::t("Hide SoundCraft"), true, None))?;
    app_menu.append(&MenuItem::with_id("hide_others", soundcraft_l10n::t("Hide Others"), true, None))?;
    app_menu.append(&PredefinedMenuItem::show_all(None))?;
    app_menu.append(&PredefinedMenuItem::separator())?;
    add_item(app, &app_menu, &mut entries, "Quit SoundCraft", "", Some("app.quit".into()))?;
    menu.append(&app_menu)?;
    let aliases = menus::ui_aliases();
    for root in menus::tree() {
        let submenu = add_branch(app, &root, &aliases, &mut entries)?;
        menu.append(&submenu)?;
    }
    Ok((menu, entries))
}

/// SoundCraft › Language: one item per supported language, checked for the current one.
/// Items dispatch `ui.language` and carry the language id in `path`.
fn add_language_menu(entries: &mut Vec<Entry>, parent: &Submenu) -> muda::Result<()> {
    let submenu = Submenu::new(soundcraft_l10n::t("Language"), true);
    let current = soundcraft_l10n::current();
    for lang in Language::ALL {
        let name = lang.native_name();
        let item = CheckMenuItem::new(name, true, current == lang, None);
        submenu.append(&item)?;
        entries.push(Entry { item, command: Some("ui.language".into()), path: lang.id().into(), enabled: true, checked: current == lang });
    }
    parent.append(&submenu)
}

/// One catalog subtree, translated at display time; leaves go through `add_item`.
fn add_branch(app: &SoundApp, node: &menus::MenuNode, aliases: &[(&str, &str)], entries: &mut Vec<Entry>) -> muda::Result<Submenu> {
    let submenu = Submenu::new(label(&node.label), true);
    for child in &node.children {
        if child.children.is_empty() {
            add_item(app, &submenu, entries, &child.label, &child.path, menus::command_for_path(&child.path, aliases))?;
        } else {
            submenu.append(&add_branch(app, child, aliases, entries)?)?;
        }
    }
    Ok(submenu)
}

/// One leaf: a checked menu item whose label is the translation of the English source.
fn add_item(app: &SoundApp, parent: &Submenu, entries: &mut Vec<Entry>, en_label: &str, path: &str, command: Option<String>) -> muda::Result<()> {
    let (enabled, checked) = menus::item_state(app, path, command.as_deref());
    // egui owns keyboard shortcuts, so a key press is dispatched only once.
    let item = CheckMenuItem::new(label(en_label), enabled, checked, None);
    parent.append(&item)?;
    entries.push(Entry { item, command, path: path.into(), enabled, checked });
    Ok(())
}
