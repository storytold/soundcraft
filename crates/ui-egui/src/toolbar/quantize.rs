//! Compact quantize settings. Every edit dispatches the existing input-quantize command;
//! applying to selected notes uses the same values with `event.quantize`.

use super::{at, icon_at, key, key_fill, lcd, slot};
use crate::SoundApp;
use crate::theme::{Tokens, bold, mono, regular};
use egui::{Align2, Color32, Rect, Sense, Stroke, Ui, pos2};
use serde_json::json;
use soundcraft_time::TICKS_PER_QUARTER;

fn grids() -> impl Iterator<Item = (&'static str, i64)> {
    [
        ("1/1 note", TICKS_PER_QUARTER * 4),
        ("1/2 note", TICKS_PER_QUARTER * 2),
        ("1/4 note", TICKS_PER_QUARTER),
        ("1/8 note", TICKS_PER_QUARTER / 2),
        ("1/8 triplet", TICKS_PER_QUARTER / 3),
        ("1/16 note", TICKS_PER_QUARTER / 4),
        ("1/16 triplet", TICKS_PER_QUARTER / 6),
        ("1/32 note", TICKS_PER_QUARTER / 8),
        ("1/64 note", TICKS_PER_QUARTER / 16),
    ]
    .into_iter()
}

fn grid_ticks(app: &SoundApp) -> i64 {
    let ticks = app.engine.session().edit.value("midi.input_quantize.grid_ticks", (TICKS_PER_QUARTER / 4) as f64);
    if ticks.is_finite() { ticks.clamp(1.0, (TICKS_PER_QUARTER * 64) as f64) as i64 } else { TICKS_PER_QUARTER / 4 }
}

fn grid_menu(app: &mut SoundApp, resp: &egui::Response) {
    egui::Popup::menu(resp).show(|ui| {
        let current = grid_ticks(app);
        for (label, ticks) in grids() {
            if ui.selectable_label(current == ticks, label).clicked() {
                let _ = app.run("event.input_quantize", json!({"grid": ticks}));
                ui.close();
            }
        }
    });
}

fn options_menu(app: &mut SoundApp, resp: &egui::Response) {
    egui::Popup::menu(resp).show(|ui| {
        let edit = &app.engine.session().edit;
        let midi = edit.flag("view.transport.midi_controls");
        let focus = edit.keyboard_focus == "commands";
        for (label, command, on) in [
            ("MIDI Controls (Tempo, Meter, Count Off)", "view.transport_midi_controls", midi),
            ("Commands Keyboard Focus", "options.keyboard_focus", focus),
        ] {
            if ui.selectable_label(on, label).clicked() {
                let _ = app.run(command, json!({}));
                ui.close();
            }
        }
        ui.separator();
        if ui.button("MIDI Operations…").clicked() {
            let _ = app.run("window.midi_ops", json!({}));
            ui.close();
        }
    });
}

fn percent(ui: &mut Ui, r: Rect, value: &mut f64, name: &str, color: Color32) -> bool {
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(r));
    child.spacing_mut().interact_size.y = r.height();
    child.style_mut().override_font_id = Some(mono(9.0));
    child.visuals_mut().override_text_color = Some(color);
    let widgets = &mut child.visuals_mut().widgets;
    for widget in [&mut widgets.inactive, &mut widgets.hovered, &mut widgets.active] {
        widget.weak_bg_fill = Color32::TRANSPARENT;
        widget.bg_stroke = Stroke::NONE;
    }
    child
        .add_sized(r.size(), egui::DragValue::new(value).speed(1.0).range(0.0..=100.0).fixed_decimals(0).suffix("%"))
        .accessible_name(name)
        .on_hover_text("Drag to adjust, or click to type a percentage")
        .changed()
}

pub(super) fn show(app: &mut SoundApp, ui: &mut Ui) {
    let t = Tokens::current();
    let g = slot(ui, 208.0);
    let r = at(g, 16.0, 9.0, 190.0, 43.0);
    lcd(ui, r);
    let ticks = grid_ticks(app);
    let label = grids().find(|(_, value)| *value == ticks).map_or_else(|| format!("{ticks} ticks"), |(label, _)| label.to_string());
    ui.painter().text(pos2(r.min.x + 4.0, r.min.y + 10.0), Align2::LEFT_CENTER, "Grid:", regular(9.0), t.counter_label);
    crate::icons::draw(ui.painter(), at(r, 31.0, 2.0, 15.0, 15.0), "note", t.toolbar_readout);
    ui.painter().text(pos2(r.max.x - 18.0, r.min.y + 10.0), Align2::RIGHT_CENTER, label, mono(9.5), t.toolbar_readout);
    crate::icons::draw(ui.painter(), at(r, 176.0, 5.0, 10.0, 10.0), "triangle_down", t.toolbar_icon);
    let grid = ui.interact(at(r, 30.0, 0.0, 160.0, 21.0), ui.scope_id().with("quantize_grid"), Sense::click());
    grid.widget_info(|| egui::WidgetInfo::labeled(egui::Role::ComboBox, ui.is_enabled(), "Quantize grid"));
    grid_menu(app, &grid);

    let e = &app.engine.session().edit;
    let strength = e.value("midi.input_quantize.strength", 100.0);
    let swing = e.value("midi.input_quantize.swing", 0.0);
    let enabled = e.flag("midi.input_quantize");
    let mut strength_edit = if strength.is_finite() { strength.clamp(0.0, 100.0) } else { 100.0 };
    let mut swing_edit = if swing.is_finite() { swing.clamp(0.0, 100.0) } else { 0.0 };
    ui.painter().text(pos2(r.min.x + 4.0, r.min.y + 31.0), Align2::LEFT_CENTER, "Strength:", bold(8.5), t.toolbar_readout);
    ui.painter().text(pos2(r.min.x + 104.0, r.min.y + 31.0), Align2::LEFT_CENTER, "Swing:", bold(8.5), t.toolbar_readout);
    let strength_changed = percent(ui, at(r, 54.0, 23.0, 43.0, 16.0), &mut strength_edit, "Quantize strength (%)", t.toolbar_readout);
    let swing_changed = percent(ui, at(r, 140.0, 23.0, 45.0, 16.0), &mut swing_edit, "Quantize swing (%)", t.toolbar_readout);
    if strength_changed || swing_changed {
        let _ = app.run("event.input_quantize", json!({"strength": strength_edit, "swing": swing_edit}));
    }
    let q = at(g, 16.0, 55.0, 30.0, 18.0);
    let can_apply =
        soundcraft_engine::cmd::command_specs().iter().find(|c| c.id == "event.quantize").is_some_and(|c| (c.enabled)(&app.engine).is_ok());
    ui.add_enabled_ui(can_apply, |ui| {
        let resp = ui.interact(q, ui.scope_id().with("quantize_apply"), Sense::click());
        resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, ui.is_enabled(), "Apply quantize"));
        key(ui.painter(), q, &resp, key_fill(&resp));
        ui.painter().text(q.center(), Align2::CENTER_CENTER, "Q", regular(13.0), if can_apply { t.toolbar_icon } else { t.text_dim });
        if resp.on_hover_text("Quantize selected MIDI notes using the toolbar settings").clicked() {
            let _ = app.run("event.quantize", json!({"grid": ticks, "strength": strength_edit, "swing": swing_edit}));
        }
    });
    if icon_at(ui, at(g, 51.0, 55.0, 30.0, 18.0), "input_quantize", enabled, "Input Quantize", t.toolbar_icon).clicked() {
        let _ = app.run("event.input_quantize", json!({"enabled": !enabled}));
    }
    let gear = icon_at(ui, at(g, 109.0, 55.0, 97.0, 18.0), "gear", false, "Toolbar options", t.toolbar_icon);
    options_menu(app, &gear);
}
