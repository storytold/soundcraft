use super::{SoundApp, json, sampler_window};

impl SoundApp {
    /// Feed queued synthetic input (from the control channel) into egui: one pointer event per
    /// frame (so drags span frames like real ones), keys as press+release pairs, text at once.
    pub fn raw_input_hook(&mut self, ctx: &egui::Context, raw: &mut egui::RawInput) {
        self.editor_poll_due = true;
        self.note_host_geometry(ctx, raw);
        // Expire before injecting: a gesture that can no longer reach a UI pass must not put one
        // last copy of its events into this pass after its request was already canceled.
        crate::control::expire_gestures(self);
        // A host that could not run a UI pass retains the input this hook mutated (eframe's
        // `pending_raw_input`, the web runner's `input.raw`) and hands it back next tick. Strip the
        // copy appended last time before appending the still-pending batch again: otherwise a
        // single queued key press would arrive once per hidden tick and run N times on restore.
        self.remove_injected(raw);
        self.host_keys_down = None;
        // An expired key/scroll batch was already stripped from the retained input above. A UI
        // pass that received it before the deadline crossed has consumed these flags; a hidden
        // tick cannot.
        self.cancel_synthetic_keys = false;
        self.cancel_synthetic_scroll = false;
        self.input_before_scroll = None;
        if std::mem::take(&mut self.cancel_synthetic_pointer) {
            // Do this before egui begins its next input/interaction pass. stop_dragging marks a
            // drag_stopped in the current snapshot; the next pass replaces it without a release.
            // Calling it from the UI instead could turn cancellation into a track reorder/drop.
            ctx.input_mut(|i| i.pointer = Default::default());
            ctx.stop_dragging();
            egui::DragAndDrop::clear_payload(ctx);
            self.pointer_down = false;
        }
        // While a synthetic gesture plays (and briefly after), the real pointer is ignored so it
        // cannot interleave with the scripted one. A batch still waiting for a UI pass counts as
        // "playing": the grace must not run out while that gesture is pending.
        if !self.synthetic.is_empty() || self.synthetic_batch.is_some() {
            self.synthetic_grace = 6;
        }
        if self.synthetic_grace > 0 {
            self.synthetic_grace -= 1;
            raw.events.retain(|e| {
                !matches!(e, egui::Event::PointerMoved(_) | egui::Event::PointerButton { .. } | egui::Event::PointerGone | egui::Event::MouseMoved(_))
            });
        }
        // Exactly one batch is in flight. It is only taken from the queue when no earlier batch is
        // still waiting for a settled UI pass, so hidden logic ticks neither apply nor discard it.
        if self.synthetic_batch.is_none()
            && let Some(first) = self.synthetic.first().cloned()
        {
            let take = match first {
                egui::Event::Key { pressed: true, .. } => {
                    // Press and the matching release together.
                    if matches!(self.synthetic.get(1), Some(egui::Event::Key { pressed: false, .. })) { 2 } else { 1 }
                }
                _ => 1,
            };
            let n = take.min(self.synthetic.len());
            self.synthetic_batch = Some(self.synthetic.drain(..n).collect());
        }
        let Some(batch) = self.synthetic_batch.clone() else { return };
        if batch.iter().any(|e| matches!(e, egui::Event::Key { .. })) {
            // egui derives a press's `repeat` flag and the final `keys_down` from every event in the
            // batch. A scripted press+release while the host physically holds the same key would
            // therefore be marked as a repeat and would leave `keys_down` reporting the key as up.
            // Remember the host-only state; `ui` puts it back after egui processed the batch. The
            // hook itself never mutates egui's input, so a hidden logic-only tick cannot corrupt it.
            let previous = ctx.input(|i| i.keys_down.clone());
            self.host_keys_down = Some(Self::host_keys_after(previous, &raw.events));
        }
        if batch.iter().any(|e| matches!(e, egui::Event::MouseWheel { .. } | egui::Event::Zoom(_))) {
            // Remember the pass input and the host's own raw input from before egui folds this
            // batch into its scroll/zoom aggregates, so an expiry in `logic` can replay the host
            // input without it.
            self.input_before_scroll = Some((ctx.input(|i| i.clone()), raw.clone()));
        }
        let mut injected = Vec::with_capacity(batch.len() * 2);
        for e in &batch {
            if matches!(e, egui::Event::PointerButton { pressed: true, .. }) {
                self.synthetic_pointer_active = true;
            }
            if let egui::Event::PointerButton { pos, .. } | egui::Event::PointerMoved(pos) = e {
                // Keep the hover position current for widgets that check it.
                injected.push(egui::Event::PointerMoved(*pos));
            }
        }
        injected.extend(batch);
        self.injected_at = raw.events.len();
        raw.events.extend(injected.iter().cloned());
        self.injected_events = injected;
    }

    /// The physical key state after the host's own events, ignoring any synthetic batch appended
    /// later. egui starts from the previous pass's `keys_down`, then applies key events in order.
    fn host_keys_after(mut down: std::collections::HashSet<egui::Key>, events: &[egui::Event]) -> std::collections::HashSet<egui::Key> {
        for event in events {
            match event {
                egui::Event::Key { key, pressed: true, .. } => {
                    down.insert(*key);
                }
                egui::Event::Key { key, pressed: false, .. } => {
                    down.remove(key);
                }
                // egui clears all held keys when the window loses focus.
                egui::Event::WindowFocused(false) => down.clear(),
                _ => {}
            }
        }
        down
    }

    /// Rebuild this pass's input without the expired scroll/zoom batch. egui has already folded
    /// the batch into `InputState.smooth_scroll_delta`, `zoom_delta` and the persistent wheel
    /// smoothing state, none of which can be undone by draining `events`. Replaying the host's own
    /// raw input from the pre-injection state removes that contribution while keeping simultaneous
    /// host events, held keys and pointer/touch state.
    pub(super) fn rebuild_input_without_expired_scroll(&mut self, ctx: &egui::Context) {
        let Some((before, mut host_raw)) = self.input_before_scroll.take() else { return };
        // `begin_pass` derives `stable_dt` from its own `requested_immediate_repaint_prev_frame`
        // argument. Keep the real pass's value; layout reads it in this pass.
        let stable_dt = ctx.input(|i| i.stable_dt);
        // `ui` already moved any dropped files from this pass's raw input into `pending_drops`.
        // Do not replay them into the raw as well, or the next pass would route them twice.
        host_raw.dropped_files.clear();
        let options = ctx.options(|o| o.input_options);
        let mut fresh = before.begin_pass(host_raw, false, ctx.pixels_per_point(), options);
        fresh.stable_dt = stable_dt;
        ctx.input_mut(|i| *i = fresh);
        // This real pass consumed the hook-mutated raw, so there is no retained copy left to strip.
        // Keeping the old run would let the next hook's last-identical-run fallback eat a genuine
        // host event that happens to match it, such as the next identical physical wheel notch.
        self.injected_events.clear();
    }

    /// Load dropped audio into the sampler track under the drop point; leave everything else for
    /// the native/browser shell to open or import.
    pub(super) fn route_dropped_files(&mut self) {
        if self.pending_drops.is_empty() {
            return;
        }
        let mut claimed = false;
        for (file, pos) in std::mem::take(&mut self.pending_drops) {
            // As before, one dropped audio file goes to the sampler under the pointer; the rest
            // are left to the shell.
            let claim = !claimed && sampler_window::is_audio_name(file.path());
            let Some(track) = self.drop_track(pos).filter(|_| claim) else {
                self.host_drops.push(file);
                continue;
            };
            claimed = true;
            if file.path().is_absolute() {
                match crate::dialogs::unicode(file.path()) {
                    Ok(path) => {
                        let r = self.run("mix.sampler_load", json!({"track": track.0, "path": path}));
                        self.sample_loaded(track, path, r);
                    }
                    Err(e) => self.ui.status = e.to_string(),
                }
            } else {
                self.sample_reads.push(sampler_window::SampleRead { track, file });
            }
        }
    }
}
