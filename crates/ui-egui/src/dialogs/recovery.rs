//! The offer to recover unsaved work an earlier run left in the autosave folder.

use crate::SoundApp;
use crate::theme::{Tokens, bold};
use egui::{Align2, vec2};

/// Offer the newest leftover autosave, one at a time.
pub fn show(app: &mut SoundApp, ctx: &egui::Context) {
    let Some(left) = app.leftovers.first().cloned() else { return };
    let mut open = true;
    let mut recover = None;
    // Escape closes the question for now, as the window's close button does: the autosave stays for the next run.
    if crate::framework::escape_pressed(ctx) {
        open = false;
    }
    let window = egui::Window::new(egui::RichText::new(crate::localization::text("Recover Unsaved Work")).font(bold(13.0)))
        .id(egui::Id::unique("Recover Unsaved Work"))
        .open(&mut open)
        .collapsible(false)
        .resizable(false)
        .anchor(Align2::CENTER_CENTER, vec2(0.0, -80.0))
        .show(ctx, |ui| {
            let age = left.saved.elapsed().map_or_else(|_| crate::localization::text("recently").to_owned(), |d| ago(d.as_secs()));
            let what = crate::localization::text(if left.recording { "unsaved changes and an unfinished recording" } else { "unsaved changes" });
            ui.label(crate::localized_format!("“{name}” has {what} from an earlier run, autosaved {age}.", name = left.name, what = what, age = age));
            ui.separator();
            ui.horizontal(|ui| {
                let discard = ui.button(crate::localization::text("Discard"));
                if discard.clicked() {
                    recover = Some(false);
                }
                let recover_button =
                    egui::Button::new(egui::RichText::new(crate::localization::text("Recover")).strong()).fill(Tokens::current().accent);
                let can_recover = !app.engine.transport.recording;
                let recover_resp =
                    ui.add_enabled(can_recover, recover_button).on_disabled_hover_text(crate::localization::text("Stop recording first"));
                // The question starts on Recover, its default; on Discard while Recover is unavailable. Only while the focus
                // is outside the window, so Tab can still reach its close button.
                if !crate::dialogs::focus_is_on_layer(ui.ctx(), ui.layer_id()) {
                    if can_recover { recover_resp.request_focus() } else { discard.request_focus() }
                }
                if recover_resp.clicked() {
                    recover = Some(true);
                }
            });
        });
    if let Some(window) = &window {
        // It interrupts the start with a question about the user's work, so it is an alert.
        crate::dialogs::modal(ctx, &window.response, egui::accesskit::Role::AlertDialog);
    }
    if !open || recover.is_some() {
        app.leftovers.retain(|l| *l != left);
    }
    match recover {
        Some(true) => app.recover(&left),
        Some(false) => soundcraft_engine::autosave::discard(&left),
        None => {}
    }
}

fn ago(secs: u64) -> String {
    let (n, unit) = match secs {
        0..60 => return crate::localization::text("less than a minute ago").to_owned(),
        60..3_600 => (secs / 60, "minute"),
        3_600..86_400 => (secs / 3_600, "hour"),
        _ => (secs / 86_400, "day"),
    };
    let unit = crate::localization::text(unit);
    if n == 1 {
        crate::localized_format!("{n} {unit} ago", n = n, unit = unit)
    } else {
        crate::localized_format!("{n} {unit}s ago", n = n, unit = unit)
    }
}
