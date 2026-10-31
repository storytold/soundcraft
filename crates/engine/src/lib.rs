//! The SoundCraft command engine.
//!
//! Every user-visible action is a registered command ([`cmd::CommandSpec`]): menus, shortcuts, the
//! CLI, the JSON control channel and MCP all dispatch the same commands by id with JSON params.
//! The engine owns the [`Session`] behind an `Arc` (copy-on-write), so undo snapshots are cheap.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod autosave;
pub mod catalog;
pub mod catalog_more;
pub mod clip_group_file;
pub mod cmd;
pub mod demo;
mod dirty;
pub mod edit;
pub mod file_access;
pub mod inspect;
pub mod io;
pub mod score;

use serde_json::{Value, json};
use soundcraft_model::{Clip, Session, TrackId, ZoomState};
use soundcraft_time::Samples;
use std::cell::Cell;
use std::sync::Arc;

use dirty::same_document;

pub use cmd::{CommandInfo, CommandSpec, command_specs, find_command};
pub use soundcraft_model as model;

/// Upper bound on journaled commands, so a long session cannot grow memory without limit.
const MAX_JOURNAL_ENTRIES: usize = 10_000;
/// Upper bound on a single journaled entry's params (serialized), so junk fields from MCP or the
/// control channel cannot keep a huge blob alive for the life of the process.
const MAX_JOURNAL_ENTRY_BYTES: usize = 64 * 1024; // 64 KiB

/// Shuts the third-party plugin hosts down, running the module exit of every plugin binary they
/// loaded. Call it at the end of `main`, once every plugin instance (mixer, player) is gone: a
/// plugin that has been used can crash when its module exit only runs from an exit handler.
pub fn shutdown_plugin_hosts() {
    soundcraft_vst3_host::shutdown();
}

/// A new [`soundcraft_model::AutoName::id`] for a scheme in `s`: a hash of a counter, never 0 and
/// not held by another scheme in `s`. Native builds seed the hash from the system, so each run of
/// the app gives other ids.
pub fn new_scheme_id(s: &Session) -> u64 {
    use std::hash::{BuildHasher, Hasher};
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    loop {
        let mut h = std::collections::hash_map::RandomState::new().build_hasher();
        h.write_u64(NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed));
        let id = h.finish();
        if id != 0 && !s.auto_names.values().any(|a| a.id == id) {
            return id;
        }
    }
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
    /// Start playing at this position (or move there if already playing), ignoring the selection.
    PlayFrom(Samples),
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
    /// The Auto Rename schemes of the copied clips, keyed as in their `auto_name`.
    pub auto_names: std::collections::BTreeMap<u64, soundcraft_model::AutoName>,
}

impl Clipboard {
    /// An empty clipboard for material copied from `range`.
    pub fn of_range(s: &Session, range: soundcraft_time::Range) -> Clipboard {
        let line = s.edit.grid.line_at_or_before(range.start, s.sample_rate, &s.tempo, s.frame_rate);
        Clipboard { length: range.len(), grid_offset: range.start.saturating_sub(line), ..Clipboard::default() }
    }

    /// Keep a copy of the Auto Rename schemes that the copied clips use, as `s` holds them.
    pub fn keep_auto_names(&mut self, s: &Session) {
        self.auto_names = self.tracks.iter().flatten().filter_map(|c| c.auto_name).filter_map(|k| Some((k, s.auto_names.get(&k)?.clone()))).collect();
    }

    /// Before a paste into `s`: point the copied clips at schemes that `s` holds. A scheme that `s`
    /// holds with the same [`AutoName::id`](soundcraft_model::AutoName::id), name, zeros and suffix
    /// is used as it is, and its numbers count on. Any other goes into `s` under a new key, counting
    /// on from where it was when it was copied.
    pub fn adopt_auto_names(&mut self, s: &mut Session) {
        let mut keys = std::collections::BTreeMap::new();
        for (k, a) in &self.auto_names {
            let held = s
                .auto_names
                .iter()
                .find(|(_, h)| a.id != 0 && h.id == a.id && h.name == a.name && h.zeros == a.zeros && h.suffix == a.suffix)
                .map(|(key, _)| *key);
            let key = match held {
                Some(key) => key,
                None => {
                    let key = s.alloc();
                    s.auto_names.insert(key, a.clone());
                    key
                }
            };
            keys.insert(*k, key);
        }
        for c in self.tracks.iter_mut().flatten() {
            c.auto_name = c.auto_name.and_then(|k| keys.get(&k).copied());
        }
        self.auto_names = keys.values().filter_map(|k| Some((*k, s.auto_names.get(k)?.clone()))).collect();
    }
}

#[derive(Debug, Clone)]
struct HistoryEntry {
    label: String,
    doc: Arc<Session>,
}

/// Most decoded imports [`Engine::offer_import`] holds at once (each can be gigabytes).
const MAX_OFFERED_IMPORTS: usize = 4;

/// Engine state: the document, history, clipboard and transport plumbing.
pub struct Engine {
    doc: Arc<Session>,
    undo: Vec<HistoryEntry>,
    redo: Vec<HistoryEntry>,
    pub history_limit: usize,
    pub clipboard: Clipboard,
    pub transport_requests: Vec<TransportRequest>,
    pub transport: TransportStatus,
    /// Command journal (id, params) for macro recording / agents. Ring buffer: capped at
    /// [`MAX_JOURNAL_ENTRIES`] entries of up to [`MAX_JOURNAL_ENTRY_BYTES`] params each.
    pub journal: std::collections::VecDeque<(String, Value)>,
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
    /// Audio decoded off the UI thread, waiting for its `file.import_audio` (see `offer_import`).
    imports: Vec<io::DecodedImport>,
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
            journal: std::collections::VecDeque::new(),
            path: None,
            revision: 1,
            dirty: false,
            dirty_at: Cell::new((0, false)),
            merge_key: None,
            merge_hold: None,
            messages: Vec::new(),
            last_selection: None,
            imports: Vec::new(),
        }
    }

    /// Hands over audio [`io::decode_import`] made off the UI thread: the next `file.import_audio`
    /// of the same file at the session's rate places it instead of reading the file again. The
    /// command still checks the path may be read. Only the few newest are kept.
    pub fn offer_import(&mut self, d: io::DecodedImport) {
        self.imports.retain(|x| x.key != d.key);
        self.imports.push(d);
        let extra = self.imports.len().saturating_sub(MAX_OFFERED_IMPORTS);
        self.imports.drain(..extra);
    }

    /// Drops audio offered for imports that never ran (the command failed or was not sent).
    pub fn drop_offered_imports(&mut self) {
        self.imports.clear();
    }

    /// The offered audio of the file at `path`, if it was decoded for the session's current rate.
    pub(crate) fn take_import(&mut self, path: &std::path::Path) -> Option<io::DecodedImport> {
        let key = io::file_key(path);
        let rate = self.session().sample_rate.hz();
        let i = self.imports.iter().position(|d| d.key == key && d.rate == rate)?;
        Some(self.imports.remove(i))
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
        let side_effects = (self.clipboard.clone(), self.messages.clone(), self.transport_requests.clone(), self.last_selection.clone());
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| (spec.run)(self, params)));
        let result = match result {
            Ok(r) => r,
            Err(p) => {
                self.doc = before;
                self.revision = rev.wrapping_add(1);
                self.dirty = dirty;
                (self.clipboard, self.messages, self.transport_requests, self.last_selection) = side_effects;
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
                    self.journal_append(id, params);
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

    /// The gesture `execute_merged` is continuing, if any.
    pub fn merge_key(&self) -> Option<&str> {
        self.merge_key.as_deref()
    }

    /// End a gesture started with `execute_merged`.
    pub fn end_merge(&mut self) {
        self.merge_key = None;
    }

    /// Append a journaled command, clamping its params to [`MAX_JOURNAL_ENTRY_BYTES`] and the
    /// buffer to [`MAX_JOURNAL_ENTRIES`] entries (oldest are dropped).
    fn journal_append(&mut self, id: &str, params: &Value) {
        let clamped = (id.to_string(), journal_params(params));
        self.journal.push_back(clamped);
        if self.journal.len() > MAX_JOURNAL_ENTRIES {
            self.journal.pop_front();
        }
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
            log::error!("panic: {info}\n{}", std::backtrace::Backtrace::force_capture());
            prev(info);
        }));
    });
}

fn panic_message(p: &(dyn std::any::Any + Send)) -> String {
    p.downcast_ref::<&str>().map(ToString::to_string).or_else(|| p.downcast_ref::<String>().cloned()).unwrap_or_else(|| "panic".into())
}

/// The params to keep in the journal for a command: the value itself when it serializes within
/// [`MAX_JOURNAL_ENTRY_BYTES`], or a truncation marker when it is too large or unparseable.
///
/// Serializes into a [`std::io::Write`] sink that fails as soon as the cap is exceeded, so an
/// oversized `params` (a multi-MiB line from MCP or the control channel) is never fully
/// serialized, cloned, or allocated just to be discarded.
fn journal_params(params: &Value) -> Value {
    // Write through a bounded sink: serialization stops (WriteZero) the moment the cap is passed,
    // so an oversized value costs at most ~1× the cap, not its full serialized size.
    struct Bounded(Vec<u8>);
    impl std::io::Write for Bounded {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            let room = MAX_JOURNAL_ENTRY_BYTES.saturating_sub(self.0.len());
            if room == 0 {
                return Err(std::io::Error::new(std::io::ErrorKind::WriteZero, "journal entry cap"));
            }
            let take = buf.len().min(room);
            self.0.extend_from_slice(&buf[..take]);
            if take < buf.len() {
                return Err(std::io::Error::new(std::io::ErrorKind::WriteZero, "journal entry cap"));
            }
            Ok(take)
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut sink = Bounded(Vec::new());
    // Fails with WriteZero as soon as the cap is exceeded, so the allocation stays bounded even
    // for a multi-MiB params.
    let ok = serde_json::to_writer(&mut sink, params).is_ok();
    if !ok {
        return json!({"__truncated__": true});
    }
    params.clone()
}

#[cfg(test)]
mod journal_tests {
    use super::*;

    #[test]
    fn journal_is_bounded_to_max_entries() {
        let mut e = Engine::default();
        // Calling the private append helper directly exercises the ring buffer without the cost
        // of cloning the whole document on 10,000+ `track.new` calls.
        for i in 0..MAX_JOURNAL_ENTRIES + 500 {
            let n = format!("t{i}");
            e.journal_append("track.new", &json!({"name": n}));
        }
        assert_eq!(e.journal.len(), MAX_JOURNAL_ENTRIES);
        // The oldest entries were dropped, the newest kept: the first remaining is the 501st pushed,
        // and the newest still there.
        let first = e.journal.front().map(|(_, p)| p["name"].as_str().unwrap_or_default());
        assert_eq!(first, Some("t500"));
        let last = e.journal.back().map(|(_, p)| p["name"].as_str().unwrap_or_default());
        let last_name = format!("t{}", MAX_JOURNAL_ENTRIES + 499);
        assert_eq!(last, Some(last_name.as_str()));
        assert_eq!(e.journal.back().map(|(id, _)| id.as_str()), Some("track.new"));
    }

    #[test]
    fn oversized_params_are_truncated_in_the_journal() {
        let big = "x".repeat(MAX_JOURNAL_ENTRY_BYTES * 4);
        assert_eq!(journal_params(&json!({"name": big})), json!({"__truncated__": true}));
    }

    #[test]
    fn small_params_are_kept_whole() {
        let p = json!({"track": "Kick", "db": -6.0});
        assert_eq!(journal_params(&p), p);
    }

    #[test]
    fn cap_boundary_is_exact() {
        // A value just under the cap is kept whole; one just over is truncated. "Just" means the
        // bounded writer made at most ~two allocations of cap size, not a full multi-MiB buffer.
        // `{"n":"…"}` adds 9 bytes around the string.
        let small_str = MAX_JOURNAL_ENTRY_BYTES - 64;
        let small = json!({"n": "x".repeat(small_str)});
        assert_eq!(journal_params(&small), small);
        let big = json!({"n": "x".repeat(MAX_JOURNAL_ENTRY_BYTES)});
        assert_eq!(journal_params(&big), json!({"__truncated__": true}));
    }
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

    #[test]
    fn dirty_state_follows_save_undo_and_redo_after_navigation() {
        let mut e = Engine::default();
        let track = e.session_mut().add_track(soundcraft_model::TrackKind::Audio, soundcraft_model::ChannelFormat::Mono, None);
        e.mark_clean();
        e.execute("mix.volume", &json!({"track": track.0, "db": -6.0})).unwrap();
        assert!(e.is_dirty());
        // A repeated check uses the revision cache.
        assert!(e.is_dirty());
        e.mark_clean();
        assert!(!e.is_dirty());
        e.execute("edit.select", &json!({"tracks": [track.0], "start": 100, "end": 200})).unwrap();
        e.execute("transport.locate", &json!({"at": 300})).unwrap();
        e.execute("view.zoom_in", &json!({})).unwrap();
        assert!(!e.is_dirty());
        assert!(e.undo());
        assert!(e.is_dirty(), "undoing away from the saved fader value is dirty");
        assert!(e.redo());
        assert!(!e.is_dirty(), "redoing to the saved fader value is clean despite navigation changes");
        e.execute("mix.volume", &json!({"track": track.0, "db": -12.0})).unwrap();
        assert!(e.is_dirty());
        assert!(e.undo());
        assert!(!e.is_dirty());
        assert!(e.redo());
        assert!(e.is_dirty());
    }
}

#[cfg(test)]
mod panic_tests {
    use super::*;

    #[test]
    fn a_panicking_command_restores_side_effect_state() {
        let mut e = Engine::default();
        // Non-default values, so clearing them instead of restoring them would fail.
        e.clipboard.length = 42;
        e.messages.push("prior".into());
        e.transport_requests.push(TransportRequest::Stop);
        e.last_selection = Some((soundcraft_time::Range::new(20, 30), Vec::new()));
        let (clipboard, messages, requests, selection) =
            (e.clipboard.clone(), e.messages.clone(), e.transport_requests.clone(), e.last_selection.clone());
        let r = e.execute("test.panic_probe", &json!({"panic": true}));
        assert!(matches!(r, Err(EngineError::Internal(..))));
        assert_eq!(e.clipboard, clipboard);
        assert_eq!(e.messages, messages);
        assert_eq!(e.transport_requests, requests);
        assert_eq!(e.last_selection, selection);
    }
}
