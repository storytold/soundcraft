//! macOS file dialogs as sheets. Shown in line, the panel's modal loop runs inside winit's event
//! handler; an event winit held back meanwhile (a resize of the panel ending) re-enters the handler
//! and winit's panic there, inside an AppKit callback, aborts the app. A sheet returns at once and
//! the window keeps running; a thread of its own waits for the answer.

use std::path::PathBuf;
use std::sync::{Arc, mpsc};

use soundcraft_ui_egui::{Pick, StartPick};

type Answer = std::pin::Pin<Box<dyn std::future::Future<Output = Option<PathBuf>> + Send>>;

/// `Services::start_pick`: shows `pick` as a sheet on `window`; `ui` is woken when it answers.
pub fn start_pick(window: Arc<winit::window::Window>, ui: egui::Context) -> StartPick {
    Box::new(move |pick| {
        let (sheet_tx, sheet_rx) = mpsc::channel::<Answer>();
        let (tx, rx) = mpsc::channel();
        let ui = ui.clone();
        // The thread first, so the answer of a sheet once shown always has somewhere to go.
        std::thread::Builder::new()
            .name("file-dialog".into())
            .spawn(move || {
                if let Ok(answer) = sheet_rx.recv() {
                    // The app gone meanwhile has no use for the answer.
                    let _ = tx.send(pollster::block_on(answer));
                    ui.request_repaint();
                }
            })
            .ok()?;
        // rfd builds the panel and begins the sheet here, on the main thread (as it must).
        let dialog = rfd::AsyncFileDialog::new().set_parent(&*window);
        let path = |f: rfd::FileHandle| f.path().to_path_buf();
        let answer: Answer = match pick {
            Pick::Open(purpose) => {
                let (name, exts) = crate::open_filter(&purpose);
                let picked = dialog.add_filter(name, &exts).pick_file();
                Box::pin(async move { picked.await.map(path) })
            }
            Pick::Save(purpose, start) => {
                let (name, exts) = crate::save_filter(&purpose);
                let mut dialog = dialog.add_filter(name, &exts);
                if let Some(dir) = start.parent().filter(|dir| dir.is_dir()) {
                    dialog = dialog.set_directory(dir);
                }
                if let Some(n) = start.file_name().and_then(std::ffi::OsStr::to_str) {
                    dialog = dialog.set_file_name(n);
                }
                let picked = dialog.save_file();
                Box::pin(async move { picked.await.map(path) })
            }
            Pick::Folder(start) => {
                let picked = dialog.set_directory(start).pick_folder();
                Box::pin(async move { picked.await.map(path) })
            }
        };
        let _ = sheet_tx.send(answer);
        Some(rx)
    })
}
