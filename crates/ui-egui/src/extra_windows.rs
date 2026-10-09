//! Window menu extras: the Renderer (speaker layout of the main output with live levels and the
//! object tracks' positions), UI Customization, and auto-updating the active window configuration.

use crate::SoundApp;
use crate::i18n::tr;
use crate::theme::{Tokens, bold, regular};
use egui::{Align2, Color32, Sense, Stroke, pos2, vec2};
use serde_json::{Value, json};
use soundcraft_model::{Speaker, TrackKind};

/// How often an auto-updating configuration is refreshed (seconds).
const AUTO_UPDATE_SECS: f64 = 2.0;

#[derive(Debug, Clone)]
pub struct ExtraState {
    pub show_renderer: bool,
    pub show_ui_customization: bool,
    pub auto_update_config: bool,
    pub ui_scale: f32,
    pub reduce_motion: bool,
    pub tooltips: bool,
    last_auto_update: f64,
    applied: Option<(f32, bool)>,
}

impl Default for ExtraState {
    fn default() -> Self {
        Self {
            show_renderer: false,
            show_ui_customization: false,
            auto_update_config: false,
            ui_scale: 1.0,
            reduce_motion: false,
            tooltips: true,
            last_auto_update: 0.0,
            applied: None,
        }
    }
}

/// UI commands handled here (`window.renderer`, `window.ui_customization`, `window.config_auto_update`).
pub fn run(app: &mut SoundApp, id: &str, p: &Value) -> Option<Value> {
    let flag = |v: &mut bool| {
        *v = p.get("value").and_then(Value::as_bool).unwrap_or(!*v);
        json!({"value": *v})
    };
    Some(match id {
        "window.renderer" => flag(&mut app.extra.show_renderer),
        "window.ui_customization" => flag(&mut app.extra.show_ui_customization),
        "window.config_auto_update" => flag(&mut app.extra.auto_update_config),
        _ => return None,
    })
}

pub fn show(app: &mut SoundApp, ctx: &egui::Context) {
    apply_customization(app, ctx);
    auto_update(app, ctx);
    renderer(app, ctx);
    ui_customization(app, ctx);
}

fn apply_customization(app: &mut SoundApp, ctx: &egui::Context) {
    let want = (app.extra.ui_scale.clamp(0.75, 1.5), app.extra.reduce_motion);
    if app.extra.applied == Some(want) {
        return;
    }
    // Only touch the zoom once the user has changed it, so platform scaling stays untouched.
    if app.extra.applied.is_some() || (want.0 - 1.0).abs() > f32::EPSILON {
        ctx.set_zoom_factor(want.0);
    }
    ctx.global_style_mut(|s| s.animation_time = if want.1 { 0.0 } else { 1.0 / 12.0 });
    app.extra.applied = Some(want);
}

fn auto_update(app: &mut SoundApp, ctx: &egui::Context) {
    if !app.extra.auto_update_config || app.ui.configurations.is_empty() {
        return;
    }
    let now = ctx.input(|i| i.time);
    if now - app.extra.last_auto_update >= AUTO_UPDATE_SECS {
        app.extra.last_auto_update = now;
        let _ = crate::menus::run_ui_command(app, "window.config_update", &json!({}));
    }
}

fn renderer(app: &mut SoundApp, ctx: &egui::Context) {
    let mut open = app.extra.show_renderer;
    if !open {
        return;
    }
    let t = Tokens::current();
    let fmt = app.engine.session().main_format();
    let peaks: Vec<f32> = app.player.as_ref().map(|p| p.meters().main.peaks).unwrap_or_default();
    let objects: Vec<(String, f32, f32, f32)> = app
        .engine
        .session()
        .tracks
        .iter()
        .filter(|tr| tr.kind != TrackKind::Master && app.engine.session().edit.flag(&format!("object.{}", tr.id.0)))
        .map(|tr| {
            let sp = tr.mixer.surround.unwrap_or_default();
            (tr.name.clone(), sp.x, sp.y, sp.z)
        })
        .collect();
    egui::Window::new(egui::RichText::new(tr("Renderer")).font(bold(13.0)))
        .id(egui::Id::new("Renderer"))
        .open(&mut open)
        .default_size(vec2(380.0, 440.0))
        .resizable(true)
        .show(ctx, |ui| {
            ui.label(
                egui::RichText::new(crate::i18n::render("Main output: {0}", &[("{0}", (tr(fmt.label())).to_string())]))
                    .font(regular(11.0))
                    .color(t.text),
            );
            let side = ui.available_width().min(360.0);
            let (r, _) = ui.allocate_exact_size(vec2(side, side), Sense::hover());
            let p = ui.painter_at(r);
            p.rect_filled(r, 6.0, Color32::from_rgb(16, 19, 23));
            let c = r.center();
            let rad = side * 0.40;
            p.circle_stroke(c, rad, Stroke::new(1.0, Color32::from_rgb(60, 68, 78)));
            p.circle_stroke(c, rad * 0.55, Stroke::new(1.0, Color32::from_rgb(44, 50, 58)));
            p.text(pos2(c.x, r.min.y + 8.0), Align2::CENTER_TOP, "front", regular(9.0), t.text_dim);
            for (i, sp) in fmt.speakers().iter().enumerate() {
                let level = peaks.get(i).copied().unwrap_or(0.0).clamp(0.0, 1.0);
                let ring = if sp.is_height() { rad * 0.55 } else { rad };
                let a = sp.azimuth().to_radians();
                let pos = if *sp == Speaker::Lfe { pos2(c.x, c.y + rad * 0.2) } else { pos2(c.x + ring * a.sin(), c.y - ring * a.cos()) };
                let glow = Color32::from_rgb(40 + (level * 60.0) as u8, 90 + (level * 140.0) as u8, 70 + (level * 40.0) as u8);
                p.circle_filled(pos, 9.0 + level * 6.0, glow);
                p.circle_stroke(pos, 9.0, Stroke::new(1.0, Color32::from_rgb(150, 160, 170)));
                p.text(pos, Align2::CENTER_CENTER, format!("{sp:?}"), bold(8.0), Color32::WHITE);
            }
            for (name, x, y, z) in &objects {
                let pos = pos2(c.x + x.clamp(-1.0, 1.0) * rad, c.y - y.clamp(-1.0, 1.0) * rad);
                p.circle_filled(pos, 5.0 + z.clamp(0.0, 1.0) * 3.0, Color32::from_rgb(240, 180, 60));
                p.text(pos2(pos.x + 8.0, pos.y), Align2::LEFT_CENTER, name, regular(9.0), Color32::from_rgb(240, 200, 120));
            }
            ui.add_space(4.0);
            if objects.is_empty() {
                ui.label(
                    egui::RichText::new(tr("No object tracks. Use the Object view in the Mix window to route tracks as objects."))
                        .small()
                        .color(t.text_dim),
                );
            } else {
                ui.label(
                    egui::RichText::new(crate::i18n::render("{0} object track(s)", &[("{0}", (objects.len()).to_string())]))
                        .small()
                        .color(t.text_dim),
                );
            }
        });
    app.extra.show_renderer = open;
}

fn ui_customization(app: &mut SoundApp, ctx: &egui::Context) {
    let mut open = app.extra.show_ui_customization;
    if !open {
        return;
    }
    egui::Window::new(egui::RichText::new(tr("Appearance")).font(bold(13.0)))
        .id(egui::Id::new("Appearance"))
        .open(&mut open)
        .default_size(vec2(320.0, 180.0))
        .resizable(false)
        .show(ctx, |ui| {
            egui::Grid::new("ui_custom").num_columns(2).spacing(vec2(12.0, 8.0)).show(ui, |ui| {
                ui.label(crate::i18n::tr("Language"));
                egui::ComboBox::from_id_salt("ui_language").selected_text(app.ui.language.label()).show_ui(ui, |ui| {
                    for language in crate::i18n::Language::ALL {
                        if ui.selectable_value(&mut app.ui.language, language, language.label()).changed() {
                            ui.ctx().request_repaint();
                        }
                    }
                });
                ui.end_row();
                ui.label(tr("Interface scale"));
                ui.add(egui::Slider::new(&mut app.extra.ui_scale, 0.75..=1.5).step_by(0.05).custom_formatter(|v, _| format!("{:.0} %", v * 100.0)));
                ui.end_row();
                ui.label(tr("Reduce motion"));
                ui.checkbox(&mut app.extra.reduce_motion, "");
                ui.end_row();
                ui.label(tr("Theme"));
                let mut chosen = None;
                egui::ComboBox::from_id_salt("theme_mode").selected_text(tr(app.ui.theme.label())).show_ui(ui, |ui| {
                    for mode in crate::theme::ThemeMode::ALL {
                        if ui.selectable_label(app.ui.theme == mode, tr(mode.label())).clicked() {
                            chosen = Some(mode);
                        }
                    }
                });
                if let Some(mode) = chosen {
                    let _ = app.run("ui.theme", json!({"mode": mode.id()}));
                }
                ui.end_row();
                ui.label(tr("Auto-update configuration"));
                ui.checkbox(&mut app.extra.auto_update_config, "");
                ui.end_row();
            });
            if ui.button(tr("Reset")).clicked() {
                app.extra.ui_scale = 1.0;
                app.extra.reduce_motion = false;
                let _ = app.run("ui.theme", json!({"mode": crate::theme::ThemeMode::default().id()}));
            }
        });
    app.extra.show_ui_customization = open;
}
