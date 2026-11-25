//! Software updates, background check at startup, and startup prompt modal.

use crate::SoundApp;

#[cfg(not(target_arch = "wasm32"))]
use craft_update::dialog::{CheckResultDialog, CheckResultResponse};
#[cfg(not(target_arch = "wasm32"))]
use craft_update::updater::{Action, Status, Updater};

pub(crate) struct Runtime {
    /// Whether this launch may touch the suite updater at all: Nightly is installed, updated and
    /// removed by ArtCraft Launcher, so the check, its dialogs, the restart helper and the menu
    /// entry are all inert there (a Nightly user must never be handed a stable release).
    stable_services: bool,
    #[cfg(not(target_arch = "wasm32"))]
    updater: Updater,
    /// The desktop app turns the check at start on; tests and the web build leave it off.
    #[cfg(not(target_arch = "wasm32"))]
    startup_check: bool,
    #[cfg(not(target_arch = "wasm32"))]
    check_requested: bool,
    /// The check-result dialog (native) or the web build's notice is showing.
    manual_window_open: bool,
    #[cfg(not(target_arch = "wasm32"))]
    result_dialog: CheckResultDialog,
}

impl Runtime {
    /// The updater and its windows are for a Stable launch only.
    fn enabled(&self) -> bool {
        self.stable_services
    }
}

impl Default for Runtime {
    fn default() -> Self {
        Self {
            // The launch identity is selected before any UI state exists (main.rs `prepare_launch`).
            stable_services: crate::os_window::stable_services_allowed(),
            #[cfg(not(target_arch = "wasm32"))]
            updater: Updater::new(&crate::UPDATE_APP, env!("CARGO_PKG_VERSION")),
            #[cfg(not(target_arch = "wasm32"))]
            startup_check: false,
            #[cfg(not(target_arch = "wasm32"))]
            check_requested: false,
            manual_window_open: false,
            #[cfg(not(target_arch = "wasm32"))]
            result_dialog: CheckResultDialog::new(&crate::UPDATE_APP, env!("CARGO_PKG_VERSION")),
        }
    }
}

impl SoundApp {
    /// Whether the app asks for a newer release when it starts (off until the desktop app turns it on).
    #[cfg(not(target_arch = "wasm32"))]
    pub fn set_startup_update_check(&mut self, on: bool) {
        self.updates_rt.startup_check = on;
    }

    /// Starts the helper of an update that waits for the app to close. Call it when the exit is certain.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn start_pending_update(&mut self) {
        if self.updates_rt.enabled() {
            self.updates_rt.updater.start_pending_update();
        }
    }

    /// Whether this launch may check for, offer or install a release at all: only Stable may.
    pub(crate) fn update_services_allowed(&self) -> bool {
        self.updates_rt.enabled()
    }
}

#[cfg(test)]
impl SoundApp {
    /// Tests select the launch the fields would carry: `main.rs` decides it once, before the UI
    /// state exists, and never changes it afterwards.
    pub(crate) fn set_update_services_for_test(&mut self, allowed: bool) {
        self.updates_rt.stable_services = allowed;
    }
}

/// Whether the result dialog shows: while the updater reports something (a requested check, or what the last update
/// came to, which the check at start reads), and not while the startup prompt is open.
#[cfg(not(target_arch = "wasm32"))]
fn shows_result(status: &Status, prompt_open: bool) -> bool {
    !matches!(status, Status::Idle) && !prompt_open
}

pub(crate) fn check(app: &mut SoundApp) {
    if !app.update_services_allowed() {
        return;
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        app.updates_rt.check_requested = true;
    }
    #[cfg(target_arch = "wasm32")]
    {
        app.updates_rt.manual_window_open = true;
    }
}

pub(crate) fn tick(app: &mut SoundApp, ctx: &egui::Context) {
    if !app.update_services_allowed() {
        return;
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let rt = &mut app.updates_rt;
        if std::mem::take(&mut rt.check_requested) {
            rt.updater.check_now(ctx);
        } else if rt.startup_check {
            rt.updater.check_at_start(ctx);
        }
        rt.updater.tick(ctx);
    }
    #[cfg(target_arch = "wasm32")]
    let _ = (app, ctx);
}

pub(crate) fn show(app: &mut SoundApp, ctx: &egui::Context) {
    if !app.update_services_allowed() {
        return;
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        // The restart helper is scheduled before the app closes, so no update starts over unsaved work or a take being recorded.
        let clean = !app.has_unsaved_changes() && !app.engine.transport.recording;
        match app.updates_rt.updater.show_if(ctx, clean) {
            Action::OpenUrl(url) => ctx.open_url(egui::OpenUrl::new_tab(url)),
            Action::Close => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
            Action::None => {}
        }
        // Help ▸ Check for Updates, and the first start after an update, show their outcome in the shared result dialog
        // until the user closes it. A found update goes to the startup prompt, which takes over from the dialog.
        let rt = &mut app.updates_rt;
        rt.manual_window_open = shows_result(rt.updater.status(), rt.updater.prompt_open());
        if rt.manual_window_open {
            match rt.result_dialog.show(ctx, rt.updater.status()) {
                CheckResultResponse::Close => rt.manual_window_open = false,
                CheckResultResponse::CheckAgain => rt.updater.check_now(ctx),
                CheckResultResponse::RestartToUpdate => rt.updater.restart_to_update(),
                CheckResultResponse::None => {}
            }
        }
        // Closing the dialog, or the prompt taking over, dismisses the outcome it showed.
        if !rt.manual_window_open {
            rt.updater.dismiss_status();
        }
    }

    #[cfg(target_arch = "wasm32")]
    if app.updates_rt.manual_window_open {
        let mut open = true;
        egui::Window::new(crate::localization::text("Check for Updates"))
            .id(egui::Id::unique("Check for Updates"))
            .open(&mut open)
            .resizable(false)
            .collapsible(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .show(ctx, |ui| {
                ui.label(crate::localization::text("Update check is not supported in the web browser."));
            });
        app.updates_rt.manual_window_open = open;
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;
    use crate::Services;
    use craft_update::Paths;
    use std::sync::Arc;

    /// One pass of the update check and its windows, with `events` as the frame's input, dropping texture deltas the
    /// tests do not inspect.
    fn frame(app: &mut SoundApp, ctx: &egui::Context, events: Vec<egui::Event>) {
        let raw = egui::RawInput { events, ..Default::default() };
        let mut out = ctx.run_ui(raw, |ui| {
            tick(app, ui.ctx());
            show(app, ui.ctx());
        });
        out.textures_delta.clear();
    }

    #[test]
    fn the_result_dialog_shows_every_reported_status_until_the_prompt_opens() {
        for status in [
            Status::Checking,
            Status::UpToDate,
            Status::Failed("offline".into()),
            Status::Installing,
            Status::Ready,
            Status::Managed,
            Status::Updated("0.2.0".into()),
            Status::InstallFailed("disk full".into()),
        ] {
            assert!(shows_result(&status, false), "{status:?}");
            assert!(!shows_result(&status, true), "{status:?} with the prompt open");
        }
        assert!(!shows_result(&Status::Idle, false));
    }

    /// Nightly is installed and updated by ArtCraft Launcher alone, so its launch never asks the
    /// feed, never opens an update window and never starts a restart helper: a Nightly user cannot
    /// be handed a stable release. A Stable launch of the same app still checks, so the gate is
    /// what stops it.
    #[test]
    fn a_nightly_launch_never_checks_for_or_offers_a_release() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let dir = std::env::temp_dir().join(format!("soundcraft-update-nightly-{}", std::process::id()));
        let asked = Arc::new(AtomicUsize::new(0));
        let mut app = SoundApp::new(soundcraft_engine::Engine::default(), None, Services::default());
        app.set_update_services_for_test(false);
        let counted = asked.clone();
        app.updates_rt.updater = Updater::new(&crate::UPDATE_APP, "0.20.1")
            .with_source(Arc::new(move || {
                counted.fetch_add(1, Ordering::Relaxed);
                Ok(None)
            }))
            .with_pins_path(dir.join("pins.json"))
            .with_paths(Paths::under(&dir));
        std::fs::create_dir_all(Paths::under(&dir).data).unwrap();
        app.set_startup_update_check(true);
        let ctx = egui::Context::default();
        // Help ▸ Check for Updates, the automation command and the startup check all go through here.
        check(&mut app);
        assert!(!app.updates_rt.check_requested, "a Nightly launch must not queue a check");
        for _ in 0..3 {
            frame(&mut app, &ctx, Vec::new());
        }
        assert_eq!(asked.load(Ordering::Relaxed), 0, "a Nightly launch must not ask the update feed");
        assert_eq!(app.updates_rt.updater.status(), &Status::Idle);
        assert!(!app.updates_rt.manual_window_open);
        app.start_pending_update();
        assert_eq!(app.updates_rt.updater.status(), &Status::Idle);

        // The same frames on a Stable launch do reach the feed, so the assertions above are the gate.
        let mut stable = SoundApp::new(soundcraft_engine::Engine::default(), None, Services::default());
        let counted = asked.clone();
        stable.updates_rt.updater = Updater::new(&crate::UPDATE_APP, "0.20.1")
            .with_source(Arc::new(move || {
                counted.fetch_add(1, Ordering::Relaxed);
                Ok(None)
            }))
            .with_pins_path(dir.join("pins.json"))
            .with_paths(Paths::under(&dir));
        stable.set_startup_update_check(true);
        // The check runs on a worker of its own, so give it a bounded moment rather than one frame.
        for _ in 0..200 {
            frame(&mut stable, &ctx, Vec::new());
            if asked.load(Ordering::Relaxed) > 0 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(asked.load(Ordering::Relaxed) > 0, "a Stable launch still checks for updates");
        let _ = std::fs::remove_dir_all(dir);
    }

    /// The first start after an update says so in the result dialog, which stays until the user closes it.
    #[test]
    fn the_first_start_after_an_update_reports_it_until_closed() {
        let dir = std::env::temp_dir().join(format!("soundcraft-update-result-{}", std::process::id()));
        let paths = Paths::under(&dir);
        std::fs::create_dir_all(&paths.data).unwrap();
        std::fs::write(paths.data.join("self-update-done.txt"), "v0.2.0").unwrap();
        let mut app = SoundApp::new(soundcraft_engine::Engine::default(), None, Services::default());
        app.updates_rt.updater =
            Updater::new(&crate::UPDATE_APP, "0.2.0").with_source(Arc::new(|| Ok(None))).with_pins_path(dir.join("pins.json")).with_paths(paths);
        app.set_startup_update_check(true);
        let ctx = egui::Context::default();
        for _ in 0..3 {
            frame(&mut app, &ctx, Vec::new());
        }
        assert_eq!(app.updates_rt.updater.status(), &Status::Updated("0.2.0".into()));
        assert!(app.updates_rt.manual_window_open);
        let escape = egui::Event::Key { key: egui::Key::Escape, physical_key: None, pressed: true, repeat: false, modifiers: egui::Modifiers::NONE };
        frame(&mut app, &ctx, vec![escape]);
        assert_eq!(app.updates_rt.updater.status(), &Status::Idle);
        assert!(!app.updates_rt.manual_window_open);
        let _ = std::fs::remove_dir_all(dir);
    }
}
