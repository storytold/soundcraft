use super::{SoundApp, Value, bounce, menus};

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
                Ok(v)
            }
            Err(e) => {
                self.ui.status = error_text(&e);
                log::warn!("{id}: {e}");
                Err(e.to_string())
            }
        }
    }
}

/// An engine error in the display language: the error's fixed wording is translated, and so are the
/// stock reasons commands give and the messages that match a template; names and numbers pass through.
fn error_text(e: &soundcraft_engine::EngineError) -> String {
    use crate::i18n;
    use soundcraft_engine::EngineError as E;
    match e {
        E::UnknownCommand(id) => i18n::trf("unknown command `{}`", &[id]),
        E::Disabled(id, why) => i18n::trf("`{}` is not available: {}", &[id, &i18n::tr_message(why)]),
        E::BadParams(id, why) => i18n::trf("bad parameters for `{}`: {}", &[id, &i18n::tr_message(why)]),
        E::NotFound(m) | E::Io(m) => i18n::tr_message(m).into_owned(),
        E::Unsupported(m) => i18n::trf("unsupported: {}", &[&i18n::tr_message(m)]),
        E::Internal(id, m) => i18n::trf("internal error in `{}`: {}", &[id, m]),
    }
}
