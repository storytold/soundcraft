//! The SoundCraft command engine.
//!
//! Every user-visible action is a registered command ([`cmd::CommandSpec`]): menus, shortcuts, the
//! CLI, the JSON control channel and MCP all dispatch the same commands by id with JSON params.
//! The engine owns the [`Session`] behind an `Arc` (copy-on-write), so undo snapshots are cheap.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod aaf;
pub mod autosave;
pub mod catalog;
pub mod catalog_more;
pub mod clip_group_file;
pub mod cmd;
pub mod demo;
pub mod edit;
pub mod file_access;
pub mod inspect;
pub mod io;
pub mod score;
#[cfg(test)]
#[cfg(test)]
mod test_scratch;

use serde_json::{Value, json};
use soundcraft_model::{Clip, Session, TrackId, ZoomState};
use soundcraft_time::Samples;
use std::cell::Cell;
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
    /// Stop the old session's transport without applying stop-time edits to the new document.
    ResetSession,
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
    /// How far the copied range started after a grid line; Relative Grid pastes keep it.
    pub grid_offset: Samples,
    /// Copy Special › Automation: pasting it leaves the clips alone, in Shuffle mode too.
    pub automation_only: bool,
}

impl Clipboard {
    /// An empty clipboard for material copied from `range`.
    pub fn of_range(s: &Session, range: soundcraft_time::Range) -> Clipboard {
        let line = s.edit.grid.line_at_or_before(range.start, s.sample_rate, &s.tempo, s.frame_rate);
        Clipboard { length: range.len(), grid_offset: range.start.saturating_sub(line), ..Clipboard::default() }
    }
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
    /// Revision counter; bumps on every document change but zoom/scroll (UI caches key on it).
    pub revision: u64,
    dirty: bool,
    /// The document as last opened or saved, for `is_dirty`.
    saved: Arc<Session>,
    /// `is_dirty` as of a revision.
    dirty_at: Cell<(u64, bool)>,
    /// Gesture key for undo coalescing (see `execute_merged`).
    merge_key: Option<String>,
    /// The key every merged gesture takes while held (see `hold_merge`).
    merge_hold: Option<String>,
    /// Messages for the UI status line / agents.
    pub messages: Vec<String>,
    /// The edit selection (range, tracks) before the last change, for Restore Last Selection.
    /// Kept out of the document so selecting never marks the session modified.
    pub last_selection: Option<(soundcraft_time::Range, Vec<TrackId>)>,
}

impl Default for Engine {
    fn default() -> Self {
        Engine::new(Session::default())
    }
}

impl Engine {
    pub fn new(session: Session) -> Self {
        install_panic_hook();
        let doc = Arc::new(session);
        Engine {
            saved: Arc::clone(&doc),
            doc,
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
            dirty_at: Cell::new((0, false)),
            merge_key: None,
            merge_hold: None,
            messages: Vec::new(),
            last_selection: None,
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

    /// Like `session_mut`, for `edit.zoom` only and without bumping `revision`.
    pub fn zoom_mut(&mut self) -> &mut ZoomState {
        self.dirty = true;
        &mut Arc::make_mut(&mut self.doc).edit.zoom
    }

    /// Replace the whole document (open / new); clears history.
    pub fn replace_session(&mut self, s: Session) {
        self.transport = TransportStatus { position: s.edit.selection.start.max(0), ..TransportStatus::default() };
        self.transport_requests.clear();
        self.transport_requests.push(TransportRequest::ResetSession);
        self.doc = Arc::new(s);
        self.saved = Arc::clone(&self.doc);
        self.undo.clear();
        self.redo.clear();
        self.last_selection = None;
        self.revision = self.revision.wrapping_add(1);
        self.dirty = false;
    }

    /// Whether the document changed since it was opened or saved. Selecting, locating and zooming
    /// don't count, and undoing back to the saved state makes it clean again.
    pub fn is_dirty(&self) -> bool {
        if !self.dirty {
            return false;
        }
        let (rev, dirty) = self.dirty_at.get();
        if rev == self.revision {
            return dirty;
        }
        let dirty = !same_document(&self.saved, &self.doc);
        self.dirty_at.set((self.revision, dirty));
        dirty
    }
    pub fn mark_clean(&mut self) {
        self.dirty = false;
        self.saved = Arc::clone(&self.doc);
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
        self.keep_edit_mode(&cur);
        self.redo.push(HistoryEntry { label: h.label, doc: cur });
        self.revision = self.revision.wrapping_add(1);
        self.dirty = true;
        true
    }

    pub fn redo(&mut self) -> bool {
        let Some(h) = self.redo.pop() else { return false };
        let cur = std::mem::replace(&mut self.doc, h.doc);
        self.keep_edit_mode(&cur);
        self.undo.push(HistoryEntry { label: h.label, doc: cur });
        self.revision = self.revision.wrapping_add(1);
        self.dirty = true;
        true
    }

    /// Undo and Redo never change the Edit mode or its settings (Shuffle Lock above all).
    fn keep_edit_mode(&mut self, from: &Session) {
        use edit::{RELATIVE_GRID, SHUFFLE_LOCK, SNAP_TO_GRID};
        let flags = [SNAP_TO_GRID, SHUFFLE_LOCK, RELATIVE_GRID];
        let ed = &self.doc.edit;
        if ed.edit_mode != from.edit.edit_mode || flags.iter().any(|f| ed.flag(f) != from.edit.flag(f)) {
            let ed = &mut Arc::make_mut(&mut self.doc).edit;
            ed.edit_mode = from.edit.edit_mode;
            for f in flags {
                ed.set_flag(f, from.edit.flag(f));
            }
        }
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

    /// Whether command `id` must wait for the take being recorded: nothing may replace the
    /// session or quit under it.
    pub fn blocked_while_recording(&self, id: &str) -> bool {
        self.transport.recording && matches!(id, "session.new" | "session.open" | "session.close" | "session.revert" | "app.quit")
    }

    /// Run a command by id. Never panics: escaped panics become `EngineError::Internal` and the
    /// document is restored.
    pub fn execute(&mut self, id: &str, params: &Value) -> Result<Value> {
        let spec = find_command(id).ok_or_else(|| EngineError::UnknownCommand(id.to_string()))?;
        if self.blocked_while_recording(id) {
            return Err(EngineError::Disabled(id.to_string(), "stop recording first".into()));
        }
        if let Err(reason) = (spec.enabled)(self) {
            // A call that names its targets (tracks, clips, a range) does not need a selection;
            // the command itself validates them.
            let names_targets = ["track", "tracks", "clip", "clips", "sources", "start", "end", "at"].iter().any(|k| params.get(k).is_some());
            if !names_targets {
                return Err(EngineError::Disabled(id.to_string(), reason));
            }
        }
        if spec.view {
            return self.execute_view(spec, params);
        }
        let before = Arc::clone(&self.doc);
        let rev = self.revision;
        let dirty = self.dirty;
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| (spec.run)(self, params)));
        let result = match result {
            Ok(r) => r,
            Err(p) => {
                self.doc = before;
                self.revision = rev.wrapping_add(1);
                self.dirty = dirty;
                return Err(EngineError::Internal(id.to_string(), panic_message(&*p)));
            }
        };
        if !Arc::ptr_eq(&before, &self.doc) {
            // Only a change to the document ends a gesture, so Stop doesn't split the pass it ends.
            self.merge_key = None;
        }
        match result {
            Ok(v) => {
                cmd::remember_selection(self, &before);
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

    fn execute_view(&mut self, spec: &CommandSpec, params: &Value) -> Result<Value> {
        let zoom = self.doc.edit.zoom;
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| (spec.run)(self, params)))
            .unwrap_or_else(|p| Err(EngineError::Internal(spec.id.to_string(), panic_message(&*p))));
        if result.is_err() && self.doc.edit.zoom != zoom {
            Arc::make_mut(&mut self.doc).edit.zoom = zoom;
        }
        result
    }

    /// Execute as part of a continuous gesture (fader drag, knob turn, slider): consecutive calls
    /// with the same `key` collapse into a single undo step.
    pub fn execute_merged(&mut self, id: &str, params: &Value, key: &str) -> Result<Value> {
        let key = self.merge_hold.clone().unwrap_or_else(|| key.to_string());
        let continuing = self.merge_key.as_ref() == Some(&key);
        let before_len = self.undo.len();
        // The step pushed for a continuing tick is dropped again below, so it must not trim the
        // oldest entry first when the history is full.
        let limit = self.history_limit;
        if continuing && before_len > 0 {
            self.history_limit = limit.max(before_len.saturating_add(1));
        }
        let r = self.execute(id, params);
        self.history_limit = limit;
        let r = r?;
        if continuing && self.undo.len() == before_len + 1 && before_len > 0 {
            // Drop the step just pushed; the earlier one still holds the pre-gesture document.
            self.undo.pop();
        }
        self.merge_key = Some(key);
        Ok(r)
    }

    /// While `key` is held, every `execute_merged` gesture takes it instead of its own, so they
    /// all join one undo step (an automation pass and whatever is dragged during it).
    pub fn hold_merge(&mut self, key: Option<&str>) {
        if self.merge_hold.as_deref() != key {
            self.merge_hold = key.map(str::to_string);
        }
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

/// Equal apart from what changes without editing: the selection, the playhead and the zoom.
fn same_document(saved: &Session, doc: &Session) -> bool {
    let mut doc = doc.clone();
    let (from, to) = (&saved.edit, &mut doc.edit);
    to.selection = from.selection;
    to.timeline_selection = from.timeline_selection;
    to.selected_tracks.clone_from(&from.selected_tracks);
    to.selected_clips.clone_from(&from.selected_clips);
    to.playhead = from.playhead;
    to.zoom = from.zoom;
    *saved == doc
}

fn install_panic_hook() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let prev = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            log::error!("panic: {info}\n{}", std::backtrace::Backtrace::force_capture());
            prev(info);
        }));
    });
}

fn panic_message(p: &(dyn std::any::Any + Send)) -> String {
    p.downcast_ref::<&str>().map(|s| s.to_string()).or_else(|| p.downcast_ref::<String>().cloned()).unwrap_or_else(|| "panic".into())
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

    #[test]
    fn a_held_merge_joins_other_gestures_into_its_step() {
        let mut e = Engine::default();
        let t = e.session_mut().add_track(soundcraft_model::TrackKind::Audio, soundcraft_model::ChannelFormat::Mono, None);
        e.hold_merge(Some("pass"));
        for db in [-1.0, -2.0, -3.0] {
            e.execute_merged("mix.volume", &json!({"track": t.0, "db": db}), "fader").unwrap();
            e.execute_merged("mix.pan", &json!({"track": t.0, "pan": db / 4.0}), "pan").unwrap();
        }
        assert_eq!(e.undo_history().len(), 1);
        e.hold_merge(None);
        e.execute_merged("mix.volume", &json!({"track": t.0, "db": -6.0}), "fader").unwrap();
        assert_eq!(e.undo_history().len(), 2);
    }

    #[test]
    fn only_a_change_to_the_document_ends_a_gesture() {
        let mut e = Engine::default();
        let t = e.session_mut().add_track(soundcraft_model::TrackKind::Audio, soundcraft_model::ChannelFormat::Mono, None);
        e.execute_merged("mix.volume", &json!({"track": t.0, "db": -1.0}), "fader").unwrap();
        e.execute("transport.stop", &json!({})).unwrap();
        e.execute_merged("mix.volume", &json!({"track": t.0, "db": -2.0}), "fader").unwrap();
        assert_eq!(e.undo_history().len(), 1);
        e.execute("mix.mute", &json!({"track": t.0})).unwrap();
        e.execute_merged("mix.volume", &json!({"track": t.0, "db": -3.0}), "fader").unwrap();
        assert_eq!(e.undo_history().len(), 3);
    }
}

#[cfg(test)]
mod dirty_tests {
    use super::*;

    #[test]
    fn selecting_locating_and_zooming_leave_the_session_clean() {
        let mut e = crate::demo::demo_engine();
        for (id, p) in [
            ("edit.select", json!({"tracks": ["Kick"], "start": 0, "end": 48_000})),
            ("edit.select_none", json!({})),
            ("transport.locate", json!({"at": 96_000})),
            ("view.zoom_in", json!({})),
            ("view.scroll", json!({"to": 48_000})),
        ] {
            e.execute(id, &p).unwrap();
            assert!(!e.is_dirty(), "{id}");
        }
        e.execute("mix.mute", &json!({"track": "Kick"})).unwrap();
        assert!(e.is_dirty());
        e.undo();
        assert!(!e.is_dirty());
    }
}
