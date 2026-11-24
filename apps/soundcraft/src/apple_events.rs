//! macOS: sessions and media opened from Finder (double-click, Open With, a drop on the Dock icon)
//! or with `open -a SoundCraft file.scraft`.
//!
//! macOS doesn't put these paths on the command line. LaunchServices sends the app a
//! `kAEOpenDocuments` ('odoc') Apple event. winit 0.30 owns the application delegate and doesn't
//! answer it, so AppKit hands the files to `NSDocumentController`, which fails with "The document
//! … could not be opened. SoundCraft cannot open files in the "SoundCraft Session" format." when
//! the double-click starts SoundCraft, and does nothing when SoundCraft is already running.
//!
//! Handling the event takes Objective-C class declarations, i.e. `unsafe`, which this workspace
//! forbids. The `fmv-macos-events` crate (PhotoCraft, PdfCraft, EffectCraft and DesignCraft use the
//! same pinned version) wraps exactly that behind a safe main-thread queue: an
//! `NSAppleEventManager` handler registered before the launch event arrives, leaving winit's
//! delegate alone. The app drains the queue every frame ([`AppleEvents::take`]) and opens each path
//! the way a file dropped on the window opens ([`apply`]).
//!
//! The crate answers the 'quit' Apple event too (the Dock's Quit, logging out). [`apply`] runs it
//! as SoundCraft's own Quit: unsaved changes are asked about first (a take still recording is
//! stopped). AppKit's handler ended the process at once and left the changes to autosave recovery.
//! A hidden SoundCraft (⌘H) is unhidden when either event arrives, so that it can come to the front
//! to ask; with nothing unsaved, the window shows for a moment before the app quits.

use std::path::PathBuf;

use fmv_macos_events::{Event, Inbox, Registration};
use objc2::MainThreadMarker;
use objc2_app_kit::NSApplication;
use serde_json::json;
use soundcraft_ui_egui::{SoundApp, dialogs};

/// Keeps the Apple-event handlers registered; hold it until the event loop returns.
pub(crate) struct AppleEvents {
    _registration: Registration,
    inbox: Inbox,
}

impl AppleEvents {
    /// Register the handlers. Call on the main thread before the event loop starts, so the event
    /// that launched the app (a Finder double-click) is caught too.
    pub(crate) fn install() -> Self {
        let (registration, inbox) = Registration::install();
        Self { _registration: registration, inbox }
    }

    /// Repaint `ctx` when an event arrives while the app runs, so the next frame takes it.
    pub(crate) fn connect(&self, ctx: &egui::Context) {
        let ctx = ctx.clone();
        self.inbox.set_wake(move || {
            // A hidden app (⌘H) that only asks for focus comes back without becoming the active
            // app: the save question of a Dock Quit sat behind whatever the user was in.
            if let Some(mtm) = MainThreadMarker::new() {
                let app = NSApplication::sharedApplication(mtm);
                if app.isHidden() {
                    app.unhideWithoutActivation();
                }
            }
            ctx.request_repaint();
        });
    }

    /// What arrived since the last call.
    pub(crate) fn take(&self) -> Requests {
        Requests::from_events(self.inbox.drain())
    }
}

/// What macOS asked of the app since the last frame.
#[derive(Debug, Default, PartialEq)]
pub(crate) struct Requests {
    /// Files to open, in the order they arrived.
    pub(crate) open: Vec<PathBuf>,
    /// The Dock's Quit or a log-out.
    pub(crate) quit: bool,
}

impl Requests {
    fn from_events(events: Vec<Event>) -> Self {
        let mut requests = Self::default();
        for event in events {
            match event {
                Event::Open(paths) => requests.open.extend(paths),
                Event::Quit => requests.quit = true,
            }
        }
        requests
    }
}

/// Open the requested files like files dropped on the window (a session replaces the open one,
/// asking first when it has changes; audio and MIDI import), then quit if asked, the way the Quit
/// menu item does. When that asks to save, the window comes to the front, so the question isn't
/// left behind other apps. (A hidden SoundCraft was already unhidden when the request arrived, see
/// [`AppleEvents::connect`].)
pub(crate) fn apply(app: &mut SoundApp, ctx: &egui::Context, requests: Requests) {
    for path in requests.open.iter().filter(|p| !p.as_os_str().is_empty()) {
        dialogs::open_path(app, path);
    }
    if requests.quit {
        if let Err(e) = dialogs::run_or_ask(app, "app.quit", json!({})) {
            log::warn!("quit: {e}");
        }
        if !app.quit_requested {
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use soundcraft_engine::Engine;

    /// A scratch folder of this test's own; created exclusively, so a folder left there (someone
    /// else's) can't redirect the writes through a link inside it.
    fn scratch(name: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.subsec_nanos());
        let dir = std::env::temp_dir().join(format!("soundcraft-apple-events-{name}-{}-{nanos}", std::process::id()));
        std::fs::create_dir(&dir).unwrap();
        dir
    }

    #[test]
    fn events_become_requests_in_arrival_order() {
        let events =
            vec![Event::Open(vec![PathBuf::from("/a/Song.scraft")]), Event::Open(vec![PathBuf::from("/a/kick.wav"), PathBuf::from("/a/groove.mid")])];
        let requests = Requests::from_events(events);
        assert_eq!(requests.open, [PathBuf::from("/a/Song.scraft"), PathBuf::from("/a/kick.wav"), PathBuf::from("/a/groove.mid")]);
        assert!(!requests.quit);
        assert!(Requests::from_events(vec![Event::Quit]).quit);
        assert_eq!(Requests::from_events(Vec::new()), Requests::default());
    }

    /// The Finder double-click: the session named by the event opens, as if dropped on the window.
    #[test]
    fn an_opened_session_replaces_the_untitled_one() {
        let dir = scratch("open");
        let path = dir.join("Finder Song.scraft");
        let mut saved = Engine::default();
        saved.execute("track.new", &json!({"count": 3, "format": "mono"})).unwrap();
        saved.execute("session.save_as", &json!({"path": path})).unwrap();

        let mut app = SoundApp::new(Engine::default(), None, soundcraft_ui_egui::Services::default());
        apply(&mut app, &egui::Context::default(), Requests::from_events(vec![Event::Open(vec![path.clone()])]));
        assert_eq!(app.engine.path.as_deref(), path.to_str());
        assert_eq!(app.engine.session().tracks.len(), 3);
        assert!(!app.quit_requested);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A missing file is reported, and an empty path (a URL without one) is skipped; neither quits.
    #[test]
    fn a_missing_file_leaves_the_session_alone() {
        let mut app = SoundApp::new(Engine::default(), None, soundcraft_ui_egui::Services::default());
        apply(&mut app, &egui::Context::default(), Requests { open: vec![PathBuf::new(), PathBuf::from("/nonexistent/Gone.scraft")], quit: false });
        assert!(app.engine.path.is_none());
        assert!(app.engine.session().tracks.is_empty());
        assert!(!app.quit_requested);
    }

    /// Runs `requests` in a frame; whether the window was brought to the front.
    fn focused(app: &mut SoundApp, requests: Requests) -> bool {
        let ctx = egui::Context::default();
        let mut requests = Some(requests);
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
            if let Some(r) = requests.take() {
                apply(app, ui.ctx(), r);
            }
        });
        out.textures_delta.clear();
        out.viewport_output.values().flat_map(|v| &v.commands).any(|c| matches!(c, egui::ViewportCommand::Focus))
    }

    /// The Dock's Quit with nothing unsaved quits, like the Quit menu item.
    #[test]
    fn quit_without_changes_quits() {
        let mut app = SoundApp::new(Engine::default(), None, soundcraft_ui_egui::Services::default());
        assert!(!focused(&mut app, Requests::from_events(vec![Event::Quit])));
        assert!(app.quit_requested);
        assert!(app.dialogs.open.is_none());
    }

    /// With unsaved changes it asks first, in front, and doesn't quit yet: quitting without the
    /// question would also drop the changes' autosave, which the Quit menu item only does after
    /// "Don't Save".
    #[test]
    fn quit_with_changes_asks_to_save_in_front() {
        let mut engine = Engine::default();
        engine.execute("track.new", &json!({"count": 1, "format": "mono"})).unwrap();
        assert!(engine.is_dirty());
        let mut app = SoundApp::new(engine, None, soundcraft_ui_egui::Services::default());
        assert!(focused(&mut app, Requests::from_events(vec![Event::Quit])));
        assert!(!app.quit_requested);
        assert!(matches!(&app.dialogs.open, Some(soundcraft_ui_egui::dialogs::Dialog::SaveChanges { then }) if then.0 == "app.quit"));
    }
}
