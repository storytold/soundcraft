use super::{SoundApp, TrackId, json, mix_window};

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
/// this same gesture scheduler/state-capture route.
pub(crate) fn editor_changes(
    pending: &mut Vec<(TrackId, usize)>,
    edits: Vec<soundcraft_playback::EditorEdit>,
    changed: Vec<(TrackId, usize)>,
    closed: impl Fn((TrackId, usize)) -> bool,
    capture: impl FnOnce(&[(TrackId, usize)]) -> soundcraft_playback::PluginStates,
) -> (Vec<soundcraft_playback::EditorEdit>, soundcraft_playback::PluginStates) {
    let due = editor_states_due(pending, &edits, changed, closed);
    let states = if due.is_empty() { Vec::new() } else { capture(&due) };
    (edits, states)
}

impl SoundApp {
    /// Reads every live third-party plugin's state and stores it on its insert
    /// (`mix.insert_state`), so saves and bounces include it.
    pub fn capture_plugin_states(&mut self) {
        let Some(p) = &self.player else { return };
        let states = p.capture_states();
        self.store_plugin_states(states);
    }

    pub(crate) fn store_plugin_states(&mut self, states: soundcraft_playback::PluginStates) {
        for (t, slot, data) in states {
            let slot = if slot == soundcraft_mix::INSTRUMENT_SLOT { json!("instrument") } else { json!(slot) };
            let params = json!({"track": t.0, "slot": slot, "state": soundcraft_model::b64::encode(&data)});
            // A state read while a knob is dragged (or a pass recorded) joins that gesture, so it
            // stays one undo step.
            let stored = match self.engine.merge_key().map(str::to_string) {
                Some(key) => self.engine.execute_merged("mix.insert_state", &params, &key),
                None => self.engine.execute("mix.insert_state", &params),
            };
            if let Err(e) = stored {
                log::warn!("storing plugin state: {e}");
            }
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
    /// instead, as it does for plugins that said their state changed: either way the session is
    /// modified, so it is autosaved and asks to be saved. The state is read once per gesture (see
    /// `editor_states_due`).
    pub(super) fn apply_editor_edits(&mut self) {
        if !std::mem::take(&mut self.editor_poll_due) {
            return;
        }
        #[cfg(test)]
        if let Some(poll) = self.editor_poll_fixture.as_mut() {
            let (edits, states) = poll(&mut self.editor_states);
            self.store_editor_changes(edits, states);
            return;
        }
        let Some(p) = &self.player else {
            self.editor_states.clear();
            return;
        };
        let (edits, states) = editor_changes(
            &mut self.editor_states,
            p.idle(),
            p.changed_states(),
            |(t, slot)| !p.editor_open(t, slot),
            |due| p.capture_states_of(due),
        );
        self.store_editor_changes(edits, states);
    }

    pub(crate) fn store_editor_changes(&mut self, edits: Vec<soundcraft_playback::EditorEdit>, states: soundcraft_playback::PluginStates) {
        for (t, slot, param, value) in edits {
            if slot == soundcraft_mix::INSTRUMENT_SLOT {
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
