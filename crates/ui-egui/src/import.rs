//! Import Audio reads and decodes on a worker thread, so the app keeps drawing and stays usable
//! while a long file loads (issue #597). When the audio is ready it is handed to the engine
//! (`Engine::offer_import`) and `file.import_audio` runs as before, now only placing the clip:
//! undo, the journal, the CLI, MCP and the control channel see the same command, and calls from
//! the control channel still run synchronously.

use crate::SoundApp;
use serde_json::Value;
use std::collections::VecDeque;

/// Imports waiting and the one decoding now. One file decodes at a time: each can take gigabytes.
#[derive(Default)]
pub struct Imports {
    queue: VecDeque<Value>,
    #[cfg(not(target_arch = "wasm32"))]
    running: Option<Running>,
}

#[cfg(not(target_arch = "wasm32"))]
struct Running {
    params: Value,
    name: String,
    /// The session rate the audio is decoded for.
    rate: u32,
    rx: std::sync::mpsc::Receiver<Result<soundcraft_engine::io::DecodedImport, String>>,
}

impl Imports {
    /// Whether a file is decoding or waiting.
    pub fn busy(&self) -> bool {
        #[cfg(not(target_arch = "wasm32"))]
        if self.running.is_some() {
            return true;
        }
        !self.queue.is_empty()
    }

    /// The file decoding now and how many wait after it.
    fn label(&self) -> Option<(String, usize)> {
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(r) = &self.running {
            return Some((r.name.clone(), self.queue.len()));
        }
        None
    }
}

/// Imports the audio file `params["path"]` names (the `file.import_audio` params) in the
/// background. Anything else (several `paths`, no path, the browser) runs the command now.
pub fn start(app: &mut SoundApp, params: Value) {
    if cfg!(target_arch = "wasm32") || params.get("path").and_then(Value::as_str).is_none() {
        run_now(app, params);
        return;
    }
    app.imports.queue.push_back(params);
}

/// Starts the next import, and places a finished one. Called every frame from `logic`.
pub fn step(app: &mut SoundApp, ctx: &egui::Context) {
    #[cfg(not(target_arch = "wasm32"))]
    {
        finish(app);
        if app.imports.running.is_none()
            && let Some(params) = app.imports.queue.pop_front()
        {
            spawn(app, params, ctx);
        }
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = ctx;
        while let Some(params) = app.imports.queue.pop_front() {
            run_now(app, params);
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn spawn(app: &mut SoundApp, params: Value, ctx: &egui::Context) {
    let path = params.get("path").and_then(Value::as_str).unwrap_or_default().to_string();
    let name = std::path::Path::new(&path).file_name().map_or_else(|| path.clone(), |n| n.to_string_lossy().into_owned());
    let rate = app.engine.session().sample_rate.hz();
    let (tx, rx) = std::sync::mpsc::channel();
    let ctx = ctx.clone();
    let work = move || {
        let r = soundcraft_engine::io::decode_import(&path, rate).map_err(|e| e.to_string());
        let _ = tx.send(r);
        ctx.request_repaint();
    };
    match std::thread::Builder::new().name("audio-import".into()).spawn(work) {
        // No status line here: it may hold the previous file's failure, and the progress note names this file.
        Ok(_) => app.imports.running = Some(Running { params, name, rate, rx }),
        // No thread to spare: import the old way rather than not at all.
        Err(_) => {
            run_now(app, params);
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn finish(app: &mut SoundApp) {
    let Some(r) = &app.imports.running else { return };
    let done = match r.rx.try_recv() {
        Ok(done) => done,
        Err(std::sync::mpsc::TryRecvError::Empty) => return,
        Err(std::sync::mpsc::TryRecvError::Disconnected) => Err(format!("{}: the import stopped", r.name)),
    };
    let Some(Running { params, name, rate, .. }) = app.imports.running.take() else { return };
    // The session changed rate meanwhile (another session was opened): decode again for it.
    if rate != app.engine.session().sample_rate.hz() {
        app.imports.queue.push_front(params);
        return;
    }
    match done {
        Ok(decoded) => {
            app.engine.offer_import(decoded);
            if run_now(app, params) {
                app.ui.status = format!("Imported {name}");
            }
            app.engine.drop_offered_imports();
        }
        Err(e) => app.ui.status = e,
    }
}

/// Runs `file.import_audio` on this thread; the status line shows a failure. True on success.
fn run_now(app: &mut SoundApp, params: Value) -> bool {
    match app.run("file.import_audio", params) {
        Ok(_) => true,
        Err(e) => {
            app.ui.status = e;
            false
        }
    }
}

/// A small, non-blocking note while files import: the rest of the app stays usable.
pub fn show(app: &SoundApp, ctx: &egui::Context) {
    let Some((name, waiting)) = app.imports.label() else { return };
    egui::Area::new(egui::Id::unique("audio_import_progress")).anchor(egui::Align2::RIGHT_BOTTOM, egui::vec2(-16.0, -40.0)).interactable(false).show(
        ctx,
        |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.add(egui::Spinner::new());
                    let more = if waiting > 0 { format!(" ({waiting} more)") } else { String::new() };
                    ui.label(format!("Importing {name}{more}"));
                });
            });
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Services, SoundApp};
    use serde_json::json;

    fn tone_wav(name: &str) -> String {
        let dir = std::env::temp_dir().join(format!("sc-ui-import-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let tone: Vec<f32> = (0..9_600).map(|i| (i as f32 * 0.05).sin() * 0.2).collect();
        let buf = soundcraft_audio_io::AudioBuffer { sample_rate: 48_000, channels: vec![tone.clone(), tone] };
        let bytes = soundcraft_audio_io::encode(&buf, &soundcraft_audio_io::EncodeOptions::default()).unwrap();
        let path = dir.join(name);
        std::fs::write(&path, bytes).unwrap();
        path.to_string_lossy().into_owned()
    }

    fn settle(app: &mut SoundApp, ctx: &egui::Context) {
        for _ in 0..2_000 {
            step(app, ctx);
            if !app.imports.busy() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        panic!("import never finished");
    }

    /// Issue #597: starting an import returns at once; the clip lands, as one undo step, later.
    #[test]
    fn import_runs_in_the_background_and_places_one_undoable_clip() {
        let ctx = egui::Context::default();
        let mut app = SoundApp::new(soundcraft_engine::Engine::default(), None, Services::default());
        let wav = tone_wav("bg.wav");
        start(&mut app, json!({"path": wav, "at": 480}));
        assert!(app.imports.busy());
        assert!(app.engine.session().tracks.is_empty(), "nothing is placed before the audio is ready");
        settle(&mut app, &ctx);
        let s = app.engine.session();
        assert_eq!(s.tracks.len(), 1);
        assert_eq!(s.tracks[0].clips()[0].start, 480);
        assert_eq!(s.sources[0].frames, 9_600);
        assert!(app.ui.status.starts_with("Imported bg.wav"), "{}", app.ui.status);
        app.run("edit.undo", json!({})).unwrap();
        assert!(app.engine.session().tracks.is_empty());
    }

    #[test]
    fn imports_queue_and_a_bad_file_reports_without_stopping_the_rest() {
        let ctx = egui::Context::default();
        let mut app = SoundApp::new(soundcraft_engine::Engine::default(), None, Services::default());
        let junk = tone_wav("junk.wav");
        std::fs::write(&junk, b"not audio").unwrap();
        let good = tone_wav("good.wav");
        start(&mut app, json!({"path": junk}));
        start(&mut app, json!({"path": good}));
        step(&mut app, &ctx);
        assert_eq!(app.imports.label().map(|l| l.1), Some(1), "the second waits for the first");
        settle(&mut app, &ctx);
        assert_eq!(app.engine.session().tracks.len(), 1, "the good file still imports");
    }

    #[test]
    fn a_failed_import_stays_on_the_status_line_while_the_next_one_starts() {
        let ctx = egui::Context::default();
        let mut app = SoundApp::new(soundcraft_engine::Engine::default(), None, Services::default());
        let junk = tone_wav("junk-status.wav");
        std::fs::write(&junk, b"not audio").unwrap();
        start(&mut app, json!({"path": junk}));
        start(&mut app, json!({"path": tone_wav("next-status.wav")}));
        for _ in 0..2_000 {
            step(&mut app, &ctx);
            if app.imports.label().is_some_and(|(name, _)| name == "next-status.wav") {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(app.imports.label().is_some_and(|(name, _)| name == "next-status.wav"), "the second import never started");
        assert!(!app.ui.status.is_empty() && !app.ui.status.starts_with("Importing"), "the failure was overwritten: {}", app.ui.status);
        settle(&mut app, &ctx);
    }
}
