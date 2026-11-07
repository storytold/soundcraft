//! SoundCraft desktop app.
//!
//! Usage: `soundcraft [--demo] [--control <port>] [--no-audio] [session.scraft | audio files…]`; `soundcraft --help` lists the flags and `SOUNDCRAFT_CONTROL_PORT`.
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
mod env;
#[cfg(any(target_os = "windows", test))]
mod graphics;
mod launch_args;
mod logging;
#[cfg(target_os = "macos")]
mod native_menu;
mod switch;

use env::Env;
use soundcraft_audio_io::FileFormat;
use soundcraft_engine::Engine;
use soundcraft_ui_egui::{Services, SoundApp, UiState, dialogs};
use std::sync::Arc;

struct App(SoundApp, Option<&'static logging::AppLogger>, Env, #[cfg(target_os = "macos")] Option<native_menu::NativeMenu>);

/// Sends the title only when it changes, remembered here as X11 and Wayland can't read it back.
fn set_title(ctx: &egui::Context, title: String) {
    let id = egui::Id::unique("window_title");
    if ctx.data(|d| d.get_temp::<String>(id)).as_deref() != Some(title.as_str()) {
        ctx.data_mut(|d| d.insert_temp(id, title.clone()));
        ctx.send_viewport_cmd(egui::ViewportCommand::Title(title));
    }
}

/// Closes the window after SoundCraft's Quit, and keeps it open while unsaved changes are asked
/// about. On macOS a close that goes ahead is canceled and the app quits through
/// `-[NSApplication terminate:]`, which still runs `on_exit`. Returns whether the app quits that way.
/// On a Mac with a Touch Bar, AppKit crashes after eframe drops the window (storytold/craft-libs#3).
fn close_window(app: &mut SoundApp, ctx: &egui::Context) -> bool {
    if app.quit_requested {
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
    } else if ctx.input(|i| i.viewport().close_requested()) && dialogs::ask_to_save(app, "app.quit", &serde_json::json!({})) {
        ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
    }
    craft_quit::on_close_request(ctx)
}

impl eframe::App for App {
    /// Per-frame app logic: report audio-thread logs, run the shared app logic with the macOS menu driven around it,
    /// then handle dropped paths, quit/close requests, window geometry, recent-list persistence and the window title.
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // What the realtime audio thread logged is written here, off that thread.
        if let Some(logger) = self.1 {
            logger.report_audio_thread();
        }
        #[cfg(target_os = "macos")]
        if let Some(menu) = &mut self.3 {
            menu.process(&mut self.0);
        }
        self.0.guarded(|app| app.logic(ctx));
        #[cfg(target_os = "macos")]
        if let Some(menu) = &mut self.3 {
            menu.sync(&self.0);
        }
        // Files dropped on the window: audio on an instrument track or a Sampler window loads
        // the sampler; otherwise sessions open and audio/MIDI import.
        for f in self.0.take_dropped_files() {
            if !f.path().as_os_str().is_empty() {
                dialogs::open_path(&mut self.0, f.path());
            }
        }
        close_window(&mut self.0, ctx);
        soundcraft_ui_egui::fit_window_to_screen(ctx);
        let (inner, maximized) = ctx.input(|i| (i.viewport().inner_rect, i.viewport().maximized.unwrap_or(false)));
        self.0.ui.window_maximized = maximized;
        if let Some(r) = inner.filter(|_| !maximized) {
            self.0.ui.window_size = (r.size() * ctx.zoom_factor()).round().into();
        }
        self.0.ui.ui_scale = self.0.extra.ui_scale;
        // Saved now rather than at exit: the recent list matters most after a crash.
        if self.0.take_recent_changed() {
            save_prefs(&self.2, &self.0.ui);
        }
        // With the system menu bar there is no egui bar showing the window and dirty state, so the
        // title carries them; elsewhere the egui menu bar already does.
        let title = if self.0.native_menu_bar {
            format!(
                "{}{} — SoundCraft — {}",
                self.0.engine.session().name,
                if self.0.engine.is_dirty() { " *" } else { "" },
                if self.0.ui.window == soundcraft_ui_egui::MainWindow::Edit { "Edit" } else { "Mix" }
            )
        } else {
            format!("{} — SoundCraft", self.0.engine.session().name)
        };
        set_title(ctx, title);
    }
    /// Forward raw input to the shared app hook so scripted gestures and host-key protection behave like the desktop
    /// shell.
    fn raw_input_hook(&mut self, ctx: &egui::Context, raw: &mut egui::RawInput) {
        self.0.raw_input_hook(ctx, raw);
    }
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.0.guarded(|app| app.ui(ui));
    }
    fn on_exit(&mut self) {
        self.0.close_autosave();
        save_prefs(&self.2, &self.0.ui);
        if let Some(logger) = self.1 {
            logger.report_audio_thread();
        }
        // The system Quit (⌘Q) ends the process from inside the event loop, so `run_native` never
        // returns: release the player's plugin instances and shut plugin hosting down here.
        self.0.player = None;
        soundcraft_engine::shutdown_plugin_hosts();
    }
}

fn prefs_path(env: &Env) -> Option<std::path::PathBuf> {
    env.config_dir.as_ref().map(|b| b.join("ui.json"))
}

/// Runs without preferences (`SOUNDCRAFT_NO_PREFS`, agents' test runs) neither read nor write them.
fn load_prefs(env: &Env) -> Option<UiState> {
    if !env.prefs_enabled() {
        return None;
    }
    let path = prefs_path(env)?;
    match config::load_ui(&path) {
        Ok(state) => state,
        Err(error) => {
            log::warn!("{error}");
            None
        }
    }
}

fn save_prefs(env: &Env, ui: &UiState) {
    if !env.prefs_enabled() {
        return;
    }
    if let Some(path) = prefs_path(env)
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
                "session.open" => d.add_filter("SoundCraft Session", &["scraft"]),
                "file.import_midi" => d.add_filter("MIDI", &["mid", "midi", "smf"]),
                _ => d.add_filter("Audio", &FileFormat::readable_extensions()),
            };
            d.pick_file()
        })),
        pick_save: Some(Box::new(|purpose: &str, path: &std::path::Path| {
            let mut d = match purpose {
                "file.bounce_mix" => rfd::FileDialog::new().add_filter("Audio", &FileFormat::writable_extensions()),
                _ => rfd::FileDialog::new().add_filter("SoundCraft Session", &["scraft"]),
            };
            if let Some(dir) = path.parent().filter(|dir| dir.is_dir()) {
                d = d.set_directory(dir);
            }
            if let Some(n) = path.file_name().and_then(std::ffi::OsStr::to_str) {
                d = d.set_file_name(n);
            }
            d.save_file()
        })),
        pick_folder: Some(Box::new(|start: &std::path::Path| rfd::FileDialog::new().set_directory(start).pick_folder())),
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
    // Windows: the update helper is a copy of this program started with `--finish-update`.
    #[cfg(windows)]
    if let Some(result) = craft_update::self_update::finish(&soundcraft_ui_egui::UPDATE_APP, &std::env::args_os().skip(1).collect::<Vec<_>>()) {
        std::process::exit(i32::from(result.is_err()));
    }
    // First, so every start-up record (and the engine's panic hook, installed with the first
    // Engine) is captured; see `logging`.
    let logger = logging::install(env::rust_log().as_deref());
    // Help, version and usage errors print and exit here, before a window or a log file exists.
    let launch = launch_args::parse(launch_args::command(), std::env::args_os().skip(1)).unwrap_or_else(|e| e.exit());
    // After the arguments and the logger: reading resolves the settings folder, which creates it,
    // and a malformed switch is logged.
    let env = Env::read();
    let (demo, no_audio, control_port) = (launch.demo, launch.no_audio, launch.control_port);
    let files = launch.files;
    log::info!("SoundCraft {}", soundcraft_automation::args::long_version());
    // The log file lives in the settings directory; opened after the arguments, so `--version`
    // leaves no file behind. Records logged until now are written to it first. Runs without
    // preferences (agents' test runs) log to standard error only, so they don't rotate away the
    // user's own logs.
    if let Some(logger) = logger {
        match env.config_dir.as_deref() {
            _ if !env.prefs_enabled() => logger.no_file(),
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
    let prefs = load_prefs(&env).unwrap_or_default();
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
    #[cfg(all(unix, not(target_os = "macos")))]
    // winit has no file drag-and-drop on Wayland (only on X11), so dropping files from the file
    // manager showed a "no" cursor. Run through XWayland when it's there; SOUNDCRAFT_WAYLAND=1
    // keeps the native Wayland backend.
    if env.use_xwayland(std::env::var_os("DISPLAY").as_ref()) {
        options.event_loop_builder = Some(Box::new(|b| {
            use winit::platform::x11::EventLoopBuilderExtX11;
            b.with_x11();
        }));
    }
    eframe::run_native(
        "SoundCraft",
        options,
        Box::new(move |cc| {
            let player =
                if no_audio { None } else { Some(soundcraft_playback::Player::with_config(Arc::new(engine.session().clone()), &prefs.audio)) };
            let mut app = SoundApp::new(engine, player, services());
            app.set_startup_update_check(true);
            app.set_system_fonts(env.system_fonts());
            app.autosave_dir = env.config_dir.as_ref().map(|d| d.join("Autosave"));
            app.leftovers = app.autosave_dir.as_deref().map(soundcraft_engine::autosave::leftovers).unwrap_or_default();
            app.preset_dir = env.config_dir.as_ref().map(|d| d.join("Presets"));
            app.extra.ui_scale = prefs.ui_scale;
            app.ui = prefs;
            if let Some(port) = control_port {
                let rx = control_server::start(port, cc.egui_ctx.clone());
                app = app.with_control(rx);
            }
            for f in &files {
                dialogs::open_path(&mut app, f);
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
                env,
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
    use super::{close_window, set_title};
    use egui::ViewportCommand::{CancelClose, Close};
    use soundcraft_ui_egui::{Services, SoundApp, menus};

    fn demo_app() -> SoundApp {
        SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default())
    }

    /// Runs one frame of `close_window` in the root window, with or without a click on its close button.
    /// Returns whether the app quits through the platform, and the root window's commands from that frame.
    /// On macOS that quit is `terminate:` scheduled on the main queue, which a test binary never runs.
    fn close_frame(app: &mut SoundApp, close: bool) -> (bool, Vec<egui::ViewportCommand>) {
        let ctx = egui::Context::default();
        let mut input = egui::RawInput::default();
        if close {
            input.viewports.entry(egui::ViewportId::ROOT).or_default().events.push(egui::ViewportEvent::Close);
        }
        let mut quits = false;
        let mut out = ctx.run_ui(input, |ui| quits = close_window(app, ui.ctx()));
        out.textures_delta.clear();
        (quits, out.viewport_output.remove(&egui::ViewportId::ROOT).map(|o| o.commands).unwrap_or_default())
    }

    #[test]
    fn closing_a_saved_session_quits_through_the_platform_on_macos() {
        let mut app = demo_app();
        let (quits, commands) = close_frame(&mut app, true);
        assert!(app.dialogs.open.is_none());
        assert_eq!(quits, cfg!(target_os = "macos"));
        assert_eq!(commands.contains(&CancelClose), cfg!(target_os = "macos"), "on macOS the window stays for terminate: to close");
    }

    #[test]
    fn closing_with_unsaved_changes_asks_and_does_not_quit() {
        let mut app = demo_app();
        app.run("mix.mute", serde_json::json!({"track": "Kick"})).unwrap();
        let (quits, commands) = close_frame(&mut app, true);
        assert_eq!(app.dialogs.open_name(), Some("save_changes"));
        assert!(!quits);
        assert!(commands.contains(&CancelClose));
        assert!(!app.quit_requested);
    }

    #[test]
    fn quit_from_the_menu_closes_the_window_then_quits_through_the_platform_on_macos() {
        let mut app = demo_app();
        menus::invoke_menu(&mut app, "app.quit", "");
        assert!(app.quit_requested);
        let (quits, commands) = close_frame(&mut app, false);
        assert!(!quits);
        assert!(commands.contains(&Close));
        let (quits, commands) = close_frame(&mut app, true);
        assert_eq!(quits, cfg!(target_os = "macos"));
        assert_eq!(commands.contains(&CancelClose), cfg!(target_os = "macos"));
    }

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
