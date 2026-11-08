use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, TryRecvError};

/// A file dialog to show: [`Services::ask`].
#[derive(Clone, Debug)]
pub enum Pick {
    /// The purpose (a command id).
    Open(String),
    /// The purpose and the file to start from.
    Save(String, PathBuf),
    /// The folder to start in.
    Folder(PathBuf),
}

/// Shows a file dialog without waiting for it → the receiver of the path picked (`None`: cancelled).
/// `None` when no dialog could be shown.
pub type StartPick = Box<dyn Fn(Pick) -> Option<Receiver<Option<PathBuf>>>>;

/// Host services (file dialogs) injected by the app; absent in tests and on the web. They return
/// paths as picked: the caller converts one to text where a command takes it ([`crate::dialogs::unicode`]).
#[derive(Default)]
pub struct Services {
    pub pick_open: Option<Box<dyn Fn(&str, &[&str]) -> Option<PathBuf>>>,
    /// The purpose and the file to start from.
    pub pick_save: Option<Box<dyn Fn(&str, &Path) -> Option<PathBuf>>>,
    /// The folder to start in.
    pub pick_folder: Option<Box<dyn Fn(&Path) -> Option<PathBuf>>>,
    /// Where set, [`Self::ask`] shows dialogs with this and the answer arrives in a later frame.
    /// macOS needs it: a panel run modally inside the window's event handler gets the window events
    /// of its own resizing sent back into that handler, and winit aborts the app.
    pub start_pick: Option<StartPick>,
    /// (Hosts leave the next two to `..Default::default()`.) The dialog that is open, with the tag of what asked for it.
    pub pending: Option<(String, Receiver<Option<PathBuf>>)>,
    /// A path picked, for the `tag` that asked for it to [`Self::take`].
    pub picked: Option<(String, PathBuf)>,
}

impl Services {
    /// Shows `pick` for `tag` (one dialog at a time). The path picked is then [`Self::take`]n with
    /// the same tag: at once with the blocking pickers, in a later frame with `start_pick`.
    pub fn ask(&mut self, tag: &str, pick: Pick) {
        if self.pending.is_some() {
            return;
        }
        // A path nobody took must not answer this request.
        self.picked = None;
        if let Some(start) = &self.start_pick {
            if let Some(answer) = start(pick) {
                self.pending = Some((tag.to_string(), answer));
            }
            return;
        }
        let path = match &pick {
            Pick::Open(purpose) => self.pick_open.as_ref().and_then(|f| f(purpose, &[])),
            Pick::Save(purpose, start) => self.pick_save.as_ref().and_then(|f| f(purpose, start)),
            Pick::Folder(start) => self.pick_folder.as_ref().and_then(|f| f(start)),
        };
        self.picked = path.map(|p| (tag.to_string(), p));
    }

    /// Collects the answer of the open dialog, once it has one.
    pub fn poll(&mut self) {
        let Some((tag, answer)) = &self.pending else { return };
        match answer.try_recv() {
            Err(TryRecvError::Empty) => {}
            Ok(path) => {
                self.picked = path.map(|p| (tag.clone(), p));
                self.pending = None;
            }
            Err(TryRecvError::Disconnected) => self.pending = None,
        }
    }

    /// The path picked for `tag`, once.
    pub fn take(&mut self, tag: &str) -> Option<PathBuf> {
        if self.picked.as_ref().is_some_and(|(t, _)| t == tag) { self.picked.take().map(|(_, p)| p) } else { None }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::rc::Rc;
    use std::sync::mpsc;

    /// With `start_pick` (macOS sheets) the blocking pickers are never called, and the path is taken
    /// only once the dialog has answered.
    #[test]
    fn a_sheet_answers_later_and_never_blocks() {
        let blocked = Rc::new(Cell::new(false));
        let b = Rc::clone(&blocked);
        let (tx, rx) = mpsc::channel();
        let rx = Rc::new(Cell::new(Some(rx)));
        let mut services = Services {
            pick_open: Some(Box::new(move |_, _| {
                b.set(true);
                None
            })),
            start_pick: Some(Box::new(move |_| rx.take())),
            ..Default::default()
        };
        services.ask("a", Pick::Open("session.open".into()));
        services.ask("b", Pick::Open("session.open".into()));
        services.poll();
        assert_eq!(services.take("a"), None, "no answer yet");
        tx.send(Some(PathBuf::from("/tmp/x.scraft"))).unwrap();
        services.poll();
        assert_eq!(services.take("b"), None, "another tag's answer is not taken");
        assert_eq!(services.take("a"), Some(PathBuf::from("/tmp/x.scraft")));
        assert_eq!(services.take("a"), None, "taken once");
        assert!(!blocked.get(), "the blocking picker was never called");
        services.picked = Some(("a".into(), PathBuf::from("/tmp/old")));
        services.ask("a", Pick::Open("session.open".into()));
        assert_eq!(services.take("a"), None, "a new request drops an untaken path");
    }

    /// Without `start_pick` the blocking picker answers at once.
    #[test]
    fn the_blocking_picker_answers_at_once() {
        let mut services = Services { pick_folder: Some(Box::new(|_| Some(PathBuf::from("/tmp")))), ..Default::default() };
        services.ask("dir", Pick::Folder(PathBuf::from("/")));
        assert_eq!(services.take("dir"), Some(PathBuf::from("/tmp")));
    }
}
