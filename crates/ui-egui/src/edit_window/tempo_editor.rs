//! Graphical tempo editing with one atomic command per gesture.
use super::layout::{sample_at, x_of};
use crate::{
    SoundApp,
    theme::{Tokens, regular},
};
use egui::{Align2, Color32, Rect, Sense, Shape, Stroke, Ui, pos2, vec2};
use serde_json::json;
use soundcraft_model::Session;
use std::sync::Arc;

#[derive(Clone)]
struct StrokeEdit {
    session: Arc<Session>,
    start: (i64, f64),
    end: (i64, f64),
    points: Vec<(i64, f64)>,
    original: Option<i64>,
    selecting: bool,
}

pub(super) fn draw(app: &mut SoundApp, ui: &mut Ui, s: &Arc<Session>, tl: Rect, row: Rect) {
    let lo = s.edit.value("tempo_editor.min_bpm", 80.0);
    let hi = s.edit.value("tempo_editor.max_bpm", 160.0).max(lo + 1.0);
    let density = s.edit.value("tempo_editor.density", 120.0).clamp(1.0, 3840.0) as i64;
    let shape = s.edit.flags.iter().find_map(|f| f.strip_prefix("tempo_editor.shape=")).unwrap_or("Free Hand");
    controls(app, ui, s, row, tl, (lo, hi));
    let painter = ui.painter().with_clip_rect(row);
    let selected = ui.ctx().data_mut(|d| d.get_temp::<(i64, i64)>(ui.scope_id().with("tempo_selection")));
    let color = Color32::from_rgb(140, 230, 160);
    painter.rect_filled(row, 0.0, Color32::from_rgb(24, 40, 32));
    let y_of = |bpm: f64| row.max.y - 4.0 - ((bpm - lo) / (hi - lo)) as f32 * (row.height() - 8.0);
    let mut line = Vec::new();
    for (i, e) in s.tempo.tempos().iter().enumerate() {
        let x = x_of(s, tl, s.tempo.tick_to_samples(e.tick, s.sample_rate));
        let end = s.tempo.tempos().get(i + 1).map_or(row.max.x, |n| x_of(s, tl, s.tempo.tick_to_samples(n.tick, s.sample_rate)));
        line.push(pos2(x, y_of(e.bpm)));
        line.push(pos2(end, y_of(e.bpm)));
        painter.circle_filled(pos2(x, y_of(e.bpm)), 3.0, color);
    }
    painter.add(Shape::line(line, Stroke::new(1.5, color)));
    for (bpm, y) in [(hi, row.min.y + 8.0), (lo, row.max.y - 8.0)] {
        painter.text(pos2(row.min.x + 4.0, y), Align2::LEFT_CENTER, format!("{bpm:.0}"), regular(9.0), color);
    }
    if let Some((a, b)) = selected {
        let x0 = x_of(s, tl, s.tempo.tick_to_samples(a, s.sample_rate));
        let x1 = x_of(s, tl, s.tempo.tick_to_samples(b, s.sample_rate));
        painter.rect_filled(Rect::from_min_max(pos2(x0, row.min.y), pos2(x1, row.max.y)), 0.0, Color32::from_rgba_unmultiplied(100, 180, 255, 45));
    }
    let resp = ui.interact(row, ui.scope_id().with("tempo_graph"), Sense::click_and_drag());
    let id = ui.scope_id().with("tempo_stroke");
    let mut edit = ui.ctx().data_mut(|d| d.get_temp::<StrokeEdit>(id));
    if ui.input(|i| !i.focused || i.key_pressed(egui::Key::Escape)) || edit.as_ref().is_some_and(|e| !Arc::ptr_eq(&e.session, s)) {
        edit = None;
    }
    let point = |p: egui::Pos2| {
        let tick = s.tempo.samples_to_ticks(sample_at(s, tl, p.x).max(0), s.sample_rate);
        let bpm = (lo + f64::from((row.max.y - 4.0 - p.y) / (row.height() - 8.0)) * (hi - lo)).clamp(5.0, 1000.0);
        (tick - tick.rem_euclid(density), (bpm * 10.0).round() / 10.0)
    };
    if resp.drag_started()
        && let Some(p) = ui.input(|i| i.pointer.press_origin())
    {
        let start = point(p);
        let nearest = s.tempo.tempos().iter().find(|e| {
            let x = x_of(s, tl, s.tempo.tick_to_samples(e.tick, s.sample_rate));
            (x - p.x).abs() < 7.0 && (y_of(e.bpm) - p.y).abs() < 9.0
        });
        let original = if s.edit.tool == soundcraft_model::Tool::Pencil { None } else { nearest.map(|e| e.tick) };
        edit = Some(StrokeEdit { session: s.clone(), start, end: start, points: vec![start], original, selecting: ui.input(|i| i.modifiers.shift) });
    }
    if let Some(e) = edit.as_mut()
        && resp.dragged()
        && let Some(p) = resp.interact_pointer_pos()
    {
        e.end = point(p);
        if shape == "Free Hand" && e.points.len() < 10_000 {
            e.points.push(e.end);
        }
        let preview = points(e, shape, density);
        painter.add(Shape::line(
            preview.iter().map(|(tick, bpm)| pos2(x_of(s, tl, s.tempo.tick_to_samples(*tick, s.sample_rate)), y_of(*bpm))).collect(),
            Stroke::new(2.0, Color32::WHITE),
        ));
    }
    if resp.drag_stopped()
        && let Some(e) = edit.take()
    {
        let points = points(&e, shape, density);
        if e.selecting {
            ui.ctx().data_mut(|d| d.insert_temp(ui.scope_id().with("tempo_selection"), (e.start.0.min(e.end.0), e.start.0.max(e.end.0))));
        } else {
            let mut params = json!({"points":points.iter().map(|(tick,bpm)|json!({"tick":tick,"bpm":bpm})).collect::<Vec<_>>()});
            if let Some(original) = e.original {
                params["remove_ticks"] = json!([original]);
            } else {
                params["start_tick"] = json!(e.start.0.min(e.end.0));
                params["end_tick"] = json!(e.start.0.max(e.end.0).saturating_add(density));
            }
            let _ = app.run("event.tempo_points", params);
        }
    }
    if resp.clicked()
        && let Some(p) = resp.interact_pointer_pos()
    {
        if ui.input(|i| i.modifiers.alt) {
            if let Some(event) = s
                .tempo
                .tempos()
                .iter()
                .filter(|e| e.tick > 0)
                .min_by_key(|e| ((x_of(s, tl, s.tempo.tick_to_samples(e.tick, s.sample_rate)) - p.x).abs() * 10.0) as i64)
            {
                let _ = app.run("event.tempo_remove", json!({"at":s.tempo.tick_to_samples(event.tick,s.sample_rate)}));
            }
        } else {
            let (tick, bpm) = point(p);
            let _ = app.run("event.tempo_points", json!({"points":[{"tick":tick,"bpm":bpm}]}));
        }
    }
    ui.ctx().data_mut(|d| {
        d.remove::<StrokeEdit>(id);
        if let Some(edit) = edit {
            d.insert_temp(id, edit);
        }
    });
}
fn points(e: &StrokeEdit, shape: &str, density: i64) -> Vec<(i64, f64)> {
    if e.original.is_some() {
        return vec![(if e.original == Some(0) { 0 } else { e.end.0 }, e.end.1)];
    }
    if shape == "Free Hand" {
        return e.points.iter().copied().collect::<std::collections::BTreeMap<_, _>>().into_iter().collect();
    }
    let (a, b) = if e.start.0 <= e.end.0 { (e.start, e.end) } else { (e.end, e.start) };
    let count = ((b.0 - a.0) / density).clamp(1, 9999);
    (0..=count)
        .map(|i| {
            let f = i as f64 / count as f64;
            let g = match shape {
                "Parabolic" => f * f,
                "S-Curve" => 0.5 - 0.5 * (f * std::f64::consts::PI).cos(),
                _ => f,
            };
            (a.0 + ((b.0 - a.0) as f64 * f).round() as i64, a.1 + (b.1 - a.1) * g)
        })
        .collect()
}
fn controls(app: &mut SoundApp, ui: &mut Ui, s: &Session, row: Rect, tl: Rect, range: (f64, f64)) {
    let rect = Rect::from_min_max(pos2(tl.min.x - 130.0, row.min.y), pos2(tl.min.x - 4.0, row.max.y));
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect));
    let mut shape = s.edit.flags.iter().find_map(|f| f.strip_prefix("tempo_editor.shape=")).unwrap_or("Free Hand").to_string();
    let before = shape.clone();
    craft_ui::ComboBox::from_id_salt("tempo_shape").selected_text(&shape).width(116.0).show_ui(&mut child, |ui| {
        for value in ["Free Hand", "Line", "Parabolic", "S-Curve"] {
            ui.selectable_value(&mut shape, value.into(), value);
        }
    });
    if shape != before {
        let _ = app.run("view.tempo_editor", json!({"shape":shape}));
    }
    child.horizontal(|ui| {
        for (label, factor) in [("−", 1.5), ("+", 0.6667)] {
            if ui.button(label).clicked() {
                let center = (range.0 + range.1) * 0.5;
                let half = (range.1 - range.0) * 0.5 * factor;
                let _ = app.run("view.tempo_editor", json!({"min_bpm":(center-half).max(5.0),"max_bpm":(center+half).min(1000.0)}));
            }
        }
    });
    let mut density = s.edit.value("tempo_editor.density", 120.0) as i64;
    child.horizontal(|ui| {
        ui.label("Ticks");
        if ui.add(egui::DragValue::new(&mut density).range(1..=3840)).changed() {
            let _ = app.run("view.tempo_editor", json!({"density":density}));
        }
    });
    if let Some((a, b)) = ui.ctx().data_mut(|d| d.get_temp::<(i64, i64)>(ui.scope_id().with("tempo_selection"))) {
        child.horizontal(|ui| {
            for (label, factor) in [("Scale −", 0.95), ("Scale +", 1.05)] {
                if ui.button(label).clicked() {
                    let points: Vec<_> = s
                        .tempo
                        .tempos()
                        .iter()
                        .filter(|e| e.tick >= a && e.tick <= b)
                        .map(|e| json!({"tick":e.tick,"bpm":(e.bpm*factor).clamp(5.0,1000.0)}))
                        .collect();
                    if !points.is_empty() {
                        let _ = app.run("event.tempo_points", json!({"points":points}));
                    }
                }
            }
        });
    }
    let mut center = (range.0 + range.1) * 0.5;
    if child.add(egui::DragValue::new(&mut center).prefix("BPM ").range(5.0..=1000.0)).changed() {
        let half = (range.1 - range.0) * 0.5;
        let _ = app.run("view.tempo_editor", json!({"min_bpm":(center-half).max(5.0),"max_bpm":(center+half).min(1000.0)}));
    }
    let resize = Rect::from_min_size(pos2(rect.min.x, row.max.y - 4.0), vec2(row.width() + 130.0, 4.0));
    let response = ui.interact(resize, ui.scope_id().with("tempo_resize"), Sense::drag()).on_hover_cursor(egui::CursorIcon::ResizeVertical);
    if response.dragged() {
        let _ = app.run("view.tempo_editor", json!({"height":f64::from(row.height()+response.drag_delta().y)}));
    }
    child.painter().hline(rect.x_range(), row.max.y - 1.0, Stroke::new(1.0, Tokens::current().border));
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pencil_shapes_are_bounded_monotonic_and_keep_endpoints() {
        let e = StrokeEdit {
            session: Arc::new(Session::default()),
            start: (0, 100.0),
            end: (960, 180.0),
            points: vec![(0, 100.0), (960, 180.0)],
            original: None,
            selecting: false,
        };
        for shape in ["Line", "Parabolic", "S-Curve"] {
            let p = points(&e, shape, 120);
            assert_eq!(p.first(), Some(&(0, 100.0)));
            assert_eq!(p.last(), Some(&(960, 180.0)));
            assert!(p.windows(2).all(|p| p[0].0 < p[1].0 && p[0].1 <= p[1].1));
        }
        let mut moving = e;
        moving.original = Some(0);
        assert_eq!(points(&moving, "Line", 120), vec![(0, 180.0)]);
    }
}
