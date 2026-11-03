//! SoundCraft Help (Help › SoundCraft Help, Cmd+?): a help window with Contents, Index and Search
//! beside a page for every command.
//!
//! Pages are built from the command registry (label, menu, shortcut) plus a plain-words description
//! per command id in `help/commands.json`. A test fails when a command has no description, so new
//! commands can't ship undocumented.

use crate::SoundApp;
use crate::theme::{Tokens, bold, mono, regular};
use egui::{Align, Key, Layout, Margin, Modifiers, RichText, Stroke, Ui, vec2};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::sync::OnceLock;

/// Where Help › Report a Problem… sends people.
pub const REPORT_URL: &str = "https://github.com/storytold/soundcraft/issues";

/// The descriptions, compiled in so help works offline and in the web build.
const DATA: &str = include_str!("../help/commands.json");

/// Back/Forward remember at most this many pages.
const HISTORY: usize = 100;
/// Search shows at most this many results.
const MAX_RESULTS: usize = 60;
const NAV_WIDTH: f32 = 240.0;

#[derive(Debug, Default, Deserialize)]
struct Category {
    id: String,
    title: String,
    summary: String,
}

#[derive(Debug, Default, Deserialize)]
struct CommandHelp {
    category: String,
    text: String,
    /// What doesn't fully work yet, so help never promises more than the app does.
    #[serde(default)]
    limits: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct ProcessHelp {
    process: String,
    title: String,
    text: String,
}

#[derive(Debug, Default, Deserialize)]
struct Data {
    categories: Vec<Category>,
    commands: BTreeMap<String, CommandHelp>,
    audiosuite: Vec<ProcessHelp>,
}

fn parse() -> Result<Data, serde_json::Error> {
    serde_json::from_str(DATA)
}

/// Help never takes the app down: unreadable data (a test catches that) leaves help empty.
fn data() -> &'static Data {
    static DATA_CELL: OnceLock<Data> = OnceLock::new();
    DATA_CELL.get_or_init(|| parse().unwrap_or_default())
}

/// A help page.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "id", rename_all = "lowercase")]
pub enum Page {
    #[default]
    Welcome,
    Category(String),
    Command(String),
    /// An AudioSuite process, by its SoundCraft process id.
    Process(String),
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Tab {
    #[default]
    Contents,
    Index,
    Search,
}

/// Help window state. The page and tab are kept with the preferences; the rest lasts a session.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct HelpState {
    pub page: Page,
    pub tab: Tab,
    #[serde(skip)]
    pub query: String,
    #[serde(skip)]
    pub index_filter: String,
    #[serde(skip)]
    back: Vec<Page>,
    #[serde(skip)]
    forward: Vec<Page>,
    /// Put the cursor in the search box on the next frame (Help › Search Help…).
    #[serde(skip)]
    pub focus_search: bool,
    /// Open the selected page's category in Contents and scroll to it on the next frame.
    #[serde(skip)]
    reveal: bool,
    /// Whether the window was showing last frame: on opening, Contents reveals the current page.
    #[serde(skip)]
    was_open: bool,
    /// Help › Report a Problem… opens the browser on the next frame (it needs the egui context).
    #[serde(skip)]
    pub open_report: bool,
}

impl HelpState {
    /// Show `page`, remembering the current one for Back.
    pub fn go(&mut self, page: Page) {
        if page == self.page {
            return;
        }
        self.back.push(std::mem::replace(&mut self.page, page));
        if self.back.len() > HISTORY {
            self.back.remove(0);
        }
        self.forward.clear();
        self.reveal = true;
    }

    pub fn can_go_back(&self) -> bool {
        !self.back.is_empty()
    }

    pub fn can_go_forward(&self) -> bool {
        !self.forward.is_empty()
    }

    pub fn go_back(&mut self) {
        if let Some(p) = self.back.pop() {
            self.forward.push(std::mem::replace(&mut self.page, p));
            self.reveal = true;
        }
    }

    pub fn go_forward(&mut self) {
        if let Some(p) = self.forward.pop() {
            self.back.push(std::mem::replace(&mut self.page, p));
            self.reveal = true;
        }
    }
}

/// One page as Index and Search list it.
#[derive(Debug, Clone)]
struct Entry {
    page: Page,
    title: String,
    /// Menu locations, written `File › Save`.
    paths: Vec<String>,
    text: String,
    /// Lowercase title, text, menus, shortcut and id: what search looks through.
    hay: String,
}

/// Menu paths (`File > Save Session`) behind every command, as the menu bar resolves them.
fn menu_paths() -> &'static HashMap<String, Vec<String>> {
    static PATHS: OnceLock<HashMap<String, Vec<String>>> = OnceLock::new();
    PATHS.get_or_init(|| {
        fn walk(nodes: &[crate::menus::MenuNode], aliases: &[(&str, &str)], out: &mut HashMap<String, Vec<String>>) {
            for n in nodes {
                if n.children.is_empty() {
                    if let Some(id) = crate::menus::command_for_path(&n.path, aliases) {
                        out.entry(id).or_default().push(n.path.clone());
                    }
                } else {
                    walk(&n.children, aliases, out);
                }
            }
        }
        let mut out = HashMap::new();
        walk(&crate::menus::tree(), &crate::menus::ui_aliases(), &mut out);
        out
    })
}

fn pretty_path(path: &str) -> String {
    path.replace(" > ", " › ")
}

/// A command's name without the trailing "..." that marks a menu item opening a dialog.
fn command_title(id: &str) -> String {
    let label = soundcraft_engine::find_command(id)
        .map(|c| c.label)
        .or_else(|| crate::menus::UI_COMMANDS.iter().find(|c| c.0 == id).map(|c| c.1))
        .unwrap_or(id);
    label.trim_end_matches("...").trim_end_matches('…').to_string()
}

/// Menu paths of an AudioSuite process (several menu items can run the same process).
fn process_paths(process: &str) -> Vec<String> {
    crate::menus::AUDIOSUITE.iter().filter(|(_, p)| *p == process).map(|(path, _)| (*path).to_string()).collect()
}

fn entries() -> &'static [Entry] {
    static ENTRIES: OnceLock<Vec<Entry>> = OnceLock::new();
    ENTRIES.get_or_init(|| {
        let d = data();
        let mut v = Vec::new();
        for (id, h) in &d.commands {
            let title = command_title(id);
            let paths: Vec<String> = menu_paths().get(id).map(|p| p.iter().map(|s| pretty_path(s)).collect()).unwrap_or_default();
            let shortcut = crate::menus::shortcut_for(id).unwrap_or("");
            let hay = format!("{title} {} {} {shortcut} {id}", h.text, paths.join(" ")).to_lowercase();
            v.push(Entry { page: Page::Command(id.clone()), title, paths, text: h.text.clone(), hay });
        }
        for p in &d.audiosuite {
            let paths: Vec<String> = process_paths(&p.process).iter().map(|s| pretty_path(s)).collect();
            let hay = format!("{} {} {} audiosuite", p.title, p.text, paths.join(" ")).to_lowercase();
            v.push(Entry { page: Page::Process(p.process.clone()), title: p.title.clone(), paths, text: p.text.clone(), hay });
        }
        v.sort_by_cached_key(|e| e.title.to_lowercase());
        v
    })
}

/// Search results, best first. Every word must appear somewhere on the page, or the title must
/// match loosely (letters in order, so "bnc mix" finds Bounce Mix).
fn search(query: &str) -> Vec<&'static Entry> {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return Vec::new();
    }
    let words: Vec<&str> = q.split_whitespace().collect();
    let mut hits: Vec<(i32, &Entry)> = entries()
        .iter()
        .filter_map(|e| {
            let title = e.title.to_lowercase();
            let s = if let Some(at) = title.find(&q) {
                3000 - i32::try_from(at).unwrap_or(999)
            } else if words.iter().all(|w| title.contains(w)) {
                2500
            } else if words.iter().all(|w| e.hay.contains(w)) {
                1000
            } else {
                crate::palette::score(&e.title, &q).filter(|s| *s > 450)?
            };
            Some((s, e))
        })
        .collect();
    hits.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.title.cmp(&b.1.title)));
    hits.truncate(MAX_RESULTS);
    hits.into_iter().map(|(_, e)| e).collect()
}

/// Every page help can show, for tests and agents.
pub fn pages() -> Vec<Page> {
    let mut v = vec![Page::Welcome];
    v.extend(data().categories.iter().map(|c| Page::Category(c.id.clone())));
    v.extend(entries().iter().map(|e| e.page.clone()));
    v
}

fn entry_for(page: &Page) -> Option<&'static Entry> {
    entries().iter().find(|e| &e.page == page)
}

fn category(id: &str) -> Option<&'static Category> {
    data().categories.iter().find(|c| c.id == id)
}

/// The category a page sits under in Contents.
fn category_of(page: &Page) -> Option<&str> {
    match page {
        Page::Welcome => None,
        Page::Category(c) => Some(c.as_str()),
        Page::Command(id) => data().commands.get(id).map(|h| h.category.as_str()),
        Page::Process(_) => Some("processing"),
    }
}

/// Pages under a category, in title order.
fn pages_in(category: &str) -> impl Iterator<Item = &'static Entry> + '_ {
    entries().iter().filter(move |e| category_of(&e.page) == Some(category))
}

/// The first sentence of a description, for lists ("e.g." and "i.e." don't end a sentence).
fn first_sentence(text: &str) -> &str {
    let mut from = 0;
    while let Some(i) = text.get(from..).and_then(|t| t.find(". ")).map(|i| i + from) {
        let before = text.get(..=i).unwrap_or(text);
        if !(before.ends_with("e.g.") || before.ends_with("i.e.")) {
            return before;
        }
        from = i + 2;
    }
    text
}

/// The help window and the Help menu's pending browser link. Runs every frame.
pub fn show(app: &mut SoundApp, ctx: &egui::Context) {
    if std::mem::take(&mut app.ui.help.open_report) {
        ctx.open_url(egui::OpenUrl::new_tab(REPORT_URL));
    }
    if !app.ui.show_help {
        app.ui.help.was_open = false;
        return;
    }
    if !std::mem::replace(&mut app.ui.help.was_open, true) {
        app.ui.help.reveal = true;
    }
    let mut open = true;
    let shown = egui::Window::new("SoundCraft Help")
        .open(&mut open)
        .default_size(vec2(900.0, 600.0))
        .min_size(vec2(560.0, 360.0))
        .collapsible(false)
        .show(ctx, |ui| window_body(app, ui));
    // Alt+Left / Alt+Right are Back and Forward while the pointer is over help.
    if shown.is_some_and(|r| r.response.contains_pointer()) {
        let (back, forward) = ctx.input_mut(|i| (i.consume_key(Modifiers::ALT, Key::ArrowLeft), i.consume_key(Modifiers::ALT, Key::ArrowRight)));
        if back {
            app.ui.help.go_back();
        }
        if forward {
            app.ui.help.go_forward();
        }
    }
    app.ui.show_help = open;
}

fn window_body(app: &mut SoundApp, ui: &mut Ui) {
    toolbar(app, ui);
    ui.separator();
    let height = ui.available_height();
    ui.horizontal_top(|ui| {
        ui.allocate_ui_with_layout(vec2(NAV_WIDTH, height), Layout::top_down(Align::Min), |ui| {
            ui.set_min_size(vec2(NAV_WIDTH, height));
            navigation(app, ui);
        });
        ui.separator();
        egui::ScrollArea::vertical().id_salt("help_page").auto_shrink([false, false]).show(ui, |ui| {
            // The row above lays out left to right; a page reads top to bottom, at most 720 wide.
            let width = ui.available_width().min(720.0);
            ui.allocate_ui_with_layout(vec2(width, 0.0), Layout::top_down(Align::Min), |ui| {
                ui.set_max_width(width);
                page(app, ui);
            });
        });
    });
    app.ui.help.reveal = false;
}

fn toolbar(app: &mut SoundApp, ui: &mut Ui) {
    let t = Tokens::current();
    ui.horizontal(|ui| {
        let h = &mut app.ui.help;
        if ui.add_enabled(h.can_go_back(), egui::Button::new(RichText::new("‹").font(bold(16.0)))).on_hover_text("Back (Alt+Left)").clicked() {
            h.go_back();
        }
        if ui.add_enabled(h.can_go_forward(), egui::Button::new(RichText::new("›").font(bold(16.0)))).on_hover_text("Forward (Alt+Right)").clicked()
        {
            h.go_forward();
        }
        ui.add_space(8.0);
        let edit = egui::TextEdit::singleline(&mut h.query).hint_text("Search help").desired_width(320.0).font(regular(13.0));
        let r = ui.add(edit);
        if std::mem::take(&mut h.focus_search) {
            r.request_focus();
        }
        if r.changed() && !h.query.trim().is_empty() {
            h.tab = Tab::Search;
        }
        if !h.query.is_empty() && ui.small_button("✕").on_hover_text("Clear the search").clicked() {
            h.query.clear();
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.label(RichText::new(format!("{} pages", entries().len())).small().color(t.text_dim));
        });
    });
}

fn navigation(app: &mut SoundApp, ui: &mut Ui) {
    ui.horizontal(|ui| {
        for (tab, name) in [(Tab::Contents, "Contents"), (Tab::Index, "Index"), (Tab::Search, "Search")] {
            if ui.selectable_label(app.ui.help.tab == tab, name).clicked() {
                app.ui.help.tab = tab;
                if tab == Tab::Search {
                    app.ui.help.focus_search = true;
                }
            }
        }
    });
    ui.add_space(4.0);
    match app.ui.help.tab {
        Tab::Contents => contents(app, ui),
        Tab::Index => index(app, ui),
        Tab::Search => search_results(app, ui),
    }
}

fn contents(app: &mut SoundApp, ui: &mut Ui) {
    let h = &mut app.ui.help;
    let current = category_of(&h.page);
    let mut go = None;
    egui::ScrollArea::vertical().id_salt("help_contents").auto_shrink([false, false]).show(ui, |ui| {
        if ui.selectable_label(h.page == Page::Welcome, "Welcome").clicked() {
            go = Some(Page::Welcome);
        }
        for c in &data().categories {
            let id = ui.make_persistent_id(("help_category", &c.id));
            let mut state = egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, false);
            if h.reveal && current == Some(c.id.as_str()) {
                state.set_open(true);
            }
            state
                .show_header(ui, |ui| {
                    let r = ui.selectable_label(h.page == Page::Category(c.id.clone()), &c.title);
                    if r.clicked() {
                        go = Some(Page::Category(c.id.clone()));
                    }
                    if h.reveal && h.page == Page::Category(c.id.clone()) {
                        r.scroll_to_me(Some(Align::Center));
                    }
                })
                .body(|ui| {
                    for e in pages_in(&c.id) {
                        let r = ui.selectable_label(h.page == e.page, &e.title);
                        if r.clicked() {
                            go = Some(e.page.clone());
                        }
                        if h.reveal && h.page == e.page {
                            r.scroll_to_me(Some(Align::Center));
                        }
                    }
                });
        }
    });
    if let Some(p) = go {
        h.go(p);
        // Clicking in the tree should not also re-scroll it.
        h.reveal = false;
    }
}

fn index(app: &mut SoundApp, ui: &mut Ui) {
    let t = Tokens::current();
    let h = &mut app.ui.help;
    ui.add(egui::TextEdit::singleline(&mut h.index_filter).hint_text("Type a word to jump to it").desired_width(f32::INFINITY));
    let filter = h.index_filter.trim().to_lowercase();
    let mut go = None;
    egui::ScrollArea::vertical().id_salt("help_index").auto_shrink([false, false]).show(ui, |ui| {
        let mut letter = None;
        let titles: Vec<&Entry> = entries().iter().filter(|e| e.title.to_lowercase().starts_with(&filter) || filter.is_empty()).collect();
        for (i, e) in titles.iter().enumerate() {
            let first = e.title.chars().next().map(|c| c.to_ascii_uppercase());
            if first != letter {
                letter = first;
                ui.add_space(4.0);
                ui.label(RichText::new(first.map(String::from).unwrap_or_default()).font(bold(12.0)).color(t.text_dim));
            }
            // The same name twice (Clip List, Workspace) gets its menu so the two can be told apart.
            let twin = titles.get(i.wrapping_sub(1)).is_some_and(|p| p.title == e.title) || titles.get(i + 1).is_some_and(|n| n.title == e.title);
            let label = match e.paths.first() {
                Some(path) if twin => format!("{}  ({path})", e.title),
                _ => e.title.clone(),
            };
            if ui.selectable_label(h.page == e.page, label).clicked() {
                go = Some(e.page.clone());
            }
        }
        if titles.is_empty() {
            ui.label(RichText::new("Nothing starts with that. Try Search instead.").color(t.text_dim));
        }
    });
    if let Some(p) = go {
        h.go(p);
    }
}

fn search_results(app: &mut SoundApp, ui: &mut Ui) {
    let t = Tokens::current();
    let h = &mut app.ui.help;
    let hits = search(&h.query);
    let mut go = None;
    egui::ScrollArea::vertical().id_salt("help_search").auto_shrink([false, false]).show(ui, |ui| {
        if h.query.trim().is_empty() {
            ui.label(RichText::new("Type in the search box above: a word, a menu name or a shortcut such as Cmd+S.").color(t.text_dim));
            return;
        }
        if hits.is_empty() {
            ui.label(RichText::new("No pages match. Try fewer or different words.").color(t.text_dim));
        }
        for e in hits {
            let r = ui.selectable_label(h.page == e.page, RichText::new(&e.title).font(bold(12.5)));
            ui.label(RichText::new(first_sentence(&e.text)).small().color(t.text_dim));
            ui.add_space(4.0);
            if r.clicked() {
                go = Some(e.page.clone());
            }
        }
    });
    if let Some(p) = go {
        h.go(p);
    }
}

/// A rounded tag: a menu location or a shortcut.
fn chip(ui: &mut Ui, text: RichText) {
    let t = Tokens::current();
    egui::Frame::new().stroke(Stroke::new(1.0, t.border_light)).corner_radius(10).inner_margin(Margin::symmetric(8, 2)).show(ui, |ui| {
        ui.label(text);
    });
}

fn page(app: &mut SoundApp, ui: &mut Ui) {
    let current = app.ui.help.page.clone();
    match current {
        Page::Welcome => welcome(app, ui),
        Page::Category(id) => category_page(app, ui, &id),
        Page::Command(id) => command_page(app, ui, &id),
        Page::Process(id) => process_page(app, ui, &id),
    }
}

fn title(ui: &mut Ui, text: &str) {
    ui.add_space(6.0);
    ui.label(RichText::new(text).font(bold(20.0)));
    ui.add_space(6.0);
}

fn welcome(app: &mut SoundApp, ui: &mut Ui) {
    let t = Tokens::current();
    title(ui, "SoundCraft Help");
    ui.label("Every command in SoundCraft has a page here that says what it does, where it is in the menus and its shortcut.");
    ui.add_space(4.0);
    ui.label(RichText::new("Browse by topic in Contents, look a word up in Index, or type what you want to do in the search box.").color(t.text_dim));
    ui.add_space(12.0);
    let mut go = None;
    egui::Grid::new("help_topics").num_columns(2).spacing(vec2(16.0, 10.0)).show(ui, |ui| {
        for (i, c) in data().categories.iter().enumerate() {
            ui.vertical(|ui| {
                ui.set_width(300.0);
                if ui.link(RichText::new(&c.title).font(bold(13.0))).clicked() {
                    go = Some(Page::Category(c.id.clone()));
                }
                ui.label(RichText::new(&c.summary).small().color(t.text_dim));
            });
            if i % 2 == 1 {
                ui.end_row();
            }
        }
    });
    if let Some(p) = go {
        app.ui.help.go(p);
    }
}

fn category_page(app: &mut SoundApp, ui: &mut Ui, id: &str) {
    let t = Tokens::current();
    let Some(c) = category(id) else {
        title(ui, "Page not found");
        return;
    };
    title(ui, &c.title);
    ui.label(RichText::new(&c.summary).color(t.text_dim));
    ui.add_space(10.0);
    let mut go = None;
    for e in pages_in(id) {
        if ui.link(RichText::new(&e.title).font(bold(12.5))).clicked() {
            go = Some(e.page.clone());
        }
        ui.label(first_sentence(&e.text));
        ui.add_space(6.0);
    }
    if let Some(p) = go {
        app.ui.help.go(p);
    }
}

fn breadcrumb(app: &mut SoundApp, ui: &mut Ui, category_id: &str) {
    let t = Tokens::current();
    if let Some(c) = category(category_id)
        && ui.link(RichText::new(&c.title).small().color(t.text_dim)).clicked()
    {
        app.ui.help.go(Page::Category(c.id.clone()));
    }
}

fn limits_note(ui: &mut Ui, limits: &str) {
    let t = Tokens::current();
    ui.add_space(8.0);
    egui::Frame::new().stroke(Stroke::new(1.0, t.meter_yellow)).corner_radius(8).inner_margin(Margin::same(10)).show(ui, |ui| {
        ui.label(RichText::new("Current limits").font(bold(12.0)).color(t.meter_yellow));
        ui.label(limits);
    });
}

fn command_page(app: &mut SoundApp, ui: &mut Ui, id: &str) {
    let t = Tokens::current();
    let (Some(h), Some(e)) = (data().commands.get(id), entry_for(&Page::Command(id.to_string()))) else {
        title(ui, "Page not found");
        return;
    };
    breadcrumb(app, ui, &h.category);
    title(ui, &e.title);
    let mac = ui.ctx().os().is_mac();
    let shortcut = crate::menus::shortcut_for(id).unwrap_or("");
    ui.horizontal_wrapped(|ui| {
        for path in &e.paths {
            chip(ui, RichText::new(path));
        }
        if !shortcut.is_empty() {
            let label = crate::shortcuts::shortcut_label(shortcut, mac);
            if label.is_empty() {
                // A Mac Control chord: Windows and Linux have no key for it.
                chip(ui, RichText::new(format!("{shortcut} (Mac only)")).font(mono(11.5)));
            } else {
                chip(ui, RichText::new(label).font(mono(11.5)));
            }
        }
    });
    ui.add_space(8.0);
    ui.label(RichText::new(&h.text).font(regular(14.0)));
    if let Some(limits) = &h.limits {
        limits_note(ui, limits);
    }
    ui.add_space(12.0);
    match e.paths.first().map(|p| p.replace(" › ", " > ")) {
        Some(path) => {
            let (enabled, _) = crate::menus::item_state(app, &path, Some(id));
            let r = ui.add_enabled(enabled, egui::Button::new(RichText::new("Do it now").font(bold(13.0))).min_size(vec2(110.0, 28.0)));
            let r = r.on_hover_text(format!("Same as choosing {} from the menu", pretty_path(&path)));
            if let Some(why) = why_disabled(app, id).filter(|_| !enabled) {
                ui.label(RichText::new(format!("Not available right now: {why}")).small().color(t.text_dim));
            }
            if r.clicked() {
                crate::menus::invoke_menu(app, id, &path);
            }
        }
        None if shortcut.is_empty() => {
            ui.label(RichText::new("Not in the menus. Scripts and agents run it by its command id.").small().color(t.text_dim));
        }
        None => {}
    }
    ui.add_space(16.0);
    ui.label(RichText::new(format!("Command id: {id}")).font(mono(10.5)).color(t.text_dim));
}

/// Why an engine command can't run now, as the command itself explains it.
fn why_disabled(app: &SoundApp, id: &str) -> Option<String> {
    let c = soundcraft_engine::find_command(id)?;
    if app.engine.blocked_while_recording(id) {
        return Some("stop recording first.".into());
    }
    (c.enabled)(&app.engine).err()
}

fn process_page(app: &mut SoundApp, ui: &mut Ui, process: &str) {
    let t = Tokens::current();
    let Some(p) = data().audiosuite.iter().find(|p| p.process == process) else {
        title(ui, "Page not found");
        return;
    };
    breadcrumb(app, ui, "processing");
    title(ui, &p.title);
    let paths = process_paths(process);
    ui.label(RichText::new("Find it in the AudioSuite menu:").small().color(t.text_dim));
    ui.horizontal_wrapped(|ui| {
        for path in &paths {
            chip(ui, RichText::new(pretty_path(path)));
        }
    });
    ui.add_space(8.0);
    ui.label(RichText::new(&p.text).font(regular(14.0)));
    ui.add_space(4.0);
    ui.label(RichText::new("AudioSuite processes the selected clips offline and writes the result as new audio files.").color(t.text_dim));
    ui.add_space(12.0);
    if let Some(path) = paths.first()
        && ui.add(egui::Button::new(RichText::new("Open it").font(bold(13.0))).min_size(vec2(110.0, 28.0))).clicked()
    {
        crate::menus::invoke_menu(app, "audiosuite.process", path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn help_data_parses_and_names_known_categories() {
        let d = parse().unwrap();
        assert_eq!(d.categories.len(), 18);
        let ids: HashSet<&str> = d.categories.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(ids.len(), 18, "category ids are unique");
        for (id, h) in &d.commands {
            assert!(ids.contains(h.category.as_str()), "{id}: unknown category {}", h.category);
            assert!(!h.text.trim().is_empty(), "{id}: empty description");
            assert!(h.limits.as_ref().is_none_or(|l| !l.trim().is_empty()), "{id}: empty limits");
        }
    }

    /// Every command in the app has a help page, and every help page is a real command: a new
    /// command must ship with its plain-words description in `help/commands.json`.
    #[test]
    fn every_command_has_a_help_page() {
        let d = parse().unwrap();
        let engine = soundcraft_engine::command_specs().iter().map(|c| c.id);
        let ui = crate::menus::UI_COMMANDS.iter().map(|c| c.0);
        let all: HashSet<&str> = engine.chain(ui).collect();
        let missing: Vec<&&str> = all.iter().filter(|id| !d.commands.contains_key(**id)).collect();
        assert!(missing.is_empty(), "add a description to crates/ui-egui/help/commands.json for: {missing:?}");
        let stale: Vec<&String> = d.commands.keys().filter(|id| !all.contains(id.as_str())).collect();
        assert!(stale.is_empty(), "help/commands.json describes commands that no longer exist: {stale:?}");
    }

    #[test]
    fn every_audiosuite_process_has_a_help_page() {
        let d = parse().unwrap();
        let processes: HashSet<&str> = crate::menus::AUDIOSUITE.iter().map(|(_, p)| *p).collect();
        let described: HashSet<&str> = d.audiosuite.iter().map(|p| p.process.as_str()).collect();
        assert_eq!(processes, described);
    }

    #[test]
    fn search_finds_titles_words_shortcuts_and_loose_spellings() {
        let first = |q: &str| search(q).first().map(|e| e.page.clone());
        assert_eq!(first("bounce mix"), Some(Page::Command("file.bounce_mix".into())));
        assert_eq!(first("bnc mix"), Some(Page::Command("file.bounce_mix".into())));
        assert!(search("reverb").iter().any(|e| e.page == Page::Process("room_reverb".into())));
        let save = crate::menus::shortcut_for("session.save").unwrap().to_lowercase();
        assert!(search(&save).iter().any(|e| e.page == Page::Command("session.save".into())));
        assert!(search("zzqqxx").is_empty());
        assert!(search("   ").is_empty());
    }

    #[test]
    fn commands_in_the_menus_list_where_they_are() {
        let e = entry_for(&Page::Command("session.save".into())).unwrap();
        assert!(e.paths.iter().any(|p| p.starts_with("File › ")), "{:?}", e.paths);
        let e = entry_for(&Page::Command("help.open".into())).unwrap();
        assert_eq!(e.paths, ["Help › SoundCraft Help"]);
    }

    #[test]
    fn first_sentence_skips_abbreviations() {
        assert_eq!(first_sentence("Adds a send (e.g. to a reverb). Then more."), "Adds a send (e.g. to a reverb).");
        assert_eq!(first_sentence("One. Two."), "One.");
        assert_eq!(first_sentence("No full stop"), "No full stop");
        assert_eq!(first_sentence("Ends with e.g. "), "Ends with e.g. ");
    }

    #[test]
    fn back_and_forward_walk_the_history() {
        let mut h = HelpState::default();
        assert!(!h.can_go_back());
        h.go(Page::Category("mixing".into()));
        h.go(Page::Command("session.save".into()));
        h.go(Page::Command("session.save".into()));
        h.go_back();
        assert_eq!(h.page, Page::Category("mixing".into()));
        h.go_back();
        assert_eq!(h.page, Page::Welcome);
        assert!(!h.can_go_back());
        h.go_forward();
        assert_eq!(h.page, Page::Category("mixing".into()));
        h.go(Page::Welcome);
        assert!(!h.can_go_forward(), "a new page drops Forward");
        for i in 0..2 * HISTORY {
            h.go(Page::Command(format!("x{i}")));
        }
        assert_eq!(h.back.len(), HISTORY);
    }

    #[test]
    fn page_and_tab_are_saved_with_the_preferences() {
        let mut h = HelpState { tab: Tab::Index, ..HelpState::default() };
        h.go(Page::Command("session.save".into()));
        let v = serde_json::to_value(&h).unwrap();
        assert_eq!(v, serde_json::json!({"page": {"kind": "command", "id": "session.save"}, "tab": "index"}));
        let back: HelpState = serde_json::from_value(v).unwrap();
        assert_eq!(back.page, h.page);
        assert_eq!(back.tab, Tab::Index);
    }
}
