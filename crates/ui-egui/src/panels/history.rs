use super::{SoundApp, Tokens, Ui, json};

pub(crate) fn undo_history(app: &mut SoundApp, ui: &mut Ui) {
    let hist = app.engine.undo_history();
    let n = hist.len();
    egui::ScrollArea::vertical().show(ui, |ui| {
        for (i, h) in hist.iter().enumerate() {
            if ui.selectable_label(i + 1 == n, format!("{}  {h}", i + 1)).clicked() {
                for _ in 0..(n - i - 1) {
                    let _ = app.run("edit.undo", json!({}));
                }
            }
        }
        if let Some(r) = app.engine.redo_label() {
            ui.label(egui::RichText::new(format!("(redo) {r}")).italics().color(Tokens::current().text_dim));
        }
    });
}
