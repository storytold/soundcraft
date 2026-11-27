use std::path::{Path, PathBuf};

/// Host services (file dialogs) injected by the app; absent in tests and on the web. They return
/// paths as picked: the caller converts one to text where a command takes it ([`crate::dialogs::unicode`]).
#[derive(Default)]
pub struct Services {
    pub pick_open: Option<Box<dyn Fn(&str, &[&str]) -> Option<PathBuf>>>,
    /// Like `pick_open`, but the user may select several files (Import Audio). Empty when cancelled.
    pub pick_open_many: Option<Box<dyn Fn(&str) -> Vec<PathBuf>>>,
    /// The purpose and the file to start from.
    pub pick_save: Option<Box<dyn Fn(&str, &Path) -> Option<PathBuf>>>,
    /// The folder to start in.
    pub pick_folder: Option<Box<dyn Fn(&Path) -> Option<PathBuf>>>,
    /// The app binary's last step as the app leaves, run once at the end of its `on_exit`. On
    /// macOS every quit ends the process inside AppKit's terminate, after eframe's `on_exit`, so
    /// code after `eframe::run_native` never runs there.
    pub at_exit: Option<AtExitFn>,
}

/// Runs once when the app leaves ([`Services::at_exit`]).
pub type AtExitFn = Box<dyn FnOnce()>;

impl Services {
    /// Run [`Services::at_exit`] at most once: a second exit, or a hook that already ran from the
    /// app's own exit path, does nothing.
    pub fn run_exit_hook(&mut self) {
        if let Some(at_exit) = self.at_exit.take() {
            at_exit();
        }
    }
}
