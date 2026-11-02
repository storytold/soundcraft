//! The offer to recover unsaved work an earlier run left in the autosave folder.

use crate::SoundApp;
use crate::i18n::{tr, trf};
use crate::theme::{Tokens, bold};
use egui::{Align2, vec2};

/// Offer the newest leftover autosave, one at a time.
pub fn show(app: &mut SoundApp, ctx: &egui::Context) {
    let Some(left) = app.leftovers.first().cloned() else { return };
    let mut open = true;
    let mut recover = None;
    egui::Window::new(egui::RichText::new(tr("Recover Unsaved Work")).font(bold(13.0)))
        .open(&mut open)
        .collapsible(false)
        .resizable(false)
        .anchor(Align2::CENTER_CENTER, vec2(0.0, -80.0))
        .show(ctx, |ui| {
            let age = left.saved.elapsed().map_or_else(|_| tr("recently").into(), |d| ago(d.as_secs()));
            let what = if left.recording {
                "“{}” has unsaved changes and an unfinished recording from an earlier run, autosaved {}."
            } else {
                "“{}” has unsaved changes from an earlier run, autosaved {}."
            };
            ui.label(trf(what, &[&left.name, &age]));
            ui.separator();
            ui.horizontal(|ui| {
                if ui.button(tr("Discard")).clicked() {
                    recover = Some(false);
                }
                let recover_button = egui::Button::new(egui::RichText::new(tr("Recover")).strong()).fill(Tokens::current().accent);
                if ui.add_enabled(!app.engine.transport.recording, recover_button).on_disabled_hover_text(tr("Stop recording first")).clicked() {
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
    let (n, one, many) = match secs {
        0..60 => return tr("less than a minute ago").into(),
        60..3_600 => (secs / 60, "1 minute ago", "{} minutes ago"),
        3_600..86_400 => (secs / 3_600, "1 hour ago", "{} hours ago"),
        _ => (secs / 86_400, "1 day ago", "{} days ago"),
    };
    if n == 1 { tr(one).into() } else { trf(many, &[&n]) }
}
