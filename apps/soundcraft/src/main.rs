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

#[cfg(test)]
mod build_channel;
mod build_flavor;
mod config;
mod control_server;
mod env;
#[cfg(any(target_os = "windows", test))]
mod graphics;
mod launch_args;
mod launch_profile;
mod logging;
#[cfg(target_os = "macos")]
mod native_menu;
mod nightly_session;
mod process_identity;
mod profile_root;
mod switch;

use env::Env;
use soundcraft_engine::Engine;
use soundcraft_ui_egui::{Services, SoundApp, UiState, dialogs, os_window};
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
            save_prefs(&self.2, &self.0.ui);
        }
        // With the system menu bar there is no egui bar showing the window and dirty state, so the
        // title carries them; elsewhere the egui menu bar already does.
        let app_name = os_window::name_for(os_window::identity());
        let document = (self.0.engine.session().name.as_str(), self.0.engine.is_dirty());
        let title = if self.0.native_menu_bar {
            let section = if self.0.ui.window == soundcraft_ui_egui::MainWindow::Edit { "Edit" } else { "Mix" };
            soundcraft_ui_egui::os_title_with_section(app_name, Some(document), section)
        } else {
            soundcraft_ui_egui::os_title(app_name, Some(document.0))
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
        // returns: release the player's plugin instances and shut plugin hosting down here, and
        // start the helper of an update that waits for the app to close.
        self.0.player = None;
        soundcraft_engine::shutdown_plugin_hosts();
        self.0.start_pending_update();
        // The binary's last step: on Nightly, finishing the session-health record. macOS ends the
        // process from inside AppKit's terminate, so this is that channel's only exit path.
        self.0.services.run_exit_hook();
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
                "file.import_aaf" => d.add_filter("AAF session data", &["aaf"]),
                "file.import_midi" => d.add_filter("MIDI", &["mid", "midi", "smf"]),
                _ => d.add_filter("Audio", craft_audio::DECODE_EXTENSIONS),
            };
            d.pick_file()
        })),
        pick_open_many: Some(Box::new(|_purpose: &str| {
            rfd::FileDialog::new().add_filter("Audio", craft_audio::DECODE_EXTENSIONS).pick_files().unwrap_or_default()
        })),
        pick_save: Some(Box::new(|purpose: &str, path: &std::path::Path| {
            let mut d = match purpose {
                "file.bounce_mix" => rfd::FileDialog::new().add_filter("Audio", craft_audio::ENCODE_EXTENSIONS),
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
        ..Services::default()
    }
}

/// The window, Dock, taskbar and Alt-Tab icon. macOS gets Apple's icon grid (transparent margin);
/// elsewhere the full tile. Regenerate with `packaging/icons.sh`.
fn app_icon() -> Option<egui::IconData> {
    let nightly = build_flavor::COMPILED == build_flavor::BuildFlavor::NightlyV1;
    #[cfg(target_os = "macos")]
    let png: &[u8] = if nightly {
        include_bytes!("../../../assets/app-icon/soundcraft-nightly-macos-512.png")
    } else {
        include_bytes!("../../../assets/app-icon/soundcraft-macos-512.png")
    };
    #[cfg(not(target_os = "macos"))]
    let png: &[u8] = if nightly {
        include_bytes!("../../../assets/app-icon/hicolor/256x256/apps/ai.storyteller.soundcraft-nightly.png")
    } else {
        include_bytes!("../../../assets/app-icon/hicolor/256x256/apps/ai.storyteller.soundcraft.png")
    };
    eframe::icon_data::from_png_bytes(png).map_err(|e| log::warn!("app icon: {e}")).ok()
}

/// Set the immutable Windows AUMID for the Nightly flavor before any window or COM service.
fn initialize_nightly_process_identity() -> Result<(), process_identity::IdentityError> {
    process_identity::initialize(
        build_flavor::COMPILED == build_flavor::BuildFlavor::NightlyV1,
        os_window::id_for(os_window::LaunchIdentity::NightlyV1),
    )
}

/// Report a refused launch profile on standard error, and in a dialog on a desktop session: a
/// Nightly build started from a file manager has no console to read.
fn report_launch_profile_error(error: &str) {
    eprintln!("SoundCraft launch profile: {error}");
    let desktop_session = cfg!(windows)
        || cfg!(target_os = "macos")
        || (cfg!(target_os = "linux") && ["DISPLAY", "WAYLAND_DISPLAY"].into_iter().any(|key| std::env::var_os(key).is_some_and(|v| !v.is_empty())));
    if build_flavor::needs_native_error(build_flavor::COMPILED, desktop_session) {
        if let Err(error) = initialize_nightly_process_identity() {
            eprintln!("SoundCraft launch profile: {error}");
            return;
        }
        #[cfg(any(windows, target_os = "linux", target_os = "macos"))]
        let _ = rfd::MessageDialog::new()
            .set_title("SoundCraft Nightly")
            .set_description(error)
            .set_level(rfd::MessageLevel::Error)
            .set_buttons(rfd::MessageButtons::Ok)
            .show();
    }
}

/// Select the immutable launch profile before any start-up service or side effect. `None` means
/// this process is done: a version query already answered, or a refused profile (exit status 2).
fn prepare_launch() -> Option<Vec<std::ffi::OsString>> {
    // The trusted Launcher protocol is decided before the logger, updates, fonts or saved state.
    let argv: Vec<_> = std::env::args_os().skip(1).collect();
    // Plain `--version`, or the Launcher's `--launch-profile nightly-v1 --profile-root <dir>
    // --version`: answered before root validation, process identity, IPC or any window, as
    // `SoundCraft <version> (nightly-v1)` on standard output with nothing on standard error.
    if build_flavor::COMPILED == build_flavor::BuildFlavor::NightlyV1 && (build_flavor::version_query(&argv) || launch_profile::version_query(&argv))
    {
        use std::io::Write as _;
        let _ = writeln!(std::io::stdout(), "SoundCraft {}", build_flavor::version(build_flavor::COMPILED, env!("SOUNDCRAFT_BUILT_VERSION")));
        return None;
    }
    let early = launch_profile::parse(argv)
        .and_then(|launch| {
            build_flavor::require_profile(build_flavor::COMPILED, &launch.profile)?;
            initialize_nightly_process_identity().map_err(|error| error.to_string())?;
            Ok(launch)
        })
        .and_then(|launch| {
            launch_profile::prepare(launch, |root| {
                launch_profile::validate_root(root, |key| std::env::var_os(key), std::env::current_exe().ok().as_deref())
            })
        })
        .unwrap_or_else(|error| {
            report_launch_profile_error(&error);
            std::process::exit(2)
        });
    let identity = if early.profile.is_nightly() { os_window::LaunchIdentity::NightlyV1 } else { os_window::LaunchIdentity::Stable };
    if let Err(error) = launch_profile::install(early.profile).and_then(|()| os_window::select_identity(identity)) {
        report_launch_profile_error(&error);
        std::process::exit(2);
    }
    Some(early.args)
}

fn main() -> eframe::Result {
    let Some(arguments) = prepare_launch() else { return Ok(()) };
    // The compiled identity, so About and diagnostics report the Nightly feed version verbatim.
    soundcraft_ui_egui::set_build_version(env!("SOUNDCRAFT_BUILT_VERSION").to_owned());
    // Windows: the update helper is a copy of this program started with `--finish-update`.
    // Nightly has no updater of its own: ArtCraft Launcher carries the next build.
    #[cfg(windows)]
    if !launch_profile::current().is_nightly()
        && let Some(result) = craft_update::self_update::finish(&soundcraft_ui_egui::UPDATE_APP, &std::env::args_os().skip(1).collect::<Vec<_>>())
    {
        std::process::exit(i32::from(result.is_err()));
    }
    // First, so every start-up record (and the engine's panic hook, installed with the first
    // Engine) is captured; see `logging`.
    let logger = logging::install(env::rust_log().as_deref());
    // Help, version and usage errors print and exit here, before a window or a log file exists.
    let launch = launch_args::parse(launch_args::command_for_profile(launch_profile::current().is_nightly()), arguments).unwrap_or_else(|e| e.exit());
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
            .with_title(os_window::name_for(os_window::identity()))
            .with_inner_size(sane_window_size(prefs.window_size))
            .with_maximized(prefs.window_maximized)
            .with_min_inner_size([960.0, 600.0])
            .with_drag_and_drop(true)
            .with_app_id(os_window::app_id()),
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
    // Nightly session health: an owned lease plus a record removed only by a clean exit, so the
    // next start can say when the previous one did not finish. Stable and a secondary window keep
    // their existing behaviour and touch nothing. No document names or paths are collected.
    let (nightly_session, session_health_warning) =
        match nightly_session::Session::begin_with_health(launch_profile::current().root(), env!("SOUNDCRAFT_BUILT_VERSION"), None) {
            Ok(state) => state,
            Err(error) => {
                log::warn!("Nightly session health is unavailable: {error}");
                (None, true)
            }
        };
    let previous_nightly_unclean = nightly_session.as_ref().is_some_and(nightly_session::Session::previous_unclean);
    // Set once a window has posted the interruption notice; until then the record must survive.
    let unclean_notice_shown = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let notice_shown_in_callback = unclean_notice_shown.clone();
    let nightly_session: nightly_session::Shared = std::sync::Arc::new(std::sync::Mutex::new(nightly_session));
    // The app's exit hook is the controlled return on macOS, where every quit ends the process
    // inside AppKit's terminate and `run_native` never returns.
    #[cfg(target_os = "macos")]
    let finish_at_exit = {
        let shared = nightly_session.clone();
        let shown = unclean_notice_shown.clone();
        move || nightly_session::finish_once(&shared, shown.load(std::sync::atomic::Ordering::Relaxed))
    };
    let result = eframe::run_native(
        os_window::name_for(os_window::identity()),
        options,
        Box::new(move |cc| {
            let player =
                if no_audio { None } else { Some(soundcraft_playback::Player::with_config(Arc::new(engine.session().clone()), &prefs.audio)) };
            let mut app = SoundApp::new(engine, player, services());
            // Nightly is updated by ArtCraft Launcher only, never from inside the app.
            app.set_startup_update_check(!launch_profile::current().is_nightly());
            app.set_system_fonts(env.system_fonts());
            app.autosave_dir = env.config_dir.as_ref().map(|d| d.join("Autosave"));
            app.leftovers = app.autosave_dir.as_deref().map(soundcraft_engine::autosave::leftovers).unwrap_or_default();
            app.preset_dir = env.config_dir.as_ref().map(|d| d.join("Presets"));
            app.extra.ui_scale = prefs.ui_scale;
            app.ui = prefs;
            // The Nightly window has no control server: no inherited port, no stable token file.
            if let Some(port) = control_port.filter(|_| !launch_profile::current().is_nightly()) {
                let rx = control_server::start(port, cc.egui_ctx.clone());
                app = app.with_control(rx);
            }
            if previous_nightly_unclean || session_health_warning {
                app.ui.status = if previous_nightly_unclean {
                    "The previous SoundCraft Nightly session did not finish normally. You can report a problem from About. No diagnostics have been sent.".to_owned()
                } else {
                    "SoundCraft Nightly could not record session health in its private profile; abnormal-exit detection is unavailable.".to_owned()
                };
                notice_shown_in_callback.store(previous_nightly_unclean, std::sync::atomic::Ordering::Relaxed);
            }
            #[cfg(target_os = "macos")]
            {
                app.services.at_exit = Some(Box::new(finish_at_exit));
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
    );
    // A controlled return, even for a handled start-up error. On macOS a window that was created
    // has already finished in its exit hook, so this is then a no-op.
    nightly_session::finish_once(&nightly_session, unclean_notice_shown.load(std::sync::atomic::Ordering::Relaxed));
    result
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
