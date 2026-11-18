//! The face of a built-in plugin's window: its parameters as rotary knobs in columns, one column
//! per band or section (an EQ's LF, LMF … HF; a channel strip's Gate, Comp, EQ), as hardware-style
//! plugins lay them out, with a response graph above for EQs and a transfer curve with a
//! gain-reduction meter for dynamics.
//!
//! Third-party plugins keep the plain list: their parameter names follow no convention here.

use crate::theme::{Tokens, bold, mono, regular};
use egui::{Align2, Color32, Rect, Sense, Shape, Stroke, StrokeKind, Ui, pos2, vec2};
use soundcraft_dsp::{ParamInfo, PluginInfo, Taper, Unit};

/// A knob's diameter, its column's width and the gap between columns.
const KNOB: f32 = 38.0;
const COLUMN_W: f32 = 74.0;
/// How far a drag must travel (points) to sweep the whole range; Shift divides the speed by 10.
const DRAG_RANGE: f32 = 220.0;

/// One column of the panel: its title (empty for a lone control) and its parameters.
struct Column<'a> {
    title: &'a str,
    params: Vec<&'a ParamInfo>,
}

/// The part of a name before its first space, when another parameter shares it ("LF Freq",
/// "LF Gain" → "LF"); a name that is only the prefix ("HPF" beside "HPF Freq") belongs too.
fn prefix(name: &str) -> &str {
    name.split_once(' ').map_or(name, |(head, _)| head)
}

/// Columns in the order their first parameter appears, so Input stays first and Output last.
fn columns(info: &PluginInfo) -> Vec<Column<'_>> {
    let shared = |p: &str| info.params.iter().filter(|q| prefix(q.name) == p).count() >= 2;
    let mut out: Vec<Column<'_>> = Vec::new();
    for p in info.params {
        let head = prefix(p.name);
        if shared(head) {
            if let Some(c) = out.iter_mut().find(|c| c.title == head) {
                c.params.push(p);
                continue;
            }
            out.push(Column { title: head, params: vec![p] });
        } else {
            out.push(Column { title: "", params: vec![p] });
        }
    }
    // Lone knobs side by side share one untitled column of at most three, rather than a column each.
    let mut merged: Vec<Column<'_>> = Vec::new();
    for c in out {
        match merged.last_mut() {
            Some(last) if last.title.is_empty() && c.title.is_empty() && last.params.len() < 3 => last.params.extend(c.params),
            _ => merged.push(c),
        }
    }
    merged
}

/// The label under a knob: the name without its column's prefix ("LF Freq" in LF → "Freq").
fn short_name<'a>(name: &'a str, title: &str) -> &'a str {
    if title.is_empty() {
        return name;
    }
    match name.strip_prefix(title).map(str::trim) {
        Some("") => "On",
        Some(rest) => rest,
        None => name,
    }
}

/// A value as 0..1 knob travel, honouring a logarithmic taper.
fn to_travel(p: &ParamInfo, v: f32) -> f32 {
    let (lo, hi) = (p.min, p.max);
    if hi <= lo {
        return 0.0;
    }
    let t = if p.taper == Taper::Log && lo > 0.0 { (v.max(lo) / lo).ln() / (hi / lo).ln() } else { (v - lo) / (hi - lo) };
    if t.is_finite() { t.clamp(0.0, 1.0) } else { 0.0 }
}

fn from_travel(p: &ParamInfo, t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    let v = if p.taper == Taper::Log && p.min > 0.0 { p.min * (p.max / p.min).powf(t) } else { p.min + (p.max - p.min) * t };
    p.clamp(v)
}

/// A rotary knob for `p`: drag up or down (Shift for fine steps), double-click or Alt-click for
/// the default. Returns the new value when it changed.
fn knob(ui: &mut Ui, p: &ParamInfo, value: f32, label: &str, shown: &str, color: Color32) -> Option<f32> {
    let t = Tokens::current();
    let (r, resp) = ui.allocate_exact_size(vec2(COLUMN_W - 8.0, KNOB + 30.0), Sense::click_and_drag());
    let resp = resp.on_hover_text(format!("{label}: {shown}"));
    let center = pos2(r.center().x, r.min.y + 12.0 + KNOB * 0.5);
    let p_ = ui.painter();
    p_.text(pos2(r.center().x, r.min.y + 5.0), Align2::CENTER_CENTER, label, regular(10.0), t.text_dim);
    // Body, track and value arc from 7 o'clock to 5 o'clock.
    let (a0, a1) = (std::f32::consts::PI * 0.75, std::f32::consts::PI * 2.25);
    let arc = |from: f32, to: f32, radius: f32| -> Vec<egui::Pos2> {
        let n = 32;
        (0..=n).map(|i| from + (to - from) * i as f32 / n as f32).map(|a| center + vec2(a.cos(), a.sin()) * radius).collect()
    };
    p_.circle_filled(center, KNOB * 0.5 - 4.0, Color32::from_rgb(38, 40, 44));
    p_.circle_stroke(center, KNOB * 0.5 - 4.0, Stroke::new(1.0, Color32::from_rgb(70, 74, 80)));
    p_.add(Shape::line(arc(a0, a1, KNOB * 0.5), Stroke::new(3.0, Color32::from_rgb(24, 26, 30))));
    let travel = to_travel(p, value);
    // Bipolar ranges (gain, pan) light from the centre; the rest from the bottom left.
    let from = if p.min < 0.0 && p.max > 0.0 { to_travel(p, 0.0) } else { 0.0 };
    let (lo, hi) = (from.min(travel), from.max(travel));
    if hi - lo > 0.001 {
        p_.add(Shape::line(arc(a0 + (a1 - a0) * lo, a0 + (a1 - a0) * hi, KNOB * 0.5), Stroke::new(3.0, color)));
    }
    let a = a0 + (a1 - a0) * travel;
    p_.line_segment([center + vec2(a.cos(), a.sin()) * 4.0, center + vec2(a.cos(), a.sin()) * (KNOB * 0.5 - 6.0)], Stroke::new(2.0, Color32::WHITE));
    p_.text(pos2(r.center().x, r.max.y - 7.0), Align2::CENTER_CENTER, shown, mono(10.0), t.counter_text);
    if resp.double_clicked() || (resp.clicked() && ui.input(|i| i.modifiers.alt)) {
        return Some(p.default);
    }
    if resp.dragged() {
        let fine = if ui.input(|i| i.modifiers.shift) { 0.1 } else { 1.0 };
        let dy = -resp.drag_delta().y + resp.drag_delta().x * 0.5;
        let next = from_travel(p, travel + dy / DRAG_RANGE * fine);
        if (next - value).abs() > f32::EPSILON {
            return Some(next);
        }
    }
    None
}

/// A two-state button for a toggle parameter.
fn toggle(ui: &mut Ui, p: &ParamInfo, value: f32, label: &str, color: Color32) -> Option<f32> {
    let on = value >= 0.5;
    let t = Tokens::current();
    let (r, resp) = ui.allocate_exact_size(vec2(COLUMN_W - 8.0, 22.0), Sense::click());
    let fill = if on { color } else { Color32::from_rgb(44, 46, 50) };
    ui.painter().rect(r.shrink2(vec2(6.0, 2.0)), 3.0, fill, Stroke::new(1.0, Color32::from_rgb(20, 20, 22)), StrokeKind::Inside);
    ui.painter().text(r.center(), Align2::CENTER_CENTER, label, bold(10.0), if on { Color32::BLACK } else { t.text });
    resp.clicked().then(|| if on { p.min } else { p.max.min(1.0).max(p.min) })
}

/// Draws the panel; returns every parameter the user changed this frame with its new value.
/// `value` reads a parameter's current value; `shown` formats one for display.
pub(super) fn show(
    ui: &mut Ui,
    salt: (u64, usize),
    info: &'static PluginInfo,
    value: &dyn Fn(&ParamInfo) -> f32,
    shown: &dyn Fn(&ParamInfo, f32) -> String,
) -> Vec<(&'static str, f32)> {
    let t = Tokens::current();
    let mut changed = Vec::new();
    let cols = columns(info);
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(4.0, 6.0);
        for (ci, col) in cols.iter().enumerate() {
            let color = band_color(ci, col.title);
            egui::Frame::new()
                .fill(Color32::from_rgb(28, 30, 33))
                .stroke(Stroke::new(1.0, Color32::from_rgb(52, 55, 60)))
                .corner_radius(4.0)
                .inner_margin(4.0)
                .show(ui, |ui| {
                    ui.vertical(|ui| {
                        ui.set_width(COLUMN_W - 8.0);
                        ui.spacing_mut().item_spacing.y = 2.0;
                        let title = if col.title.is_empty() { " " } else { crate::localization::text(col.title) };
                        ui.label(egui::RichText::new(title).font(bold(11.0)).color(if col.title.is_empty() { t.text_dim } else { color }));
                        for p in &col.params {
                            let v = value(p);
                            let label = crate::localization::text(short_name(p.name, col.title));
                            let new = match p.unit {
                                Unit::Toggle => toggle(ui, p, v, label, color),
                                Unit::Choice if !p.choices.is_empty() => {
                                    let mut idx = v.round().max(0.0) as usize;
                                    let before = idx;
                                    ui.label(egui::RichText::new(label).font(regular(10.0)).color(t.text_dim));
                                    let text =
                                        egui::RichText::new(p.choices.get(idx).map_or("?", |c| crate::localization::text(c))).font(regular(10.0));
                                    craft_ui::ComboBox::from_id_salt(("pp-choice", salt, p.id))
                                        .width(COLUMN_W - 12.0)
                                        .wrap_mode(egui::TextWrapMode::Truncate)
                                        .selected_text(text)
                                        .show_ui(ui, |ui| {
                                            for (i, c) in p.choices.iter().enumerate() {
                                                ui.selectable_value(&mut idx, i, crate::localization::text(c));
                                            }
                                        });
                                    (idx != before).then_some(idx as f32)
                                }
                                _ => knob(ui, p, v, label, &shown(p, v), color),
                            };
                            if let Some(n) = new {
                                changed.push((p.id, n));
                            }
                        }
                    });
                });
        }
    });
    changed
}

/// A colour per band or section, as EQs colour their bands: untitled columns stay one neutral blue.
fn band_color(index: usize, title: &str) -> Color32 {
    const COLORS: [Color32; 8] = [
        Color32::from_rgb(232, 96, 96),
        Color32::from_rgb(240, 160, 70),
        Color32::from_rgb(232, 210, 80),
        Color32::from_rgb(110, 200, 110),
        Color32::from_rgb(80, 190, 220),
        Color32::from_rgb(110, 140, 240),
        Color32::from_rgb(190, 120, 230),
        Color32::from_rgb(230, 120, 190),
    ];
    if title.is_empty() { Color32::from_rgb(150, 200, 255) } else { COLORS[index % COLORS.len()] }
}

/// The EQ graph: response curve over a log-frequency, ±18 dB grid, with a dot at each band's
/// frequency and gain in its column colour.
pub(super) fn eq_graph(ui: &mut Ui, info: &'static PluginInfo, params: &[(&str, f32)]) {
    let t = Tokens::current();
    let w = ui.available_width().max(360.0);
    let (r, _) = ui.allocate_exact_size(vec2(w, 170.0), Sense::hover());
    let p = ui.painter();
    p.rect_filled(r, 4.0, Color32::from_rgb(12, 15, 19));
    let plot = r.shrink2(vec2(30.0, 12.0));
    let x_of = |f: f32| plot.min.x + plot.width() * ((f / 20.0).ln() / 1000f32.ln()).clamp(0.0, 1.0);
    let y_of = |db: f32| plot.center().y - db.clamp(-18.0, 18.0) / 18.0 * plot.height() * 0.5;
    let grid = Stroke::new(1.0, Color32::from_rgb(34, 42, 52));
    for db in [-12.0f32, -6.0, 0.0, 6.0, 12.0] {
        let y = y_of(db);
        p.line_segment([pos2(plot.min.x, y), pos2(plot.max.x, y)], if db == 0.0 { Stroke::new(1.0, Color32::from_rgb(60, 72, 86)) } else { grid });
        p.text(pos2(r.min.x + 4.0, y), Align2::LEFT_CENTER, format!("{db:+.0}"), regular(9.0), t.text_dim);
    }
    for (f, label) in
        [(50.0, "50"), (100.0, "100"), (200.0, "200"), (500.0, "500"), (1000.0, "1k"), (2000.0, "2k"), (5000.0, "5k"), (10_000.0, "10k")]
    {
        let x = x_of(f);
        p.line_segment([pos2(x, plot.min.y), pos2(x, plot.max.y)], grid);
        p.text(pos2(x, r.max.y - 2.0), Align2::CENTER_BOTTOM, label, regular(9.0), t.text_dim);
    }
    let n = 240;
    let freqs: Vec<f32> = (0..n).map(|i| 20.0 * 1000f32.powf(i as f32 / (n - 1) as f32)).collect();
    let resp = if info.id == "eq_7band" {
        soundcraft_dsp::eq7_response(params, &freqs, 48_000.0)
    } else {
        soundcraft_dsp::eq1_response(params, &freqs, 48_000.0)
    };
    let pts: Vec<egui::Pos2> = freqs.iter().zip(&resp).map(|(f, db)| pos2(x_of(*f), y_of(*db))).collect();
    p.add(Shape::line(pts, Stroke::new(2.0, Color32::from_rgb(140, 210, 255))));
    let get = |id: &str| params.iter().find(|(k, _)| *k == id).map(|(_, v)| *v);
    for (ci, col) in columns(info).iter().enumerate() {
        let freq = col.params.iter().find(|q| q.name.ends_with("Freq")).and_then(|q| get(q.id).or(Some(q.default)));
        let gain = col.params.iter().find(|q| q.name.ends_with("Gain")).map(|q| get(q.id).unwrap_or(q.default)).unwrap_or(0.0);
        if let Some(f) = freq {
            let c = band_color(ci, col.title);
            p.circle_filled(pos2(x_of(f), y_of(gain)), 5.0, c);
            p.circle_stroke(pos2(x_of(f), y_of(gain)), 5.0, Stroke::new(1.0, Color32::BLACK));
        }
    }
}

/// The dynamics graph: input level (x) against output level (y) from −60 to 0 dB for the current
/// threshold, ratio, knee and range, beside a gain-reduction meter (`gr` in dB, positive).
pub(super) fn dynamics_graph(ui: &mut Ui, info: &'static PluginInfo, params: &[(&str, f32)], gr: f32) {
    let t = Tokens::current();
    let get =
        |name: &str| info.params.iter().find(|q| q.name == name).map(|q| params.iter().find(|(k, _)| *k == q.id).map_or(q.default, |(_, v)| *v));
    let threshold = get("Threshold").or_else(|| get("Comp Thresh")).unwrap_or(-20.0);
    let ratio = get("Ratio").or_else(|| get("Comp Ratio")).unwrap_or(if info.id == "maximizer" { 100.0 } else { 4.0 }).max(1.0);
    let knee = get("Knee").unwrap_or(0.0).max(0.0);
    let expander = info.id == "expander_gate";
    let range = get("Range").unwrap_or(-80.0);
    let out_of = |x: f32| -> f32 {
        if expander {
            if x >= threshold { x } else { (threshold + (x - threshold) * ratio).max(x + range.min(0.0)) }
        } else {
            let over = x - threshold;
            if knee > 0.0 && over.abs() <= knee * 0.5 {
                x + (1.0 / ratio - 1.0) * (over + knee * 0.5).powi(2) / (2.0 * knee)
            } else if over > 0.0 {
                threshold + over / ratio
            } else {
                x
            }
        }
    };
    ui.horizontal(|ui| {
        let (r, _) = ui.allocate_exact_size(vec2(170.0, 170.0), Sense::hover());
        let p = ui.painter();
        p.rect_filled(r, 4.0, Color32::from_rgb(12, 15, 19));
        let plot = r.shrink(10.0);
        let at = |x: f32, y: f32| pos2(plot.min.x + plot.width() * (x + 60.0) / 60.0, plot.max.y - plot.height() * (y + 60.0) / 60.0);
        let grid = Stroke::new(1.0, Color32::from_rgb(34, 42, 52));
        for db in [-48.0f32, -36.0, -24.0, -12.0] {
            p.line_segment([at(db, -60.0), at(db, 0.0)], grid);
            p.line_segment([at(-60.0, db), at(0.0, db)], grid);
        }
        p.line_segment([at(-60.0, -60.0), at(0.0, 0.0)], Stroke::new(1.0, Color32::from_rgb(60, 72, 86)));
        let pts: Vec<egui::Pos2> = (0..=120).map(|i| -60.0 + i as f32 * 0.5).map(|x| at(x, out_of(x).clamp(-60.0, 0.0))).collect();
        p.add(Shape::line(pts, Stroke::new(2.0, Color32::from_rgb(255, 196, 90))));
        p.line_segment(
            [at(threshold.clamp(-60.0, 0.0), -60.0), at(threshold.clamp(-60.0, 0.0), 0.0)],
            Stroke::new(1.0, Color32::from_rgb(255, 120, 80)),
        );
        p.text(pos2(plot.min.x + 2.0, plot.min.y + 2.0), Align2::LEFT_TOP, crate::localization::text("Out"), regular(9.0), t.text_dim);
        p.text(pos2(plot.max.x - 2.0, plot.max.y - 2.0), Align2::RIGHT_BOTTOM, crate::localization::text("In"), regular(9.0), t.text_dim);
        // Gain reduction, filling down from the top over 0..24 dB.
        let (m, _) = ui.allocate_exact_size(vec2(22.0, 170.0), Sense::hover());
        let p = ui.painter();
        p.rect_filled(m, 3.0, Color32::from_rgb(12, 15, 19));
        let bar = Rect::from_min_max(pos2(m.min.x + 6.0, m.min.y + 16.0), pos2(m.max.x - 6.0, m.max.y - 6.0));
        let k = (gr / 24.0).clamp(0.0, 1.0);
        p.rect_filled(Rect::from_min_max(bar.min, pos2(bar.max.x, bar.min.y + bar.height() * k)), 0.0, Color32::from_rgb(240, 170, 60));
        p.rect_stroke(bar, 0.0, Stroke::new(1.0, Color32::from_rgb(52, 55, 60)), StrokeKind::Outside);
        p.text(pos2(m.center().x, m.min.y + 7.0), Align2::CENTER_CENTER, crate::localization::text("GR"), bold(9.0), t.text_dim);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(id: &str) -> &'static PluginInfo {
        soundcraft_dsp::plugins().iter().copied().find(|p| p.id == id).unwrap()
    }

    #[test]
    fn eq_bands_become_columns_in_order() {
        let titles: Vec<&str> = columns(info("eq_7band")).iter().map(|c| c.title).collect();
        assert_eq!(titles, ["", "HPF", "LF", "LMF", "MF", "HMF", "HF", "LPF", ""]);
        let lf = columns(info("eq_7band")).into_iter().find(|c| c.title == "LF").unwrap();
        assert_eq!(lf.params.iter().map(|p| short_name(p.name, "LF")).collect::<Vec<_>>(), ["Freq", "Gain", "Q"]);
        assert_eq!(short_name("HPF", "HPF"), "On");
    }

    #[test]
    fn lone_controls_share_a_column_and_every_parameter_appears_once() {
        for p in soundcraft_dsp::plugins() {
            let n: usize = columns(p).iter().map(|c| c.params.len()).sum();
            assert_eq!(n, p.params.len(), "{}", p.id);
        }
        let comp = columns(info("compressor"));
        assert!(comp.iter().all(|c| c.params.len() <= 3 || !c.title.is_empty()));
    }

    #[test]
    fn knob_travel_round_trips_on_both_tapers() {
        for p in soundcraft_dsp::plugins().iter().flat_map(|p| p.params.iter()) {
            if matches!(p.unit, Unit::Toggle | Unit::Choice) || p.max <= p.min {
                continue;
            }
            for t in [0.0f32, 0.25, 0.5, 1.0] {
                let v = from_travel(p, t);
                assert!((to_travel(p, v) - t).abs() < 1e-3, "{} at {t}", p.name);
            }
        }
    }
}
