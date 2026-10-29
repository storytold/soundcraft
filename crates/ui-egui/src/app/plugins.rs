use super::{SoundApp, TrackId, Value, json, mix_window};

/// The plugins whose state the session takes this frame, out of those changed in their editors
/// (`pending`, kept across frames). A plugin editor doesn't say when a gesture ends: a plugin's
/// gesture lasts while its editor reports a change every frame, and its state is read on the first
/// frame its editor reports none, or as soon as the editor is closed. A drag reads and stores the
/// state once, not every frame, and a plugin that keeps changing doesn't hold back the others.
pub(crate) fn editor_states_due(
    pending: &mut Vec<(TrackId, usize)>,
    edits: &[soundcraft_playback::EditorEdit],
    changed: Vec<(TrackId, usize)>,
    closed: impl Fn((TrackId, usize)) -> bool,
) -> Vec<(TrackId, usize)> {
    let instruments = edits.iter().filter(|e| e.1 == soundcraft_mix::INSTRUMENT_SLOT).map(|e| (e.0, e.1));
    let reported: Vec<(TrackId, usize)> = instruments.chain(changed).collect();
    for k in &reported {
        if !pending.contains(k) {
            pending.push(*k);
        }
    }
    let (due, rest) = std::mem::take(pending).into_iter().partition(|&k| !reported.contains(&k) || closed(k));
    *pending = rest;
    due
}

/// Collect one admitted editor poll. Both native hosting and the lifecycle test fixture use
/// this same gesture scheduler/state-capture route. The third value is each due instrument track
/// whose capture returned no state (the plugin refused `getState()`, or it has no state reader).
pub(crate) fn editor_changes(
    pending: &mut Vec<(TrackId, usize)>,
    edits: Vec<soundcraft_playback::EditorEdit>,
    changed: Vec<(TrackId, usize)>,
    closed: impl Fn((TrackId, usize)) -> bool,
    capture: impl FnOnce(&[(TrackId, usize)]) -> soundcraft_playback::PluginStates,
) -> (Vec<soundcraft_playback::EditorEdit>, soundcraft_playback::PluginStates, Vec<TrackId>) {
    let due = editor_states_due(pending, &edits, changed, closed);
    let states = if due.is_empty() { Vec::new() } else { capture(&due) };
    let missed = instrument_tracks_without_state(&due, &states);
    (edits, states, missed)
}

/// Due instrument tracks that are absent from `states`. A capture that returns `None` omits the
/// slot, and the held parameters queued for that track still have to reach the session.
fn instrument_tracks_without_state(due: &[(TrackId, usize)], states: &soundcraft_playback::PluginStates) -> Vec<TrackId> {
    let mut missed = Vec::new();
    for &(track, slot) in due {
        if slot != soundcraft_mix::INSTRUMENT_SLOT {
            continue;
        }
        if states.iter().any(|(t, s, _)| *t == track && *s == slot) {
            continue;
        }
        if !missed.contains(&track) {
            missed.push(track);
        }
    }
    missed
}

impl SoundApp {
    /// Reads every live third-party plugin's state and stores it on its insert
    /// (`mix.insert_state`), so saves and bounces include it. Held instrument parameters whose
    /// plugin returned no state are stored on their own (`mix.insert_params`), not dropped.
    pub fn capture_plugin_states(&mut self) {
        if let Some(p) = &self.player {
            let states = p.capture_states();
            self.store_plugin_states(states);
        }
        // Anything still queued belongs to a track the capture did not return. A save must not
        // write the session without those values, and `state: null` would clear a stored blob.
        let queued = self.queued_instrument_tracks();
        self.persist_queued_instrument_params(&queued);
    }

    pub(crate) fn store_plugin_states(&mut self, states: soundcraft_playback::PluginStates) {
        for (t, slot, data) in states {
            let instrument = slot == soundcraft_mix::INSTRUMENT_SLOT;
            let slot = if instrument { json!("instrument") } else { json!(slot) };
            let mut params = json!({"track": t.0, "slot": slot, "state": soundcraft_model::b64::encode(&data)});
            let mut edited = Vec::new();
            if instrument {
                let (taken, rest): (Vec<_>, Vec<_>) = std::mem::take(&mut self.editor_instrument_params).into_iter().partition(|e| e.0 == t);
                self.editor_instrument_params = rest;
                if !taken.is_empty() {
                    // Same command as the state, so the gesture is one document revision. A value
                    // that moved is one undo step inside that command.
                    let values: serde_json::Map<String, Value> = taken.iter().map(|(_, k, v)| (k.clone(), json!(*v))).collect();
                    params["params"] = json!(values);
                }
                edited = taken;
            }
            if !self.execute_in_gesture("mix.insert_state", &params) {
                self.editor_instrument_params.extend(edited);
            }
        }
    }

    /// Tracks that still have held instrument parameters waiting to be stored.
    fn queued_instrument_tracks(&self) -> Vec<TrackId> {
        let mut tracks = Vec::new();
        for (t, _, _) in &self.editor_instrument_params {
            if !tracks.contains(t) {
                tracks.push(*t);
            }
        }
        tracks
    }

    /// Stores queued held parameters for `tracks` with `mix.insert_params` (slot `"instrument"`).
    /// A failed command puts the values back. Never sends `mix.insert_state` with a null state:
    /// that would clear the blob the plugin already has.
    fn persist_queued_instrument_params(&mut self, tracks: &[TrackId]) {
        for &t in tracks {
            let (edited, rest): (Vec<_>, Vec<_>) = std::mem::take(&mut self.editor_instrument_params).into_iter().partition(|e| e.0 == t);
            self.editor_instrument_params = rest;
            if edited.is_empty() {
                continue;
            }
            if !self.engine.session().track(t).is_some_and(|tr| tr.instrument.is_some()) {
                continue;
            }
            let values: serde_json::Map<String, Value> = edited.iter().map(|(_, k, v)| (k.clone(), json!(*v))).collect();
            let params = json!({"track": t.0, "slot": "instrument", "params": values});
            if !self.execute_in_gesture("mix.insert_params", &params) {
                self.editor_instrument_params.extend(edited);
            }
        }
    }

    /// Runs a command that stores what a plugin editor changed. While a knob is dragged (or a pass
    /// recorded) it joins that gesture, so it stays one undo step. False when the command failed
    /// (the caller still holds whatever it took out of a queue).
    fn execute_in_gesture(&mut self, id: &str, params: &Value) -> bool {
        let stored = match self.engine.merge_key().map(str::to_string) {
            Some(key) => self.engine.execute_merged(id, params, &key),
            None => self.engine.execute(id, params),
        };
        if let Err(e) = stored {
            log::warn!("storing plugin state ({id}): {e}");
            false
        } else {
            true
        }
    }

    /// Opens the insert just picked from its menu: a hosted plugin's own editor, else (a built-in
    /// plugin, no audio engine, or an editor that can't open here) its plugin window.
    pub(super) fn open_inserted(&mut self) {
        let Some((track, slot)) = self.pending_open.take() else { return };
        let Some(plugin) = self.engine.session().track(track).and_then(|t| t.mixer.inserts.get(slot).cloned().flatten()).map(|i| i.plugin) else {
            return;
        };
        if soundcraft_mix::is_third_party(&plugin)
            && let Some(p) = &self.player
            && p.has_editor(track, slot)
        {
            match p.open_editor(track, slot) {
                Ok(()) => return,
                Err(e) => self.ui.status = format!("{}: {e}", mix_window::plugin_info(&plugin).map_or(plugin.as_str(), |i| i.name)),
            }
        }
        if !self.ui.plugin_windows.contains(&(track, slot)) {
            self.ui.plugin_windows.push((track, slot));
        }
    }

    /// Parameter edits made in plugin editors go into the session (one undo step per gesture).
    /// Hosted instruments keep their parameters in their own state, which the session takes
    /// instead (a parameter the session also holds, set by command, is kept in step), as it does for plugins that said their state changed: either way the session is
    /// modified, so it is autosaved and asks to be saved. The state is read once per gesture (see
    /// `editor_states_due`).
    pub(crate) fn apply_editor_edits(&mut self) {
        if !std::mem::take(&mut self.editor_poll_due) {
            return;
        }
        #[cfg(test)]
        if let Some(poll) = self.editor_poll_fixture.as_mut() {
            let (edits, states, missed) = poll(&mut self.editor_states);
            self.finish_editor_capture(edits, states, &missed);
            return;
        }
        let Some(p) = &self.player else {
            // No live plugin left to read. The queued values are still edits the session holds.
            let queued = self.queued_instrument_tracks();
            self.persist_queued_instrument_params(&queued);
            self.editor_states.clear();
            return;
        };
        let (edits, states, missed) = editor_changes(
            &mut self.editor_states,
            p.idle(),
            p.changed_states(),
            |(t, slot)| !p.editor_open(t, slot),
            |due| p.capture_states_of(due),
        );
        self.finish_editor_capture(edits, states, &missed);
    }

    /// Stores a finished capture. Due instrument tracks missing from `states` (the plugin returned
    /// no state) persist their queued parameters without touching the stored blob.
    fn finish_editor_capture(&mut self, edits: Vec<soundcraft_playback::EditorEdit>, states: soundcraft_playback::PluginStates, missed: &[TrackId]) {
        self.store_editor_changes(edits, states);
        self.persist_queued_instrument_params(missed);
    }

    pub(crate) fn store_editor_changes(&mut self, edits: Vec<soundcraft_playback::EditorEdit>, states: soundcraft_playback::PluginStates) {
        for (t, slot, param, value) in edits {
            if slot == soundcraft_mix::INSTRUMENT_SLOT {
                // The instrument's state carries its edits. A value the session holds as well (set
                // by command) follows the editor when the state is stored, or it would undo the edit
                // on top of the state when the session is opened again.
                let held = self.engine.session().track(t).and_then(|tr| tr.instrument.as_ref()).is_some_and(|i| i.params.contains_key(&param));
                if held {
                    self.editor_instrument_params.retain(|(pt, pp, _)| (*pt, pp) != (t, &param));
                    self.editor_instrument_params.push((t, param, value));
                }
                continue;
            }
            let key = format!("param:{}:{slot}:{param}", t.0);
            let _ = self.engine.execute_merged("mix.insert_param", &json!({"track": t.0, "slot": slot, "param": param, "value": value}), &key);
        }
        self.store_plugin_states(states);
    }
}

/// The status line naming the plugin in `slot` (an insert index or
/// [`soundcraft_mix::INSTRUMENT_SLOT`]), which the engine took out of the signal path.
pub(crate) fn failed_plugin_status(s: &soundcraft_model::Session, track: TrackId, slot: usize) -> Option<String> {
    let t = s.track(track)?;
    let ins = if slot == soundcraft_mix::INSTRUMENT_SLOT { t.instrument.as_ref() } else { t.mixer.inserts.get(slot)?.as_ref() }?;
    let name = crate::mix_window::plugin_info(&ins.plugin).map_or(ins.plugin.as_str(), |p| p.name);
    Some(format!("{name} on {} put out invalid audio, so it is bypassed until playback stops or starts again.", t.name))
}
