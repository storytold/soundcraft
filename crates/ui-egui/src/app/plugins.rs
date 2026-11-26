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
        if let Some(p) = &self.player {
            // A save or bounce can arrive between editor polls, even during a gesture.
            let edits = p.idle();
            let states = p.capture_states();
            self.store_editor_changes(edits, states);
        }
        let pending: Vec<_> = self.instrument_edits.keys().copied().collect();
        for t in pending {
            self.store_plugin_state(t, soundcraft_mix::INSTRUMENT_SLOT, None);
        }
    }

    pub(crate) fn store_plugin_states(&mut self, states: soundcraft_playback::PluginStates) {
        for (t, slot, data) in states {
            self.store_plugin_state(t, slot, Some(&data));
        }
    }

    fn store_plugin_state(&mut self, t: TrackId, slot: usize, data: Option<&[u8]>) {
        let instrument = slot == soundcraft_mix::INSTRUMENT_SLOT;
        let mut params = json!({"track": t.0, "slot": if instrument { json!("instrument") } else { json!(slot) }});
        if let Some(data) = data {
            params["state"] = json!(soundcraft_model::b64::encode(data));
        }
        if instrument
            && let Some((plugin, values)) = self.instrument_edits.remove(&t)
            && self.engine.session().track(t).and_then(|tr| tr.instrument.as_ref()).is_some_and(|ins| ins.plugin == plugin)
        {
            params["params"] = serde_json::Value::Object(values);
        }
        if data.is_none() && params.get("params").is_none() {
            return;
        }
        let stored = match self.engine.merge_key().map(str::to_string) {
            Some(key) => self.engine.execute_merged("mix.insert_state", &params, &key),
            None => self.engine.execute("mix.insert_state", &params),
        };
        if let Err(e) = stored {
            log::warn!("storing plugin state: {e}");
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
                Err(e) => {
                    let name = mix_window::plugin_info(&plugin).map_or(plugin.as_str(), |i| i.name);
                    let reason = crate::localization::error_text(&e.to_string());
                    self.ui.status = format!("{name}: {reason}");
                }
            }
        }
        if !self.ui.plugin_windows.contains(&(track, slot)) {
            self.ui.plugin_windows.push((track, slot));
        }
    }

    /// Parameter edits made in plugin editors go into the session (one undo step per gesture).
    /// Native instrument edits are stored with their patch once per gesture, so the generic
    /// editor, MIDI Learn and newly restored instances see the same parameter values.
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

    /// Drops the edits and state-change notices open editors queued since the last poll.
    pub(super) fn discard_editor_edits(&mut self) {
        #[cfg(test)]
        if let Some(poll) = self.editor_poll_fixture.as_mut() {
            let _ = poll(&mut Vec::new());
            return;
        }
        if let Some(p) = &self.player {
            let _ = p.idle();
            let _ = p.changed_states();
        }
    }

    pub(crate) fn store_editor_changes(&mut self, edits: Vec<soundcraft_playback::EditorEdit>, states: soundcraft_playback::PluginStates) {
        for (t, slot, param, value) in edits {
            if slot == soundcraft_mix::INSTRUMENT_SLOT {
                if let Some(ins) = self.engine.session().track(t).and_then(|tr| tr.instrument.as_ref()) {
                    let (plugin, values) = self.instrument_edits.entry(t).or_default();
                    if *plugin != ins.plugin {
                        plugin.clone_from(&ins.plugin);
                        values.clear();
                    }
                    values.insert(param, json!(value));
                }
                continue;
            }
            let key = format!("param:{}:{slot}:{param}", t.0);
            let _ = self.engine.execute_merged("mix.insert_param", &json!({"track": t.0, "slot": slot, "param": param, "value": value}), &key);
        }
        self.store_plugin_states(states);
        // A plug-in may expose parameters without supporting opaque state.
        let completed: Vec<_> =
            self.instrument_edits.keys().copied().filter(|&t| !self.editor_states.contains(&(t, soundcraft_mix::INSTRUMENT_SLOT))).collect();
        for t in completed {
            self.store_plugin_state(t, soundcraft_mix::INSTRUMENT_SLOT, None);
        }
    }
}

/// The status line naming the plugin in `slot` (an insert index or
/// [`soundcraft_mix::INSTRUMENT_SLOT`]), which the engine took out of the signal path.
pub(crate) fn failed_plugin_status(s: &soundcraft_model::Session, track: TrackId, slot: usize) -> Option<String> {
    let t = s.track(track)?;
    let ins = if slot == soundcraft_mix::INSTRUMENT_SLOT { t.instrument.as_ref() } else { t.mixer.inserts.get(slot)?.as_ref() }?;
    let name = crate::mix_window::plugin_info(&ins.plugin).map_or(ins.plugin.as_str(), |p| p.name);
    Some(crate::localized_format!(
        "{name} on {track} put out invalid audio, so it is bypassed until playback stops or starts again.",
        name = name,
        track = t.name
    ))
}
