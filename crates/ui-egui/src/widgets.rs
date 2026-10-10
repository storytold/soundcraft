//! Small custom widgets in the studio style.

use crate::icons;
use crate::theme::{Tokens, bold, regular};
use egui::{Align2, Color32, CornerRadius, Rect, Response, Sense, Stroke, StrokeKind, Ui, pos2, vec2};

/// A toolbar button with an icon; `on` draws the blue selected state.
pub fn icon_button(ui: &mut Ui, size: egui::Vec2, icon: &str, on: bool, tip: &str) -> Response {
    let t = Tokens::current();
    let (r, resp) = ui.allocate_exact_size(size, Sense::click());
    let fill = if on {
        t.accent
    } else if resp.hovered() {
        t.button_hi
    } else {
        t.button
    };
    ui.painter().rect(r, CornerRadius::same(3), fill, Stroke::new(1.0, t.button_border), StrokeKind::Inside);
    let col = if on { Color32::WHITE } else { t.text };
    let s = r.height().min(r.width()) * 0.78;
    icons::draw(ui.painter(), Rect::from_center_size(r.center(), vec2(s, s)), icon, col);
    resp.on_hover_text(tip)
}

/// Square text toggle used for S / M / I / record etc.
pub fn text_toggle(ui: &mut Ui, size: egui::Vec2, text: &str, on: bool, on_color: Color32, tip: &str) -> Response {
    let t = Tokens::current();
    let (r, resp) = ui.allocate_exact_size(size, Sense::click());
    let fill = if on {
        on_color
    } else if resp.hovered() {
        t.button_hi
    } else {
        t.button
    };
    ui.painter().rect(r, CornerRadius::same(2), fill, Stroke::new(1.0, t.button_border), StrokeKind::Inside);
    let col = if on { t.text_dark } else { t.text };
    ui.painter().text(r.center(), Align2::CENTER_CENTER, text, bold((size.y * 0.62).clamp(8.0, 13.0)), col);
    resp.on_hover_text(tip)
}

/// Record-enable button (red dot).
pub fn rec_toggle(ui: &mut Ui, size: egui::Vec2, on: bool, tip: &str) -> Response {
    let t = Tokens::current();
    let (r, resp) = ui.allocate_exact_size(size, Sense::click());
    let fill = if on {
        t.rec
    } else if resp.hovered() {
        t.button_hi
    } else {
        t.button
    };
    ui.painter().rect(r, CornerRadius::same(2), fill, Stroke::new(1.0, t.button_border), StrokeKind::Inside);
    ui.painter().circle_filled(r.center(), size.y * 0.24, if on { Color32::WHITE } else { Color32::from_rgb(190, 190, 190) });
    resp.on_hover_text(tip)
}

/// A dropdown-looking selector box. Returns the response (caller opens a menu on click).
pub fn selector_box(ui: &mut Ui, width: f32, height: f32, text: &str, color: Color32) -> Response {
    let t = Tokens::current();
    let (r, resp) = ui.allocate_exact_size(vec2(width, height), Sense::click());
    ui.painter().rect(
        r,
        CornerRadius::same(2),
        if resp.hovered() { t.button_hi } else { t.button },
        Stroke::new(1.0, t.button_border),
        StrokeKind::Inside,
    );
    let tr = Rect::from_min_max(pos2(r.max.x - height * 0.8, r.min.y), r.max);
    icons::draw(ui.painter(), tr.shrink(height * 0.15), "triangle_down", t.text_dim);
    let galley = ui.painter().layout(text.to_string(), regular((height * 0.62).clamp(8.0, 12.0)), color, (width - height).max(10.0));
    let pos = pos2(r.min.x + (r.width() - height * 0.6 - galley.size().x) * 0.5, r.center().y - galley.size().y * 0.5);
    ui.painter().with_clip_rect(r.shrink(1.0)).galley(pos, galley, color);
    resp
}

/// Rotary knob for pan / send pan (-1..1). Drag vertically; double-click resets.
pub fn pan_knob(ui: &mut Ui, size: f32, value: &mut f32, tip: &str) -> Response {
    let t = Tokens::current();
    let (r, resp) = ui.allocate_exact_size(vec2(size, size), Sense::click_and_drag());
    if resp.dragged() {
        let d = resp.drag_delta();
        *value = (*value + (d.x - d.y) * 0.01).clamp(-1.0, 1.0);
    }
    if resp.double_clicked() {
        *value = 0.0;
    }
    let c = r.center();
    let rad = size * 0.42;
    ui.painter().circle(c, rad, t.knob_bg, Stroke::new(1.0, t.knob_ring));
    let n = 24;
    let start = std::f32::consts::PI * 0.75;
    let sweep = std::f32::consts::PI * 1.5;
    let arc = |a0: f32, a1: f32| -> Vec<egui::Pos2> {
        (0..=n)
            .map(|i| {
                let a = a0 + (a1 - a0) * i as f32 / n as f32;
                c + vec2(a.cos(), a.sin()) * (rad + 2.5)
            })
            .collect()
    };
    let mid = start + sweep * 0.5;
    let at = start + sweep * (*value * 0.5 + 0.5);
    let (a0, a1) = if at < mid { (at, mid) } else { (mid, at) };
    ui.painter().add(egui::Shape::line(arc(a0, a1), Stroke::new(2.0, t.counter_text)));
    let tip_pos = c + vec2(at.cos(), at.sin()) * rad * 0.85;
    ui.painter().line_segment([c, tip_pos], Stroke::new(2.0, t.knob_pointer));
    resp.on_hover_text(tip)
}

/// Format a pan value like a console: "<45", "0", "45>".
pub fn pan_text(v: f32) -> String {
    let n = (v * 100.0).round() as i32;
    match n {
        0 => "0".into(),
        n if n < 0 => format!("<{}", -n),
        n => format!("{n}>"),
    }
}

/// Vertical peak meter with green/yellow/red zones; values are linear peaks.
pub fn meter(ui: &Ui, r: Rect, level: f32, hold: f32, clip: bool) {
    let t = Tokens::current();
    ui.painter().rect_filled(r, 0.0, t.meter_bg);
    let pos = |v: f32| -> f32 {
        let db = if v <= 1e-6 { -80.0 } else { 20.0 * v.log10() };
        meter_pos(db)
    };
    let h = r.height() - 4.0;
    let inner = Rect::from_min_max(pos2(r.min.x + 1.0, r.min.y + 3.0), pos2(r.max.x - 1.0, r.max.y - 1.0));
    let lv = pos(level);
    if lv > 0.0 {
        let top = inner.max.y - h * lv;
        // Draw in zones.
        let zones =
            [(0.0, meter_pos(-12.0), t.meter_green), (meter_pos(-12.0), meter_pos(-3.0), t.meter_yellow), (meter_pos(-3.0), 1.0, t.meter_red)];
        for (a, b, col) in zones {
            let y0 = inner.max.y - h * a;
            let y1 = (inner.max.y - h * b).max(top);
            if y1 < y0 {
                ui.painter().rect_filled(Rect::from_min_max(pos2(inner.min.x, y1), pos2(inner.max.x, y0)), 0.0, col);
            }
        }
    }
    let hv = pos(hold);
    if hv > 0.01 {
        let y = inner.max.y - h * hv;
        ui.painter().line_segment(
            [pos2(inner.min.x, y), pos2(inner.max.x, y)],
            Stroke::new(1.0, if hv > meter_pos(-3.0) { t.meter_red } else { t.meter_yellow }),
        );
    }
    let clip_r = Rect::from_min_max(r.min, pos2(r.max.x, r.min.y + 2.5));
    ui.painter().rect_filled(clip_r, 0.0, if clip { t.meter_red } else { t.meter_clip_off });
}

/// dB → 0..1 meter position (logarithmic-ish scale like hardware meters).
pub fn meter_pos(db: f32) -> f32 {
    if !db.is_finite() || db <= -80.0 {
        return 0.0;
    }
    if db >= 0.0 {
        return 1.0;
    }
    // -80..-40: bottom 10 %, -40..-20: 20 %, -20..0: 70 %.
    if db < -40.0 {
        (db + 80.0) / 40.0 * 0.1
    } else if db < -20.0 {
        0.1 + (db + 40.0) / 20.0 * 0.2
    } else {
        0.3 + (db + 20.0) / 20.0 * 0.7
    }
}

/// Clip gain fader taper: the top three quarters span -36..+36 dB evenly, the rest compresses to -144.
pub fn clip_gain_from_pos(pos: f32) -> f32 {
    let p = if pos.is_finite() { pos.clamp(0.0, 1.0) } else { 0.0 };
    if p >= 0.25 { -36.0 + (p - 0.25) / 0.75 * 72.0 } else { -36.0 - (1.0 - p / 0.25).powf(1.5) * 108.0 }
}

pub fn clip_gain_to_pos(db: f32) -> f32 {
    let db = if db.is_finite() { db.clamp(-144.0, 36.0) } else { -144.0 };
    if db >= -36.0 { 0.25 + (db + 36.0) / 72.0 * 0.75 } else { (1.0 - ((-36.0 - db) / 108.0).powf(1.0 / 1.5)) * 0.25 }
}

/// A fader: returns the new dB value when dragged. `db` in -144..12.
pub fn fader(ui: &mut Ui, r: Rect, db: f32, id: egui::Id) -> Option<f32> {
    let t = Tokens::current();
    let resp = ui.interact(r, id, Sense::click_and_drag());
    let slot = Rect::from_center_size(r.center(), vec2(5.0, r.height() - 14.0));
    ui.painter().rect_filled(slot, 2.0, t.fader_track);
    // Scale marks.
    for mark in [12.0f32, 6.0, 0.0, -5.0, -10.0, -15.0, -20.0, -30.0, -40.0, -60.0] {
        let y = slot.max.y - slot.height() * soundcraft_model::fader_db_to_pos(mark);
        ui.painter().line_segment([pos2(r.min.x + 2.0, y), pos2(slot.min.x - 3.0, y)], Stroke::new(1.0, Color32::from_rgb(110, 110, 110)));
        ui.painter().text(pos2(r.min.x, y), Align2::LEFT_CENTER, format!("{}", mark.abs()), regular(8.0), Color32::from_rgb(150, 150, 150));
    }
    let pos = soundcraft_model::fader_db_to_pos(db);
    let y = slot.max.y - slot.height() * pos;
    let cap = Rect::from_center_size(pos2(slot.center().x, y), vec2(r.width() * 0.62, 22.0));
    ui.painter().rect(cap, 2.0, Color32::from_rgb(70, 70, 72), Stroke::new(1.0, Color32::from_rgb(20, 20, 20)), StrokeKind::Inside);
    ui.painter().line_segment(
        [pos2(cap.min.x + 2.0, cap.center().y), pos2(cap.max.x - 2.0, cap.center().y)],
        Stroke::new(2.0, Color32::from_rgb(230, 230, 230)),
    );
    for dy in [-6.0, -3.0, 3.0, 6.0] {
        ui.painter().line_segment(
            [pos2(cap.min.x + 3.0, cap.center().y + dy), pos2(cap.max.x - 3.0, cap.center().y + dy)],
            Stroke::new(1.0, Color32::from_rgb(40, 40, 40)),
        );
    }
    if resp.double_clicked() || (resp.clicked() && ui.input(|i| i.modifiers.alt)) {
        return Some(0.0);
    }
    if resp.dragged() {
        let fine = ui.input(|i| i.modifiers.command || i.modifiers.ctrl);
        let dy = resp.drag_delta().y / slot.height() * if fine { 0.1 } else { 1.0 };
        let np = (pos - dy).clamp(0.0, 1.0);
        return Some(soundcraft_model::fader_pos_to_db(np).clamp(-144.0, 12.0));
    }
    None
}

pub fn db_text(db: f32) -> String {
    if db <= -143.9 { "-inf".into() } else { format!("{db:.1}") }
}

/// A speaker dot on the surround panner: label, square position (-1..1, y up = front), height layer.
pub struct PannerSpeaker {
    pub label: &'static str,
    pub x: f32,
    pub y: f32,
    pub height: bool,
}

/// What the user did on a [`surround_panner`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PannerEdit {
    /// The puck moved (x, y).
    Position(f32, f32),
    /// Divergence changed (Alt-drag on the square, or the slider).
    Divergence(f32),
    /// Elevation changed (height formats).
    Height(f32),
}

/// Pro Tools-style X/Y surround panner: a square room with speaker dots and a draggable puck.
/// Drag moves the puck; double-click returns it to front centre; Alt-drag (or the slider below)
/// sets divergence; height layouts get a second slider for elevation. Returns the edit, if any.
pub fn surround_panner(ui: &mut Ui, size: f32, speakers: &[PannerSpeaker], puck: (f32, f32), divergence: f32, z: Option<f32>) -> Option<PannerEdit> {
    let t = Tokens::current();
    let mut edit = None;
    let (r, resp) = ui.allocate_exact_size(vec2(size, size), Sense::click_and_drag());
    let half = size * 0.5 - 6.0;
    let c = r.center();
    let to_screen = |x: f32, y: f32| pos2(c.x + x * half, c.y - y * half);
    let painter = ui.painter_at(r.expand(1.0));
    painter.rect(r, CornerRadius::same(2), Color32::from_rgb(14, 18, 22), Stroke::new(1.0, Color32::from_rgb(70, 74, 80)), StrokeKind::Inside);
    let grid = Stroke::new(1.0, Color32::from_rgb(38, 44, 50));
    painter.line_segment([to_screen(-1.0, 0.0), to_screen(1.0, 0.0)], grid);
    painter.line_segment([to_screen(0.0, -1.0), to_screen(0.0, 1.0)], grid);
    painter.rect_stroke(Rect::from_two_pos(to_screen(-1.0, -1.0), to_screen(1.0, 1.0)), 0.0, grid, StrokeKind::Middle);
    // "Front" marker.
    painter.line_segment([pos2(c.x - 5.0, r.min.y + 2.0), pos2(c.x + 5.0, r.min.y + 2.0)], Stroke::new(2.0, Color32::from_rgb(90, 110, 130)));
    for s in speakers {
        let (x, y) = if s.height { (s.x * 0.55, s.y * 0.55) } else { (s.x, s.y) };
        let p = to_screen(x, y);
        if s.height {
            painter.circle_stroke(p, 2.5, Stroke::new(1.0, Color32::from_rgb(150, 150, 170)));
        } else {
            painter.circle_filled(p, 3.0, Color32::from_rgb(170, 170, 176));
        }
        if size >= 70.0 && !s.height {
            let inward = vec2(-(x.signum() * if x.abs() > 0.5 { 1.0 } else { 0.0 }), if y.abs() > 0.5 { y.signum() } else { 0.0 });
            let lp = p + vec2(inward.x * 9.0, inward.y * 7.0);
            painter.text(lp, Align2::CENTER_CENTER, s.label, regular(7.5), t.text_dim);
        }
    }
    let alt = ui.input(|i| i.modifiers.alt);
    if resp.double_clicked() {
        edit = Some(PannerEdit::Position(0.0, 1.0));
    } else if resp.dragged() && alt {
        let d = (divergence - resp.drag_delta().y * 0.01).clamp(0.0, 1.0);
        edit = Some(PannerEdit::Divergence(d));
    } else if (resp.dragged() || resp.clicked())
        && !alt
        && let Some(p) = resp.interact_pointer_pos()
    {
        let x = ((p.x - c.x) / half).clamp(-1.0, 1.0);
        let y = ((c.y - p.y) / half).clamp(-1.0, 1.0);
        edit = Some(PannerEdit::Position(x, y));
    }
    let (px, py) = match edit {
        Some(PannerEdit::Position(x, y)) => (x, y),
        _ => puck,
    };
    let pp = to_screen(px, py);
    let div = match edit {
        Some(PannerEdit::Divergence(d)) => d,
        _ => divergence,
    };
    if div > 0.01 {
        painter.circle(pp, 5.0 + div * half, Color32::from_rgba_unmultiplied(80, 160, 230, 28), Stroke::new(1.0, Color32::from_rgb(70, 130, 190)));
    }
    painter.circle(pp, 5.0, t.counter_text, Stroke::new(1.0, Color32::BLACK));
    let tip = crate::i18n::tr("Surround pan: drag the puck · double-click: front centre · Alt-drag: divergence");
    let _ = resp.on_hover_text(tip);
    // Sliders below: divergence, then elevation for height formats.
    let mut sliders: Vec<(&str, f32, u8)> = vec![("div", div, 0)];
    if let Some(z) = z {
        sliders.push(("z", z, 1));
    }
    for (label, v, kind) in sliders {
        let (sr, sresp) = ui.allocate_exact_size(vec2(size, 11.0), Sense::click_and_drag());
        let track = Rect::from_min_max(pos2(sr.min.x + 16.0, sr.center().y - 2.0), pos2(sr.max.x - 2.0, sr.center().y + 2.0));
        ui.painter().text(pos2(sr.min.x + 1.0, sr.center().y), Align2::LEFT_CENTER, label, regular(8.0), t.text_dim);
        ui.painter().rect_filled(track, 1.0, Color32::from_rgb(24, 26, 30));
        ui.painter().rect_filled(
            Rect::from_min_max(track.min, pos2(track.min.x + track.width() * v.clamp(0.0, 1.0), track.max.y)),
            1.0,
            t.counter_text,
        );
        if (sresp.dragged() || sresp.clicked())
            && let Some(p) = sresp.interact_pointer_pos()
        {
            let nv = ((p.x - track.min.x) / track.width().max(1.0)).clamp(0.0, 1.0);
            edit = Some(if kind == 0 { PannerEdit::Divergence(nv) } else { PannerEdit::Height(nv) });
        }
        if sresp.double_clicked() {
            edit = Some(if kind == 0 { PannerEdit::Divergence(0.0) } else { PannerEdit::Height(0.0) });
        }
        let _ = sresp.on_hover_text(crate::i18n::tr(if kind == 0 { "Divergence (0 = point source)" } else { "Elevation (height speakers)" }));
    }
    edit
}

/// A bank of thin vertical peak meters, one per channel, labelled underneath (rotated text).
pub fn multi_meter(ui: &Ui, r: Rect, levels: &[f32], labels: &[String], clip: bool) {
    let n = levels.len().max(1);
    let label_h = 16.0;
    let mr = Rect::from_min_max(r.min, pos2(r.max.x, r.max.y - label_h));
    let gap = 1.0;
    let w = ((mr.width() - gap * (n as f32 - 1.0)) / n as f32).max(1.0);
    for (i, lv) in levels.iter().enumerate() {
        let x = mr.min.x + i as f32 * (w + gap);
        let br = Rect::from_min_size(pos2(x, mr.min.y), vec2(w, mr.height()));
        meter(ui, br, *lv, *lv, clip);
        if let Some(l) = labels.get(i) {
            let galley = ui.painter().layout_no_wrap(l.clone(), regular(7.0), Tokens::current().text_dim);
            let gw = galley.size().x;
            let gh = galley.size().y;
            // Rotated 90° anticlockwise, reading bottom-to-top, centred under the bar.
            let pos = pos2(br.center().x - gh * 0.5, mr.max.y + 2.0 + gw.min(label_h - 2.0));
            ui.painter().add(egui::epaint::TextShape::new(pos, galley, Tokens::current().text_dim).with_angle(-std::f32::consts::FRAC_PI_2));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{clip_gain_from_pos, clip_gain_to_pos};

    #[test]
    fn clip_gain_taper_spans_minus_144_to_plus_36_and_round_trips() {
        assert_eq!(clip_gain_from_pos(0.0), -144.0);
        assert_eq!(clip_gain_from_pos(1.0), 36.0);
        assert_eq!(clip_gain_from_pos(f32::NAN), -144.0);
        assert_eq!(clip_gain_to_pos(f32::INFINITY), 0.0);
        for db in [-144.0, -90.0, -36.0, -12.5, 0.0, 6.0, 36.0] {
            assert!((clip_gain_from_pos(clip_gain_to_pos(db)) - db).abs() < 0.01, "{db}");
        }
    }
}
