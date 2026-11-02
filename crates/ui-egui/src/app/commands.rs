use super::{bounce, dialogs, menus, SoundApp, Value};
use soundcraft_model::{SEND_SLOTS, TrackId};

impl SoundApp {
    /// Run a command; errors go to the status line. Returns the result.
    pub fn run(&mut self, id: &str, params: Value) -> Result<Value, String> {
        if matches!(id, "session.new" | "session.open" | "session.close" | "session.revert") && bounce::refuses_session_switch(self) {
            return Err(self.ui.status.clone());
        }
        if let Some(r) = menus::run_ui_command(self, id, &params) {
            return r;
        }
        if id.starts_with("session.save") || id.starts_with("file.bounce") || id == "track.save_preset" {
            // Third-party plugins keep state the session can't see: store it first.
            self.capture_plugin_states();
        }

        // #1269: before `mix.send`, remember whether the target send slot is empty.
        // If the slot was empty before and is filled after, the send was newly created — open
        // its floating fader window automatically.
        let was_slot_empty = if id == "mix.send" {
            self.send_slot_before(&params)
        } else {
            None
        };

        let replacing = matches!(id, "session.new" | "session.open" | "session.close" | "session.revert");
        match self.engine.execute(id, &params) {
            Ok(v) => {
                if replacing {
                    // Note indices and gestures belong to the old session's clips (#443).
                    self.document_replaced();
                }
                if replacing || (id.starts_with("session.save") && !self.engine.is_dirty()) {
                    self.release_autosave(false);
                }
                if id == "app.quit" {
                    self.quit_requested = true;
                }
                if id == "options.scrolling" {
                    // Re-selecting a scrolling mode resumes the playhead follow.
                    self.edit_layout.follow_hold = false;
                }
                if id == "view.zoom_toggle_cancel" && v["canceled"] == true {
                    self.edit_layout.forget_zoom_back();
                }
                if id == "track.clear_clip_indicators" {
                    self.meters.values_mut().chain([&mut self.main_meter]).for_each(|m| m.clip = false);
                }
                // #1269: open the send window after creating a new send.
                if id == "mix.send" {
                    if let Some((track, slot)) = was_slot_empty {
                        let s = self.engine.session();
                        let now_filled = s
                            .track(track)
                            .and_then(|t| t.mixer.sends.get(slot))
                            .is_some_and(Option::is_some);
                        if now_filled && !self.dialogs.open.is_some() && !self.ui.show_search {
                            self.ui.send_window = Some((track, slot));
                        }
                    }
                }
                Ok(v)
            }
            Err(e) => {
                self.ui.status = e.to_string();
                log::warn!("{id}: {e}");
                Err(e.to_string())
            }
        }
    }

    /// Helper for #1269: returns `(track, slot)` if the target send slot is empty before the
    /// `mix.send` command runs, so the caller can detect a newly created send.
    fn send_slot_before(&self, params: &Value) -> Option<(TrackId, usize)> {
        let s = self.engine.session();

        // Resolve the track: `tracks` array first, then `track`, then selection.
        let track = Self::resolve_first_track(s, params)?;
        let tr = s.track(track)?;

        // Explicit slot? Check it directly.
        if let Some(slot) = params.get("slot").and_then(|v| v.as_u64()) {
            let slot = slot as usize;
            if slot < SEND_SLOTS && tr.mixer.sends.get(slot).is_none() {
                return Some((track, slot));
            }
            return None;
        }

        // No explicit slot: the engine picks the first empty one.
        // Remember which one that would be.
        tr.mixer.sends.iter().position(Option::is_none).map(|slot| (track, slot))
    }

    /// Resolve the first track from `params`: `tracks[0]`, `track`, or first selected track.
    fn resolve_first_track(s: &soundcraft_model::Session, params: &Value) -> Option<TrackId> {
        // `tracks` array
        if let Some(arr) = params.get("tracks").and_then(|v| v.as_array()) {
            let v = arr.first()?;
            return Self::resolve_track_id(s, v);
        }
        // `track` single param
        if let Some(v) = params.get("track") {
            return Self::resolve_track_id(s, v);
        }
        // selection
        s.edit.selected_tracks.first().copied()
    }

    fn resolve_track_id(s: &soundcraft_model::Session, v: &Value) -> Option<TrackId> {
        if let Some(n) = v.as_u64() {
            return s.track(TrackId(n)).map(|t| t.id);
        }
        if let Some(name) = v.as_str() {
            return s.track_by_name(name).map(|t| t.id);
        }
        None
    }
}
