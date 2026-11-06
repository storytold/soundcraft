use super::{SoundApp, Value, json};
use crate::control;

impl SoundApp {
    /// The app clock: a frozen test clock when set, otherwise host time shifted by the test offset.
    pub(crate) fn clock_now(&self) -> web_time::Instant {
        self.test_clock.unwrap_or_else(|| web_time::Instant::now().checked_add(self.clock_offset).unwrap_or_else(web_time::Instant::now))
    }

    /// Test hook: advance the monotonic clock without sleeping.
    ///
    /// The first call freezes the clock at the current host time; later calls move it by exactly
    /// `by`. Tests that assert frame or transport timing therefore see only the time they asked
    /// for, not how long the test machine spent rendering.
    #[doc(hidden)]
    pub fn advance_test_clock(&mut self, by: std::time::Duration) {
        let base = self.test_clock.unwrap_or_else(|| self.clock_now());
        self.test_clock = Some(base.checked_add(by).unwrap_or(base));
    }

    /// The viewport geometry new captures are bound to. Raw-input geometry wins because it is
    /// refreshed on hidden logic-only ticks; the UI input is a fallback for harnesses without a
    /// raw-input hook.
    pub(crate) fn capture_geometry(&self, ctx: &egui::Context) -> control::CaptureGeometry {
        self.host_geometry.unwrap_or_else(|| control::CaptureGeometry::from_context(ctx))
    }

    /// Remember the latest valid host viewport geometry for screenshot admission.
    pub(super) fn note_host_geometry(&mut self, ctx: &egui::Context, raw: &egui::RawInput) {
        if let Some(g) = control::CaptureGeometry::from_raw(ctx, raw) {
            self.host_geometry = Some(g);
        }
    }

    /// Remove the events [`Self::raw_input_hook`] appended to an input the host retained, so the
    /// pending batch appears exactly once no matter how many logic-only ticks reuse that input.
    pub(super) fn remove_injected(&mut self, raw: &mut egui::RawInput) {
        let injected = std::mem::take(&mut self.injected_events);
        let n = injected.len();
        if n == 0 || raw.events.len() < n {
            return;
        }
        let here = raw.events.get(self.injected_at..self.injected_at.saturating_add(n)).is_some_and(|run| run == injected.as_slice());
        // The host appends newer events after the retained copy, so it is usually still at its
        // recorded index; fall back to the last identical run for a host that rewraps its input.
        let found = if here {
            Some(self.injected_at)
        } else {
            (0..=raw.events.len() - n).rev().find(|&i| raw.events.get(i..i + n).is_some_and(|run| run == injected.as_slice()))
        };
        if let Some(i) = found {
            raw.events.drain(i..i + n);
        }
    }

    /// Drop the injected copy from this pass's `InputState.events` after its gesture expired in
    /// `logic`. Returns whether the copy is known to be gone: an empty batch, or an exact run that
    /// was drained. On a mismatch the caller must not draw, because the stale events cannot be
    /// identified safely.
    pub(super) fn remove_received_injected(&self, ctx: &egui::Context) -> bool {
        let n = self.injected_events.len();
        if n == 0 {
            return true;
        }
        ctx.input_mut(|i| {
            let Some(run) = i.events.get(self.injected_at..self.injected_at.saturating_add(n)) else { return false };
            // egui rewrites a press's `repeat` flag before the app sees it; compare like the ack.
            if run.iter().zip(&self.injected_events).all(|(received, sent)| Self::synthetic_event_received(sent, received)) {
                i.events.drain(self.injected_at..self.injected_at.saturating_add(n));
                true
            } else {
                false
            }
        })
    }

    /// Whether `received` is the event `wanted` after egui's input pass. egui rewrites the `repeat`
    /// flag of `egui::Event::Key` from the keys held in the previous pass, so a scripted press can
    /// arrive marked as a repeat; the other fields still identify the event. All other events must
    /// match exactly.
    pub(crate) fn synthetic_event_received(wanted: &egui::Event, received: &egui::Event) -> bool {
        match (wanted, received) {
            (
                egui::Event::Key { key, physical_key, pressed, modifiers, .. },
                egui::Event::Key { key: got_key, physical_key: got_physical, pressed: got_pressed, modifiers: got_modifiers, .. },
            ) => key == got_key && physical_key == got_physical && pressed == got_pressed && modifiers == got_modifiers,
            _ => wanted == received,
        }
    }

    /// The exact positions in this pass's events of the pending batch's key presses. Shortcut
    /// handling uses provenance rather than matching key/modifier values: a fresh host press with
    /// the same chord is a separate action, and egui's rewritten `repeat` flag only describes the
    /// physical key state, not which event was injected.
    pub(crate) fn scripted_key_press_indices(&self) -> Vec<usize> {
        if self.synthetic_batch.is_none() {
            return Vec::new();
        }
        self.injected_events
            .iter()
            .enumerate()
            .filter(|(_, e)| matches!(e, egui::Event::Key { pressed: true, .. }))
            .map(|(offset, _)| self.injected_at.saturating_add(offset))
            .collect()
    }

    /// Whether this pass's `InputState.events` still starts with the exact injected run. This is
    /// also valid after `expire_gestures` cleared `synthetic_batch`, when a cancel needs to know
    /// whether replaying would address this pass or an earlier one.
    pub(super) fn pass_received_injected(&self, ctx: &egui::Context) -> bool {
        if self.injected_events.is_empty() {
            return false;
        }
        let Some(end) = self.injected_at.checked_add(self.injected_events.len()) else { return false };
        ctx.input(|i| {
            i.events.len() >= end
                && self.injected_events.iter().enumerate().all(|(offset, sent)| {
                    i.events.get(self.injected_at + offset).is_some_and(|received| Self::synthetic_event_received(sent, received))
                })
        })
    }

    /// Whether this UI pass actually received the pending synthetic batch's events. Call this
    /// before menus and widgets: egui's `consume_key`/`consume_shortcut` remove matching events
    /// from `InputState.events`, so a late check would miss a batch that really was dispatched.
    pub(super) fn pass_received_synthetic(&self, ctx: &egui::Context) -> bool {
        self.synthetic_batch.is_some() && self.pass_received_injected(ctx)
    }

    /// UI state as JSON (control channel `ui.inspect`).
    pub fn inspect(&self, ctx: &egui::Context) -> Value {
        let size = ctx.content_rect().size();
        json!({
            "ui": self.ui,
            "window_size": [size.x, size.y],
            "playing": self.is_playing(),
            "position": self.position(),
            "dialog": self.dialogs.open_name(),
            "frame_ms": self.frame_ms,
            "audio_device": self.player.as_ref().map(|p| p.device_name.clone()),
            "status": self.ui.status,
        })
    }
}
