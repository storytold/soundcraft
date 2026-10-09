//! The SoundCraft command engine.
//!
//! Every user-visible action is a registered command ([`cmd::CommandSpec`]): menus, shortcuts, the
//! CLI, the JSON control channel and MCP all dispatch the same commands by id with JSON params.
//! The engine owns the [`Session`] behind an `Arc` (copy-on-write), so undo snapshots are cheap.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod catalog;
pub mod catalog_more;
pub mod clip_group_file;
pub mod cmd;
pub mod demo;
pub mod edit;
pub mod inspect;
pub mod io;
pub mod score;

use serde_json::{Value, json};
use soundcraft_model::{Clip, Session, TrackId};
use soundcraft_time::Samples;
use std::sync::Arc;

pub use cmd::{CommandInfo, CommandSpec, command_specs, find_command};
pub use soundcraft_model as model;

/// Shuts the third-party plugin hosts down, running the module exit of every plugin binary they
/// loaded. Call it at the end of `main`, once every plugin instance (mixer, player) is gone: a
/// plugin that has been used can crash when its module exit only runs from an exit handler.
pub fn shutdown_plugin_hosts() {
    soundcraft_vst3_host::shutdown();
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum EngineError {
    #[error("unknown command `{0}`")]
    UnknownCommand(String),
    #[error("`{0}` is not available: {1}")]
    Disabled(String, String),
    #[error("bad parameters for `{0}`: {1}")]
    BadParams(String, String),
    #[error("{0}")]
    NotFound(String),
    #[error("{0}")]
    Io(String),
    #[error("unsupported: {0}")]
    Unsupported(String),
    #[error("internal error in `{0}`: {1}")]
    Internal(String, String),
}

pub type Result<T> = std::result::Result<T, EngineError>;

/// Requests for the transport/audio engine, drained by the host app each frame.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub enum TransportRequest {
    Play,
    Stop,
    TogglePlay,
    Record,
    Locate(Samples),
    PlaySelection,
    Pause,
    HalfSpeed,
    Scrub(Samples),
    AllNotesOff,
}

/// Snapshot of the transport reported back by the host (playback position etc.).
#[derive(Debug, Clone, Copy, PartialEq, Default, serde::Serialize)]
pub struct TransportStatus {
    pub playing: bool,
    pub recording: bool,
    pub position: Samples,
}

/// Clipboard contents: per-track clip copies relative to the copied range start.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Clipboard {
    pub length: Samples,
    pub tracks: Vec<Vec<Clip>>,
    pub automation: Vec<Vec<soundcraft_model::AutomationLane>>,
    /// Copy Special › Clip Gain: per track, (offset from the copied range start, dB) breakpoints.
    pub clip_gain: Vec<Vec<(Samples, f32)>>,
    /// Copy Special › Clip Effects: clip-effect settings (`param` → value) of the first copied clip.
    pub clip_effects: Vec<(String, f64)>,
    /// Markers inside the copied range (starts relative to it), for Paste Special › Merge Markers.
    pub markers: Vec<soundcraft_model::MemoryLocation>,
}

#[derive(Debug, Clone)]
struct HistoryEntry {
    label: String,
    doc: Arc<Session>,
}

/// Engine state: the document, history, clipboard and transport plumbing.
pub struct Engine {
    doc: Arc<Session>,
    undo: Vec<HistoryEntry>,
    redo: Vec<HistoryEntry>,
    pub history_limit: usize,
    pub clipboard: Clipboard,
    pub transport_requests: Vec<TransportRequest>,
    pub transport: TransportStatus,
    /// Command journal (id, params) for macro recording / agents.
    pub journal: Vec<(String, Value)>,
    /// Path of the session file, if saved.
    pub path: Option<String>,
    /// Revision counter; bumps on every document change (UI caches key on it).
    pub revision: u64,
    dirty: bool,
    /// Gesture key for undo coalescing (see `execute_merged`).
    merge_key: Option<String>,
    /// Messages for the UI status line / agents.
    pub messages: Vec<String>,
}

impl Default for Engine {
    fn default() -> Self {
        Engine::new(Session::default())
    }
}

impl Engine {
    pub fn new(session: Session) -> Self {
        install_panic_hook();
        Engine {
            doc: Arc::new(session),
            undo: Vec::new(),
            redo: Vec::new(),
            history_limit: 200,
            clipboard: Clipboard::default(),
            transport_requests: Vec::new(),
            transport: TransportStatus::default(),
            journal: Vec::new(),
            path: None,
            revision: 1,
            dirty: false,
            merge_key: None,
            messages: Vec::new(),
        }
    }

    pub fn session(&self) -> &Session {
        &self.doc
    }

    pub fn session_arc(&self) -> Arc<Session> {
        Arc::clone(&self.doc)
    }

    /// Mutable access (copy-on-write). Callers outside commands should prefer `execute`.
    pub fn session_mut(&mut self) -> &mut Session {
        self.revision = self.revision.wrapping_add(1);
        self.dirty = true;
        Arc::make_mut(&mut self.doc)
    }

    /// Replace the whole document (open / new); clears history.
    pub fn replace_session(&mut self, s: Session) {
        self.doc = Arc::new(s);
        self.undo.clear();
        self.redo.clear();
        self.revision = self.revision.wrapping_add(1);
        self.dirty = false;
    }

    pub fn is_dirty(&self) -> bool {
        self.dirty
    }
    pub fn mark_clean(&mut self) {
        self.dirty = false;
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }
    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }
    pub fn undo_label(&self) -> Option<&str> {
        self.undo.last().map(|h| h.label.as_str())
    }
    pub fn redo_label(&self) -> Option<&str> {
        self.redo.last().map(|h| h.label.as_str())
    }
    pub fn undo_history(&self) -> Vec<String> {
        self.undo.iter().map(|h| h.label.clone()).collect()
    }

    pub fn undo(&mut self) -> bool {
        let Some(h) = self.undo.pop() else { return false };
        let cur = std::mem::replace(&mut self.doc, h.doc);
        self.redo.push(HistoryEntry { label: h.label, doc: cur });
        self.revision = self.revision.wrapping_add(1);
        self.dirty = true;
        true
    }

    pub fn redo(&mut self) -> bool {
        let Some(h) = self.redo.pop() else { return false };
        let cur = std::mem::replace(&mut self.doc, h.doc);
        self.undo.push(HistoryEntry { label: h.label, doc: cur });
        self.revision = self.revision.wrapping_add(1);
        self.dirty = true;
        true
    }

    /// Record an undo point manually (for interactive gestures committed by the UI).
    pub fn push_undo(&mut self, label: &str, before: Arc<Session>) {
        if Arc::ptr_eq(&before, &self.doc) {
            return;
        }
        self.undo.push(HistoryEntry { label: label.to_string(), doc: before });
        self.redo.clear();
        if self.undo.len() > self.history_limit {
            let excess = self.undo.len() - self.history_limit;
            self.undo.drain(..excess);
        }
    }

    /// Run a command by id. Never panics: escaped panics become `EngineError::Internal` and the
    /// document is restored.
    pub fn execute(&mut self, id: &str, params: &Value) -> Result<Value> {
        self.merge_key = None;
        let spec = find_command(id).ok_or_else(|| EngineError::UnknownCommand(id.to_string()))?;
        if let Err(reason) = (spec.enabled)(self) {
            // A call that names its targets (tracks, clips, a range) does not need a selection;
            // the command itself validates them.
            let names_targets = ["track", "tracks", "clip", "clips", "sources", "start", "end", "at"].iter().any(|k| params.get(k).is_some());
            if !names_targets {
                return Err(EngineError::Disabled(id.to_string(), reason));
            }
        }
        let before = Arc::clone(&self.doc);
        let rev = self.revision;
        let dirty = self.dirty;
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| (spec.run)(self, params)));
        let result = match result {
            Ok(r) => r,
            Err(p) => {
                let msg =
                    p.downcast_ref::<&str>().map(|s| s.to_string()).or_else(|| p.downcast_ref::<String>().cloned()).unwrap_or_else(|| "panic".into());
                self.doc = before;
                self.revision = rev.wrapping_add(1);
                self.dirty = dirty;
                return Err(EngineError::Internal(id.to_string(), msg));
            }
        };
        match result {
            Ok(v) => {
                if spec.undoable && !Arc::ptr_eq(&before, &self.doc) {
                    self.push_undo(spec.label, before);
                }
                if spec.journal {
                    self.journal.push((id.to_string(), params.clone()));
                }
                Ok(v)
            }
            Err(e) => {
                // Commands must not leave half-applied edits behind.
                if !Arc::ptr_eq(&before, &self.doc) {
                    self.doc = before;
                    self.revision = self.revision.wrapping_add(1);
                    self.dirty = dirty;
                }
                Err(e)
            }
        }
    }

    /// Execute as part of a continuous gesture (fader drag, knob turn, slider): consecutive calls
    /// with the same `key` collapse into a single undo step.
    pub fn execute_merged(&mut self, id: &str, params: &Value, key: &str) -> Result<Value> {
        let continuing = self.merge_key.as_deref() == Some(key);
        let before_len = self.undo.len();
        let r = self.execute(id, params)?;
        if continuing && self.undo.len() == before_len + 1 && before_len > 0 {
            // Drop the step just pushed; the earlier one still holds the pre-gesture document.
            self.undo.pop();
        }
        self.merge_key = Some(key.to_string());
        Ok(r)
    }

    /// End a gesture started with `execute_merged`.
    pub fn end_merge(&mut self) {
        self.merge_key = None;
    }

    /// Convenience: execute with `{}` params.
    pub fn run(&mut self, id: &str) -> Result<Value> {
        self.execute(id, &json!({}))
    }

    pub fn message(&mut self, m: impl Into<String>) {
        let m = m.into();
        log::info!("{m}");
        self.messages.push(m);
        if self.messages.len() > 100 {
            self.messages.remove(0);
        }
    }

    /// Tracks targeted by an edit: the selected tracks, or all tracks with playlists if none.
    pub fn target_tracks(&self) -> Vec<TrackId> {
        let s = self.session();
        if s.edit.selected_tracks.is_empty() {
            Vec::new()
        } else {
            s.tracks.iter().filter(|t| s.edit.selected_tracks.contains(&t.id)).map(|t| t.id).collect()
        }
    }
}

fn install_panic_hook() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let prev = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            log::error!("panic: {info}");
            prev(info);
        }));
    });
}

#[cfg(test)]
mod merge_tests {
    use super::*;

    #[test]
    fn merged_gesture_is_one_undo_step() {
        let mut e = Engine::default();
        let t = e.session_mut().add_track(soundcraft_model::TrackKind::Audio, soundcraft_model::ChannelFormat::Mono, None);
        for db in [-1.0, -2.0, -3.0, -4.0] {
            e.execute_merged("mix.volume", &json!({"track": t.0, "db": db}), "fader").unwrap();
        }
        e.end_merge();
        assert_eq!(e.undo_history().len(), 1);
        e.undo();
        assert_eq!(e.session().track(t).unwrap().mixer.volume_db, 0.0);
    }
}
