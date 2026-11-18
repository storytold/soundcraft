//! The sandbox's view of the engine, and what a run and each of its calls must leave as it was.

use std::sync::Arc;

use craft_plugin::{Access, Manifest};
use serde_json::Value;
use soundcraft_model::{Session, TrackId};

use super::{MAX_NOTE_CHARS, MAX_NOTES, MAX_RUN_EDITS, refusal, selection_json};
use crate::dirty::same_document;
use crate::{Clipboard, Engine, HistoryEntry, TransportRequest};

/// The sandbox's view of the engine: every call is checked by [`refusal`] and run in its JSON form
/// ([`Engine::execute`]). After each call, [`Probe`] puts back and refuses a call that reached the
/// transport, the clipboard, a live input or the session file, or, from a read-only plug-in,
/// edited the session. The undo step a call records is dropped at once: the run keeps the session
/// as it started ([`Mark`]) and records one step of its own, so a long run holds no more copies of
/// the session than a short one.
pub(super) struct EngineHost<'e> {
    e: &'e mut Engine,
    manifest: &'e Manifest,
    notes: Vec<String>,
    /// The undo history's length when the run started.
    undo: usize,
    /// Calls that recorded an undo step.
    edits: u64,
}

impl<'e> EngineHost<'e> {
    pub(super) fn new(e: &'e mut Engine, manifest: &'e Manifest) -> Self {
        let undo = e.undo.len();
        EngineHost { e, manifest, notes: Vec::new(), undo, edits: 0 }
    }

    /// What the run asked to show the user, and whether a call recorded an edit.
    pub(super) fn finish(self) -> (Vec<String>, bool) {
        (self.notes, self.edits > 0)
    }
}

impl craft_plugin::Host for EngineHost<'_> {
    fn call(&mut self, command: &str, params: &str) -> Result<String, String> {
        let command = command.trim();
        let params: Value = serde_json::from_str(params).map_err(|e| format!("the params for `{command}` aren't JSON: {e}"))?;
        if let Some(why) = refusal(self.manifest, command, &params) {
            return Err(why);
        }
        if self.edits >= MAX_RUN_EDITS {
            return Err(format!("a command plug-in run may make at most {MAX_RUN_EDITS} edits; run it again for the rest"));
        }
        let probe = Probe::take(self.e);
        let r = self.e.execute(command, &params);
        if let Some(why) = probe.violation(self.e, self.manifest, command) {
            probe.restore(self.e);
            return Err(why);
        }
        if self.e.undo.len() > self.undo {
            self.edits = self.edits.saturating_add(1);
            self.e.undo.truncate(self.undo);
        }
        r.map(|v| v.to_string()).map_err(|e| e.to_string())
    }

    fn selection(&mut self) -> String {
        selection_json(self.e).to_string()
    }

    fn notify(&mut self, message: &str) {
        if self.notes.len() < MAX_NOTES {
            self.notes.push(message.chars().take(MAX_NOTE_CHARS).collect());
        }
    }

    fn log(&mut self, line: &str) {
        // Also in the run's result (`logs`)
        log::info!("command plug-in {}: {line}", self.manifest.id);
    }

    fn setting(&mut self, key: &str) -> Option<String> {
        Some(self.e.command_plugins.prefs.settings.get(&self.manifest.id)?.get(key)?.to_string())
    }

    fn set_setting(&mut self, key: &str, value: Option<&str>) -> Result<(), String> {
        self.e.command_plugins.set_setting(&self.manifest.id, key, value)
    }
}

/// Each track's live input state (armed, monitoring input): kept out of the document and undo,
/// and heard by the audio engine at once.
fn live_inputs(s: &Session) -> Vec<(TrackId, bool, bool)> {
    s.tracks.iter().map(|t| (t.id, t.mixer.record_arm, t.mixer.input_monitor)).collect()
}

/// The engine as it was before one call, to tell what the call changed and to put it back.
pub(super) struct Probe {
    doc: Arc<Session>,
    undo: usize,
    redo: Vec<HistoryEntry>,
    merge_key: Option<String>,
    revision: u64,
    dirty: bool,
    clipboard: Clipboard,
    transport_requests: Vec<TransportRequest>,
    path: Option<String>,
    live: Vec<(TrackId, bool, bool)>,
}

impl Probe {
    pub(super) fn take(e: &Engine) -> Probe {
        Probe {
            doc: Arc::clone(&e.doc),
            undo: e.undo.len(),
            redo: e.redo.clone(),
            merge_key: e.merge_key.clone(),
            revision: e.revision,
            dirty: e.dirty,
            clipboard: e.clipboard.clone(),
            transport_requests: e.transport_requests.clone(),
            path: e.path.clone(),
            live: live_inputs(&e.doc),
        }
    }

    /// Why the call that ran since [`Probe::take`] isn't allowed, from what it changed: a request
    /// to the transport or the audio engine, the clipboard, an armed or monitoring track, the
    /// session file, or, for a read-only plug-in, the document or its undo history.
    pub(super) fn violation(&self, e: &Engine, m: &Manifest, command: &str) -> Option<String> {
        if e.transport_requests != self.transport_requests {
            return Some(format!("`{command}` reaches the transport or the audio engine, which command plug-ins can't"));
        }
        if e.clipboard != self.clipboard {
            return Some(format!("`{command}` uses the clipboard, which command plug-ins can't"));
        }
        if e.path != self.path {
            return Some(format!("`{command}` changes the session file, which command plug-ins can't"));
        }
        // A track the call added starts unarmed and not monitoring; a track it deleted is gone
        let was = |id: TrackId| self.live.iter().find(|(t, ..)| *t == id).map_or((false, false), |&(_, arm, mon)| (arm, mon));
        if live_inputs(&e.doc).into_iter().any(|(id, arm, mon)| (arm, mon) != was(id)) {
            return Some(format!("`{command}` arms tracks or monitors their input, which command plug-ins can't"));
        }
        let edited = e.undo.len() != self.undo || !same_document(&self.doc, &e.doc);
        if edited && m.permissions.access != Access::Write {
            return Some(craft_plugin::check_read_only(m, command, true).err().unwrap_or_else(|| format!("`{command}` edits the session")));
        }
        None
    }

    /// Put the document, the undo history, the clipboard and the transport requests back.
    pub(super) fn restore(self, e: &mut Engine) {
        e.doc = self.doc;
        e.undo.truncate(self.undo);
        e.redo = self.redo;
        e.merge_key = self.merge_key;
        e.clipboard = self.clipboard;
        e.transport_requests = self.transport_requests;
        e.path = self.path;
        e.dirty = self.dirty;
        // a new revision refreshes what shows the session (and the player's copy of it)
        e.revision = self.revision.max(e.revision).wrapping_add(1);
    }
}

/// The engine as it was before a run: put back when the run fails, or collapsed into one undo
/// step when it succeeds.
pub(super) struct Mark {
    doc: Arc<Session>,
    undo: usize,
    redo: Vec<HistoryEntry>,
    merge_key: Option<String>,
    dirty: bool,
    clipboard: Clipboard,
    messages: Vec<String>,
    transport_requests: Vec<TransportRequest>,
    last_selection: Option<(soundcraft_time::Range, Vec<TrackId>)>,
}

impl Mark {
    pub(super) fn take(e: &Engine) -> Mark {
        Mark {
            doc: Arc::clone(&e.doc),
            undo: e.undo.len(),
            redo: e.redo.clone(),
            merge_key: e.merge_key.clone(),
            dirty: e.dirty,
            clipboard: e.clipboard.clone(),
            messages: e.messages.clone(),
            transport_requests: e.transport_requests.clone(),
            last_selection: e.last_selection.clone(),
        }
    }

    /// A failed run: the session, its selection, the undo history and the rest as they were.
    pub(super) fn restore(self, e: &mut Engine) {
        e.doc = self.doc;
        e.undo.truncate(self.undo);
        e.redo = self.redo;
        e.merge_key = self.merge_key;
        e.dirty = self.dirty;
        e.clipboard = self.clipboard;
        e.messages = self.messages;
        e.transport_requests = self.transport_requests;
        e.last_selection = self.last_selection;
        e.revision = e.revision.wrapping_add(1);
    }

    /// A run that succeeded: what its calls did becomes one undo step, named `label`. A run whose
    /// calls `recorded` no edit and that changed nothing but the selection records no step and
    /// keeps Redo.
    pub(super) fn collapse(self, e: &mut Engine, label: &str, recorded: bool) {
        let edited = recorded || e.undo.len() > self.undo || !same_document(&self.doc, &e.doc);
        e.undo.truncate(self.undo);
        if edited {
            // the redo history went with the first step a call recorded
            e.redo.clear();
            e.undo.push(HistoryEntry { label: label.to_string(), doc: self.doc });
        }
    }
}
