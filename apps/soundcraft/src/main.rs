//! SoundCraft desktop app.
//!
//! Usage: `soundcraft [--demo] [--control <port>] [--no-audio] [session.scraft | audio files…]`
//!
//! `--control <port>` (or `SOUNDCRAFT_CONTROL_PORT`) starts a localhost JSON-lines control server
//! (`0` picks a free port and prints it): `{"id":1,"method":"ui.inspect","params":{}}` →
//! `{"id":1,"ok":true,"result":…}`. See `soundcraft_ui_egui::control` for the methods.
//!
//! `log` records go to standard error and `<settings dir>/logs/soundcraft.log` (`RUST_LOG` sets the
//! levels); see [`logging`].
#![cfg_attr(all(target_os = "windows", not(debug_assertions)), windows_subsystem = "windows")]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

mod config;
mod control_server;
#[cfg(any(target_os = "windows", test))]
mod graphics;
mod logging;
#[cfg(target_os = "macos")]
mod native_menu;

use soundcraft_engine::Engine;
use soundcraft_ui_egui::i18n::tr;
use soundcraft_ui_egui::{Services, SoundApp, UiState, dialogs};
use std::sync::Arc;

struct App(SoundApp, Option<&'static logging::AppLogger>, #[cfg(target_os = "macos")] Option<native_menu::NativeMenu>);

/// Sends the title only when it changes, remembered here as X11 and Wayland can't read it back.
fn set_title(ctx: &egui::Context, title: String) {
    let id = egui::Id::new("window_title");
    if ctx.data(|d| d.get_temp::<String>(id)).as_deref() != Some(title.as_str()) {
        ctx.data_mut(|d| d.insert_temp(id, title.clone()));
        ctx.send_viewport_cmd(egui::ViewportCommand::Title(title));
    }
}

impl eframe::App for App {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // What the realtime audio thread logged is written here, off that thread.
        if let Some(logger) = self.1 {
            logger.report_audio_thread();
        }
        #[cfg(target_os = "macos")]
        if let Some(menu) = &mut self.2 {
            menu.process(&mut self.0);
        }
        self.0.guarded(|app| app.logic(ctx));
        #[cfg(target_os = "macos")]
        if let Some(menu) = &mut self.2 {
            menu.sync(&self.0);
        }
        // Files dropped on the window: audio on an instrument track or a Sampler window loads
        // the sampler; otherwise sessions open and audio/MIDI import.
        let dropped: Vec<String> =
            self.0.take_dropped_files(ctx).iter().map(|f| f.path().to_string_lossy().into_owned()).filter(|s| !s.is_empty()).collect();
        for p in dropped {
            open_path(&mut self.0, &p);
        }
        if self.0.quit_requested {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        } else if ctx.input(|i| i.viewport().close_requested()) && dialogs::ask_to_save(&mut self.0, "app.quit", &serde_json::json!({})) {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
        }
        soundcraft_ui_egui::fit_window_to_screen(ctx);
        let (inner, maximized) = ctx.input(|i| (i.viewport().inner_rect, i.viewport().maximized.unwrap_or(false)));
        self.0.ui.window_maximized = maximized;
        if let Some(r) = inner.filter(|_| !maximized) {
            self.0.ui.window_size = (r.size() * ctx.zoom_factor()).round().into();
        }
        self.0.ui.ui_scale = self.0.extra.ui_scale;
        // Saved now rather than at exit: the recent list matters most after a crash.
        if self.0.take_recent_changed() {
            save_prefs(&self.0.ui);
        }
        // With the system menu bar there is no egui bar showing the window and dirty state, so the
        // title carries them; elsewhere the egui menu bar already does.
        let title = if self.0.native_menu_bar {
            format!(
                "{}{} — SoundCraft — {}",
                self.0.engine.session().name,
                if self.0.engine.is_dirty() { " *" } else { "" },
                tr(if self.0.ui.window == soundcraft_ui_egui::MainWindow::Edit { "Edit" } else { "Mix" })
            )
        } else {
            format!("{} — SoundCraft", self.0.engine.session().name)
        };
        set_title(ctx, title);
    }
    fn raw_input_hook(&mut self, _ctx: &egui::Context, raw: &mut egui::RawInput) {
        self.0.raw_input_hook(raw);
    }
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.0.guarded(|app| app.ui(ui));
    }
    fn on_exit(&mut self) {
        self.0.close_autosave();
        save_prefs(&self.0.ui);
        if let Some(logger) = self.1 {
            logger.report_audio_thread();
        }
        // The system Quit (⌘Q) ends the process from inside the event loop, so `run_native` never
        // returns: release the player's plugin instances and shut plugin hosting down here.
        self.0.player = None;
        soundcraft_engine::shutdown_plugin_hosts();
    }
}

fn open_path(app: &mut SoundApp, p: &str) {
    let lower = p.to_ascii_lowercase();
    let r = if lower.ends_with(".scraft") {
        dialogs::run_or_ask(app, "session.open", serde_json::json!({"path": p}))
    } else if lower.ends_with(".mid") || lower.ends_with(".midi") || lower.ends_with(".smf") {
        app.run("file.import_midi", serde_json::json!({"path": p}))
    } else {
        let at = app.engine.session().edit.selection.start;
        app.run("file.import_audio", serde_json::json!({"path": p, "at": at}))
    };
    if let Err(e) = r {
        log::warn!("{p}: {e}");
    }
}

/// The per-user settings directory: `ui.json`, `Autosave/`, `Presets/` and `logs/` live here.
fn config_dir() -> Option<std::path::PathBuf> {
    config::config_dir()
}

fn prefs_path() -> Option<std::path::PathBuf> {
    config_dir().map(|b| b.join("ui.json"))
}

/// Runs without preferences (`SOUNDCRAFT_NO_PREFS`, agents' test runs) neither read nor write them.
fn prefs_enabled() -> bool {
    std::env::var_os("SOUNDCRAFT_NO_PREFS").is_none()
}

fn load_prefs() -> Option<UiState> {
    if !prefs_enabled() {
        return None;
    }
    let path = prefs_path()?;
    match config::load_ui(&path) {
        Ok(state) => state,
        Err(error) => {
            log::warn!("{error}");
            None
        }
    }
}

fn save_prefs(ui: &UiState) {
    if !prefs_enabled() {
        return;
    }
    if let Some(path) = prefs_path()
        && let Err(error) = config::save_ui(&path, ui)
    {
        log::warn!("{error}");
    }
}

fn services() -> Services {
    Services {
        pick_open: Some(Box::new(|purpose: &str, _exts: &[&str]| {
            let d = rfd::FileDialog::new();
            let d = match purpose {
                "session.open" => d.add_filter(tr("SoundCraft Session"), &["scraft"]),
                "file.import_midi" => d.add_filter("MIDI", &["mid", "midi", "smf"]),
                _ => d.add_filter(
                    tr("Audio"),
                    &["wav", "wave", "bwf", "aif", "aiff", "aifc", "flac", "mp3", "ogg", "oga", "m4a", "m4b", "aac", "mp4", "caf", "mkv", "webm"],
                ),
            };
            d.pick_file().map(|p| p.to_string_lossy().into_owned())
        })),
        pick_save: Some(Box::new(|purpose: &str, default: &str| {
            let path = std::path::Path::new(default);
            let mut d = match purpose {
                "file.bounce_mix" => rfd::FileDialog::new().add_filter(tr("Audio"), &["wav", "aif", "aiff", "flac"]),
                _ => rfd::FileDialog::new().add_filter(tr("SoundCraft Session"), &["scraft"]),
            };
            if let Some(dir) = path.parent().filter(|dir| dir.is_dir()) {
                d = d.set_directory(dir);
            }
            if let Some(n) = path.file_name().and_then(|n| n.to_str()) {
                d = d.set_file_name(n);
            }
            d.save_file().map(|p| p.to_string_lossy().into_owned())
        })),
        pick_folder: Some(Box::new(|start: &str| {
            rfd::FileDialog::new().set_directory(start).pick_folder().map(|p| p.to_string_lossy().into_owned())
        })),
    }
}

/// The window, Dock, taskbar and Alt-Tab icon. macOS gets Apple's icon grid (transparent margin);
/// elsewhere the full tile. Regenerate with `packaging/icons.sh`.
fn app_icon() -> Option<egui::IconData> {
    #[cfg(target_os = "macos")]
    let png: &[u8] = include_bytes!("../../../assets/app-icon/soundcraft-macos-512.png");
    #[cfg(not(target_os = "macos"))]
    let png: &[u8] = include_bytes!("../../../assets/app-icon/hicolor/256x256/apps/ai.storyteller.soundcraft.png");
    eframe::icon_data::from_png_bytes(png).map_err(|e| log::warn!("app icon: {e}")).ok()
}

fn main() -> eframe::Result {
    // First, so every start-up record (and the engine's panic hook, installed with the first
    // Engine) is captured; see `logging`.
    let logger = logging::install();
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--version" || a == "-V") {
        println!("SoundCraft {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    let demo = args.iter().any(|a| a == "--demo" || a == "--sample");
    let no_audio = args.iter().any(|a| a == "--no-audio");
    let control_port: Option<u16> = args
        .iter()
        .position(|a| a == "--control")
        .and_then(|i| args.get(i + 1))
        .and_then(|p| p.parse().ok())
        .or_else(|| std::env::var("SOUNDCRAFT_CONTROL_PORT").ok().and_then(|p| p.parse().ok()));
    let files: Vec<String> = args
        .iter()
        .enumerate()
        .filter(|(i, a)| !(a.starts_with("--") || (i > &0 && args.get(i - 1).is_some_and(|p| p == "--control"))))
        .map(|(_, a)| a.clone())
        .collect();
    log::info!("SoundCraft {} ({} {})", env!("CARGO_PKG_VERSION"), std::env::consts::OS, std::env::consts::ARCH);
    // The log file lives in the settings directory; opened after the arguments, so `--version`
    // leaves no file behind. Records logged until now are written to it first. Runs without
    // preferences (agents' test runs) log to standard error only, so they don't rotate away the
    // user's own logs.
    if let Some(logger) = logger {
        match config_dir() {
            _ if !prefs_enabled() => logger.no_file(),
            Some(dir) => match logger.attach_dir(&dir.join(logging::LOG_DIR)) {
                Ok(path) => log::info!("log file {}", path.display()),
                Err(e) => log::warn!("no log file: {e}"),
            },
            None => {
                logger.no_file();
                log::warn!("no log file: no settings directory (profile override is empty or platform configuration is unavailable)");
            }
        }
    }

    let engine = if demo { soundcraft_engine::demo::demo_engine() } else { Engine::default() };
    let prefs = load_prefs().unwrap_or_default();
    let mut options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("SoundCraft")
            .with_inner_size(sane_window_size(prefs.window_size))
            .with_maximized(prefs.window_maximized)
            .with_min_inner_size([960.0, 600.0])
            .with_drag_and_drop(true)
            .with_app_id("ai.storyteller.soundcraft"),
        ..Default::default()
    };
    if let Some(icon) = app_icon() {
        options.viewport = options.viewport.with_icon(icon);
    }
    // Before eframe creates the wgpu instance: default Windows to DirectX 12 with FXC (see graphics.rs).
    #[cfg(target_os = "windows")]
    graphics::configure(&mut options, eframe::wgpu::Backends::from_env(), eframe::wgpu::Dx12Compiler::from_env());
    eframe::run_native(
        "SoundCraft",
        options,
        Box::new(move |cc| {
            let player =
                if no_audio { None } else { Some(soundcraft_playback::Player::with_config(Arc::new(engine.session().clone()), &prefs.audio)) };
            let mut app = SoundApp::new(engine, player, services());
            app.autosave_dir = prefs_path().and_then(|p| p.parent().map(|d| d.join("Autosave")));
            app.leftovers = app.autosave_dir.as_deref().map(soundcraft_engine::autosave::leftovers).unwrap_or_default();
            app.preset_dir = prefs_path().and_then(|p| p.parent().map(|d| d.join("Presets")));
            app.extra.ui_scale = prefs.ui_scale;
            app.ui = prefs;
            if let Some(port) = control_port {
                let rx = control_server::start(port, cc.egui_ctx.clone());
                app = app.with_control(rx);
            }
            for f in &files {
                open_path(&mut app, f);
            }
            // If the system menu bar cannot be built, keep the egui menu bar rather than fail to start.
            #[cfg(target_os = "macos")]
            let menu = match native_menu::NativeMenu::new(&app, &cc.egui_ctx) {
                Ok(m) => {
                    app.native_menu_bar = true;
                    Some(m)
                }
                Err(e) => {
                    log::warn!("system menu bar unavailable, using the in-window menu bar: {e}");
                    None
                }
            };
            Ok(Box::new(App(
                app,
                logger,
                #[cfg(target_os = "macos")]
                menu,
            )))
        }),
    )
}

/// A saved window size, or the default when the prefs file holds nonsense (NaN, negative, tiny).
fn sane_window_size([w, h]: [f32; 2]) -> [f32; 2] {
    if w.is_finite() && h.is_finite() && w >= 960.0 && h >= 600.0 { [w.min(16_384.0), h.min(16_384.0)] } else { [1600.0, 1000.0] }
}

#[cfg(test)]
mod tests {
    use super::set_title;

    #[test]
    fn the_title_is_sent_only_when_it_changes() {
        let ctx = egui::Context::default();
        let titles = |title: &str| {
            let mut out = ctx.run_ui(egui::RawInput::default(), |ui| set_title(ui.ctx(), title.to_string()));
            out.textures_delta.clear();
            out.viewport_output.values().flat_map(|v| &v.commands).filter(|c| matches!(c, egui::ViewportCommand::Title(_))).count()
        };
        assert_eq!(titles("Song — SoundCraft"), 1);
        assert_eq!(titles("Song — SoundCraft"), 0);
        assert_eq!(titles("Song * — SoundCraft"), 1);
    }
}
