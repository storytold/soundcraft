use super::{SoundApp, Tokens, Ui, json};
use crate::i18n::tr;

pub(crate) fn undo_history(app: &mut SoundApp, ui: &mut Ui) {
    let hist = app.engine.undo_history();
    let n = hist.len();
    egui::ScrollArea::vertical().show(ui, |ui| {
        for (i, h) in hist.iter().enumerate() {
            if ui.selectable_label(i + 1 == n, format!("{}  {}", i + 1, tr(h))).clicked() {
                for _ in 0..(n - i - 1) {
                    let _ = app.run("edit.undo", json!({}));
                }
            }
        }
        if let Some(r) = app.engine.redo_label() {
            ui.label(egui::RichText::new(crate::i18n::trf("(redo) {}", &[&tr(r)])).italics().color(Tokens::current().text_dim));
        }
    });
}
