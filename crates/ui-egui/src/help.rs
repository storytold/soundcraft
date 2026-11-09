//! SoundCraft Help (Help › SoundCraft Help, Cmd+?): a help window with Contents, Index and Search
//! beside a page for every command.
//!
//! Pages are built from the command registry (label, menu, shortcut) plus a plain-words description
//! per command id in `help/commands.json`. A test fails when a command has no description, so new
//! commands can't ship undocumented. How-to guides and key pages live in `help/guides.json`; their
//! text names commands by id (`{menu:session.save}`, `{key:session.save}`), so menus and shortcuts
//! shown in a guide always match the app.
//!
//! Cmd+? opens help on the page for the window under the pointer.

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
const GUIDES: &str = include_str!("../help/guides.json");

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

/// A how-to guide or a page of keys.
#[derive(Debug, Default, Deserialize)]
struct Guide {
    id: String,
    title: String,
    category: String,
    summary: String,
    /// Numbered steps, or notes when the guide is a table of keys.
    #[serde(default)]
    steps: Vec<String>,
    /// Key and what it does, for key pages.
    #[serde(default)]
    keys: Vec<(String, String)>,
    /// Command pages to read next.
    #[serde(default)]
    related: Vec<String>,
}

#[derive(Debug, Default, Deserialize)]
struct Data {
    categories: Vec<Category>,
    commands: BTreeMap<String, CommandHelp>,
    audiosuite: Vec<ProcessHelp>,
    #[serde(skip)]
    guides: Vec<Guide>,
}

#[derive(Deserialize)]
struct GuideFile {
    guides: Vec<Guide>,
}

/// Parse the bundled command and guide catalogs, returning either JSON error to the caller.
fn parse() -> Result<Data, serde_json::Error> {
    let mut d: Data = serde_json::from_str(DATA)?;
    d.guides = serde_json::from_str::<GuideFile>(GUIDES)?.guides;
    Ok(d)
}

/// Help never takes the app down: unreadable data (a test catches that) leaves help empty.
fn data() -> &'static Data {
    static DATA_CELL: OnceLock<Data> = OnceLock::new();
    DATA_CELL.get_or_init(|| {
        parse().unwrap_or_else(|error| {
            log::error!("Could not load the bundled SoundCraft Help catalog: {error}");
            Data::default()
        })
    })
}

/// A help page.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "id", rename_all = "lowercase")]
pub enum Page {
    /// The landing page with introductory guides and topics.
    #[default]
    Welcome,
    /// A topic page identified by its catalog category.
    Category(String),
    /// An engine or UI command identified by its command ID.
    Command(String),
    /// An AudioSuite process, by its SoundCraft process id.
    Process(String),
    /// A how-to guide or key page, by its id in `guides.json`.
    Guide(String),
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Tab {
    /// Topics with their guides and commands.
    #[default]
    Contents,
    /// An alphabetical list of pages.
    Index,
    /// Ranked matches for the search query.
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
    /// Give keyboard focus to Help's selected tab on an explicit open.
    #[serde(skip)]
    pub(crate) focus_help: bool,
    /// Open the selected page's category in Contents and scroll to it on the next frame.
    #[serde(skip)]
    reveal: bool,
    /// Whether the window was showing last frame: on opening, Contents reveals the current page.
    #[serde(skip)]
    was_open: bool,
    /// Help › Report a Problem… opens the browser on the next frame (it needs the egui context).
    #[serde(skip)]
    pub open_report: bool,
    /// A text box had the keyboard at the end of last frame. egui drops focus as soon as Escape is
    /// pressed, so this is how Escape that leaves a box is told from Escape that closes help.
    #[serde(skip)]
    typing: bool,
    /// egui can clear the focused widget before Help sees an Escape press.
    #[serde(skip)]
    keyboard_focus: bool,
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
    /// The title in lists: with its menu location added when another page has the same title
    /// (Help › Keyboard Shortcuts and Setup › Keyboard Shortcuts… are different commands).
    name: String,
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

/// The SoundCraft application menu, which is built by hand rather than from the menu tree: the
/// item's label in the app's own menu bar (Windows, Linux, web), on macOS ("" when the Mac menu
/// doesn't have it), and its command.
const APP_MENU: [(&str, &str, &str); 5] = [
    ("About SoundCraft", "About SoundCraft", "window.about"),
    ("Check for Updates…", "", "app.checkForUpdates"),
    ("Session Info", "Session Info", "window.session_info"),
    ("Session Audio Health", "Session Audio Health", "window.audio_health"),
    ("Quit", "Quit SoundCraft", "app.quit"),
];

/// Where a command is in the SoundCraft application menu on this platform.
fn app_menu_path(id: &str, mac: bool) -> Option<String> {
    let (label, mac_label, _) = APP_MENU.iter().find(|(_, _, c)| *c == id)?;
    let label = if mac { mac_label } else { label };
    (!label.is_empty()).then(|| format!("SoundCraft › {label}"))
}

/// Show a registry menu path using the separators used on help pages.
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

/// Build and cache the searchable catalog, disambiguating duplicate titles by menu location.
/// The command registry, menu tree and bundled Help files are fixed for this build.
fn entries() -> &'static [Entry] {
    static ENTRIES: OnceLock<Vec<Entry>> = OnceLock::new();
    ENTRIES.get_or_init(|| {
        let d = data();
        let mut v = Vec::new();
        for (id, h) in &d.commands {
            let title = command_title(id);
            let paths: Vec<String> = menu_paths().get(id).map(|p| p.iter().map(|s| pretty_path(s)).collect()).unwrap_or_default();
            // Search finds a shortcut as written on a Mac (Cmd+S) and on a PC (Ctrl+S).
            let keys = crate::menus::shortcut_for(id).unwrap_or("");
            let shortcut = format!("{keys} {}", crate::shortcuts::shortcut_label(keys, false));
            let app_menu = [true, false].map(|mac| app_menu_path(id, mac).unwrap_or_default()).join(" ");
            let hay = format!("{title} {} {} {app_menu} {shortcut} {id}", h.text, paths.join(" ")).to_lowercase();
            v.push(Entry { page: Page::Command(id.clone()), name: title.clone(), title, paths, text: h.text.clone(), hay });
        }
        for p in &d.audiosuite {
            let paths: Vec<String> = process_paths(&p.process).iter().map(|s| pretty_path(s)).collect();
            let hay = format!("{} {} {} audiosuite", p.title, p.text, paths.join(" ")).to_lowercase();
            v.push(Entry { page: Page::Process(p.process.clone()), title: p.title.clone(), name: p.title.clone(), paths, text: p.text.clone(), hay });
        }
        for g in &d.guides {
            // Search reads guides as shown on a Mac and on a PC, so both Cmd+S and Ctrl+S find them.
            let mut hay = format!("{} {} guide how to", g.title, g.summary);
            for mac in [true, false] {
                for line in g.steps.iter().chain(g.keys.iter().flat_map(|(k, what)| [k, what])) {
                    hay.push(' ');
                    hay.extend(spans(line, mac).into_iter().map(|(t, _)| t));
                }
            }
            v.push(Entry {
                page: Page::Guide(g.id.clone()),
                title: g.title.clone(),
                name: g.title.clone(),
                paths: Vec::new(),
                text: g.summary.clone(),
                hay: hay.to_lowercase(),
            });
        }
        v.sort_by_cached_key(|e| e.title.to_lowercase());
        let mut count: HashMap<String, usize> = HashMap::new();
        for e in &v {
            *count.entry(e.title.clone()).or_default() += 1;
        }
        for e in &mut v {
            if count.get(&e.title).is_some_and(|n| *n > 1) {
                // A twin that isn't in the menus is one that scripts and agents run.
                let app_menu = match &e.page {
                    Page::Command(id) => app_menu_path(id, false),
                    _ => None,
                };
                let place = e.paths.first().cloned().or(app_menu).unwrap_or_else(|| "for scripts".into());
                e.name = format!("{}  ({place})", e.title);
            }
        }
        v
    })
}

/// Whether `query` is this command's shortcut, spelled as on a Mac or on a PC.
fn is_shortcut_of(e: &Entry, query: &str) -> bool {
    let Page::Command(id) = &e.page else { return false };
    let Some(keys) = crate::menus::shortcut_for(id) else { return false };
    let compact = query.replace(' ', "");
    [keys.to_string(), crate::shortcuts::shortcut_label(keys, false)].iter().any(|k| k.to_lowercase().replace(' ', "") == compact)
}

/// `a` and `b` differ by at most one letter added, dropped, changed or swapped with its
/// neighbour. Short words must match exactly, or "mix" would find "max".
fn one_typo(a: &str, b: &str) -> bool {
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    if a == b {
        return true;
    }
    if a.len().min(b.len()) < 4 || a.len().abs_diff(b.len()) > 1 {
        return false;
    }
    let start = a.iter().zip(&b).take_while(|(x, y)| x == y).count();
    let (ra, rb) = (&a[start..], &b[start..]);
    let tail = |x: &[char], y: &[char]| x == y;
    match ra.len().cmp(&rb.len()) {
        std::cmp::Ordering::Equal => tail(&ra[1..], &rb[1..]) || (ra.len() >= 2 && ra[0] == rb[1] && ra[1] == rb[0] && tail(&ra[2..], &rb[2..])),
        std::cmp::Ordering::Greater => tail(&ra[1..], rb),
        std::cmp::Ordering::Less => tail(ra, &rb[1..]),
    }
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
            let s = if is_shortcut_of(e, &q) {
                // "cmd+s" or "ctrl+s": the command on that very key comes before ones that merely
                // contain it (Cmd+Shift+S…).
                4000
            } else if let Some(at) = title.find(&q) {
                3000 - i32::try_from(at).unwrap_or(999)
            } else if words.iter().all(|w| title.contains(w)) {
                2500
            } else if words.iter().all(|w| e.hay.contains(w)) {
                1000
            } else if let Some(s) = crate::palette::score(&e.title, &q).filter(|s| *s > 450) {
                s
            } else if words.iter().all(|w| title.split_whitespace().any(|t| one_typo(w, t))) {
                // A slip of one letter ("sholo" for Solo) still finds the page.
                400
            } else {
                return None;
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

/// Find the searchable entry for a command, process or guide page.
fn entry_for(page: &Page) -> Option<&'static Entry> {
    entries().iter().find(|e| &e.page == page)
}

/// Look up a Contents category by its catalog id.
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
        Page::Guide(id) => guide(id).map(|g| g.category.as_str()),
    }
}

/// Look up a how-to or keyboard guide by its catalog id.
fn guide(id: &str) -> Option<&'static Guide> {
    data().guides.iter().find(|g| g.id == id)
}

/// Identify guide entries so Contents can list them before commands.
fn is_guide(e: &Entry) -> bool {
    matches!(e.page, Page::Guide(_))
}

/// Pages under a category: its guides first, then the rest, each in title order.
fn pages_in(category: &str) -> impl Iterator<Item = &'static Entry> + '_ {
    let here = move |e: &&Entry| category_of(&e.page) == Some(category);
    entries().iter().filter(here).filter(|e| is_guide(e)).chain(entries().iter().filter(here).filter(|e| !is_guide(e)))
}

/// A guide line cut into text and the parts that name something in the app: `{menu:id}` (the
/// command's menu location), `{key:id}` (its shortcut) and `{keys:Cmd+S}` (a key chord). Shortcuts
/// show the keys of the platform (`Ctrl` for `Cmd` off macOS). The bool marks a named part.
fn spans(line: &str, mac: bool) -> Vec<(String, bool)> {
    let mut out = Vec::new();
    let mut rest = line;
    while let Some(open) = rest.find('{') {
        let Some(close) = rest.get(open..).and_then(|r| r.find('}')).map(|c| c + open) else { break };
        if open > 0 {
            out.push((rest.get(..open).unwrap_or_default().to_string(), false));
        }
        let token = rest.get(open + 1..close).unwrap_or_default();
        out.push((token_text(token, mac).unwrap_or_else(|| token.to_string()), true));
        rest = rest.get(close + 1..).unwrap_or_default();
    }
    if !rest.is_empty() {
        out.push((rest.to_string(), false));
    }
    out
}

/// What a `{kind:value}` token in a guide shows, or None when it names nothing (a test checks).
fn token_text(token: &str, mac: bool) -> Option<String> {
    let (kind, value) = token.split_once(':')?;
    match kind {
        "menu" => menu_paths().get(value).and_then(|p| p.first()).map(|p| pretty_path(p)),
        "key" => crate::menus::shortcut_for(value).map(|k| key_text(k, mac)),
        "keys" => Some(key_text(value, mac)),
        _ => None,
    }
}

/// A shortcut as the platform's keyboard spells it.
fn key_text(keys: &str, mac: bool) -> String {
    let label = crate::shortcuts::shortcut_label(keys, mac);
    if label.is_empty() { format!("{keys} (Mac only)") } else { label }
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
        app.ui.help.typing = false;
        app.ui.help.keyboard_focus = false;
        return;
    }
    if !std::mem::replace(&mut app.ui.help.was_open, true) {
        app.ui.help.reveal = true;
    }
    if data().categories.is_empty() {
        app.ui.help.page = Page::Welcome;
    } else if !page_exists(&app.ui.help.page) {
        app.ui.help = HelpState {
            tab: app.ui.help.tab,
            focus_help: app.ui.help.focus_help,
            focus_search: app.ui.help.focus_search,
            reveal: true,
            was_open: true,
            ..Default::default()
        };
    }
    let raise = app.ui.help.focus_help || app.ui.help.focus_search;
    let mut open = true;
    // Never wider or taller than the app's window, so nothing on a page is cut off at its edge.
    // Below the 24-point menu bar, so the title bar never covers it.
    let mut free = ctx.content_rect();
    free.min.y += 24.0;
    let screen = free.size() - vec2(32.0, 16.0);
    let min = vec2(560.0, 360.0).min(screen);
    let shown = egui::Window::new("SoundCraft Help")
        .open(&mut open)
        .default_size(vec2(900.0, 600.0).min(screen))
        .min_size(min)
        .max_size(screen.max(min))
        .collapsible(false)
        .default_pos(free.min + vec2(16.0, 8.0))
        .constrain_to(free)
        .show(ctx, |ui| window_body(app, ui));
    if let Some(shown) = shown {
        let layer = shown.response.layer_id;
        if raise {
            ctx.move_to_top(layer);
        }
        let focused = ctx.memory(|m| m.focused()).and_then(|id| ctx.read_response(id));
        let has_focus = focused.as_ref().is_some_and(|r| r.layer_id == layer);
        let hovered = shown.response.contains_pointer();
        let had_focus = std::mem::replace(&mut app.ui.help.keyboard_focus, has_focus);
        if app.dialogs.open.is_none() && !app.ui.show_preferences && !app.ui.help.typing && !ctx.text_edit_focused() {
            if has_focus || hovered {
                handle_history_keys(&mut app.ui.help, ctx);
            }
            if (has_focus || had_focus || hovered) && ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape)) {
                open = false;
            }
        }
        if !open
            && has_focus
            && let Some(response) = focused
        {
            response.surrender_focus();
        }
    }
    app.ui.show_help = open;
    app.ui.help.typing = ctx.text_edit_focused();
}

/// Move through Help history without handing the key presses to the editor.
fn handle_history_keys(help: &mut HelpState, ctx: &egui::Context) {
    let (back, forward) = ctx.input_mut(|i| (i.consume_key(Modifiers::ALT, Key::ArrowLeft), i.consume_key(Modifiers::ALT, Key::ArrowRight)));
    if back {
        help.go_back();
    }
    if forward {
        help.go_forward();
    }
}

/// Older preferences can point at a page that the current catalog no longer contains.
fn page_exists(page: &Page) -> bool {
    match page {
        Page::Welcome => true,
        Page::Category(id) => category(id).is_some(),
        _ => entry_for(page).is_some(),
    }
}

/// Lay out the toolbar, fixed-width navigation and scrolling page, then consume the reveal request.
fn window_body(app: &mut SoundApp, ui: &mut Ui) {
    if data().categories.is_empty() {
        app.ui.help.focus_help = false;
        app.ui.help.focus_search = false;
        ui.label("SoundCraft Help could not load its bundled catalog.");
        return;
    }
    // The highlight is the accent blue in both themes: white text on it reads in light mode too.
    ui.visuals_mut().selection.stroke.color = egui::Color32::WHITE;
    toolbar(app, ui);
    ui.separator();
    let height = ui.available_height();
    ui.horizontal_top(|ui| {
        // Exactly the same width on every tab: a long title is cut short rather than widening it.
        let (rect, _) = ui.allocate_exact_size(vec2(NAV_WIDTH, height), egui::Sense::hover());
        ui.scope_builder(egui::UiBuilder::new().max_rect(rect).layout(Layout::top_down(Align::Min)), |ui| {
            ui.set_clip_rect(rect.intersect(ui.clip_rect()));
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

/// Handle history navigation, search edits and one-shot requests to focus the search box.
fn toolbar(app: &mut SoundApp, ui: &mut Ui) {
    let t = Tokens::current();
    ui.horizontal(|ui| {
        let h = &mut app.ui.help;
        if ui
            .add_enabled(h.can_go_back(), egui::Button::new(RichText::new("Back").font(bold(13.0))).min_size(vec2(44.0, 28.0)))
            .on_hover_text("Back (Alt+Left)")
            .clicked()
        {
            h.go_back();
        }
        if ui
            .add_enabled(h.can_go_forward(), egui::Button::new(RichText::new("Forward").font(bold(13.0))).min_size(vec2(64.0, 28.0)))
            .on_hover_text("Forward (Alt+Right)")
            .clicked()
        {
            h.go_forward();
        }
        ui.add_space(8.0);
        let label = ui.label("Search");
        let width = (ui.available_width() - 96.0).clamp(60.0, 320.0);
        let edit = egui::TextEdit::singleline(&mut h.query).hint_text("Search help").desired_width(width).font(regular(13.0));
        let r = ui.add(edit).labelled_by(label.id);
        if std::mem::take(&mut h.focus_search) {
            r.request_focus();
        }
        if r.changed() && !h.query.trim().is_empty() {
            h.tab = Tab::Search;
        }
        if !h.query.is_empty() && ui.small_button("Clear").on_hover_text("Clear the search").clicked() {
            h.query.clear();
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.label(RichText::new(format!("{} pages", entries().len())).small().color(t.text_dim));
        });
    });
}

/// Select the navigation tab and draw its Contents, Index or Search panel.
fn navigation(app: &mut SoundApp, ui: &mut Ui) {
    ui.horizontal(|ui| {
        for (tab, name) in [(Tab::Contents, "Contents"), (Tab::Index, "Index"), (Tab::Search, "Search")] {
            let response = ui.selectable_label(app.ui.help.tab == tab, name);
            if app.ui.help.tab == tab && std::mem::take(&mut app.ui.help.focus_help) {
                response.request_focus();
            }
            if response.clicked() {
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

/// Draw the topic tree, revealing the current page when requested and recording page selections.
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
                    let r = ui.add(egui::Button::selectable(h.page == Page::Category(c.id.clone()), &c.title).truncate());
                    if r.clicked() {
                        go = Some(Page::Category(c.id.clone()));
                    }
                    if h.reveal && h.page == Page::Category(c.id.clone()) {
                        r.scroll_to_me(Some(Align::Center));
                    }
                })
                .body(|ui| {
                    for e in pages_in(&c.id) {
                        let r = ui.selectable_label(h.page == e.page, list_title(e, h.page == e.page));
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

/// List pages alphabetically, filtered by title prefix and grouped by initial letter.
fn index(app: &mut SoundApp, ui: &mut Ui) {
    let t = Tokens::current();
    let h = &mut app.ui.help;
    let label = ui.label("Filter index");
    ui.add(egui::TextEdit::singleline(&mut h.index_filter).hint_text("Type a word to jump to it").desired_width(f32::INFINITY)).labelled_by(label.id);
    let filter = h.index_filter.trim().to_lowercase();
    let mut go = None;
    egui::ScrollArea::vertical().id_salt("help_index").auto_shrink([false, false]).show(ui, |ui| {
        let mut letter = None;
        let titles: Vec<&Entry> = entries().iter().filter(|e| e.title.to_lowercase().starts_with(&filter) || filter.is_empty()).collect();
        for e in &titles {
            let first = e.title.chars().next().map(|c| c.to_ascii_uppercase());
            if first != letter {
                letter = first;
                ui.add_space(4.0);
                ui.label(RichText::new(first.map(String::from).unwrap_or_default()).font(bold(12.0)).color(t.text_dim));
            }
            if ui.selectable_label(h.page == e.page, &e.name).clicked() {
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

/// Show ranked matches and description previews, opening the page selected by the user.
fn search_results(app: &mut SoundApp, ui: &mut Ui) {
    let t = Tokens::current();
    let h = &mut app.ui.help;
    let hits = search(&h.query);
    let mut go = None;
    egui::ScrollArea::vertical().id_salt("help_search").auto_shrink([false, false]).show(ui, |ui| {
        // Leave room for the scroll bar so descriptions wrap before it.
        ui.set_max_width(ui.available_width() - 12.0);
        if h.query.trim().is_empty() {
            let example = if cfg!(target_os = "macos") { "Cmd+S" } else { "Ctrl+S" };
            ui.label(RichText::new(format!("Type in the search box above: a word, a menu name or a shortcut such as {example}.")).color(t.text_dim));
            return;
        }
        if hits.is_empty() {
            ui.label(RichText::new("No pages match. Try fewer or different words.").color(t.text_dim));
        }
        for e in hits {
            if result_row(ui, e, h.page == e.page) {
                go = Some(e.page.clone());
            }
        }
    });
    if let Some(p) = go {
        h.go(p);
    }
}

/// A rounded tag: a menu location or a shortcut. Call it inside `horizontal_wrapped`: a chip that
/// doesn't fit on the current line starts a new one rather than squeezing its text, and one wider
/// than the whole page is cut short with "…" rather than widening the window.
fn chip(ui: &mut Ui, text: RichText) {
    const PAD: Margin = Margin::symmetric(8, 2);
    let t = Tokens::current();
    let extra = f32::from(PAD.left + PAD.right) + 2.0;
    let galley = egui::WidgetText::from(text.clone()).into_galley(ui, Some(egui::TextWrapMode::Extend), f32::INFINITY, egui::TextStyle::Body);
    let width = galley.size().x + extra;
    // In a wrapping row `available_width` is the whole row's; what's left is up to the cursor.
    let left = ui.max_rect().max.x - ui.cursor().min.x;
    if width > left && ui.cursor().min.x > ui.max_rect().min.x + 1.0 {
        ui.end_row();
    }
    let room = (ui.max_rect().max.x - ui.cursor().min.x - extra).max(40.0);
    egui::Frame::new().stroke(Stroke::new(1.0, t.border_light)).corner_radius(10).inner_margin(PAD).show(ui, |ui| {
        ui.set_max_width(room);
        ui.add(egui::Label::new(text).truncate()).on_hover_text(galley.text());
    });
}

/// Dispatch the current page to its renderer without holding a borrow of the navigation state.
fn page(app: &mut SoundApp, ui: &mut Ui) {
    let current = app.ui.help.page.clone();
    match current {
        Page::Welcome => welcome(app, ui),
        Page::Category(id) => category_page(app, ui, &id),
        Page::Command(id) => command_page(app, ui, &id),
        Page::Process(id) => process_page(app, ui, &id),
        Page::Guide(id) => guide_page(app, ui, &id),
    }
}

/// Draw a help page heading with consistent spacing.
fn title(ui: &mut Ui, text: &str) {
    ui.add_space(6.0);
    ui.label(RichText::new(text).font(bold(20.0)));
    ui.add_space(6.0);
}

/// The width of one of the welcome page's two columns: as wide as half the page allows, at most 300.
fn column_width(ui: &Ui) -> f32 {
    ((ui.available_width() - 16.0) / 2.0).clamp(120.0, 300.0)
}

/// Draw guide and topic links in two columns sized to the available page width.
fn welcome(app: &mut SoundApp, ui: &mut Ui) {
    let t = Tokens::current();
    title(ui, "SoundCraft Help");
    ui.label("Every command in SoundCraft has a page here that says what it does, where it is in the menus and its shortcut.");
    ui.add_space(4.0);
    ui.label(RichText::new("Browse by topic in Contents, look a word up in Index, or type what you want to do in the search box.").color(t.text_dim));
    ui.label(
        RichText::new("Point at any SoundCraft window and press Cmd+? to open help on that window's page.".replace("Cmd+?", &help_key(ui)))
            .color(t.text_dim),
    );
    ui.add_space(12.0);
    let mut go = None;
    let col = column_width(ui);
    ui.label(RichText::new("How to").font(bold(15.0)));
    ui.add_space(4.0);
    egui::Grid::new("help_guides").num_columns(2).spacing(vec2(16.0, 6.0)).show(ui, |ui| {
        for (i, g) in data().guides.iter().enumerate() {
            ui.vertical(|ui| {
                ui.set_width(col);
                if ui.link(RichText::new(&g.title).font(bold(13.0))).clicked() {
                    go = Some(Page::Guide(g.id.clone()));
                }
                ui.label(RichText::new(&g.summary).small().color(t.text_dim));
            });
            if i % 2 == 1 {
                ui.end_row();
            }
        }
    });
    ui.add_space(14.0);
    ui.label(RichText::new("Topics").font(bold(15.0)));
    ui.add_space(4.0);
    egui::Grid::new("help_topics").num_columns(2).spacing(vec2(16.0, 10.0)).show(ui, |ui| {
        for (i, c) in data().categories.iter().enumerate() {
            ui.vertical(|ui| {
                ui.set_width(col);
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

/// Draw a topic summary and links to its guides and command pages.
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
        if result_row(ui, e, app.ui.help.page == e.page) {
            go = Some(e.page.clone());
        }
    }
    if let Some(p) = go {
        app.ui.help.go(p);
    }
}

/// The title and preview share a clickable and keyboard-focusable result row.
fn result_row(ui: &mut Ui, entry: &Entry, selected: bool) -> bool {
    let theme = Tokens::current();
    let title = if is_guide(entry) { format!("Guide: {}", entry.name) } else { entry.name.clone() };
    let mut job = egui::text::LayoutJob::default();
    job.append(&title, 0.0, egui::TextFormat { font_id: bold(12.5), color: theme.text, ..Default::default() });
    job.append("\n", 0.0, egui::TextFormat::default());
    job.append(first_sentence(&entry.text), 0.0, egui::TextFormat { font_id: regular(11.5), color: theme.text, ..Default::default() });
    let response = ui.add(egui::Button::selectable(selected, job).wrap());
    if response.clicked() {
        response.request_focus();
    }
    response.clicked()
}

/// Open the parent topic when its breadcrumb link is selected.
fn breadcrumb(app: &mut SoundApp, ui: &mut Ui, category_id: &str) {
    let t = Tokens::current();
    if let Some(c) = category(category_id)
        && ui.link(RichText::new(&c.title).small().color(t.text_dim)).clicked()
    {
        app.ui.help.go(Page::Category(c.id.clone()));
    }
}

/// A description with its Cmd shortcuts spelled for this platform (Cmd is Ctrl off macOS).
fn platform_keys(text: &str, mac: bool) -> String {
    if mac { text.to_string() } else { text.replace("Cmd+", "Ctrl+") }
}

/// Highlight an unfinished feature's limitations alongside its command description.
fn limits_note(ui: &mut Ui, limits: &str) {
    let t = Tokens::current();
    ui.add_space(8.0);
    egui::Frame::new().stroke(Stroke::new(1.0, t.meter_yellow)).corner_radius(8).inner_margin(Margin::same(10)).show(ui, |ui| {
        ui.label(RichText::new("Current limits").font(bold(12.0)).color(t.text));
        ui.label(limits);
    });
}

/// Draw command metadata and limitations, offering the same action and availability as its menu.
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
    let app_menu = app_menu_path(id, mac);
    ui.horizontal_wrapped(|ui| {
        for path in e.paths.iter().chain(app_menu.as_ref()) {
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
    ui.label(RichText::new(platform_keys(&h.text, mac)).font(regular(14.0)));
    if let Some(limits) = &h.limits {
        limits_note(ui, &platform_keys(limits, mac));
    }
    ui.add_space(12.0);
    match e.paths.first().map(|p| p.replace(" › ", " > ")) {
        _ if id == "app.quit" => {}
        Some(path) => {
            if help_action(app, ui, id, &path, "Do it now") {
                crate::menus::invoke_menu(app, id, &path);
            }
        }
        None => {
            if let Some(path) = &app_menu
                && help_action(app, ui, id, path, "Do it now")
            {
                let value = if id.starts_with("window.") { serde_json::json!({"value": true}) } else { serde_json::json!({}) };
                if let Err(error) = app.run(id, value) {
                    app.ui.status = error;
                }
            } else if shortcut.is_empty() && app_menu.is_none() {
                ui.label(RichText::new("Not in the menus. Scripts and agents can run it by its command id.").small().color(t.text_dim));
            }
        }
    }
    ui.add_space(16.0);
    ui.label(RichText::new(format!("Command id: {id}")).font(mono(10.5)).color(t.text_dim));
}

/// A page's name in lists. Guides are in the accent colour so they stand out among the commands,
/// except when selected, where the selection is already drawn in the accent colour.
fn list_title(e: &Entry, selected: bool) -> RichText {
    let text = if is_guide(e) { RichText::new(format!("Guide: {}", e.name)) } else { RichText::new(&e.name) };
    if is_guide(e) && !selected { text.color(Tokens::current().accent) } else { text }
}

/// The help key as this platform spells it.
fn help_key(ui: &Ui) -> String {
    if ui.ctx().os().is_mac() { "Cmd+?".into() } else { "Ctrl+Shift+/".into() }
}

/// One guide line, with the menu locations and keys it names in bold.
fn guide_line(ui: &mut Ui, line: &str, mac: bool) {
    let t = Tokens::current();
    let mut job = egui::text::LayoutJob::default();
    for (text, named) in spans(line, mac) {
        let font = if named { bold(14.0) } else { regular(14.0) };
        job.append(&text, 0.0, egui::TextFormat { font_id: font, color: t.text, ..Default::default() });
    }
    job.wrap.max_width = ui.available_width();
    ui.label(job);
}

/// A key column entry such as `Ctrl+Up / Ctrl+Down`, spelled for this platform.
fn keys_cell(keys: &str, mac: bool) -> String {
    // "Mac keys | PC keys" when the two differ by more than Cmd and Ctrl.
    let keys = match keys.split_once(" | ") {
        Some((on_mac, on_pc)) => {
            if mac {
                on_mac
            } else {
                on_pc
            }
        }
        None => keys,
    };
    keys.split(" / ")
        .map(|k| {
            let label = crate::shortcuts::shortcut_label(k, mac);
            if label.is_empty() { k.to_string() } else { label }
        })
        .collect::<Vec<_>>()
        .join(" / ")
}

/// Draw platform-specific steps or keyboard tables, followed by links to related commands.
fn guide_page(app: &mut SoundApp, ui: &mut Ui, id: &str) {
    let t = Tokens::current();
    let Some(g) = guide(id) else {
        title(ui, "Page not found");
        return;
    };
    let mac = ui.ctx().os().is_mac();
    breadcrumb(app, ui, &g.category);
    title(ui, &g.title);
    ui.label(RichText::new(&g.summary).color(t.text_dim));
    ui.add_space(10.0);
    // A page of keys reads its notes as plain paragraphs; a how-to numbers its steps.
    let numbered = g.keys.is_empty();
    for (n, step) in g.steps.iter().enumerate() {
        ui.horizontal_top(|ui| {
            if numbered {
                ui.add_sized(vec2(22.0, 18.0), egui::Label::new(RichText::new(format!("{}.", n + 1)).font(bold(14.0)).color(t.accent)));
            }
            ui.vertical(|ui| guide_line(ui, step, mac));
        });
        ui.add_space(6.0);
    }
    if !g.keys.is_empty() {
        ui.add_space(4.0);
        // Keys in a column as wide as the widest key (at most 40% of the page), what they do beside
        // them, wrapping within the page.
        let font = mono(12.0);
        let widest = g
            .keys
            .iter()
            .flat_map(|(keys, _)| keys_cell(keys, mac).split(" / ").map(str::to_string).collect::<Vec<_>>())
            .map(|key| ui.fonts_mut(|f| f.layout_no_wrap(key, font.clone(), t.text).size().x))
            .fold(0.0, f32::max);
        let key_width = (widest + 20.0).min(ui.available_width() * 0.4);
        for (keys, what) in &g.keys {
            ui.horizontal_top(|ui| {
                ui.allocate_ui_with_layout(vec2(key_width, 0.0), Layout::left_to_right(Align::Min).with_main_wrap(true), |ui| {
                    ui.set_width(key_width);
                    // Each key of a pair ("Ctrl+Up / Ctrl+Down") gets its own tag, so a pair can wrap.
                    for key in keys_cell(keys, mac).split(" / ") {
                        chip(ui, RichText::new(key).font(font.clone()));
                    }
                });
                ui.add_space(10.0);
                ui.vertical(|ui| {
                    ui.add_space(3.0);
                    ui.label(what);
                });
            });
            ui.add_space(4.0);
        }
    }
    let related: Vec<&Entry> = g.related.iter().filter_map(|c| entry_for(&Page::Command(c.clone()))).collect();
    if !related.is_empty() {
        ui.add_space(14.0);
        ui.label(RichText::new("Related commands").font(bold(13.0)));
        ui.add_space(4.0);
        let mut go = None;
        ui.horizontal_wrapped(|ui| {
            for e in related {
                // A menu location names a command better than its label alone ("Track › New...", not "New").
                let name = e.paths.first().unwrap_or(&e.title);
                if ui.link(name).clicked() {
                    go = Some(e.page.clone());
                }
                ui.add_space(8.0);
            }
        });
        if let Some(p) = go {
            app.ui.help.go(p);
        }
    }
}

/// Floating windows and the help page each opens on, for Cmd+? over the window. The titles are the
/// ones the windows are created with (egui gives a window its id from its title).
pub const WINDOW_PAGES: [(&str, &str); 29] = [
    ("About SoundCraft", "window.about"),
    ("Appearance", "window.ui_customization"),
    ("Automation", "window.automation"),
    (crate::dialogs::ops_windows::BEAT_ANALYSIS_TITLE, "window.beat_detective"),
    ("Big Counter", "window.big_counter"),
    ("Clip Effects", "window.clip_effects"),
    ("Color Palette", "window.color_palette"),
    ("Disk Usage", "window.disk_usage"),
    ("I/O Setup", "window.io_setup"),
    ("Keyboard Shortcuts", "window.shortcuts"),
    ("MIDI Event List", "window.event_list"),
    ("MIDI Keyboard", "window.midi_keyboard"),
    ("MIDI Operations", "window.midi_ops"),
    ("MIDI Real-Time Properties", "window.rtp"),
    ("Metadata Inspector", "window.metadata"),
    ("Playback Engine", "window.playback_engine"),
    ("Renderer", "window.renderer"),
    ("Score Editor", "window.score"),
    ("Session Audio Health", "window.audio_health"),
    ("Session Info", "window.session_info"),
    ("System Usage", "window.system_usage"),
    ("Task Manager", "window.task_manager"),
    ("Tempo Operations", "window.tempo_ops"),
    ("Time Operations", "window.time_ops"),
    ("Transport", "window.transport"),
    ("Video", "window.video"),
    ("Video Universe", "window.video_universe"),
    ("Window Configurations", "window.configurations"),
    ("Workspace", "window.workspace"),
];

/// The id egui gives a window titled `title`: `Window::new` names its area after the title's text,
/// which egui holds as an `Option`. A test opens every window in [`WINDOW_PAGES`] to check this.
pub fn window_id(title: &str) -> egui::Id {
    egui::Id::unique(Some(title))
}

/// The help page for what's under the pointer: a floating window's page, the open AudioSuite
/// process, or the Edit or Mix window's page over the main area. None over help itself, a menu or
/// a window help has no page for.
fn page_under_pointer(app: &SoundApp, ctx: &egui::Context) -> Option<Page> {
    let pos = ctx.input(|i| i.pointer.latest_pos())?;
    let layer = ctx.layer_id_at(pos)?;
    // Backends without native child windows embed Mix using the viewport id as its window id.
    if app.ui.mix_separate && layer.id == egui::Id::unique(crate::mix_window::viewport_id()) {
        return Some(Page::Command("window.mix".into()));
    }
    if let Some((_, id)) = WINDOW_PAGES.iter().find(|(title, _)| layer.id == window_id(title)) {
        return Some(Page::Command((*id).to_string()));
    }
    if let Some(process) = &app.ui.audiosuite
        && layer.order == egui::Order::Middle
        && data().audiosuite.iter().any(|p| &p.process == process)
        && layer.id == window_id(&crate::dialogs::audiosuite::window_title(process))
    {
        return Some(Page::Process(process.clone()));
    }
    if layer.order == egui::Order::Background {
        // Match SoundApp::ui: a separate Mix viewport leaves Edit in the main window.
        let id = if !app.ui.mix_separate && app.ui.window == crate::MainWindow::Mix { "window.mix" } else { "window.edit" };
        return Some(Page::Command(id.into()));
    }
    None
}

/// Cmd+?: open help, on the page for the window under the pointer when there is one.
pub fn open_here(app: &mut SoundApp, ctx: &egui::Context) {
    let page = page_under_pointer(app, ctx);
    let _ = app.run("help.open", serde_json::json!({}));
    if let Some(p) = page.filter(|p| entry_for(p).is_some()) {
        app.ui.help.go(p);
    }
}

/// Render a menu action with its availability and a visible explanation.
fn help_action(app: &SoundApp, ui: &mut Ui, id: &str, path: &str, label: &str) -> bool {
    let enabled = !app.ui.show_preferences && app.dialogs.open.is_none() && crate::menus::item_state(app, path, Some(id)).0;
    let button = egui::Button::new(RichText::new(label).font(bold(13.0))).min_size(vec2(110.0, 28.0));
    let clicked = ui.add_enabled(enabled, button).clicked();
    let explanation = if enabled {
        format!("Same as choosing {} from the menu.", pretty_path(path))
    } else {
        let reason = why_disabled(app, id).unwrap_or_else(|| "this action is unavailable.".into());
        format!("Not available right now: {reason}")
    };
    ui.label(RichText::new(explanation).small().color(Tokens::current().text_dim));
    clicked
}

/// Why an engine command can't run now, as the command itself explains it.
fn why_disabled(app: &SoundApp, id: &str) -> Option<String> {
    if app.ui.show_preferences {
        return Some("close Preferences first.".into());
    }
    if app.dialogs.open.is_some() {
        return Some("close the active dialog first.".into());
    }
    if id == "setup.preferences" && (app.is_playing() || app.engine.transport.recording) {
        return Some("stop the transport first.".into());
    }
    let c = soundcraft_engine::find_command(id)?;
    if app.engine.blocked_while_recording(id) {
        return Some("stop recording first.".into());
    }
    (c.enabled)(&app.engine).err()
}

/// Describe an AudioSuite process and offer to open it through its existing menu action.
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
        && help_action(app, ui, "audiosuite.process", path, "Open it")
    {
        crate::menus::invoke_menu(app, "audiosuite.process", path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    /// Every action route keeps an active modal in control and explains why the button is disabled.
    #[test]
    fn help_action_buttons_follow_modal_availability() {
        use egui_kittest::kittest::Queryable;

        let mut app = SoundApp::new(soundcraft_engine::Engine::default(), None, crate::Services::default());
        app.ui.show_preferences = true;
        let mut harness = egui_kittest::Harness::builder()
            .with_size(vec2(760.0, 700.0))
            .with_context_setup(|ctx| crate::fonts::install(ctx, false))
            .build_ui_state(|ui, app| page(app, ui), app);
        for (page, button) in [
            (Page::Command("session.new".into()), "Do it now"),
            (Page::Command("window.about".into()), "Do it now"),
            (Page::Process("gain".into()), "Open it"),
        ] {
            harness.state_mut().ui.help.page = page;
            harness.run_steps(2);
            harness.get_by_label("Not available right now: close Preferences first.");
            harness.get_by_label(button).click();
            harness.run_steps(2);
            let app = harness.state();
            assert!(app.dialogs.open.is_none());
            assert!(!app.ui.show_about);
            assert!(app.ui.audiosuite.is_none());
        }
        harness.state_mut().ui.show_preferences = false;
        harness.run_steps(2);
        harness.get_by_label("Open it").click();
        harness.run_steps(2);
        assert_eq!(harness.state().ui.audiosuite.as_deref(), Some("gain"));
        harness.state_mut().ui.help.page = Page::Command("window.about".into());
        harness.run_steps(2);
        harness.get_by_label("Do it now").click();
        harness.run_steps(2);
        assert!(harness.state().ui.show_about);

        for command in ["app.quit", "session.inspect"] {
            harness.state_mut().ui.help.page = Page::Command(command.into());
            harness.run_steps(2);
            assert!(harness.query_by_label("Do it now").is_none(), "{command}");
        }
    }

    /// A page must draw the expected heading rather than an empty or unrelated body.
    #[test]
    fn every_catalog_page_draws_its_title() {
        use egui_kittest::kittest::Queryable;

        let app = SoundApp::new(soundcraft_engine::Engine::default(), None, crate::Services::default());
        let mut harness = egui_kittest::Harness::builder()
            .with_size(vec2(760.0, 700.0))
            .with_context_setup(|ctx| crate::fonts::install(ctx, false))
            .build_ui_state(|ui, app| page(app, ui), app);
        for page in pages() {
            let expected = match &page {
                Page::Welcome => "SoundCraft Help",
                Page::Category(id) => &category(id).unwrap().title,
                _ => &entry_for(&page).unwrap().title,
            };
            harness.state_mut().ui.help.page = page.clone();
            harness.step();
            assert!(harness.query_all_by_label(expected).next().is_some(), "{page:?} did not draw {expected}");
        }
    }

    /// Validate nonempty help text and unique categories before accepting the bundled catalog.
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
    /// Keep help coverage in sync with both engine and UI command registries.
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

    /// Require catalog entries for every AudioSuite process exposed by the menus.
    #[test]
    fn every_audiosuite_process_has_a_help_page() {
        let d = parse().unwrap();
        let processes: HashSet<&str> = crate::menus::AUDIOSUITE.iter().map(|(_, p)| *p).collect();
        let described: HashSet<&str> = d.audiosuite.iter().map(|p| p.process.as_str()).collect();
        assert_eq!(processes, described);
    }

    /// Exercise title, content, shortcut and approximate matches against the real help catalog.
    #[test]
    fn search_finds_titles_words_shortcuts_and_loose_spellings() {
        let first = |q: &str| search(q).first().map(|e| e.page.clone());
        assert_eq!(first("bounce mix"), Some(Page::Command("file.bounce_mix".into())));
        assert_eq!(first("bnc mix"), Some(Page::Command("file.bounce_mix".into())));
        assert!(search("reverb").iter().any(|e| e.page == Page::Process("room_reverb".into())));
        let save = crate::menus::shortcut_for("session.save").unwrap().to_lowercase();
        assert!(search(&save).iter().any(|e| e.page == Page::Command("session.save".into())));
        assert!(search("ctrl+s").iter().any(|e| e.page == Page::Command("session.save".into())), "the PC spelling finds it too");
        assert!(search("ctrl+shift+i").iter().any(|e| e.page == Page::Command("file.import_audio".into())));
        assert!(search("zzqqxx").is_empty());
        // The command on exactly that key comes first, before Cmd+Shift+S and friends.
        assert_eq!(first(&save), Some(Page::Command("session.save".into())));
        assert_eq!(first("ctrl+s"), Some(Page::Command("session.save".into())));
        assert_eq!(first("Ctrl + S"), Some(Page::Command("session.save".into())));
        // One slipped letter still finds the page; short words don't match loosely.
        assert!(search("sholo").iter().any(|e| e.title == "Solo"), "{:?}", search("sholo").iter().map(|e| &e.title).collect::<Vec<_>>());
        assert!(search("bounse").iter().any(|e| e.page == Page::Command("file.bounce_mix".into())));
        assert!(
            one_typo("solo", "sloo")
                && one_typo("solo", "soloo")
                && !one_typo("solo", "slo")
                && one_typo("loop", "lopo")
                && !one_typo("mix", "max")
                && !one_typo("solo", "sold out")
        );
        assert!(search("   ").is_empty());
    }

    /// Check that registered menu commands retain discoverable locations in help.
    #[test]
    fn commands_in_the_menus_list_where_they_are() {
        let e = entry_for(&Page::Command("session.save".into())).unwrap();
        assert!(e.paths.iter().any(|p| p.starts_with("File › ")), "{:?}", e.paths);
        let e = entry_for(&Page::Command("help.open".into())).unwrap();
        assert_eq!(e.paths, ["Help › SoundCraft Help"]);
    }

    /// Keep common abbreviations intact when shortening search result previews.
    #[test]
    fn first_sentence_skips_abbreviations() {
        assert_eq!(first_sentence("Adds a send (e.g. to a reverb). Then more."), "Adds a send (e.g. to a reverb).");
        assert_eq!(first_sentence("One. Two."), "One.");
        assert_eq!(first_sentence("No full stop"), "No full stop");
        assert_eq!(first_sentence("Ends with e.g. "), "Ends with e.g. ");
    }

    /// Check navigation order, forward-history truncation and the history size limit.
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

    /// Check which help navigation fields survive preference serialization.
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

    /// Every character help can show has a glyph in the fonts SoundCraft bundles, so no page draws
    /// a box on a system without extra fonts.
    #[test]
    fn bundled_fonts_draw_every_character_in_help() {
        let ctx = egui::Context::default();
        ctx.set_fonts(crate::fonts::definitions(false));
        ctx.run_ui(egui::RawInput::default(), |_| {}).textures_delta.clear();
        let mut text: String = DATA.chars().chain(GUIDES.chars()).chain(include_str!("help.rs").chars()).collect();
        for e in entries() {
            text.push_str(&e.title);
            text.push_str(&e.text);
            text.extend(e.paths.iter().map(String::as_str));
        }
        let mut chars: Vec<char> = text.chars().filter(|c| !c.is_ascii()).collect();
        chars.sort_unstable();
        chars.dedup();
        assert!(!chars.is_empty());
        for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace, egui::FontFamily::Name("bold".into())] {
            let font = egui::FontId::new(13.0, family.clone());
            for c in &chars {
                assert!(ctx.fonts_mut(|f| f.has_glyphs(&font, &c.to_string())), "{family:?} has no glyph for {c:?}");
            }
        }
    }

    /// Resolve guide references on both platforms so stale identifiers cannot ship.
    #[test]
    fn every_guide_names_real_menus_keys_and_commands() {
        let d = parse().unwrap();
        assert!(d.guides.len() >= 10, "{} guides", d.guides.len());
        let mut ids = HashSet::new();
        for g in &d.guides {
            assert!(ids.insert(g.id.as_str()), "{}: id used twice", g.id);
            assert!(category(&g.category).is_some(), "{}: unknown category {}", g.id, g.category);
            assert!(!g.title.trim().is_empty() && !g.summary.trim().is_empty(), "{}: needs a title and summary", g.id);
            assert!(!g.steps.is_empty() || !g.keys.is_empty(), "{}: empty", g.id);
            for c in &g.related {
                assert!(d.commands.contains_key(c), "{}: related `{c}` has no page", g.id);
            }
            for line in g.steps.iter().chain(g.keys.iter().flat_map(|(k, what)| [k, what])) {
                assert_eq!(line.matches('{').count(), line.matches('}').count(), "{}: unbalanced braces in {line}", g.id);
                for mac in [true, false] {
                    for (text, named) in spans(line, mac) {
                        assert!(!text.contains('{') && !text.contains('}'), "{}: {line}", g.id);
                        if named {
                            assert!(!text.contains(':') || text.contains('›'), "{}: `{text}` names nothing in {line}", g.id);
                        }
                    }
                }
                let mut rest = line.as_str();
                while let Some(open) = rest.find('{') {
                    let close = rest[open..].find('}').unwrap() + open;
                    let token = &rest[open + 1..close];
                    for mac in [true, false] {
                        assert!(token_text(token, mac).is_some(), "{}: `{{{token}}}` names nothing", g.id);
                    }
                    rest = &rest[close + 1..];
                }
            }
        }
    }

    /// Check platform-specific rendering of guide shortcut tokens.
    #[test]
    fn guides_show_the_keys_of_each_platform() {
        let line = "Save with {menu:session.save} ({key:session.save}), record with {keys:Cmd+Space}.";
        let text = |mac| spans(line, mac).into_iter().map(|(t, _)| t).collect::<String>();
        assert_eq!(text(true), "Save with File › Save Session (Cmd+S), record with Cmd+Space.");
        assert_eq!(text(false), "Save with File › Save Session (Ctrl+S), record with Ctrl+Space.");
        assert_eq!(spans(line, true).iter().filter(|(_, named)| *named).count(), 3);
        // A Mac Control chord says so off the Mac; a Ctrl chord that works everywhere doesn't.
        assert_eq!(key_text("Cmd+Ctrl+S", false), "Cmd+Ctrl+S (Mac only)");
        assert_eq!(key_text("Ctrl+Up", false), "Ctrl+Up");
        assert_eq!(keys_cell("Ctrl+Up / Ctrl+Down", false), "Ctrl+Up / Ctrl+Down");
        assert_eq!(keys_cell("Z / Shift+Z", false), "Z / Shift+Z");
        assert_eq!(keys_cell("Cmd+Space", false), "Ctrl+Space");
        assert_eq!(keys_cell("Alt+Tab | Ctrl+Tab", true), "Alt+Tab");
        assert_eq!(keys_cell("Alt+Tab | Ctrl+Tab", false), "Ctrl+Tab");
    }

    /// Check guide key tables and prose for incorrect substitutions off macOS.
    #[test]
    fn guides_read_right_off_the_mac() {
        // Cmd becomes Ctrl off the Mac, so a guide must not also name Ctrl beside it, and a PC
        // must never be told to press Alt+Tab, which the desktop takes.
        for g in &data().guides {
            for mac in [true, false] {
                for line in g.steps.iter().chain(g.keys.iter().map(|(_, what)| what)) {
                    let text: String = spans(line, mac).into_iter().map(|(t, _)| t).collect();
                    for twice in ["Ctrl, Ctrl", "Ctrl or Ctrl", "Ctrl and Ctrl", "Cmd, Cmd"] {
                        assert!(!text.contains(twice), "{} ({}): {text}", g.id, if mac { "Mac" } else { "PC" });
                    }
                }
                for (keys, _) in &g.keys {
                    assert!(mac || !keys_cell(keys, mac).contains("Alt+Tab"), "{}: {keys}", g.id);
                }
            }
        }
    }

    /// Check guide discovery through both search and the topic listing.
    #[test]
    fn guides_are_found_by_search_and_listed_first_in_their_topic() {
        assert_eq!(search("record audio").first().map(|e| &e.page), Some(&Page::Guide("record_audio".into())));
        assert!(search("ctrl+shift+n").iter().any(|e| e.page == Page::Guide("first_session".into())), "PC keys are searchable");
        assert!(search("cmd+shift+n").iter().any(|e| e.page == Page::Guide("first_session".into())), "Mac keys are searchable");
        let first = pages_in("recording").next().map(|e| e.page.clone());
        assert_eq!(first, Some(Page::Guide("record_audio".into())));
        assert!(pages().contains(&Page::Guide("keys_commands_focus".into())));
        assert_eq!(category_of(&Page::Guide("keys_transport".into())), Some("transport"));
    }

    /// Require every mapped floating window to point to an existing help page.
    #[test]
    fn every_window_help_knows_has_a_page() {
        let d = parse().unwrap();
        let mut titles = HashSet::new();
        for (title, id) in WINDOW_PAGES {
            assert!(titles.insert(title), "{title} listed twice");
            assert!(d.commands.contains_key(id), "{title}: {id} has no page");
        }
    }

    /// Check that duplicate page titles include enough location information to distinguish them.
    #[test]
    fn pages_with_the_same_title_are_told_apart_in_lists() {
        let names: Vec<&str> = entries().iter().map(|e| e.name.as_str()).collect();
        let unique: HashSet<&str> = names.iter().copied().collect();
        let mut seen = HashSet::new();
        let twice: Vec<&str> = names.iter().copied().filter(|n| !seen.insert(*n)).collect();
        assert!(twice.is_empty(), "pages that share a name in Contents and Index: {twice:?}");
        assert_eq!(names.len(), unique.len());
        let shortcuts: Vec<&str> = entries().iter().filter(|e| e.title == "Keyboard Shortcuts").map(|e| e.name.as_str()).collect();
        assert_eq!(shortcuts.len(), 2);
        assert!(shortcuts.contains(&"Keyboard Shortcuts  (for scripts)"), "{shortcuts:?}");
    }

    /// Check platform substitutions in command descriptions and limitation notes.
    #[test]
    fn descriptions_show_ctrl_off_the_mac() {
        assert_eq!(platform_keys("Cmd+= switches between Mix and Edit", false), "Ctrl+= switches between Mix and Edit");
        assert_eq!(platform_keys("Cmd+= switches between Mix and Edit", true), "Cmd+= switches between Mix and Edit");
        // No description names a Mac-only Cmd+Ctrl chord, which would read wrongly as Ctrl+Ctrl.
        for (id, h) in &data().commands {
            for text in std::iter::once(&h.text).chain(h.limits.iter()) {
                assert!(!text.contains("Cmd+Ctrl"), "{id}: {text}");
            }
        }
    }

    /// Check platform-specific application-menu locations, including items absent on macOS.
    #[test]
    fn the_application_menu_items_say_where_they_are() {
        for (_, _, id) in APP_MENU {
            assert!(data().commands.contains_key(id), "{id} has no page");
        }
        assert_eq!(app_menu_path("window.about", false).as_deref(), Some("SoundCraft › About SoundCraft"));
        assert_eq!(app_menu_path("app.quit", true).as_deref(), Some("SoundCraft › Quit SoundCraft"));
        assert_eq!(app_menu_path("app.checkForUpdates", true), None, "the Mac menu has no Check for Updates");
        assert!(search("session audio health").iter().any(|e| e.page == Page::Command("window.audio_health".into())));
    }
}
