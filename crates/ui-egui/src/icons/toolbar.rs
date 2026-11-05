//! Original, flat toolbar glyphs drawn from lines and simple geometry (MIT OR Apache-2.0).

use super::p;
use egui::{Color32, Painter, Rect, Shape, Stroke, StrokeKind, pos2};

fn path(painter: &Painter, r: Rect, points: &[(f32, f32)], stroke: Stroke) {
    painter.add(Shape::line(points.iter().map(|&(x, y)| p(r, x, y)).collect(), stroke));
}

fn triangle(painter: &Painter, r: Rect, points: [(f32, f32); 3], color: Color32) {
    painter.add(Shape::convex_polygon(points.map(|(x, y)| p(r, x, y)).to_vec(), color, Stroke::NONE));
}

pub(crate) fn draw(painter: &Painter, r: Rect, name: &str, color: Color32) {
    let stroke = Stroke::new((r.width() / 18.0).clamp(1.0, 1.8), color);
    match name {
        "transport_cycle" => transport_cycle(painter, r, color),
        "transport_stop" => transport_stop(painter, r, color),
        "transport_play" => transport_action(painter, r, stroke, false),
        "transport_record" => transport_action(painter, r, stroke, true),
        "arrow_left" | "arrow_right" => arrow(painter, r, name == "arrow_left", color),
        "wave_zoom" => wave_zoom(painter, r, stroke),
        "zoom_fit" => zoom_fit(painter, r, stroke),
        "zoom_toggle" => zoom_toggle(painter, r, stroke),
        "zoom" => zoom(painter, r, stroke),
        "trim" => trim(painter, r, stroke),
        "selector" => selector(painter, r, stroke),
        "grabber" => grabber(painter, r, stroke),
        "scrubber" => scrubber(painter, r, stroke),
        "pencil" => pencil(painter, r, stroke),
        "tab_transient" => transient(painter, r, stroke),
        "link_timeline" => timeline_link(painter, r, stroke),
        "link_track" => track_link(painter, r, stroke),
        "insertion_playback" => insertion_playback(painter, r, stroke),
        "automation_edit" => automation(painter, r, stroke),
        "markers_edit" => markers(painter, r, stroke),
        "mirrored_midi" => mirrored(painter, r, stroke),
        "layered_edit" => layers(painter, r, stroke),
        _ => super::draw(painter, r, name, color),
    }
}

fn transport_cycle(painter: &Painter, r: Rect, color: Color32) {
    // A broken clock arc and a separate hand, authored as simple geometry.
    let points = (0..=18)
        .map(|i| {
            let angle = 0.95 + (4.62 - 0.95) * i as f32 / 18.0;
            p(r, 0.5 + angle.cos() * 0.17, 0.54 + angle.sin() * 0.17)
        })
        .collect();
    painter.add(Shape::line(points, Stroke::new(r.width() * 0.16, color)));
    let hand = [(0.58, 0.29), (0.68, 0.34), (0.74, 0.45), (0.75, 0.55), (0.70, 0.66), (0.60, 0.53)];
    painter.add(Shape::convex_polygon(hand.map(|(x, y)| p(r, x, y)).to_vec(), color, Stroke::NONE));
}

fn transport_stop(painter: &Painter, r: Rect, color: Color32) {
    painter.rect_filled(Rect::from_min_max(p(r, 0.27, 0.27), p(r, 0.73, 0.73)), 0.0, color);
}

/// The return frame is part of the toolbar artwork, independent of loop settings.
fn transport_action(painter: &Painter, r: Rect, s: Stroke, record: bool) {
    if record {
        painter.circle_filled(p(r, 0.53, 0.427), r.width() * 0.14, s.color);
    } else {
        triangle(painter, r, [(0.46, 0.26), (0.675, 0.425), (0.46, 0.59)], s.color);
    }
    transport_return(painter, r, s, record);
}

/// A rounded return path leaves a visible gap around the filled action symbol.
fn transport_return(painter: &Painter, r: Rect, s: Stroke, record: bool) {
    use std::f32::consts::{FRAC_PI_2, PI};
    let (base, tip) = if record { (0.19, 0.35) } else { (0.235, 0.425) };
    let mut points = Vec::with_capacity(26);
    points.push(p(r, base, 0.425));
    for (cx, cy, start, end) in
        [(0.18, 0.505, -FRAC_PI_2, -PI), (0.18, 0.655, PI, FRAC_PI_2), (0.83, 0.655, FRAC_PI_2, 0.0), (0.83, 0.505, 0.0, -FRAC_PI_2)]
    {
        points.extend((0..=5).map(|i| {
            let angle = start + (end - start) * i as f32 / 5.0;
            p(r, cx + angle.cos() * 0.08, cy + angle.sin() * 0.08)
        }));
    }
    points.push(p(r, 0.77, 0.425));
    painter.add(Shape::line(points, s));
    triangle(painter, r, [(tip, 0.425), (base, 0.36), (base, 0.49)], s.color);
}

fn arrow(painter: &Painter, r: Rect, left: bool, color: Color32) {
    let (tip, base) = if left { (0.15, 0.85) } else { (0.85, 0.15) };
    triangle(painter, r, [(tip, 0.5), (base, 0.22), (base, 0.78)], color);
}

fn vertical_arrows(painter: &Painter, r: Rect, color: Color32) {
    triangle(painter, r, [(0.5, 0.05), (0.39, 0.17), (0.61, 0.17)], color);
    triangle(painter, r, [(0.5, 0.95), (0.39, 0.83), (0.61, 0.83)], color);
}

fn wave_zoom(painter: &Painter, r: Rect, s: Stroke) {
    vertical_arrows(painter, r, s.color);
    for (i, height) in [0.08, 0.16, 0.29, 0.17, 0.24, 0.12, 0.06].into_iter().enumerate() {
        let x = 0.17 + i as f32 * 0.11;
        painter.line_segment([p(r, x, 0.5 - height), p(r, x, 0.5 + height)], s);
    }
}

fn zoom_fit(painter: &Painter, r: Rect, s: Stroke) {
    vertical_arrows(painter, r, s.color);
    for (y, half) in [(0.31, 0.20), (0.43, 0.32), (0.57, 0.19), (0.69, 0.30)] {
        painter.rect_stroke(Rect::from_min_max(p(r, 0.5 - half, y - 0.03), p(r, 0.5 + half, y + 0.03)), 0.0, s, StrokeKind::Inside);
    }
}

fn zoom_toggle(painter: &Painter, r: Rect, s: Stroke) {
    painter.line_segment([p(r, 0.13, 0.5), p(r, 0.87, 0.5)], s);
    painter.line_segment([p(r, 0.5, 0.17), p(r, 0.5, 0.83)], s);
    for (tip, base) in [(0.05, 0.20), (0.95, 0.80)] {
        triangle(painter, r, [(tip, 0.5), (base, 0.35), (base, 0.65)], s.color);
    }
    vertical_arrows(painter, r, s.color);
}

fn zoom(painter: &Painter, r: Rect, s: Stroke) {
    painter.circle_stroke(p(r, 0.39, 0.35), r.width() * 0.24, s);
    painter.line_segment([p(r, 0.56, 0.53), p(r, 0.80, 0.78)], Stroke::new(s.width * 1.4, s.color));
}

fn trim(painter: &Painter, r: Rect, s: Stroke) {
    path(painter, r, &[(0.40, 0.20), (0.18, 0.20), (0.18, 0.78), (0.40, 0.78)], s);
    path(painter, r, &[(0.60, 0.20), (0.82, 0.20), (0.82, 0.78), (0.60, 0.78)], s);
    painter.line_segment([p(r, 0.30, 0.49), p(r, 0.70, 0.49)], s);
    triangle(painter, r, [(0.23, 0.49), (0.39, 0.38), (0.39, 0.60)], s.color);
    triangle(painter, r, [(0.77, 0.49), (0.61, 0.38), (0.61, 0.60)], s.color);
}

fn selector(painter: &Painter, r: Rect, s: Stroke) {
    for (i, half) in [0.10, 0.21, 0.35, 0.17, 0.29, 0.40, 0.19, 0.10].into_iter().enumerate() {
        let x = 0.13 + i as f32 * 0.105;
        painter.line_segment([p(r, x, 0.5 - half), p(r, x, 0.5 + half)], s);
    }
    painter.line_segment([p(r, 0.5, 0.10), p(r, 0.5, 0.90)], s);
    for y in [0.10, 0.90] {
        painter.line_segment([p(r, 0.43, y), p(r, 0.57, y)], s);
    }
}

fn grabber(painter: &Painter, r: Rect, s: Stroke) {
    let points = [
        (0.34, 0.91),
        (0.26, 0.75),
        (0.15, 0.55),
        (0.14, 0.47),
        (0.19, 0.43),
        (0.25, 0.46),
        (0.33, 0.58),
        (0.25, 0.32),
        (0.27, 0.25),
        (0.32, 0.24),
        (0.36, 0.28),
        (0.43, 0.49),
        (0.38, 0.17),
        (0.42, 0.12),
        (0.47, 0.12),
        (0.51, 0.18),
        (0.54, 0.46),
        (0.56, 0.18),
        (0.61, 0.14),
        (0.66, 0.17),
        (0.68, 0.23),
        (0.66, 0.49),
        (0.72, 0.32),
        (0.77, 0.30),
        (0.82, 0.34),
        (0.82, 0.40),
        (0.76, 0.68),
        (0.69, 0.80),
        (0.65, 0.91),
    ];
    painter.add(Shape::closed_line(points.into_iter().map(|(x, y)| p(r, x, y)).collect(), s));
}

fn scrubber(painter: &Painter, r: Rect, s: Stroke) {
    path(painter, r, &[(0.17, 0.37), (0.29, 0.37), (0.45, 0.19), (0.45, 0.81), (0.29, 0.63), (0.17, 0.63), (0.17, 0.37)], s);
    for radius in [0.16, 0.29, 0.41] {
        let points = (0..=12)
            .map(|i| {
                let a = -0.8 + 1.6 * i as f32 / 12.0;
                pos2(r.min.x + r.width() * (0.45 + a.cos() * radius), r.min.y + r.height() * (0.5 + a.sin() * radius))
            })
            .collect();
        painter.add(Shape::line(points, s));
    }
}

fn pencil(painter: &Painter, r: Rect, s: Stroke) {
    path(painter, r, &[(0.16, 0.43), (0.16, 0.79), (0.46, 0.79)], s);
    path(painter, r, &[(0.26, 0.72), (0.33, 0.53), (0.70, 0.16), (0.81, 0.27), (0.44, 0.64), (0.26, 0.72)], s);
    painter.line_segment([p(r, 0.33, 0.53), p(r, 0.44, 0.64)], s);
    painter.line_segment([p(r, 0.65, 0.21), p(r, 0.76, 0.32)], s);
}

fn transient(painter: &Painter, r: Rect, s: Stroke) {
    painter.line_segment([p(r, 0.06, 0.5), p(r, 0.30, 0.5)], s);
    triangle(painter, r, [(0.43, 0.5), (0.27, 0.28), (0.27, 0.72)], s.color);
    painter.line_segment([p(r, 0.51, 0.10), p(r, 0.51, 0.90)], s);
    for (x, half) in [(0.63, 0.31), (0.75, 0.18), (0.87, 0.08)] {
        painter.line_segment([p(r, x, 0.5 - half), p(r, x, 0.5 + half)], s);
    }
}

fn timeline_link(painter: &Painter, r: Rect, s: Stroke) {
    for x in [0.18, 0.57] {
        painter.line_segment([p(r, x, 0.15), p(r, x, 0.85)], s);
        triangle(painter, r, [(x, 0.05), (x - 0.09, 0.25), (x + 0.09, 0.25)], s.color);
        triangle(painter, r, [(x, 0.95), (x - 0.09, 0.75), (x + 0.09, 0.75)], s.color);
    }
    for (x, half) in [(0.34, 0.19), (0.75, 0.31), (0.88, 0.12)] {
        painter.line_segment([p(r, x, 0.5 - half), p(r, x, 0.5 + half)], s);
    }
}

fn track_link(painter: &Painter, r: Rect, s: Stroke) {
    for y in [0.24, 0.50, 0.76] {
        painter.line_segment([p(r, 0.10, y), p(r, 0.48, y)], s);
    }
    for (x, half) in [(0.63, 0.15), (0.76, 0.34), (0.89, 0.22)] {
        painter.line_segment([p(r, x, 0.5 - half), p(r, x, 0.5 + half)], s);
    }
}

fn insertion_playback(painter: &Painter, r: Rect, s: Stroke) {
    for x in [0.12, 0.88] {
        painter.line_segment([p(r, x, 0.15), p(r, x, 0.85)], s);
    }
    painter.line_segment([p(r, 0.28, 0.25), p(r, 0.28, 0.75)], s);
    triangle(painter, r, [(0.44, 0.22), (0.76, 0.5), (0.44, 0.78)], s.color);
}

fn automation(painter: &Painter, r: Rect, s: Stroke) {
    let points = [(0.12, 0.76), (0.39, 0.30), (0.68, 0.55), (0.90, 0.16)].map(|(x, y)| p(r, x, y));
    let radius = r.width() * 0.065;
    for (a, b) in points.into_iter().zip(points.into_iter().skip(1)) {
        let delta = b - a;
        let inset = delta / delta.length().max(1.0) * radius;
        painter.line_segment([a + inset, b - inset], s);
    }
    for point in points {
        painter.circle_stroke(point, radius, s);
    }
}

fn markers(painter: &Painter, r: Rect, s: Stroke) {
    path(painter, r, &[(0.12, 0.12), (0.43, 0.12), (0.43, 0.58), (0.28, 0.90), (0.12, 0.58), (0.12, 0.12)], s);
    painter.line_segment([p(r, 0.55, 0.49), p(r, 0.86, 0.49)], s);
    triangle(painter, r, [(0.97, 0.49), (0.79, 0.30), (0.79, 0.68)], s.color);
}

fn mirrored(painter: &Painter, r: Rect, s: Stroke) {
    for (x, y) in [(0.06, 0.32), (0.65, 0.18), (0.65, 0.62)] {
        painter.rect_stroke(Rect::from_min_max(p(r, x, y), p(r, x + 0.26, y + 0.20)), 0.0, s, StrokeKind::Inside);
    }
    painter.line_segment([p(r, 0.38, 0.5), p(r, 0.57, 0.5)], s);
    triangle(painter, r, [(0.65, 0.5), (0.52, 0.36), (0.52, 0.64)], s.color);
}

fn layers(painter: &Painter, r: Rect, s: Stroke) {
    for (x, y) in [(0.10, 0.12), (0.23, 0.27), (0.36, 0.42)] {
        painter.rect_stroke(Rect::from_min_max(p(r, x, y), p(r, x + 0.54, y + 0.47)), 0.0, s, StrokeKind::Inside);
    }
}
