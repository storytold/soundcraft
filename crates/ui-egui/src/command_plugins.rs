//! The Command Plug-ins menu and its windows: the installed command plug-ins' commands, the
//! Command Plug-ins window (installed plug-ins, installing from a file, a GitHub repository or a
//! catalog, updates), the install confirmation (permissions, kind, source, SHA-256) and the
//! parameter dialog generated from a plug-in command's `params`. Everything goes through the
//! engine's `plugin.*` commands ([`soundcraft_engine::command_plugin`], RFC-0046). Downloads run
//! on a worker thread under `catch_unwind`, so the interface keeps drawing.
//!
//! These are WebAssembly command plug-ins, which run commands on the session; audio plug-ins
//! (CLAP, VST3, AU) are inserts in the Mix window and are not listed here. Plug-in names and
//! command labels are the plug-in's own and are not translated. The menu is built at run time
//! from what is installed, beside the catalog's menus; its fixed strings are English until the
//! pt-BR catalog gets entries for them.

use std::sync::mpsc::{Receiver, TryRecvError, channel};

use egui::RichText;
use serde_json::{Map, Value, json};
use soundcraft_engine::command_plugin::craft_plugin::{Access, Param, ParamKind, PluginCommand};
use soundcraft_engine::command_plugin::{self, Candidate, InstalledPlugin};

use crate::SoundApp;
use crate::theme::Tokens;

/// The UI command that shows the Command Plug-ins window.
pub const WINDOW: &str = "window.command_plugins";
/// The menu that lists the command plug-ins (built at run time, from what is installed).
pub const MENU: &str = "Command Plug-ins";
/// Width of the repository and catalog address fields (points).
const ADDRESS_WIDTH: f32 = 260.0;
/// Width of a text parameter's field (points).
const TEXT_FIELD_WIDTH: f32 = 240.0;

/// The install confirmation's content: the `plugin.install` parameters that named the bundle, and
/// what its review said about it.
#[derive(Clone, Debug)]
pub struct Review {
    pub request: Value,
    pub info: Value,
}

impl Review {
    /// Where the plug-in comes from, in a line.
    pub fn source_text(&self) -> String {
        let source = &self.info["source"];
        let t = |k: &str| source[k].as_str().unwrap_or_default();
        match t("type") {
            "github" => format!("github.com/{} {} ({})", t("repo"), t("tag"), t("asset")),
            "catalog" => format!("{} · {}", t("package"), t("catalog")),
            "other" => format!("{} · {}", t("kind"), t("location")),
            _ => t("path").to_string(),
        }
    }
}

/// A plug-in command waiting for its parameters.
#[derive(Clone, Debug)]
pub struct ParamsDialog {
    pub plugin: String,
    pub command: String,
    pub title: String,
    pub params: Vec<Param>,
    pub values: Map<String, Value>,
}

impl ParamsDialog {
    /// The dialog for `command` of plug-in `plugin` (named `name`), with each parameter's default.
    pub fn new(plugin: &str, name: &str, command: &PluginCommand) -> Self {
        ParamsDialog {
            plugin: plugin.to_string(),
            command: command.id.clone(),
            title: format!("{name}: {}", command.label.trim_end_matches(['…', '.'])),
            params: command.params.clone(),
            values: command.params.iter().map(|p| (p.name.clone(), p.default_value())).collect(),
        }
    }

    /// The `plugin.run` parameters with the values entered.
    pub fn run_params(&self) -> Value {
        json!({"id": self.plugin, "command": self.command, "params": self.values})
    }
}

/// What a download worker brings back.
enum Done {
    /// The bundle `request` named, or why it couldn't be had.
    Downloaded(Value, Result<Candidate, String>),
    /// The catalogs' packages and their warnings.
    Packages(Vec<Value>, Vec<String>),
    /// The updates found, and the plug-ins that couldn't be checked.
    Updates(Vec<Value>, Vec<String>),
    /// The worker stopped with an internal error.
    Failed(String),
}

/// The Command Plug-ins window's state, the install confirmation, the parameter dialog and the
/// download in progress.
#[derive(Default)]
pub struct CommandPluginsUi {
    /// The Command Plug-ins window shows.
    pub open: bool,
    /// The GitHub repository typed (`owner/repo` or `owner/repo@tag`).
    repo: String,
    /// The catalog address typed.
    catalog_url: String,
    /// The last action's message.
    pub status: String,
    /// The catalogs' packages, after Browse Catalogs.
    packages: Vec<Value>,
    /// The updates found, after Check for Updates.
    updates: Vec<Value>,
    /// The install confirmation, while it waits for an answer.
    pub review: Option<Review>,
    /// A plug-in command's parameters, while they are entered.
    pub params: Option<ParamsDialog>,
    /// The download running on a worker thread.
    task: Option<Receiver<Done>>,
}

impl CommandPluginsUi {
    /// Whether a download runs (the network buttons wait for it).
    pub fn busy(&self) -> bool {
        self.task.is_some()
    }
}

impl SoundApp {
    /// For the desktop host, before the first frame: the command plug-ins' settings and catalogs
    /// from the saved preferences, the user's `Plug-ins` folder in `settings_dir` (none: kept for
    /// the session) and the HTTPS client downloads use.
    pub fn start_command_plugins(&mut self, settings_dir: Option<&std::path::Path>) {
        let plugins = &mut self.engine.command_plugins;
        plugins.set_prefs(self.ui.command_plugins.clone());
        #[cfg(not(target_arch = "wasm32"))]
        {
            if let Some(dir) = settings_dir {
                plugins.open_store(dir);
            }
            plugins.set_fetch(crate::plugin_http::http_fetch());
        }
        #[cfg(target_arch = "wasm32")]
        let _ = settings_dir;
    }

    /// For the desktop host: whether the command plug-ins' settings or catalogs changed since the
    /// last call; they are then copied into [`crate::UiState`] for saving.
    pub fn take_command_plugin_prefs_changed(&mut self) -> bool {
        if !self.engine.command_plugins.take_prefs_changed() {
            return false;
        }
        self.ui.command_plugins = self.engine.command_plugins.prefs.clone();
        true
    }
}

// ---------- menu and commands ----------

/// The Command Plug-ins menu: each installed plug-in's commands (a submenu for a plug-in with
/// several), then Manage Command Plug-ins….
pub fn menu(app: &mut SoundApp, ui: &mut egui::Ui, enabled: bool) {
    ui.menu_button(MENU, |ui| {
        ui.add_enabled_ui(enabled, |ui| {
            let plugins: Vec<InstalledPlugin> = app.engine.command_plugins.list().to_vec();
            for p in &plugins {
                let m = &p.manifest;
                match m.commands.as_slice() {
                    [] => {}
                    [one] => {
                        if ui.button(m.name.as_str()).clicked() {
                            menu_run(app, &m.id, &one.id);
                            ui.close();
                        }
                    }
                    many => {
                        ui.menu_button(m.name.as_str(), |ui| {
                            for c in many {
                                if ui.button(c.label.as_str()).clicked() {
                                    menu_run(app, &m.id, &c.id);
                                    ui.close();
                                }
                            }
                        });
                    }
                }
            }
            if !plugins.is_empty() {
                ui.separator();
            }
            if ui.button("Manage Command Plug-ins…").clicked() {
                app.command_plugins.open = true;
                ui.close();
            }
        });
    });
}

/// A plug-in command picked from the menu; its outcome is on the status line.
fn menu_run(app: &mut SoundApp, id: &str, command: &str) {
    if let Err(e) = run_command(app, id, command, None) {
        app.ui.status = e;
    }
}

/// Run a plug-in command, after its parameter dialog when it declares parameters and none are
/// `given`.
pub fn run_command(app: &mut SoundApp, id: &str, command: &str, given: Option<Value>) -> Result<Value, String> {
    let p = app.engine.command_plugins.get(id).ok_or_else(|| format!("no command plug-in `{id}` is installed"))?;
    let c = p.manifest.command(Some(command)).ok_or_else(|| format!("{} has no command `{command}`", p.manifest.name))?;
    let run = json!({"id": p.manifest.id, "command": c.id, "params": given.clone().unwrap_or_else(|| json!({}))});
    if c.params.is_empty() || given.is_some() {
        return run_plugin(app, run);
    }
    app.command_plugins.params = Some(ParamsDialog::new(&p.manifest.id, &p.manifest.name, c));
    Ok(json!({"dialog": "commandPluginParams"}))
}

/// `plugin.run`, its notes (or that it is done) on the status line.
fn run_plugin(app: &mut SoundApp, params: Value) -> Result<Value, String> {
    let name = params["id"].as_str().and_then(|id| app.engine.command_plugins.get(id)).map(|p| p.manifest.name.clone()).unwrap_or_default();
    let r = app.run("plugin.run", params);
    if let Ok(out) = &r {
        let notes: Vec<&str> = out["notes"].as_array().map(|a| a.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
        let mut line = if notes.is_empty() { format!("{name}: done") } else { notes.join(" · ") };
        if let Some(panel) = out["panel"].as_str() {
            line = format!("{line} ({panel})");
        }
        app.ui.status = line;
    }
    // an error is on the status line already (`SoundApp::run`)
    r
}

/// The UI commands of this module: `window.command_plugins` shows (or with `value`, hides) the
/// Command Plug-ins window.
pub fn route(app: &mut SoundApp, id: &str, params: &Value) -> Option<Result<Value, String>> {
    if id != WINDOW {
        return None;
    }
    app.command_plugins.open = params.get("value").and_then(Value::as_bool).unwrap_or(!app.command_plugins.open);
    Some(Ok(json!({"value": app.command_plugins.open})))
}

// ---------- reviewing and installing ----------

/// Check the bundle `request` names and show its confirmation: a file or data at once, a
/// repository or a catalog's package after it downloads on a worker.
fn review(app: &mut SoundApp, ctx: &egui::Context, request: Value) {
    if request.get("path").is_some() || request.get("data").is_some() {
        review_local(app, request);
        return;
    }
    let fetcher = app.engine.command_plugins.fetcher();
    let catalogs = app.engine.command_plugins.prefs.catalogs.clone();
    start(app, ctx, move || {
        let got = fetcher.with(|f| command_plugin::download(f, &catalogs, &request));
        Done::Downloaded(request, got)
    });
}

/// [`review`] of a file or base64 data (`plugin.install {…, review: true}`).
fn review_local(app: &mut SoundApp, request: Value) {
    let mut asked = request.clone();
    asked["review"] = json!(true);
    match app.engine.execute("plugin.install", &asked) {
        Ok(info) => {
            app.command_plugins.review = Some(Review { request, info });
            app.command_plugins.status.clear();
        }
        Err(e) => app.command_plugins.status = crate::localization::error_text(&e.to_string()),
    }
}

/// Install the reviewed bundle (the one with the reviewed SHA-256 only, which the review kept).
fn install_reviewed(app: &mut SoundApp, reviewed: Review) {
    let mut request = reviewed.request;
    request["sha256"] = reviewed.info["sha256"].clone();
    app.command_plugins.status = match app.engine.execute("plugin.install", &request) {
        Ok(_) => format!("Installed {}.", reviewed.info["name"].as_str().unwrap_or_default()),
        Err(e) => crate::localization::error_text(&e.to_string()),
    };
}

/// The status line after a `plugin.*` management command: its error, or nothing.
fn outcome(r: Result<Value, soundcraft_engine::EngineError>) -> String {
    r.err().map(|e| crate::localization::error_text(&e.to_string())).unwrap_or_default()
}

/// Browse Catalogs, on a worker.
fn browse(app: &mut SoundApp, ctx: &egui::Context) {
    let fetcher = app.engine.command_plugins.fetcher();
    let urls = app.engine.command_plugins.prefs.catalogs.clone();
    let installed: Vec<InstalledPlugin> = app.engine.command_plugins.list().to_vec();
    start(app, ctx, move || {
        let (mut packages, mut warnings) = (Vec::new(), Vec::new());
        for url in &urls {
            match fetcher.with(|f| command_plugin::catalog_packages_with(f, url, &installed)) {
                Ok((found, w)) => {
                    packages.extend(found);
                    warnings.extend(w.into_iter().map(|w| format!("{url}: {w}")));
                }
                Err(e) => warnings.push(format!("{url}: {e}")),
            }
        }
        Done::Packages(packages, warnings)
    });
}

/// Check for Updates, on a worker: what `plugin.update` reports, without blocking the window.
fn check_updates(app: &mut SoundApp, ctx: &egui::Context) {
    let fetcher = app.engine.command_plugins.fetcher();
    let installed: Vec<InstalledPlugin> = app.engine.command_plugins.list().to_vec();
    start(app, ctx, move || {
        let (mut updates, mut errors) = (Vec::new(), Vec::new());
        for p in &installed {
            match fetcher.with(|f| command_plugin::update_for_with(f, p)) {
                Ok(Some((install, available))) => {
                    updates.push(json!({"id": p.manifest.id, "installed": p.manifest.version, "available": available, "install": install}))
                }
                Ok(None) => {}
                Err(e) => errors.push(format!("{}: {e}", p.manifest.name)),
            }
        }
        Done::Updates(updates, errors)
    });
}

/// Run `work` on a worker thread under `catch_unwind` (at once on the web, which has none); its
/// result is applied on a later frame ([`poll`]).
fn start(app: &mut SoundApp, ctx: &egui::Context, work: impl FnOnce() -> Done + Send + 'static) {
    if app.command_plugins.task.is_some() {
        app.command_plugins.status = "A command plug-in download is already running".into();
        return;
    }
    let (tx, rx) = channel();
    let guarded = move || std::panic::catch_unwind(std::panic::AssertUnwindSafe(work)).unwrap_or(Done::Failed("internal error (see the log)".into()));
    #[cfg(not(target_arch = "wasm32"))]
    {
        let ctx = ctx.clone();
        let spawned = std::thread::Builder::new().name("command-plugin-download".into()).spawn(move || {
            // the receiver may be gone (the app closed): nothing to tell
            let _ = tx.send(guarded());
            ctx.request_repaint();
        });
        if let Err(e) = spawned {
            app.command_plugins.status = e.to_string();
            return;
        }
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = ctx;
        // the receiver is alive (held below): sending can't fail
        let _ = tx.send(guarded());
    }
    app.command_plugins.task = Some(rx);
    app.command_plugins.status = "Downloading…".into();
}

/// Apply what a finished worker brought back.
fn poll(app: &mut SoundApp) {
    let Some(rx) = &app.command_plugins.task else { return };
    let done = match rx.try_recv() {
        Ok(done) => done,
        Err(TryRecvError::Empty) => return,
        Err(TryRecvError::Disconnected) => Done::Failed("the download stopped".into()),
    };
    let ui = &mut app.command_plugins;
    ui.task = None;
    ui.status.clear();
    match done {
        Done::Downloaded(request, Ok(c)) => match command_plugin::offer(&mut app.engine, &request, c) {
            Ok(info) => ui.review = Some(Review { request, info }),
            Err(e) => ui.status = crate::localization::error_text(&e.to_string()),
        },
        Done::Downloaded(_, Err(e)) | Done::Failed(e) => ui.status = e,
        Done::Packages(packages, warnings) => {
            ui.packages = packages;
            ui.status = warnings.join("\n");
        }
        Done::Updates(updates, errors) => {
            ui.status = if updates.is_empty() && errors.is_empty() { "Every command plug-in is up to date.".into() } else { errors.join("\n") };
            ui.updates = updates;
        }
    }
}

/// From File…: the `.craftplugin` picker, then the file's confirmation.
fn pick_file(app: &mut SoundApp) {
    let Some(pick) = app.services.pick_open.as_ref() else { return };
    let Some(path) = pick("plugin.install", &["craftplugin"]) else { return };
    match path.to_str() {
        Some(path) => review_local(app, json!({"path": path})),
        None => app.command_plugins.status = format!("{} is not a Unicode path", path.display()),
    }
}

// ---------- drawing ----------

/// The Command Plug-ins window, the install confirmation and the parameter dialog, when they
/// show (every frame), and the result of a finished download.
pub fn show(app: &mut SoundApp, ctx: &egui::Context) {
    poll(app);
    if app.command_plugins.open {
        manager(app, ctx);
    }
    if app.command_plugins.review.is_some() {
        confirmation(app, ctx);
    }
    if app.command_plugins.params.is_some() {
        parameters(app, ctx);
    }
}

/// A one-line text field.
fn field(ui: &mut egui::Ui, text: &mut String, hint: &str) {
    ui.add(egui::TextEdit::singleline(text).desired_width(ADDRESS_WIDTH).hint_text(hint));
}

/// The Command Plug-ins window.
fn manager(app: &mut SoundApp, ctx: &egui::Context) {
    let mut open = true;
    let dim = Tokens::current().text_dim;
    egui::Window::new(MENU).id(egui::Id::unique("command-plugins-window")).open(&mut open).collapsible(false).default_width(480.0).show(ctx, |ui| {
        ui.label(
            RichText::new(
                "Command plug-ins run SoundCraft's commands on the open session. Audio plug-ins (CLAP, VST3, AU) are inserts in the Mix window.",
            )
            .color(dim),
        );
        ui.add_space(6.0);
        egui::ScrollArea::vertical().auto_shrink([false, true]).max_height(520.0).show(ui, |ui| {
            installed(app, ui);
            ui.add_space(8.0);
            install_section(app, ui, ctx);
            ui.add_space(8.0);
            catalogs(app, ui, ctx);
            if !app.command_plugins.status.is_empty() {
                ui.add_space(6.0);
                ui.label(RichText::new(app.command_plugins.status.as_str()).color(dim));
            }
        });
    });
    if !open {
        app.command_plugins.open = false;
    }
}

/// The installed plug-ins, their commands and problems.
fn installed(app: &mut SoundApp, ui: &mut egui::Ui) {
    ui.strong("Installed");
    let plugins: Vec<InstalledPlugin> = app.engine.command_plugins.list().to_vec();
    let dim = Tokens::current().text_dim;
    if plugins.is_empty() {
        ui.label(RichText::new("No command plug-ins installed.").color(dim));
    }
    for p in &plugins {
        let m = &p.manifest;
        ui.horizontal(|ui| {
            ui.label(RichText::new(format!("{} {}", m.name, m.version)).strong());
            let access = if m.permissions.access == Access::Write { "edits the session" } else { "reads the session" };
            ui.label(RichText::new(access).color(dim));
            if !p.stored {
                ui.label(RichText::new("this session only").color(dim));
            }
        });
        ui.horizontal_wrapped(|ui| {
            for c in &m.commands {
                if ui.button(c.label.as_str()).clicked()
                    && let Err(e) = run_command(app, &m.id, &c.id, None)
                {
                    app.command_plugins.status = e;
                }
            }
            if ui.button(crate::localization::text("Remove")).clicked() {
                app.command_plugins.status = outcome(app.engine.execute("plugin.remove", &json!({"id": m.id})));
            }
        });
    }
    for problem in app.engine.command_plugins.problems().to_vec() {
        ui.label(RichText::new(problem).color(dim));
    }
}

/// Install: from a file, or a GitHub repository.
fn install_section(app: &mut SoundApp, ui: &mut egui::Ui, ctx: &egui::Context) {
    ui.strong("Install");
    if ui.add_enabled(app.services.pick_open.is_some(), egui::Button::new("From File…")).clicked() {
        pick_file(app);
    }
    let busy = app.command_plugins.busy();
    ui.horizontal(|ui| {
        ui.label("GitHub repository");
        let mut repo = std::mem::take(&mut app.command_plugins.repo);
        field(ui, &mut repo, "owner/repo");
        let ready = !repo.trim().is_empty() && !busy;
        if ui.add_enabled(ready, egui::Button::new("Review…")).clicked() {
            review(app, ctx, json!({"repo": repo.trim()}));
        }
        app.command_plugins.repo = repo;
    });
}

/// The catalogs added, browsing them and checking for updates.
fn catalogs(app: &mut SoundApp, ui: &mut egui::Ui, ctx: &egui::Context) {
    ui.strong("Catalogs");
    for url in app.engine.command_plugins.prefs.catalogs.clone() {
        ui.horizontal(|ui| {
            ui.label(url.as_str());
            if ui.button(crate::localization::text("Remove")).clicked() {
                app.command_plugins.status = outcome(app.engine.execute("plugin.catalog.add", &json!({"url": url, "remove": true})));
            }
        });
    }
    ui.horizontal(|ui| {
        let mut url = std::mem::take(&mut app.command_plugins.catalog_url);
        field(ui, &mut url, "https://raw.githubusercontent.com/…/index.json");
        if ui.add_enabled(!url.trim().is_empty(), egui::Button::new(crate::localization::text("Add"))).clicked() {
            match app.engine.execute("plugin.catalog.add", &json!({"url": url.trim()})) {
                Ok(_) => url.clear(),
                Err(e) => app.command_plugins.status = crate::localization::error_text(&e.to_string()),
            }
        }
        app.command_plugins.catalog_url = url;
    });
    let busy = app.command_plugins.busy();
    ui.horizontal(|ui| {
        if ui.add_enabled(!busy && !app.engine.command_plugins.prefs.catalogs.is_empty(), egui::Button::new("Browse Catalogs")).clicked() {
            browse(app, ctx);
        }
        // `plugin.update`'s check, on a worker
        if ui.add_enabled(!busy && !app.engine.command_plugins.list().is_empty(), egui::Button::new("Check for Updates")).clicked() {
            check_updates(app, ctx);
        }
    });
    packages(app, ui, ctx, busy);
    updates(app, ui, ctx, busy);
}

/// The packages Browse Catalogs found.
fn packages(app: &mut SoundApp, ui: &mut egui::Ui, ctx: &egui::Context, busy: bool) {
    let dim = Tokens::current().text_dim;
    for pkg in app.command_plugins.packages.clone() {
        ui.horizontal(|ui| {
            let installed = pkg["installed"].as_str().map(|v| format!(" (installed: {v})")).unwrap_or_default();
            ui.label(format!("{} {}{installed}", pkg["name"].as_str().unwrap_or_default(), pkg["version"].as_str().unwrap_or_default()));
            if ui.add_enabled(!busy, egui::Button::new("Install…")).clicked() {
                review(app, ctx, json!({"catalog": pkg["catalog"], "id": pkg["id"]}));
            }
        });
        if let Some(text) = pkg["description"].as_str().filter(|t| !t.is_empty()) {
            ui.label(RichText::new(text).color(dim));
        }
    }
}

/// The updates Check for Updates found.
fn updates(app: &mut SoundApp, ui: &mut egui::Ui, ctx: &egui::Context, busy: bool) {
    for u in app.command_plugins.updates.clone() {
        ui.horizontal(|ui| {
            let text = |k: &str| u[k].as_str().unwrap_or_default().to_string();
            ui.label(format!("{} {} → {}", text("id"), text("installed"), text("available")));
            if ui.add_enabled(!busy, egui::Button::new("Update…")).clicked() {
                review(app, ctx, u["install"].clone());
            }
        });
    }
}

/// The install confirmation: nothing is written before Install.
fn confirmation(app: &mut SoundApp, ctx: &egui::Context) {
    let Some(reviewed) = app.command_plugins.review.clone() else { return };
    let mut open = true;
    let mut answer = None;
    egui::Window::new("Install Command Plug-in")
        .id(egui::Id::unique("command-plugin-install"))
        .open(&mut open)
        .collapsible(false)
        .resizable(false)
        .show(ctx, |ui| {
            confirmation_body(ui, &reviewed);
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button("Install").clicked() {
                    answer = Some(true);
                }
                if ui.button(crate::localization::text("Cancel")).clicked() {
                    answer = Some(false);
                }
            });
        });
    if !open || answer.is_some() {
        app.command_plugins.review = None;
    }
    if answer == Some(true) {
        install_reviewed(app, reviewed);
    }
}

/// What the confirmation says: the plug-in, its kind, access and commands, where it comes from
/// and its SHA-256, and warnings.
fn confirmation_body(ui: &mut egui::Ui, reviewed: &Review) {
    let info = &reviewed.info;
    let text = |k: &str| info[k].as_str().unwrap_or_default().to_string();
    let dim = Tokens::current().text_dim;
    ui.heading(format!("{} {}", text("name"), text("version")));
    for k in ["author", "description", "license", "repo"] {
        if !text(k).is_empty() {
            ui.label(text(k));
        }
    }
    egui::Grid::new("command-plugin-install-grid").num_columns(2).spacing([12.0, 4.0]).show(ui, |ui| {
        ui.label("Kind");
        ui.label(format!("{} plug-in (not an audio plug-in)", text("kind")));
        ui.end_row();
        ui.label("Access");
        let write = info["permissions"]["access"] == "write";
        ui.label(if write { "Reads and edits the open session" } else { "Reads the open session" });
        ui.end_row();
        ui.label("Commands");
        let allowed: Vec<&str> = info["permissions"]["commands"].as_array().map(|a| a.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
        ui.label(if allowed.is_empty() { "Any command its access allows".to_string() } else { allowed.join(", ") });
        ui.end_row();
        if info["permissions"]["saveFile"] == true {
            ui.label("Files");
            ui.label("Asks to save files (SoundCraft doesn't offer this yet)");
            ui.end_row();
        }
        ui.label("Source");
        ui.label(reviewed.source_text());
        ui.end_row();
        ui.label("SHA-256");
        ui.add(egui::Label::new(RichText::new(text("sha256")).monospace()).wrap());
        ui.end_row();
    });
    if info["inCatalog"] == false {
        ui.label(RichText::new("This plug-in isn't in any catalog you use: install it only if you trust its author.").strong());
    }
    if let Some(old) = info["replaces"].as_str() {
        ui.label(RichText::new(format!("Replaces version {old}.")).color(dim));
    }
    if info["permissionsChanged"] == true {
        ui.label(RichText::new("This version asks for different permissions.").strong());
    }
    if info["stored"] == false {
        ui.label(RichText::new("It is kept for this session only.").color(dim));
    }
}

/// A plug-in command's parameter dialog.
fn parameters(app: &mut SoundApp, ctx: &egui::Context) {
    let Some(mut dialog) = app.command_plugins.params.take() else { return };
    let mut open = true;
    let mut answer = None;
    egui::Window::new(dialog.title.clone()).id(egui::Id::unique("command-plugin-params")).open(&mut open).collapsible(false).resizable(false).show(
        ctx,
        |ui| {
            egui::Grid::new("command-plugin-params-grid").num_columns(2).spacing([12.0, 6.0]).show(ui, |ui| {
                for p in &dialog.params {
                    ui.label(p.title());
                    let v = dialog.values.entry(p.name.clone()).or_insert_with(|| p.default_value());
                    param_field(ui, p, v);
                    ui.end_row();
                }
            });
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button("Run").clicked() {
                    answer = Some(true);
                }
                if ui.button(crate::localization::text("Cancel")).clicked() {
                    answer = Some(false);
                }
            });
        },
    );
    if open && answer.is_none() {
        app.command_plugins.params = Some(dialog);
        return;
    }
    if answer == Some(true) {
        // its outcome is on the status line
        let _ = run_plugin(app, dialog.run_params());
    }
}

/// One parameter's control, editing `v`.
fn param_field(ui: &mut egui::Ui, p: &Param, v: &mut Value) -> egui::Response {
    match p.kind {
        ParamKind::Number | ParamKind::Slider => number_field(ui, p, v),
        ParamKind::Boolean => {
            let mut b = v.as_bool().unwrap_or(false);
            let r = ui.checkbox(&mut b, "");
            if r.changed() {
                *v = json!(b);
            }
            r
        }
        ParamKind::Select | ParamKind::List => {
            let current = v.as_str().unwrap_or_default().to_string();
            let r = craft_ui::ComboBox::from_id_salt(("command-plugin-param", &p.name)).selected_text(current.clone()).show_ui(ui, |ui| {
                for o in &p.options {
                    if ui.selectable_label(*o == current, o.as_str()).clicked() {
                        *v = json!(o);
                    }
                }
            });
            r.response
        }
        // Text, and a colour as `#RRGGBB`
        _ => {
            let mut s = v.as_str().unwrap_or_default().to_string();
            let r = ui.add(egui::TextEdit::singleline(&mut s).desired_width(TEXT_FIELD_WIDTH));
            if r.changed() {
                *v = json!(s);
            }
            r
        }
    }
}

/// A number parameter: a slider between its bounds, else a drag value (within them when given).
fn number_field(ui: &mut egui::Ui, p: &Param, v: &mut Value) -> egui::Response {
    let mut n = v.as_f64().unwrap_or(0.0);
    let r = match (p.kind, p.min, p.max) {
        (ParamKind::Slider, Some(lo), Some(hi)) if lo < hi => ui.add(egui::Slider::new(&mut n, lo..=hi)),
        _ => {
            let mut drag = egui::DragValue::new(&mut n);
            if let (Some(lo), Some(hi)) = (p.min, p.max)
                && lo <= hi
            {
                drag = drag.range(lo..=hi);
            }
            ui.add(drag)
        }
    };
    if r.changed() && n.is_finite() {
        *v = json!(n);
    }
    r
}

#[cfg(test)]
mod tests {
    use soundcraft_engine::Engine;
    use soundcraft_engine::command_plugin::craft_plugin::bundle::Bundle;

    use super::*;
    use crate::Services;

    /// A plug-in whose commands each add a track; `asks` declares a parameter.
    fn install(app: &mut SoundApp) {
        let wasm = wat::parse_str(
            r#"(module
              (import "craft" "call" (func $call (param i32 i32 i32 i32) (result i32)))
              (memory (export "memory") 1)
              (data (i32.const 0) "track.new")
              (data (i32.const 32) "{\"count\":1}")
              (func (export "craft_run") (param i32) (result i32)
                (drop (call $call (i32.const 0) (i32.const 9) (i32.const 32) (i32.const 11))) (i32.const 0)))"#,
        )
        .unwrap();
        let manifest = json!({"id": "io.example.ui", "name": "UI Test", "version": "1.0.0", "main": "p.wasm",
            "apps": {"soundcraft": {"api": 6}}, "permissions": {"access": "write"},
            "commands": [{"id": "plain", "label": "Plain"},
                         {"id": "asks", "label": "Asks…", "params": [{"name": "count", "type": "number", "default": 2, "min": 1, "max": 9}]}]});
        let bytes = Bundle { manifest: manifest.to_string(), wasm, assets: vec![] }.encode();
        let data = soundcraft_model::b64::encode(&bytes);
        // Review, then install what was reviewed, as the confirmation does
        review_local(app, json!({"data": data}));
        let reviewed = app.command_plugins.review.take().unwrap_or_else(|| panic!("{}", app.command_plugins.status));
        assert_eq!(reviewed.info["permissions"]["access"], "write");
        assert_eq!(reviewed.source_text(), "data");
        install_reviewed(app, reviewed);
    }

    fn demo_app() -> SoundApp {
        SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default())
    }

    fn tracks(app: &SoundApp) -> usize {
        app.engine.session().tracks.len()
    }

    #[test]
    fn plugin_commands_run_as_one_step_and_ask_for_their_parameters() {
        let mut app = demo_app();
        install(&mut app);
        let count = tracks(&app);

        // A command without parameters runs at once, as one undo step
        run_command(&mut app, "io.example.ui", "plain", None).unwrap();
        assert_eq!(tracks(&app), count + 1);
        assert_eq!(app.engine.undo_label(), Some("UI Test: Plain"));
        assert_eq!(app.ui.status, "UI Test: done");

        // One with parameters asks first
        let r = run_command(&mut app, "io.example.ui", "asks", None).unwrap();
        assert_eq!(r["dialog"], "commandPluginParams");
        let dialog = app.command_plugins.params.clone().unwrap();
        assert_eq!(dialog.values["count"], 2.0);
        assert_eq!(dialog.title, "UI Test: Asks");
        run_plugin(&mut app, dialog.run_params()).unwrap();
        assert_eq!(tracks(&app), count + 2);

        assert!(app.run(WINDOW, json!({"value": true})).is_ok());
        assert!(app.command_plugins.open);
    }

    #[test]
    fn a_repository_in_no_catalog_says_where_it_comes_from() {
        let reviewed = Review {
            request: json!({"repo": "example/x"}),
            info: json!({"source": {"type": "github", "repo": "example/x", "tag": "v1", "asset": "x.craftplugin"}, "inCatalog": false}),
        };
        assert_eq!(reviewed.source_text(), "github.com/example/x v1 (x.craftplugin)");
    }

    #[test]
    fn a_repository_review_downloads_on_a_worker_and_reports_no_network() {
        let mut app = SoundApp::new(Engine::default(), None, Services::default());
        let ctx = egui::Context::default();
        review(&mut app, &ctx, json!({"repo": "example/tools"}));
        for _ in 0..500 {
            poll(&mut app);
            if !app.command_plugins.busy() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(!app.command_plugins.busy());
        assert!(app.command_plugins.review.is_none());
        assert!(app.command_plugins.status.contains("no network access"), "{}", app.command_plugins.status);
    }

    #[test]
    fn settings_reach_the_saved_preferences() {
        let mut app = SoundApp::new(Engine::default(), None, Services::default());
        app.ui.command_plugins.catalogs = vec!["https://raw.githubusercontent.com/o/r/main/index.json".into()];
        app.start_command_plugins(None);
        assert_eq!(app.engine.command_plugins.prefs.catalogs.len(), 1, "read from the saved preferences");
        assert!(!app.take_command_plugin_prefs_changed());
        app.engine.execute("plugin.catalog.add", &json!({"url": "https://raw.githubusercontent.com/o/r/main/other.json"})).unwrap();
        assert!(app.take_command_plugin_prefs_changed(), "the host saves them now");
        assert_eq!(app.ui.command_plugins.catalogs.len(), 2);
    }

    #[test]
    fn the_windows_and_the_menu_draw() {
        let mut app = demo_app();
        install(&mut app);
        run_command(&mut app, "io.example.ui", "asks", None).unwrap();
        app.command_plugins.open = true;
        app.command_plugins.review =
            Some(Review { request: json!({}), info: json!({"name": "X", "inCatalog": false, "source": {"type": "file", "path": "x"}}) });
        let ctx = egui::Context::default();
        let input =
            egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1600.0, 1000.0))), ..Default::default() };
        let mut output = ctx.run_ui(input, |ui| {
            egui::MenuBar::new().ui(ui, |ui| menu(&mut app, ui, true));
            show(&mut app, ui.ctx());
        });
        output.textures_delta.clear();
        assert!(app.command_plugins.params.is_some() && app.command_plugins.review.is_some(), "nothing was answered");
    }
}
