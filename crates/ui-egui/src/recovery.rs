//! The offer to recover unsaved work an earlier run left in the autosave folder.

use crate::SoundApp;
use crate::theme::{Tokens, bold};
use egui::{Align2, vec2};

/// Offer the newest leftover autosave, one at a time.
pub fn show(app: &mut SoundApp, ctx: &egui::Context) {
    let Some(left) = app.leftovers.first().cloned() else { return };
    let mut open = true;
    let mut recover = None;
    egui::Window::new(egui::RichText::new(crate::i18n::t("Recover Unsaved Work")).font(bold(13.0)))
        .open(&mut open)
        .collapsible(false)
        .resizable(false)
        .anchor(Align2::CENTER_CENTER, vec2(0.0, -80.0))
        .show(ctx, |ui| {
            let age = left.saved.elapsed().map_or_else(|_| crate::i18n::t("recently").to_string(), |d| ago(d.as_secs()));
            let what = if left.recording { crate::i18n::t("unsaved changes and an unfinished recording") } else { crate::i18n::t("unsaved changes") };
            ui.label(crate::i18n::fmt(
                crate::i18n::t("“{name}” has {what} from an earlier run, autosaved {age}."),
                &[("name", &left.name), ("what", what), ("age", &age)],
            ));
            ui.separator();
            ui.horizontal(|ui| {
                if ui.button(crate::i18n::t("Discard")).clicked() {
                    recover = Some(false);
                }
                let recover_button = egui::Button::new(egui::RichText::new(crate::i18n::t("Recover")).strong()).fill(Tokens::current().accent);
                if ui
                    .add_enabled(!app.engine.transport.recording, recover_button)
                    .on_disabled_hover_text(crate::i18n::t("Stop recording first"))
                    .clicked()
                {
                    recover = Some(true);
                }
            });
        });
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
    let (n, one, other) = match secs {
        0..60 => return crate::i18n::t("less than a minute ago").to_string(),
        60..3_600 => (secs / 60, "{n} minute ago", "{n} minutes ago"),
        3_600..86_400 => (secs / 3_600, "{n} hour ago", "{n} hours ago"),
        _ => (secs / 86_400, "{n} day ago", "{n} days ago"),
    };
    crate::i18n::trn(crate::i18n::current(), n, one, other)
}
