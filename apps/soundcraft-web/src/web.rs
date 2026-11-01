//! The browser shell: the eframe web runner around [`SoundApp`].

use soundcraft_audio_io::FileFormat;
use soundcraft_engine::{Engine, io};
use soundcraft_ui_egui::dialogs::Dialog;
use soundcraft_ui_egui::sampler_window::SampleRead;
use soundcraft_ui_egui::{Services, SoundApp};
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use wasm_bindgen::JsCast as _;

const CANVAS_ID: &str = "soundcraft_canvas";
const LOADING_ID: &str = "soundcraft_loading";

pub fn start() {
    eframe::WebLogger::init(log::LevelFilter::Info).ok();
    io::set_deliver(download);
    wasm_bindgen_futures::spawn_local(async {
        let Some(document) = web_sys::window().and_then(|w| w.document()) else {
            log::error!("no document");
            return;
        };
        let Some(canvas) = document.get_element_by_id(CANVAS_ID).and_then(|e| e.dyn_into::<web_sys::HtmlCanvasElement>().ok()) else {
            log::error!("missing <canvas id=\"{CANVAS_ID}\">");
            return;
        };
        let mut options = eframe::WebOptions::default();
        if query().contains("webgl")
            && let eframe::egui_wgpu::WgpuSetup::CreateNew(create) = &mut options.wgpu_options.wgpu_setup
        {
            create.instance_descriptor.backends = eframe::wgpu::Backends::GL;
        }
        let result = eframe::WebRunner::new()
            .start(
                canvas,
                options,
                Box::new(move |cc| {
                    if let Some(rs) = &cc.wgpu_render_state {
                        log::info!("soundcraft-web: wgpu backend {:?}", rs.adapter.get_info().backend);
                    }
                    let engine = if query().contains("empty") { Engine::default() } else { soundcraft_engine::demo::demo_engine() };
                    let player = soundcraft_playback::Player::new(Arc::new(engine.session().clone()));
                    let app = SoundApp::new(engine, Some(player), Services::default());
                    Ok(Box::new(WebShell { app, inbox: Rc::default(), samples: Rc::default() }))
                }),
            )
            .await;
        if let Some(el) = document.get_element_by_id(LOADING_ID) {
            match result {
                Ok(()) => el.remove(),
                Err(e) => el.set_inner_html(&format!("<p>SoundCraft failed to start: {e:?}</p><p>A browser with WebGPU or WebGL2 is required.</p>")),
            }
        }
    });
}

fn query() -> String {
    web_sys::window().and_then(|w| w.location().search().ok()).unwrap_or_default()
}

/// What a file read from the browser is for.
#[derive(Debug, Clone, Copy)]
enum Purpose {
    Session,
    Audio,
    Midi,
}

const SESSION_EXTS: &[&str] = &[io::BUNDLE_EXTENSION, soundcraft_model::SESSION_EXTENSION];
const MIDI_EXTS: &[&str] = &["mid", "midi", "smf"];

impl Purpose {
    /// A dropped file goes where its extension says: sessions open, MIDI and audio import.
    fn of(name: &str) -> Self {
        let ext = name.rsplit_once('.').map(|(_, x)| x.to_ascii_lowercase()).unwrap_or_default();
        if SESSION_EXTS.contains(&ext.as_str()) {
            Self::Session
        } else if MIDI_EXTS.contains(&ext.as_str()) {
            Self::Midi
        } else {
            Self::Audio
        }
    }

    /// The command whose path prompt this replaces in the browser.
    fn for_command(cmd: &str) -> Option<Self> {
        match cmd {
            "session.open" => Some(Self::Session),
            "file.import_audio" => Some(Self::Audio),
            "file.import_midi" => Some(Self::Midi),
            _ => None,
        }
    }

    fn filter(self) -> (&'static str, Vec<&'static str>) {
        match self {
            Self::Session => ("SoundCraft Session", SESSION_EXTS.to_vec()),
            Self::Audio => ("Audio", FileFormat::readable_extensions()),
            Self::Midi => ("MIDI", MIDI_EXTS.to_vec()),
        }
    }
}

/// Files picked or dropped, read asynchronously (browsers can only read files that way) and
/// handled on the next frame. The browser has no paths: files travel as bytes.
type Inbox = Rc<RefCell<Vec<(Purpose, String, Vec<u8>)>>>;

/// Audio dropped on an instrument track or a Sampler window, its bytes read, waiting for the
/// next frame to load into that track's sampler.
type SampleInbox = Rc<RefCell<Vec<(SampleRead, Result<Vec<u8>, String>)>>>;

struct WebShell {
    app: SoundApp,
    inbox: Inbox,
    samples: SampleInbox,
}

impl WebShell {
    /// File > Open Session… / Import > Audio… / MIDI… would show a text box for a path, which the
    /// browser can't read (#63, #64). Show the browser's file picker instead.
    fn take_path_prompt(&mut self, ctx: &egui::Context) {
        let purpose = match &self.app.dialogs.open {
            Some(Dialog::PathPrompt { cmd, .. }) => Purpose::for_command(cmd),
            _ => None,
        };
        let Some(purpose) = purpose else { return };
        self.app.dialogs.open = None;
        let (inbox, ctx) = (self.inbox.clone(), ctx.clone());
        wasm_bindgen_futures::spawn_local(async move {
            let (label, exts) = purpose.filter();
            let Some(file) = rfd::AsyncFileDialog::new().add_filter(label, &exts).pick_file().await else { return };
            let bytes = file.read().await;
            inbox.borrow_mut().push((purpose, file.file_name(), bytes));
            ctx.request_repaint();
        });
    }

    /// Read files dropped on the browser window asynchronously: audio for an instrument track or a Sampler goes to
    /// that sampler, while other drops are classified by extension and handled on a later frame.
    fn take_drops(&mut self, ctx: &egui::Context) {
        // In the browser a dropped file's `path()` is only its name: read its bytes instead.
        // Audio dropped on an instrument track or a Sampler window goes to its sampler instead.
        let dropped = self.app.take_dropped_files();
        for read in self.app.take_sample_reads() {
            let (samples, ctx) = (self.samples.clone(), ctx.clone());
            wasm_bindgen_futures::spawn_local(async move {
                let bytes = read.file.bytes_async().await;
                samples.borrow_mut().push((read, bytes));
                ctx.request_repaint();
            });
        }
        for f in dropped {
            let name = f.path().file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "Audio.wav".into());
            let (inbox, ctx) = (self.inbox.clone(), ctx.clone());
            wasm_bindgen_futures::spawn_local(async move {
                match f.bytes_async().await {
                    Ok(bytes) => {
                        inbox.borrow_mut().push((Purpose::of(&name), name, bytes));
                        ctx.request_repaint();
                    }
                    Err(e) => log::error!("couldn't read dropped file {name}: {e}"),
                }
            });
        }
    }

    fn handle(&mut self, purpose: Purpose, name: String, bytes: Vec<u8>) {
        let e = &mut self.app.engine;
        let at = e.session().edit.selection.start.max(0);
        let result = match purpose {
            // Asks first when the open session has changes: the bytes wait for the answer.
            Purpose::Session => return soundcraft_ui_egui::dialogs::open_session_bytes(&mut self.app, name, bytes),
            // Imports are edits: one undo step each, like the import commands.
            Purpose::Audio => {
                let before = e.session_arc();
                io::import_audio_bytes(e, &name, &bytes, None, None, at).map(|_| {
                    e.push_undo("Import Audio", before);
                    format!("Imported {name}")
                })
            }
            Purpose::Midi => {
                let before = e.session_arc();
                io::import_midi_bytes(e, &bytes, at, true).map(|_| {
                    e.push_undo("Import MIDI", before);
                    format!("Imported {name}")
                })
            }
        };
        self.app.ui.status = result.unwrap_or_else(|err| err.to_string());
    }
}

impl eframe::App for WebShell {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.take_drops(ctx);
        self.take_path_prompt(ctx);
        let pending = std::mem::take(&mut *self.inbox.borrow_mut());
        for (purpose, name, bytes) in pending {
            self.handle(purpose, name, bytes);
        }
        let samples = std::mem::take(&mut *self.samples.borrow_mut());
        for (read, bytes) in samples {
            self.app.load_sample_bytes(&read, bytes);
        }
        self.app.logic(ctx);
        // A prompt opened during this frame becomes a picker on the next one.
        self.take_path_prompt(ctx);
    }

    /// Forward raw input to the shared app hook so scripted gestures and host-key protection behave like the desktop
    /// shell.
    fn raw_input_hook(&mut self, ctx: &egui::Context, raw: &mut egui::RawInput) {
        self.app.raw_input_hook(ctx, raw);
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.app.ui(ui);
    }
}

/// Hand a saved or exported file to the person as a download named `name` (the engine's delivery
/// hook on the web).
fn download(name: &str, bytes: &[u8]) -> Result<(), String> {
    let js = |e: wasm_bindgen::JsValue| format!("download failed: {e:?}");
    let window = web_sys::window().ok_or("no window")?;
    let document = window.document().ok_or("no document")?;
    let parts = js_sys::Array::of1(&js_sys::Uint8Array::from(bytes));
    let opts = web_sys::BlobPropertyBag::new();
    let ext = name.rsplit_once('.').map(|(_, x)| x.to_ascii_lowercase());
    opts.set_type(match ext.as_deref() {
        Some("scraftz") => "application/zip",
        Some("mid" | "midi") => "audio/midi",
        Some("svg") => "image/svg+xml",
        Some("txt") => "text/plain",
        Some("json" | "scraft") => "application/json",
        Some("musicxml") => "application/vnd.recordare.musicxml+xml",
        ext => ext.and_then(FileFormat::from_extension).map_or("application/octet-stream", FileFormat::mime_type),
    });
    let blob = web_sys::Blob::new_with_u8_array_sequence_and_options(&parts, &opts).map_err(js)?;
    let url = web_sys::Url::create_object_url_with_blob(&blob).map_err(js)?;
    let a: web_sys::HtmlAnchorElement = document.create_element("a").map_err(js)?.dyn_into().map_err(|_| "not an anchor")?;
    a.set_href(&url);
    a.set_download(name);
    a.style().set_property("display", "none").map_err(js)?;
    document.body().ok_or("no body")?.append_child(&a).map_err(js)?;
    a.click();
    a.remove();
    // Revoke after the click was dispatched; the download keeps its own reference.
    let revoke = wasm_bindgen::closure::Closure::once_into_js(move || {
        let _ = web_sys::Url::revoke_object_url(&url);
    });
    window.set_timeout_with_callback_and_timeout_and_arguments_0(revoke.unchecked_ref(), 10_000).map_err(js)?;
    Ok(())
}
