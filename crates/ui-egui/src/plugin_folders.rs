//! Setup › Plug-in Folders: the folders scanned for VST3 and CLAP plugins, the user's own ones
//! added and removed, rescans, and the files that would not load. Not in Pro Tools (its plug-ins
//! live in one fixed folder); REAPER keeps the same controls under Preferences › Plug-ins.
//!
//! Scans run on a background thread (see [`start`]) and are remembered in the settings folder,
//! so that only new and changed plugin files are loaded at each launch.

use crate::SoundApp;
use crate::theme::Tokens;
use egui::vec2;
use std::path::{Path, PathBuf};

/// The user's own plugin folders, scanned besides the standard ones (kept with the preferences).
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct PluginDirs {
    pub vst3: Vec<String>,
    pub clap: Vec<String>,
}

#[derive(Clone, Copy, PartialEq)]
enum Format {
    Vst3,
    Clap,
}

impl Format {
    fn name(self) -> &'static str {
        match self {
            Format::Vst3 => "VST3",
            Format::Clap => "CLAP",
        }
    }
    fn dirs(self, all: &mut PluginDirs) -> &mut Vec<String> {
        match self {
            Format::Vst3 => &mut all.vst3,
            Format::Clap => &mut all.clap,
        }
    }
    fn search_dirs(self) -> (Vec<PathBuf>, Vec<PathBuf>) {
        match self {
            Format::Vst3 => soundcraft_vst3_host::search_dirs(),
            Format::Clap => soundcraft_clap_host::search_dirs(),
        }
    }
    fn set_extra_dirs(self, dirs: &[String]) {
        let dirs = dirs.iter().map(PathBuf::from).collect();
        match self {
            Format::Vst3 => soundcraft_vst3_host::set_extra_dirs(dirs),
            Format::Clap => soundcraft_clap_host::set_extra_dirs(dirs),
        }
    }
    /// The number of plugins found, `None` while scanning or before the first scan.
    fn found(self) -> Option<usize> {
        match self {
            Format::Vst3 => soundcraft_vst3_host::cached().map(|l| l.len()),
            Format::Clap => soundcraft_clap_host::cached().map(|l| l.len()),
        }
    }
    fn scanning(self) -> bool {
        match self {
            Format::Vst3 => soundcraft_vst3_host::scanning(),
            Format::Clap => soundcraft_clap_host::scanning(),
        }
    }
    fn rescan(self, clear_cache: bool) {
        match self {
            Format::Vst3 => soundcraft_vst3_host::rescan_in_background(clear_cache),
            Format::Clap => soundcraft_clap_host::rescan_in_background(clear_cache),
        }
    }
    fn progress(self) -> Option<(usize, usize, PathBuf)> {
        match self {
            Format::Vst3 => soundcraft_vst3_host::progress(),
            Format::Clap => soundcraft_clap_host::progress(),
        }
    }
    fn skip(self) {
        match self {
            Format::Vst3 => soundcraft_vst3_host::skip_scan(),
            Format::Clap => soundcraft_clap_host::skip_scan(),
        }
    }
    fn problems(self) -> Vec<(PathBuf, String)> {
        match self {
            Format::Vst3 => soundcraft_vst3_host::problems(),
            Format::Clap => soundcraft_clap_host::problems(),
        }
    }
}

/// At launch: remembers scans in `settings_dir` (when there is one), adds the user's folders and
/// starts scanning on a background thread, so that the plugin menus are ready when first opened.
/// Plugin files are loaded for the scan by `scanner` (this app's own program, run with
/// [`scan_child`]'s arguments) in a child process, never in the app.
pub fn start(dirs: &PluginDirs, settings_dir: Option<&Path>, scanner: Option<PathBuf>) {
    Format::Vst3.set_extra_dirs(&dirs.vst3);
    Format::Clap.set_extra_dirs(&dirs.clap);
    soundcraft_vst3_host::set_scanner(scanner.clone());
    soundcraft_clap_host::set_scanner(scanner);
    if let Some(dir) = settings_dir {
        soundcraft_vst3_host::set_cache_file(Some(dir.join("plugins-vst3.json")));
        soundcraft_clap_host::set_cache_file(Some(dir.join("plugins-clap.json")));
        soundcraft_vst3_host::scan_in_background();
        soundcraft_clap_host::scan_in_background();
    }
}

/// The scan child process's work: with `<flag> <file>` arguments (see [`start`]), loads that one
/// plugin file and returns the line to print; `None` for any other arguments.
pub fn scan_child(args: &[String]) -> Option<String> {
    let [flag, file] = args else { return None };
    let file = Path::new(file);
    if flag == soundcraft_vst3_host::SCAN_FLAG {
        Some(soundcraft_vst3_host::scan_reply(file))
    } else if flag == soundcraft_clap_host::SCAN_FLAG {
        Some(soundcraft_clap_host::scan_reply(file))
    } else {
        None
    }
}

/// A "Scanning …" line with a spinner; keeps the UI redrawing until the scan ends.
pub(crate) fn scanning_label(ui: &mut egui::Ui, what: &str) {
    ui.horizontal(|ui| {
        ui.spinner();
        ui.weak(format!("Scanning {what}…"));
    });
    ui.ctx().request_repaint_after(std::time::Duration::from_millis(200));
}

/// How long a scan runs before its notice shows (a scan that only reads the cache is quicker).
const NOTICE_DELAY_S: f64 = 0.5;

/// While a scan loads plugin files, a notice at the bottom right of the main window says which
/// one ("Scanning VST3 plug-ins 5/17: Clear"), whatever window is shown, with Skip: the app stays
/// usable meanwhile, but the plugin menus wait for the list. Skip ends the scan after the file it
/// is loading; the skipped files are loaded at the next scan.
pub fn scan_notice(ctx: &egui::Context) {
    let since_id = egui::Id::unique("plugin_scan_since");
    let now = ctx.input(|i| i.time);
    let Some((format, (i, n, file))) = [Format::Vst3, Format::Clap].into_iter().find_map(|f| f.progress().map(|p| (f, p))) else {
        ctx.data_mut(|d| d.remove_temp::<f64>(since_id));
        if soundcraft_vst3_host::scanning() || soundcraft_clap_host::scanning() {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
        return;
    };
    let since = ctx.data_mut(|d| *d.get_temp_mut_or_insert_with(since_id, || now));
    ctx.request_repaint_after(std::time::Duration::from_millis(100));
    if now - since < NOTICE_DELAY_S {
        return;
    }
    let name = file.file_stem().map_or_else(|| file.display().to_string(), |s| s.to_string_lossy().into_owned());
    egui::Area::new(egui::Id::unique("plugin_scan_notice"))
        .anchor(egui::Align2::RIGHT_BOTTOM, vec2(-12.0, -30.0))
        .order(egui::Order::Foreground)
        .show(ctx, |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(format!("Scanning {} plug-ins {}/{n}: {name}", format.name(), i + 1));
                    if ui
                        .button("Skip")
                        .on_hover_text("Stop after this file; the rest load at the next scan (Setup ▸ Plug-in Folders ▸ Rescan)")
                        .clicked()
                    {
                        Format::Vst3.skip();
                        Format::Clap.skip();
                    }
                });
            });
        });
}

pub fn window(app: &mut SoundApp, ctx: &egui::Context) {
    let mut open = app.ui.show_plugin_folders;
    if !open {
        return;
    }
    let tab_id = egui::Id::unique("plugin_folders_tab");
    let mut tab = ctx.data(|d| d.get_temp::<bool>(tab_id)).map_or(Format::Vst3, |clap| if clap { Format::Clap } else { Format::Vst3 });
    let mut changed = false;
    egui::Window::new("Plug-in Folders").open(&mut open).default_size(vec2(560.0, 380.0)).show(ctx, |ui| {
        let t = Tokens::current();
        ui.horizontal(|ui| {
            for f in [Format::Vst3, Format::Clap] {
                ui.selectable_value(&mut tab, f, f.name());
            }
        });
        ui.separator();
        // Folders change between scans only (a running scan would miss the change).
        let busy = tab.scanning();
        let (standard, _) = tab.search_dirs();
        ui.label("Standard folders");
        for d in &standard {
            ui.label(egui::RichText::new(d.display().to_string()).color(t.text_dim));
        }
        ui.add_space(6.0);
        ui.label("Your folders");
        let mine = tab.dirs(&mut app.ui.plugin_dirs);
        let mut remove = None;
        for (i, d) in mine.iter().enumerate() {
            ui.horizontal(|ui| {
                if ui.add_enabled(!busy, egui::Button::new("Remove").small()).clicked() {
                    remove = Some(i);
                }
                ui.label(d);
            });
        }
        if mine.is_empty() {
            ui.weak("None");
        }
        if let Some(i) = remove {
            mine.remove(i);
            changed = true;
        }
        if let Some(pick) = &app.services.pick_folder
            && ui.add_enabled(!busy, egui::Button::new("Add Folder…")).clicked()
            // The dialog hands back a path; the session keeps folders as text.
            && let Some(dir) = pick(mine.last().map_or(Path::new(""), |d| Path::new(d)))
            && let Some(dir) = dir.to_str().map(str::to_string)
            && !mine.contains(&dir)
        {
            mine.push(dir);
            changed = true;
        }
        ui.separator();
        match tab.found() {
            Some(n) if !busy => {
                ui.label(format!("{n} {} plugin{} found", tab.name(), if n == 1 { "" } else { "s" }));
            }
            _ => scanning_label(ui, &format!("{} plugins", tab.name())),
        }
        ui.horizontal(|ui| {
            if ui.add_enabled(!busy, egui::Button::new("Rescan")).on_hover_text("Load the plugin files that are new or changed").clicked() {
                tab.rescan(false);
            }
            if ui
                .add_enabled(!busy, egui::Button::new("Clear Cache & Rescan"))
                .on_hover_text("Load every plugin file again, including those that failed")
                .clicked()
            {
                tab.rescan(true);
            }
        });
        let problems = tab.problems();
        if !problems.is_empty() {
            ui.add_space(4.0);
            egui::CollapsingHeader::new(format!("{} file{} could not be loaded", problems.len(), if problems.len() == 1 { "" } else { "s" }))
                .id_salt("plugin_problems")
                .show(ui, |ui| {
                    egui::ScrollArea::vertical().max_height(140.0).show(ui, |ui| {
                        for (path, why) in &problems {
                            ui.label(path.display().to_string()).on_hover_text(why);
                            ui.label(egui::RichText::new(why).small().color(t.text_dim));
                        }
                    });
                });
        }
    });
    if changed {
        tab.set_extra_dirs(tab.dirs(&mut app.ui.plugin_dirs));
        tab.rescan(false);
    }
    ctx.data_mut(|d| d.insert_temp(tab_id, tab == Format::Clap));
    app.ui.show_plugin_folders = open;
}
