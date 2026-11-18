//! The face of a built-in plugin's window, drawn as one hardware-style panel: a display on top
//! (an EQ's response curve with draggable band points; a dynamics plugin's transfer curve and
//! gain-reduction meter), then the parameters as shaded rotary knobs. A band or section (an EQ's
//! LF, LMF … HF; a compressor's side-chain) gets a card of its own whose rows line up with the
//! other cards (Freq, Gain, Q); lone controls sit in a row of single cells.
//!
//! Third-party plugins keep the plain list: their parameter names follow no convention here.

use crate::theme::{Tokens, bold, mono, regular};
use crate::widgets::{rounded_vgradient, shade};
use egui::{Align2, Color32, Mesh, Pos2, Rect, Sense, Shape, Stroke, StrokeKind, Ui, pos2, vec2};
use soundcraft_dsp::{ParamInfo, PluginInfo, Taper, Unit};

/// A knob's diameter, a cell's size and the gap between cards.
const KNOB: f32 = 40.0;
const CELL_W: f32 = 68.0;
const CELL_H: f32 = 84.0;
const GAP: f32 = 6.0;
/// How far a drag must travel (points) to sweep the whole range; Shift divides the speed by 10.
const DRAG_RANGE: f32 = 220.0;

/// The panel's colours: a dark brushed face, slightly lighter cards, and an LCD for values.
const FACE_TOP: Color32 = Color32::from_rgb(44, 46, 51);
const FACE_BOTTOM: Color32 = Color32::from_rgb(30, 32, 36);
const CARD_TOP: Color32 = Color32::from_rgb(52, 55, 60);
const CARD_BOTTOM: Color32 = Color32::from_rgb(38, 40, 45);
const CARD_EDGE: Color32 = Color32::from_rgb(18, 19, 22);
const LCD: Color32 = Color32::from_rgb(12, 15, 17);
const LCD_EDGE: Color32 = Color32::from_rgb(64, 68, 74);
const DISPLAY_TOP: Color32 = Color32::from_rgb(18, 24, 31);
const DISPLAY_BOTTOM: Color32 = Color32::from_rgb(8, 10, 13);

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
    out
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

/// A titled column's parameters by row, so rows line up across cards: Freq first, then Gain, then
/// Q; anything else (a filter's switch or slope) takes the first free row.
fn rows<'a>(col: &Column<'a>) -> Vec<Option<&'a ParamInfo>> {
    let role = |p: &ParamInfo| match short_name(p.name, col.title) {
        "Freq" => Some(0usize),
        "Gain" => Some(1),
        "Q" => Some(2),
        _ => None,
    };
    if col.title.is_empty() || !col.params.iter().any(|p| role(p).is_some()) {
        return col.params.iter().copied().map(Some).collect();
    }
    let mut out: Vec<Option<&ParamInfo>> = vec![None; col.params.len().max(3)];
    let mut rest = Vec::new();
    for p in &col.params {
        match role(p) {
            Some(r) if out.get(r).is_some_and(Option::is_none) => {
                if let Some(slot) = out.get_mut(r) {
                    *slot = Some(*p);
                }
            }
            _ => rest.push(*p),
        }
    }
    for p in rest {
        match out.iter_mut().find(|s| s.is_none()) {
            Some(slot) => *slot = Some(p),
            None => out.push(Some(p)),
        }
    }
    while out.last().is_some_and(Option::is_none) {
        out.pop();
    }
    out
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

/// Points along a circle about `c`, from angle `a` to `b`.
fn arc(c: Pos2, radius: f32, a: f32, b: f32) -> Vec<Pos2> {
    let n = ((b - a).abs() * 12.0).ceil().max(2.0) as usize;
    (0..=n).map(|i| a + (b - a) * i as f32 / n as f32).map(|t| c + vec2(t.cos(), t.sin()) * radius).collect()
}

/// A dark readout box with `text` centred in it.
fn lcd(p: &egui::Painter, r: Rect, text: &str, color: Color32) {
    p.rect(r, 3.0, LCD, Stroke::new(1.0, LCD_EDGE), StrokeKind::Inside);
    p.hline(r.min.x + 2.0..=r.max.x - 2.0, r.min.y + 1.5, Stroke::new(1.0, Color32::from_black_alpha(120)));
    p.text(r.center(), Align2::CENTER_CENTER, text, mono(10.0), color);
}

/// A rotary knob for `p` in one cell: name above, value below. Drag up or right to raise it
/// (Shift for fine steps); double-click or Alt-click for the default. Returns the new value when
/// it changed.
fn knob(ui: &mut Ui, p: &ParamInfo, value: f32, label: &str, shown: &str, color: Color32) -> Option<f32> {
    let t = Tokens::current();
    let (r, resp) = ui.allocate_exact_size(vec2(CELL_W, CELL_H), Sense::click_and_drag());
    let resp = resp.on_hover_text(format!("{label}: {shown}"));
    let hot = resp.hovered() || resp.dragged();
    let pa = ui.painter();
    pa.text(pos2(r.center().x, r.min.y + 7.0), Align2::CENTER_CENTER, label, regular(10.0), if hot { t.text } else { t.text_dim });
    let c = pos2(r.center().x, r.min.y + 16.0 + KNOB * 0.5);
    let rad = KNOB * 0.5;
    // 7 o'clock to 5 o'clock.
    let (a0, a1) = (std::f32::consts::PI * 0.75, std::f32::consts::PI * 2.25);
    for i in 0..=10 {
        let a = a0 + (a1 - a0) * i as f32 / 10.0;
        let d = vec2(a.cos(), a.sin());
        let len = if i % 5 == 0 { 4.0 } else { 2.5 };
        pa.line_segment([c + d * (rad + 2.0), c + d * (rad + 2.0 + len)], Stroke::new(1.0, Color32::from_gray(96)));
    }
    pa.add(Shape::line(arc(c, rad - 1.5, a0, a1), Stroke::new(3.5, Color32::from_rgb(16, 17, 20))));
    let travel = to_travel(p, value);
    // Bipolar ranges (gain, pan) light from the centre; the rest from the bottom left.
    let from = if p.min < 0.0 && p.max > 0.0 { to_travel(p, 0.0) } else { 0.0 };
    let (lo, hi) = (from.min(travel), from.max(travel));
    if hi - lo > 0.001 {
        let pts = arc(c, rad - 1.5, a0 + (a1 - a0) * lo, a0 + (a1 - a0) * hi);
        pa.add(Shape::line(pts.clone(), Stroke::new(7.0, color.gamma_multiply(0.22))));
        pa.add(Shape::line(pts, Stroke::new(3.0, color)));
    }
    // The cap: a soft shadow, then a body lit from above.
    let body = rad - 6.0;
    pa.circle_filled(c + vec2(0.0, 2.0), body + 1.0, Color32::from_black_alpha(110));
    pa.circle_filled(c, body, Color32::from_rgb(26, 27, 30));
    for i in 0..7 {
        let k = i as f32 / 6.0;
        let g = (54.0 + 46.0 * k + if hot { 10.0 } else { 0.0 }) as u8;
        pa.circle_filled(c - vec2(0.0, body * 0.12 * k), (body - 1.0) * (1.0 - 0.13 * k), Color32::from_rgb(g, g, g.saturating_add(4)));
    }
    pa.circle_filled(c, (body - 1.0) * 0.55, Color32::from_rgb(70, 72, 77));
    pa.circle_stroke(c, body, Stroke::new(1.0, Color32::from_rgb(16, 16, 18)));
    let a = a0 + (a1 - a0) * travel;
    let d = vec2(a.cos(), a.sin());
    pa.line_segment([c + d * (body * 0.3), c + d * (body - 2.0)], Stroke::new(2.5, Color32::from_rgb(244, 244, 240)));
    lcd(pa, Rect::from_center_size(pos2(r.center().x, r.max.y - 9.0), vec2(CELL_W - 6.0, 16.0)), shown, t.counter_text);
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

/// A lit push button for a toggle parameter, centred in one cell.
fn toggle(ui: &mut Ui, p: &ParamInfo, value: f32, label: &str, color: Color32) -> Option<f32> {
    let on = value >= 0.5;
    let t = Tokens::current();
    let (r, resp) = ui.allocate_exact_size(vec2(CELL_W, CELL_H), Sense::click());
    let b = Rect::from_center_size(r.center(), vec2(CELL_W - 8.0, 24.0));
    let pa = ui.painter();
    pa.rect_filled(b.translate(vec2(0.0, 1.5)), 4.0, Color32::from_black_alpha(100));
    let lift = if resp.hovered() { 0.06 } else { 0.0 };
    rounded_vgradient(pa, b, 4.0, shade(Color32::from_rgb(78, 81, 88), lift), shade(Color32::from_rgb(50, 52, 57), lift));
    pa.rect_stroke(b, 4.0, Stroke::new(1.0, CARD_EDGE), StrokeKind::Inside);
    let led = pos2(b.min.x + 10.0, b.center().y);
    if on {
        pa.circle_filled(led, 6.0, color.gamma_multiply(0.3));
    }
    pa.circle_filled(led, 3.5, if on { color } else { Color32::from_rgb(28, 29, 32) });
    pa.text(pos2(b.center().x + 6.0, b.center().y), Align2::CENTER_CENTER, label, bold(10.0), if on { t.text } else { t.text_dim });
    resp.clicked().then(|| if on { p.min } else { p.max.min(1.0).max(p.min) })
}

/// A choice parameter in one cell: its name and a drop-down.
fn choice(ui: &mut Ui, salt: (u64, usize), p: &ParamInfo, value: f32, label: &str) -> Option<f32> {
    let t = Tokens::current();
    let mut idx = value.round().max(0.0) as usize;
    let before = idx;
    let (r, _) = ui.allocate_exact_size(vec2(CELL_W, CELL_H), Sense::hover());
    ui.painter().text(pos2(r.center().x, r.min.y + 7.0), Align2::CENTER_CENTER, label, regular(10.0), t.text_dim);
    let combo = Rect::from_center_size(r.center(), vec2(CELL_W - 4.0, 22.0));
    let mut c = ui.new_child(egui::UiBuilder::new().max_rect(combo));
    let text = egui::RichText::new(p.choices.get(idx).map_or("?", |c| crate::localization::text(c))).font(regular(10.0));
    craft_ui::ComboBox::from_id_salt(("pp-choice", salt, p.id))
        .width(CELL_W - 8.0)
        .wrap_mode(egui::TextWrapMode::Truncate)
        .selected_text(text)
        .show_ui(&mut c, |ui| {
            for (i, c) in p.choices.iter().enumerate() {
                ui.selectable_value(&mut idx, i, crate::localization::text(c));
            }
        });
    (idx != before).then_some(idx as f32)
}

/// One parameter's control in a cell.
fn control(
    ui: &mut Ui,
    salt: (u64, usize),
    p: &ParamInfo,
    title: &str,
    color: Color32,
    value: &dyn Fn(&ParamInfo) -> f32,
    shown: &dyn Fn(&ParamInfo, f32) -> String,
) -> Option<f32> {
    let v = value(p);
    let label = crate::localization::text(short_name(p.name, title));
    match p.unit {
        Unit::Toggle => toggle(ui, p, v, label, color),
        Unit::Choice if !p.choices.is_empty() => choice(ui, salt, p, v, label),
        _ => knob(ui, p, v, label, &shown(p, v), color),
    }
}

/// A card: a lit, raised plate with an optional coloured title strip. Returns its contents' output.
fn card<R>(ui: &mut Ui, title: &str, color: Color32, add: impl FnOnce(&mut Ui) -> R) -> R {
    let titled = !title.is_empty();
    // Reserve the plate's place in the paint order before the controls draw over it.
    let plate = ui.painter().add(Shape::Noop);
    let inner = egui::Frame::new().inner_margin(egui::Margin { left: 3, right: 3, top: if titled { 20 } else { 4 }, bottom: 4 }).show(ui, add);
    let r = inner.response.rect;
    let pa = ui.painter();
    let mut shapes = vec![
        Shape::rect_filled(r.translate(vec2(0.0, 2.0)), 6.0, Color32::from_black_alpha(90)),
        gradient_rect(r, 6.0, CARD_TOP, CARD_BOTTOM),
        Shape::rect_stroke(r, 6.0, Stroke::new(1.0, CARD_EDGE), StrokeKind::Inside),
        Shape::line_segment(
            [pos2(r.min.x + 5.0, r.min.y + 1.5), pos2(r.max.x - 5.0, r.min.y + 1.5)],
            Stroke::new(1.0, Color32::from_white_alpha(22)),
        ),
    ];
    if titled {
        shapes.push(Shape::line_segment([pos2(r.min.x + 6.0, r.min.y + 3.0), pos2(r.max.x - 6.0, r.min.y + 3.0)], Stroke::new(2.0, color)));
        let galley = pa.layout_no_wrap(crate::localization::text(title).to_owned(), bold(11.0), color);
        let at = pos2(r.center().x - galley.size().x * 0.5, r.min.y + 12.0 - galley.size().y * 0.5);
        shapes.push(Shape::galley(at, galley, color));
    }
    pa.set(plate, Shape::Vec(shapes));
    inner.inner
}

/// A rounded rectangle filled with a vertical gradient, as one shape.
fn gradient_rect(r: Rect, radius: f32, top: Color32, bottom: Color32) -> Shape {
    let mut mesh = Mesh::default();
    let steps = 6;
    let rr = radius.min(r.height() * 0.5).min(r.width() * 0.5);
    // Inset each row by the corner's curve so the plate is rounded at both ends.
    let inset = |y: f32| -> f32 {
        let d = if y < r.min.y + rr {
            r.min.y + rr - y
        } else if y > r.max.y - rr {
            y - (r.max.y - rr)
        } else {
            0.0
        };
        rr - (rr * rr - d * d).max(0.0).sqrt()
    };
    let mut ys: Vec<f32> = (0..=steps).map(|i| r.min.y + rr * i as f32 / steps as f32).collect();
    ys.extend((0..=steps).map(|i| r.max.y - rr + rr * i as f32 / steps as f32));
    for y in ys {
        let k = ((y - r.min.y) / r.height().max(1.0)).clamp(0.0, 1.0);
        let c = top.lerp_to_gamma(bottom, k);
        let i = inset(y);
        mesh.colored_vertex(pos2(r.min.x + i, y), c);
        mesh.colored_vertex(pos2(r.max.x - i, y), c);
    }
    let n = mesh.vertices.len() as u32 / 2;
    for j in 0..n.saturating_sub(1) {
        let (a, b, c, d) = (2 * j, 2 * j + 1, 2 * j + 2, 2 * j + 3);
        mesh.add_triangle(a, b, c);
        mesh.add_triangle(b, d, c);
    }
    Shape::mesh(mesh)
}

/// Draws the whole face: the display (if the plugin has one) and the controls. Returns every
/// parameter the user changed this frame with its new value. `params` are the stored values,
/// `gr` the current gain reduction in dB, `value` reads a parameter, `shown` formats one.
pub(super) fn face(
    ui: &mut Ui,
    salt: (u64, usize),
    info: &'static PluginInfo,
    params: &[(&str, f32)],
    gr: f32,
    value: &dyn Fn(&ParamInfo) -> f32,
    shown: &dyn Fn(&ParamInfo, f32) -> String,
) -> Vec<(&'static str, f32)> {
    let mut changed = Vec::new();
    // Reserve the face plate's place in the paint order before anything draws on it.
    let idx = ui.painter().add(Shape::Noop);
    let r = egui::Frame::new()
        .inner_margin(10.0)
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing = vec2(GAP, GAP);
            let eq = matches!(info.id, "eq_7band" | "eq_1band");
            let dynamics = matches!(info.id, "compressor" | "expander_gate" | "maximizer");
            if eq {
                changed.extend(eq_graph(ui, salt, info, params));
                ui.add_space(4.0);
                changed.extend(controls(ui, salt, info, value, shown, Fill::Down(usize::MAX)));
            } else if dynamics {
                ui.horizontal_top(|ui| {
                    dynamics_graph(ui, info, params, gr);
                    // Two rows of controls beside the graph, a section's controls side by side.
                    ui.vertical(|ui| changed.extend(controls(ui, salt, info, value, shown, Fill::Across(6))));
                });
            } else {
                changed.extend(controls(ui, salt, info, value, shown, Fill::Down(6)));
            }
        })
        .response
        .rect;
    // The face plate, drawn under everything (the frame reserved its place first).
    ui.painter().set(
        idx,
        Shape::Vec(vec![
            gradient_rect(r, 8.0, FACE_TOP, FACE_BOTTOM),
            Shape::rect_stroke(r, 8.0, Stroke::new(1.0, CARD_EDGE), StrokeKind::Inside),
            Shape::line_segment(
                [pos2(r.min.x + 8.0, r.min.y + 1.5), pos2(r.max.x - 8.0, r.min.y + 1.5)],
                Stroke::new(1.0, Color32::from_white_alpha(26)),
            ),
        ]),
    );
    changed
}

/// How [`controls`] fills its rows.
#[derive(Clone, Copy)]
enum Fill {
    /// A section's controls stacked in its card, at most this many cards a row.
    Down(usize),
    /// A section's controls side by side in its card, at most this many controls a row.
    Across(usize),
}

/// The cards: one per band or section, rows lined up; a single cell for each lone control.
fn controls(
    ui: &mut Ui,
    salt: (u64, usize),
    info: &'static PluginInfo,
    value: &dyn Fn(&ParamInfo) -> f32,
    shown: &dyn Fn(&ParamInfo, f32) -> String,
    fill: Fill,
) -> Vec<(&'static str, f32)> {
    let mut changed = Vec::new();
    let cols = columns(info);
    let tall = cols.iter().filter(|c| !c.title.is_empty()).map(|c| rows(c).len()).max().unwrap_or(1);
    let across = matches!(fill, Fill::Across(_));
    // Group the cards into rows: by card count when stacking, by control count side by side.
    let mut grouped: Vec<Vec<(usize, &Column<'_>)>> = vec![Vec::new()];
    let mut used = 0;
    for (ci, col) in cols.iter().enumerate() {
        let (cost, limit) = match fill {
            Fill::Down(n) => (1, n),
            Fill::Across(n) => (rows(col).iter().flatten().count().max(1), n),
        };
        if used > 0 && used + cost > limit.max(1) {
            grouped.push(Vec::new());
            used = 0;
        }
        if let Some(row) = grouped.last_mut() {
            row.push((ci, col));
        }
        used += cost;
    }
    for row in grouped {
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing = vec2(GAP, GAP);
            for (ci, col) in row {
                let color = band_color(ci, col.title);
                let cells: Vec<Option<&ParamInfo>> = if across { rows(col).into_iter().filter(Option::is_some).collect() } else { rows(col) };
                let n = if col.title.is_empty() || across { cells.len() } else { tall };
                card(ui, col.title, color, |ui| {
                    let mut draw = |ui: &mut Ui| {
                        ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
                        for k in 0..n {
                            match cells.get(k).copied().flatten() {
                                Some(p) => {
                                    if let Some(v) = control(ui, salt, p, col.title, color, value, shown) {
                                        changed.push((p.id, v));
                                    }
                                }
                                None => {
                                    ui.allocate_exact_size(vec2(CELL_W, CELL_H), Sense::hover());
                                }
                            }
                        }
                    };
                    if across {
                        ui.horizontal_top(|ui| draw(ui));
                    } else {
                        ui.vertical(|ui| draw(ui));
                    }
                });
            }
        });
    }
    changed
}

/// A colour per band or section, as EQs colour their bands: lone controls share one cool blue.
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
    if title.is_empty() { Color32::from_rgb(120, 190, 255) } else { COLORS[index % COLORS.len()] }
}

/// A sunken display screen with a dark gradient and a bevel.
fn screen(p: &egui::Painter, r: Rect) {
    p.rect_filled(r.expand(1.0), 5.0, Color32::from_white_alpha(14));
    p.add(gradient_rect(r, 5.0, DISPLAY_TOP, DISPLAY_BOTTOM));
    p.rect_stroke(r, 5.0, Stroke::new(1.0, Color32::from_black_alpha(200)), StrokeKind::Inside);
}

/// Fills between a curve and a horizontal baseline at `base`.
fn fill_to(pts: &[Pos2], base: f32, color: Color32) -> Shape {
    let mut mesh = Mesh::default();
    for p in pts {
        mesh.colored_vertex(*p, color);
        mesh.colored_vertex(pos2(p.x, base), color);
    }
    for j in 0..(pts.len() as u32).saturating_sub(1) {
        let (a, b, c, d) = (2 * j, 2 * j + 1, 2 * j + 2, 2 * j + 3);
        mesh.add_triangle(a, b, c);
        mesh.add_triangle(b, d, c);
    }
    Shape::mesh(mesh)
}

/// The EQ display: response curve over a log-frequency, ±18 dB grid, and a point per band in its
/// colour that drags the band's frequency (sideways) and gain (up and down). Returns the changes.
fn eq_graph(ui: &mut Ui, salt: (u64, usize), info: &'static PluginInfo, params: &[(&str, f32)]) -> Vec<(&'static str, f32)> {
    let t = Tokens::current();
    let w = ui.available_width().max(360.0);
    let (r, _) = ui.allocate_exact_size(vec2(w, 200.0), Sense::hover());
    let mut changed = Vec::new();
    let plot = Rect::from_min_max(pos2(r.min.x + 30.0, r.min.y + 10.0), pos2(r.max.x - 30.0, r.max.y - 18.0));
    let (f_lo, f_span) = (20.0f32, 1000f32.ln());
    let x_of = |f: f32| plot.min.x + plot.width() * ((f.max(1.0) / f_lo).ln() / f_span).clamp(0.0, 1.0);
    let f_of = |x: f32| f_lo * (((x - plot.min.x) / plot.width()).clamp(0.0, 1.0) * f_span).exp();
    let y_of = |db: f32| plot.center().y - db.clamp(-18.0, 18.0) / 18.0 * plot.height() * 0.5;
    let db_of = |y: f32| ((plot.center().y - y) / (plot.height() * 0.5) * 18.0).clamp(-18.0, 18.0);
    let get = |id: &str| params.iter().find(|(k, _)| *k == id).map(|(_, v)| *v);
    // Points first so a drag this frame shows in the curve.
    let mut bands = Vec::new();
    for (ci, col) in columns(info).iter().enumerate() {
        let Some(fp) = col.params.iter().find(|q| short_name(q.name, col.title) == "Freq" || q.name.ends_with("Freq")) else { continue };
        if col.title.is_empty() {
            continue;
        }
        let gp = col.params.iter().find(|q| short_name(q.name, col.title) == "Gain");
        let f = get(fp.id).unwrap_or(fp.default);
        let g = gp.map_or(0.0, |q| get(q.id).unwrap_or(q.default));
        let at = pos2(x_of(f), y_of(g));
        let resp = ui.interact(Rect::from_center_size(at, vec2(20.0, 20.0)), egui::Id::unique(("eq-point", salt, ci)), Sense::click_and_drag());
        let (mut f2, mut g2) = (f, g);
        if resp.dragged()
            && let Some(pos) = resp.interact_pointer_pos()
        {
            f2 = fp.clamp(f_of(pos.x));
            if let Some(q) = gp {
                g2 = q.clamp(db_of(pos.y));
            }
            if (f2 - f).abs() > f32::EPSILON {
                changed.push((fp.id, f2));
            }
            if let Some(q) = gp
                && (g2 - g).abs() > f32::EPSILON
            {
                changed.push((q.id, g2));
            }
        }
        if resp.double_clicked()
            && let Some(q) = gp
        {
            g2 = q.default;
            changed.push((q.id, g2));
        }
        bands.push((band_color(ci, col.title), col.title, f2, g2, gp.is_some(), resp.hovered() || resp.dragged()));
    }
    let mut live: Vec<(&str, f32)> = params.to_vec();
    for (id, v) in &changed {
        match live.iter_mut().find(|(k, _)| k == id) {
            Some(e) => e.1 = *v,
            None => live.push((id, *v)),
        }
    }
    let p = ui.painter();
    screen(p, r);
    let minor = Stroke::new(1.0, Color32::from_rgb(24, 31, 40));
    let major = Stroke::new(1.0, Color32::from_rgb(38, 48, 60));
    for decade in [10.0f32, 100.0, 1000.0, 10_000.0] {
        for k in 2..10 {
            let f = decade * k as f32;
            if (20.0..=20_000.0).contains(&f) {
                let x = x_of(f);
                p.line_segment([pos2(x, plot.min.y), pos2(x, plot.max.y)], minor);
            }
        }
    }
    for (f, label) in [(100.0, "100"), (1000.0, "1k"), (10_000.0, "10k")] {
        let x = x_of(f);
        p.line_segment([pos2(x, plot.min.y), pos2(x, plot.max.y)], major);
        p.text(pos2(x, r.max.y - 4.0), Align2::CENTER_BOTTOM, label, regular(9.0), t.text_dim);
    }
    for (f, label) in [(50.0, "50"), (200.0, "200"), (500.0, "500"), (2000.0, "2k"), (5000.0, "5k")] {
        p.text(pos2(x_of(f), r.max.y - 4.0), Align2::CENTER_BOTTOM, label, regular(9.0), t.text_dim.gamma_multiply(0.7));
    }
    for db in [-12.0f32, -6.0, 0.0, 6.0, 12.0] {
        let y = y_of(db);
        p.line_segment([pos2(plot.min.x, y), pos2(plot.max.x, y)], if db == 0.0 { Stroke::new(1.0, Color32::from_rgb(70, 84, 100)) } else { major });
        let label = if db == 0.0 { "0".to_owned() } else { format!("{db:+.0}") };
        p.text(pos2(r.min.x + 6.0, y), Align2::LEFT_CENTER, &label, regular(9.0), t.text_dim);
        p.text(pos2(r.max.x - 6.0, y), Align2::RIGHT_CENTER, label, regular(9.0), t.text_dim);
    }
    let n = 320;
    let freqs: Vec<f32> = (0..n).map(|i| 20.0 * 1000f32.powf(i as f32 / (n - 1) as f32)).collect();
    let resp = if info.id == "eq_7band" {
        soundcraft_dsp::eq7_response(&live, &freqs, 48_000.0)
    } else {
        soundcraft_dsp::eq1_response(&live, &freqs, 48_000.0)
    };
    let pts: Vec<Pos2> = freqs.iter().zip(&resp).map(|(f, db)| pos2(x_of(*f), y_of(*db))).collect();
    let accent = Color32::from_rgb(130, 205, 255);
    p.add(fill_to(&pts, y_of(0.0), Color32::from_rgba_unmultiplied(120, 200, 255, 34)));
    p.add(Shape::line(pts.clone(), Stroke::new(6.0, accent.gamma_multiply(0.18))));
    p.add(Shape::line(pts, Stroke::new(2.0, accent)));
    for (c, title, f, g, has_gain, hot) in bands {
        let at = pos2(x_of(f), y_of(g));
        if hot {
            p.line_segment([pos2(at.x, plot.min.y), pos2(at.x, plot.max.y)], Stroke::new(1.0, c.gamma_multiply(0.5)));
        }
        p.circle_filled(at, if hot { 9.0 } else { 7.0 }, c.gamma_multiply(0.25));
        p.circle_filled(at, if hot { 6.5 } else { 5.5 }, c);
        p.circle_stroke(at, if hot { 6.5 } else { 5.5 }, Stroke::new(1.5, if hot { Color32::WHITE } else { Color32::from_black_alpha(200) }));
        if hot {
            let freq = if f >= 1000.0 { format!("{:.2} kHz", f / 1000.0) } else { format!("{f:.0} Hz") };
            let text = if has_gain {
                format!("{}  {freq}  {g:+.1} dB", crate::localization::text(title))
            } else {
                format!("{}  {freq}", crate::localization::text(title))
            };
            let galley = p.layout_no_wrap(text, mono(10.0), Color32::WHITE);
            let below = at.y < plot.center().y;
            let pos = pos2(
                (at.x - galley.size().x * 0.5).clamp(plot.min.x, plot.max.x - galley.size().x),
                if below { at.y + 12.0 } else { at.y - 12.0 - galley.size().y },
            );
            p.rect_filled(Rect::from_min_size(pos, galley.size()).expand(3.0), 3.0, Color32::from_black_alpha(200));
            p.galley(pos, galley, Color32::WHITE);
        }
    }
    changed
}

/// The dynamics display: input level (x) against output level (y) from −60 to 0 dB for the current
/// threshold, ratio, knee and range, beside a gain-reduction meter (`gr` in dB, positive).
fn dynamics_graph(ui: &mut Ui, info: &'static PluginInfo, params: &[(&str, f32)], gr: f32) {
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
    // As tall as two rows of controls, one of them a titled section.
    let side = CELL_H * 2.0 + 38.0;
    let (r, _) = ui.allocate_exact_size(vec2(side, side), Sense::hover());
    let p = ui.painter();
    screen(p, r);
    let plot = Rect::from_min_max(pos2(r.min.x + 24.0, r.min.y + 10.0), pos2(r.max.x - 10.0, r.max.y - 22.0));
    let at = |x: f32, y: f32| pos2(plot.min.x + plot.width() * (x + 60.0) / 60.0, plot.max.y - plot.height() * (y + 60.0) / 60.0);
    let grid = Stroke::new(1.0, Color32::from_rgb(32, 41, 52));
    for db in [-48.0f32, -36.0, -24.0, -12.0] {
        p.line_segment([at(db, -60.0), at(db, 0.0)], grid);
        p.line_segment([at(-60.0, db), at(0.0, db)], grid);
        p.text(pos2(plot.min.x - 4.0, at(-60.0, db).y), Align2::RIGHT_CENTER, format!("{db:.0}"), regular(8.0), t.text_dim);
        p.text(pos2(at(db, -60.0).x, plot.max.y + 3.0), Align2::CENTER_TOP, format!("{db:.0}"), regular(8.0), t.text_dim);
    }
    p.rect_stroke(plot, 0.0, Stroke::new(1.0, Color32::from_rgb(44, 56, 70)), StrokeKind::Outside);
    p.line_segment([at(-60.0, -60.0), at(0.0, 0.0)], Stroke::new(1.0, Color32::from_rgb(60, 72, 86)));
    let amber = Color32::from_rgb(255, 190, 80);
    let pts: Vec<Pos2> = (0..=240).map(|i| -60.0 + i as f32 * 0.25).map(|x| at(x, out_of(x).clamp(-60.0, 0.0))).collect();
    p.add(fill_to(&pts, plot.max.y, Color32::from_rgba_unmultiplied(255, 180, 70, 26)));
    p.add(Shape::line(pts.clone(), Stroke::new(6.0, amber.gamma_multiply(0.18))));
    p.add(Shape::line(pts, Stroke::new(2.0, amber)));
    let th = threshold.clamp(-60.0, 0.0);
    p.line_segment([at(th, -60.0), at(th, 0.0)], Stroke::new(1.0, Color32::from_rgba_unmultiplied(255, 110, 80, 140)));
    p.circle_filled(at(th, out_of(th).clamp(-60.0, 0.0)), 4.5, Color32::from_rgb(255, 120, 80));
    p.text(pos2(plot.min.x + 4.0, plot.min.y + 3.0), Align2::LEFT_TOP, crate::localization::text("Out"), regular(9.0), t.text_dim);
    p.text(pos2(plot.max.x - 4.0, plot.max.y - 3.0), Align2::RIGHT_BOTTOM, crate::localization::text("In"), regular(9.0), t.text_dim);
    // Gain reduction: segments lighting down from the top over 0..24 dB, with a scale and a readout.
    let (m, _) = ui.allocate_exact_size(vec2(40.0, side), Sense::hover());
    let p = ui.painter();
    screen(p, m);
    p.text(pos2(m.center().x, m.min.y + 9.0), Align2::CENTER_CENTER, crate::localization::text("GR"), bold(9.0), t.text_dim);
    let bar = Rect::from_min_max(pos2(m.min.x + 6.0, m.min.y + 20.0), pos2(m.min.x + 18.0, m.max.y - 24.0));
    let k = (gr / 24.0).clamp(0.0, 1.0);
    let segs = 24;
    let sh = bar.height() / segs as f32;
    for i in 0..segs {
        let seg = Rect::from_min_size(pos2(bar.min.x, bar.min.y + i as f32 * sh), vec2(bar.width(), sh - 1.0));
        let lit = (i as f32 + 0.5) / segs as f32 <= k;
        let c = if i < 6 {
            Color32::from_rgb(250, 200, 70)
        } else if i < 14 {
            Color32::from_rgb(250, 160, 50)
        } else {
            Color32::from_rgb(240, 100, 50)
        };
        p.rect_filled(seg, 1.0, if lit { c } else { c.gamma_multiply(0.12) });
    }
    for db in [3.0f32, 6.0, 12.0, 24.0] {
        let y = bar.min.y + bar.height() * db / 24.0;
        p.text(pos2(bar.max.x + 4.0, y), Align2::LEFT_CENTER, format!("{db:.0}"), regular(8.0), t.text_dim);
    }
    lcd(
        p,
        Rect::from_center_size(pos2(m.center().x, m.max.y - 11.0), vec2(m.width() - 6.0, 16.0)),
        &format!("{:.1}", -gr.max(0.0)),
        Color32::from_rgb(250, 190, 80),
    );
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
        assert!(comp.iter().all(|c| c.params.len() == 1 || !c.title.is_empty()));
    }

    #[test]
    fn band_rows_line_up_freq_gain_q() {
        let cols = columns(info("eq_7band"));
        let names = |t: &str| -> Vec<Option<&str>> {
            let c = cols.iter().find(|c| c.title == t).unwrap();
            rows(c).iter().map(|p| p.map(|p| short_name(p.name, t))).collect()
        };
        assert_eq!(names("LF"), [Some("Freq"), Some("Gain"), Some("Q")]);
        assert_eq!(names("HPF"), [Some("Freq"), Some("On"), Some("Slope")]);
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
