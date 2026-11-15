//! The JSFX Browser: search ReaPack repositories and install JSFX effects into SoundCraft's own
//! Effects folder. Downloads, installs and removals run on a worker thread (the window stays
//! responsive); searching only reads the indexes already downloaded, and waits while the worker
//! holds the repositories.

use crate::SoundApp;
use crate::theme::{Tokens, bold, regular};
use egui::{Button, RichText, ScrollArea, TextEdit, vec2};
use soundcraft_reapack::{Hit, Installed, RefreshOutcome, Repo, RepoInfo};
use std::sync::mpsc::{Receiver, channel};

/// Matches shown at most; narrowing the search shows the rest.
const MAX_HITS: usize = 200;
const OPEN_FLAG: &str = "jsfx_browser_open";

/// What a worker thread reports back.
enum Msg {
    Refreshed(Result<Vec<RefreshOutcome>, String>),
    Installed(Result<Installed, String>),
    RepoAdded(Result<Repo, String>),
    /// The package removed and how many files went.
    Uninstalled(Result<(String, usize), String>),
}

#[derive(Default)]
pub struct JsfxBrowser {
    pub open: bool,
    query: String,
    new_repo: String,
    hits: Vec<Hit>,
    repos: Vec<RepoInfo>,
    status: String,
    /// What the worker thread is doing, if anything.
    busy: Option<String>,
    rx: Option<Receiver<Msg>>,
    loaded: bool,
    show_repos: bool,
}

/// Asks the browser to open (used by menus that have no access to the app state).
pub fn request_open(ctx: &egui::Context) {
    ctx.data_mut(|d| d.insert_temp(egui::Id::unique(OPEN_FLAG), true));
}

impl JsfxBrowser {
    /// Re-reads the repository list and re-runs the search against the downloaded indexes.
    fn reload(&mut self) {
        match soundcraft_reapack::system() {
            Ok(m) => {
                self.repos = m.repos();
                self.hits = m.search(&self.query, None, MAX_HITS).unwrap_or_default();
                self.loaded = true;
            }
            Err(e) => self.status = e.to_string(),
        }
    }

    fn spawn(&mut self, what: &str, job: impl FnOnce() -> Msg + Send + 'static) {
        if self.busy.is_some() {
            return;
        }
        if cfg!(target_arch = "wasm32") {
            self.status = "Downloading JSFX is not available in the web version".into();
            return;
        }
        let (tx, rx) = channel();
        self.rx = Some(rx);
        self.busy = Some(what.to_string());
        self.status.clear();
        std::thread::spawn(move || {
            let _ = tx.send(job());
        });
    }

    fn poll(&mut self) {
        let Some(msg) = self.rx.as_ref().and_then(|rx| rx.try_recv().ok()) else { return };
        self.busy = None;
        self.rx = None;
        self.status = match msg {
            Msg::Refreshed(Ok(out)) => {
                let failed: Vec<String> = out.iter().filter_map(|o| o.error.as_ref().map(|e| format!("{}: {e}", o.name))).collect();
                let total: usize = out.iter().map(|o| o.packages).sum();
                if failed.is_empty() {
                    format!("Downloaded {} repositor{} ({total} effects)", out.len(), if out.len() == 1 { "y" } else { "ies" })
                } else {
                    failed.join("; ")
                }
            }
            Msg::Installed(Ok(rec)) => format!("Installed {} {} — find it in Insert › JSFX", rec.package, rec.version),
            Msg::RepoAdded(Ok(repo)) => format!("Added {}", repo.name),
            Msg::Uninstalled(Ok((package, n))) => format!("Removed {package} ({n} files)"),
            Msg::Refreshed(Err(e)) | Msg::Installed(Err(e)) | Msg::RepoAdded(Err(e)) | Msg::Uninstalled(Err(e)) => e,
        };
        self.reload();
    }
}

pub fn show(app: &mut SoundApp, ctx: &egui::Context) {
    if ctx.data_mut(|d| d.remove_temp::<bool>(egui::Id::unique(OPEN_FLAG))).unwrap_or(false) {
        app.jsfx_browser.open = true;
    }
    let b = &mut app.jsfx_browser;
    if !b.open {
        return;
    }
    if !b.loaded {
        b.reload();
    }
    b.poll();
    if b.busy.is_some() {
        ctx.request_repaint_after(std::time::Duration::from_millis(100));
    }
    let t = Tokens::current();
    let mut open = b.open;
    egui::Window::new(RichText::new("JSFX Browser").font(bold(13.0))).open(&mut open).default_size(vec2(640.0, 520.0)).resizable(true).show(
        ctx,
        |ui| {
            let idle = b.busy.is_none();
            ui.horizontal(|ui| {
                let edit = ui.add(
                    TextEdit::singleline(&mut b.query)
                        .hint_text("Search name, description, category or author")
                        .desired_width(ui.available_width() - 170.0),
                );
                // While the worker downloads it holds the repositories: the search runs when it's done.
                if edit.changed() && idle {
                    b.reload();
                }
                if ui.add_enabled(idle, Button::new("Refresh repositories")).on_hover_text("Download the latest index of every repository").clicked()
                {
                    b.spawn("Downloading repository indexes…", || {
                        Msg::Refreshed(soundcraft_reapack::system().and_then(|m| m.refresh(None)).map_err(|e| e.to_string()))
                    });
                }
            });

            ui.collapsing(format!("Repositories ({})", b.repos.len()), |ui| {
                b.show_repos = true;
                let mut remove = None;
                for r in &b.repos {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(&r.name).font(bold(11.0)));
                        let state = if r.cached { format!("{} effects", r.packages) } else { "not downloaded yet".to_string() };
                        ui.label(RichText::new(format!("{state}  ·  {}", r.url)).font(regular(10.0)).color(t.text_dim));
                        if ui.add_enabled(idle, Button::new("Remove").small()).clicked() {
                            remove = Some(r.url.clone());
                        }
                    });
                }
                if let Some(url) = remove
                    && let Ok(m) = soundcraft_reapack::system()
                {
                    b.status = m.remove_repo(&url).map_or_else(|e| e.to_string(), |()| "Repository removed".to_string());
                    b.reload();
                }
                ui.horizontal(|ui| {
                    ui.add(TextEdit::singleline(&mut b.new_repo).hint_text("https://…/index.xml").desired_width(360.0));
                    if ui.add_enabled(idle && !b.new_repo.trim().is_empty(), Button::new("Add repository")).clicked() {
                        let url = std::mem::take(&mut b.new_repo);
                        b.spawn("Adding repository…", move || {
                            Msg::RepoAdded(soundcraft_reapack::system().and_then(|m| m.add_repo(&url)).map_err(|e| e.to_string()))
                        });
                    }
                });
            });

            ui.add_space(2.0);
            ui.horizontal(|ui| {
                if let Some(what) = &b.busy {
                    ui.spinner();
                    ui.label(RichText::new(what).font(regular(11.0)));
                } else if !b.status.is_empty() {
                    ui.label(RichText::new(&b.status).font(regular(11.0)).color(t.text));
                }
            });
            ui.separator();

            if b.hits.is_empty() {
                let any_cached = b.repos.iter().any(|r| r.cached);
                let msg = if !any_cached {
                    "No repository has been downloaded yet. Press “Refresh repositories” to get the ReaTeam JSFX index."
                } else if b.query.trim().is_empty() {
                    "The downloaded repositories contain no JSFX effects."
                } else {
                    "No effect matches this search."
                };
                ui.label(RichText::new(msg).font(regular(11.0)).color(t.text_dim));
            } else {
                let more = if b.hits.len() >= MAX_HITS { format!(" (first {MAX_HITS}, narrow the search to see others)") } else { String::new() };
                ui.label(RichText::new(format!("{} effects{more}", b.hits.len())).font(regular(10.0)).color(t.text_dim));
            }

            let mut install = None;
            let mut uninstall = None;
            ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                for h in &b.hits {
                    ui.horizontal(|ui| {
                        ui.vertical(|ui| {
                            ui.label(RichText::new(&h.desc).font(bold(11.0)));
                            let by = if h.author.is_empty() { String::new() } else { format!("  ·  {}", h.author) };
                            ui.label(
                                RichText::new(format!("{} / {}  ·  v{}{by}", h.category, h.package, h.version)).font(regular(10.0)).color(t.text_dim),
                            );
                        });
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let label = match (&h.installed_version, h.update_available) {
                                (None, _) => "Install",
                                (Some(_), true) => "Update",
                                (Some(_), false) => "Installed",
                            };
                            let enabled = idle && (h.installed_version.is_none() || h.update_available);
                            if ui.add_enabled(enabled, Button::new(label).min_size(vec2(72.0, 0.0))).clicked() {
                                install = Some(h.clone());
                            }
                            if h.installed_version.is_some() && ui.add_enabled(idle, Button::new("Remove")).clicked() {
                                uninstall = Some(h.clone());
                            }
                        });
                    });
                    ui.separator();
                }
            });
            if let Some(h) = install {
                b.spawn(&format!("Installing {}…", h.package), move || {
                    let r = soundcraft_reapack::system().and_then(|m| m.install(&h.repo, &h.category, &h.package));
                    soundcraft_jsfx_host::rescan();
                    Msg::Installed(r.map_err(|e| e.to_string()))
                });
            }
            if let Some(h) = uninstall {
                b.spawn(&format!("Removing {}…", h.package), move || {
                    let r = soundcraft_reapack::system().and_then(|m| m.uninstall(&h.repo, &h.category, &h.package));
                    soundcraft_jsfx_host::rescan();
                    Msg::Uninstalled(r.map(|n| (h.package.clone(), n)).map_err(|e| e.to_string()))
                });
            }
        },
    );
    app.jsfx_browser.open = open;
}
