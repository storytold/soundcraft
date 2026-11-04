use super::{Dialog, SoundApp, Value, json};

/// When `id` would close a session with unsaved changes, ask first; true when the question is
/// open instead. A take in progress is stopped first, so it counts as a change.
pub fn ask_to_save(app: &mut SoundApp, id: &str, params: &Value) -> bool {
    if !matches!(id, "session.new" | "session.open" | "session.close" | "session.revert" | "app.quit" | OPEN_PENDING)
        || soundcraft_engine::find_command(id).is_some_and(|c| (c.enabled)(&app.engine).is_err())
    {
        return false;
    }
    if app.engine.transport.recording {
        app.stop_play();
    }
    if !app.engine.is_dirty() {
        return false;
    }
    app.dialogs.open = Some(Dialog::SaveChanges { then: (id.into(), params.clone()) });
    true
}

/// Runs `id`, unless it asks to save first (`ask_to_save`).
pub fn run_or_ask(app: &mut SoundApp, id: &str, params: Value) -> Result<Value, String> {
    if ask_to_save(app, id, &params) {
        return Ok(Value::Null);
    }
    app.run(id, params)
}

/// A path the engine needs as text (its commands take `"path": string`) that is not valid Unicode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotUnicode(pub std::path::PathBuf);

impl std::fmt::Display for NotUnicode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?} is not valid Unicode", self.0)
    }
}

impl std::error::Error for NotUnicode {}

/// `path` as the text the engine's commands take. Convert only where a command gets the path:
/// a lossy rendering would name another file.
pub fn unicode(path: &std::path::Path) -> Result<&str, NotUnicode> {
    path.to_str().ok_or_else(|| NotUnicode(path.to_path_buf()))
}

/// Opens a dropped, picked or command-line file by its extension: a session opens (asking to save
/// first), MIDI imports, and anything else imports as audio, decoded in the background so a long
/// file doesn't freeze the window (#597). A name that is not valid Unicode goes on the status line.
pub fn open_path(app: &mut SoundApp, path: &std::path::Path) {
    let p = match unicode(path) {
        Ok(p) => p,
        Err(e) => {
            log::warn!("{e}");
            app.ui.status = e.to_string();
            return;
        }
    };
    let lower = p.to_ascii_lowercase();
    let r = if lower.ends_with(".scraft") {
        run_or_ask(app, "session.open", json!({"path": p}))
    } else if lower.ends_with(".mid") || lower.ends_with(".midi") || lower.ends_with(".smf") {
        app.run("file.import_midi", json!({"path": p}))
    } else {
        let at = app.engine.session().edit.selection.start;
        crate::import::start(app, json!({"path": p, "at": at}));
        Ok(Value::Null)
    };
    if let Err(e) = r {
        log::warn!("{p}: {e}");
    }
}

/// What `SaveChanges` carries on with to open the session the browser read.
pub(super) const OPEN_PENDING: &str = "ui.open_pending_session";

/// Opens a session the browser read as bytes (picked or dropped), asking first when the open one
/// has changes. The read finishes later than the pick, so the question is asked only now.
pub fn open_session_bytes(app: &mut SoundApp, name: String, bytes: Vec<u8>) {
    if crate::bounce::refuses_session_switch(app) {
        return;
    }
    app.dialogs.pending_session = Some((name, bytes));
    if !ask_to_save(app, OPEN_PENDING, &json!({})) {
        open_pending_session(app);
    }
}

/// Open the session bytes read by the browser after the save prompt, stopping transport first. A successful open
/// marks the document replaced so pending captures cancel and reports any missing media.
pub(super) fn open_pending_session(app: &mut SoundApp) {
    let Some((name, bytes)) = app.dialogs.pending_session.take() else { return };
    if crate::bounce::refuses_session_switch(app) {
        return;
    }
    let _ = app.run("transport.stop", json!({}));
    app.ui.status = match soundcraft_engine::io::open_session_bytes(&mut app.engine, &name, &bytes) {
        Ok(missing) => {
            // This path replaces the document without going through `SoundApp::run`.
            app.document_replaced();
            if missing.is_empty() { format!("Opened {name}") } else { format!("Opened {name} ({} missing media)", missing.len()) }
        }
        Err(e) => e.to_string(),
    };
}

/// A path prompt for any command whose parameters start with a required `path` or `dir` (plus
/// Print Score, whose path is optional), with a default file name from the command.
pub fn takes_path(id: &str) -> bool {
    path_key(id).is_some()
}

fn path_key(id: &str) -> Option<&'static str> {
    let spec = soundcraft_engine::command_specs().iter().find(|c| c.id == id)?;
    if spec.params.starts_with("{path?") && id != "file.print_score" {
        None
    } else if spec.params.starts_with("{path") {
        Some("path")
    } else if spec.params.starts_with("{dir") {
        Some("dir")
    } else {
        None
    }
}

pub(super) fn path_dialog(id: &str, home: &str, name: &str) -> Option<Dialog> {
    let spec = soundcraft_engine::command_specs().iter().find(|c| c.id == id)?;
    let key = path_key(id)?;
    let importing = id.contains("import") || id.contains("open");
    let ext = match id {
        "file.export_midi" => ".mid",
        "file.export_sibelius" => ".musicxml",
        "file.print_score" => ".svg",
        "file.export_clip_groups" => ".scgrp",
        "file.export_session_text" => ".txt",
        _ if id.contains("session") => ".scraft",
        _ => "",
    };
    let path = if importing || key == "dir" { format!("{home}/") } else { format!("{home}/{name}{ext}") };
    let title = format!("{} — {}", spec.menu.last().copied().unwrap_or("File"), spec.label.trim_end_matches("..."));
    Some(Dialog::PathPrompt { cmd: id.into(), title, path, key: key.into() })
}

pub(super) fn default_dir() -> String {
    #[cfg(not(target_arch = "wasm32"))]
    {
        std::env::var("HOME").or_else(|_| std::env::var("USERPROFILE")).unwrap_or_else(|_| ".".into())
    }
    #[cfg(target_arch = "wasm32")]
    {
        String::new()
    }
}
