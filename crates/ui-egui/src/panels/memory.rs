use super::{SoundApp, Ui, json, mono};

/// One recallable memory-location row: number button, editable name, time, length, kind and delete.
fn memory_location_row(app: &mut SoundApp, ui: &mut Ui, n: u32, name: &str, loc: &str, len: &str, kind: &str) {
    if ui.button(n.to_string()).on_hover_text("Recall").clicked() {
        let _ = app.run("markers.recall", json!({"number": n}));
    }
    let key = egui::Id::unique(("mem_name", n));
    let mut edit: String = ui.ctx().memory(|m| m.data.get_temp(key)).unwrap_or_else(|| name.to_string());
    let r = crate::widgets::named_text_field(ui.add(egui::TextEdit::singleline(&mut edit).desired_width(140.0)), "Name");
    if r.changed() {
        ui.ctx().memory_mut(|m| m.data.insert_temp(key, edit.clone()));
    }
    if r.lost_focus() && edit != name {
        let _ = app.run("markers.edit", json!({"number": n, "rename": edit}));
        ui.ctx().memory_mut(|m| m.data.remove::<String>(key));
    }
    ui.label(egui::RichText::new(loc).font(mono(11.0)));
    ui.label(egui::RichText::new(len).font(mono(11.0)));
    ui.label(kind);
    if crate::widgets::close_button(ui, "Delete marker").clicked() {
        let _ = app.run("markers.delete", json!({"number": n}));
    }
    ui.end_row();
}

pub(crate) fn memory_locations(app: &mut SoundApp, ui: &mut Ui) {
    ui.horizontal(|ui| {
        if ui.button("+ Marker").clicked() {
            let _ = app.run("markers.add", json!({}));
        }
        if ui.button("+ Selection").clicked() {
            let _ = app.run("markers.add", json!({"kind": "selection"}));
        }
    });
    let rows: Vec<(u32, String, String, String, String)> = {
        let s = app.engine.session();
        let f = |x: i64| s.format_position(x, s.edit.main_counter);
        s.markers
            .iter()
            .map(|m| {
                let kind = match m.kind {
                    soundcraft_model::MarkerKind::Marker => "marker",
                    soundcraft_model::MarkerKind::Selection => "selection",
                    soundcraft_model::MarkerKind::None => "none",
                };
                let len = if m.kind == soundcraft_model::MarkerKind::Selection { f(m.end - m.start) } else { String::new() };
                (m.number, m.name.clone(), f(m.start), len, kind.to_string())
            })
            .collect()
    };
    egui::ScrollArea::vertical().show(ui, |ui| {
        egui::Grid::new("mem_grid").striped(true).num_columns(6).show(ui, |ui| {
            for h in ["#", "Name", "Location", "Length", "Type", ""] {
                ui.label(egui::RichText::new(h).strong());
            }
            ui.end_row();
            for (n, name, loc, len, kind) in &rows {
                memory_location_row(app, ui, *n, name, loc, len, kind);
            }
        });
    });
}
