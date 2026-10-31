//! Icons drawn procedurally with the egui painter. Original SoundCraft artwork (MIT OR Apache-2.0);
//! no image files and nothing derived from any other product's icons.

use egui::{Color32, Painter, Pos2, Rect, Shape, Stroke, pos2, vec2};

fn p(r: Rect, x: f32, y: f32) -> Pos2 {
    pos2(r.min.x + r.width() * x, r.min.y + r.height() * y)
}

/// Draw icon `name` inside `r` with colour `c`.
pub fn draw(painter: &Painter, r: Rect, name: &str, c: Color32) {
    let s = Stroke::new((r.width() / 14.0).clamp(1.2, 2.4), c);
    match name {
        "play" => {
            painter.add(Shape::convex_polygon(vec![p(r, 0.28, 0.18), p(r, 0.82, 0.5), p(r, 0.28, 0.82)], c, Stroke::NONE));
        }
        "stop" => {
            painter.rect_filled(Rect::from_min_max(p(r, 0.26, 0.26), p(r, 0.74, 0.74)), 1.0, c);
        }
        "record" => {
            painter.circle_filled(r.center(), r.width() * 0.27, c);
        }
        "rtz" => {
            painter.rect_filled(Rect::from_min_max(p(r, 0.24, 0.26), p(r, 0.34, 0.74)), 0.0, c);
            painter.add(Shape::convex_polygon(vec![p(r, 0.76, 0.26), p(r, 0.76, 0.74), p(r, 0.36, 0.5)], c, Stroke::NONE));
        }
        "end" => {
            painter.rect_filled(Rect::from_min_max(p(r, 0.66, 0.26), p(r, 0.76, 0.74)), 0.0, c);
            painter.add(Shape::convex_polygon(vec![p(r, 0.24, 0.26), p(r, 0.64, 0.5), p(r, 0.24, 0.74)], c, Stroke::NONE));
        }
        "rewind" => {
            for dx in [0.0, 0.26] {
                painter.add(Shape::convex_polygon(vec![p(r, 0.5 + dx, 0.28), p(r, 0.5 + dx, 0.72), p(r, 0.22 + dx, 0.5)], c, Stroke::NONE));
            }
        }
        "ffwd" => {
            for dx in [0.0, 0.26] {
                painter.add(Shape::convex_polygon(vec![p(r, 0.24 + dx, 0.28), p(r, 0.52 + dx, 0.5), p(r, 0.24 + dx, 0.72)], c, Stroke::NONE));
            }
        }
        "loop" => {
            let rr = Rect::from_min_max(p(r, 0.2, 0.3), p(r, 0.8, 0.7));
            painter.rect_stroke(rr, rr.height() * 0.5, s, egui::StrokeKind::Middle);
            painter.add(Shape::convex_polygon(vec![p(r, 0.55, 0.2), p(r, 0.68, 0.3), p(r, 0.55, 0.4)], c, Stroke::NONE));
        }
        "zoom" => {
            painter.circle_stroke(p(r, 0.42, 0.4), r.width() * 0.2, s);
            painter.line_segment([p(r, 0.56, 0.55), p(r, 0.8, 0.8)], Stroke::new(s.width * 1.6, c));
        }
        "zoom_toggle" => {
            for (x, y, dx, dy) in [(0.18, 0.24, 0.16, 0.14), (0.82, 0.24, -0.16, 0.14), (0.18, 0.76, 0.16, -0.14), (0.82, 0.76, -0.16, -0.14)] {
                painter.add(Shape::line(vec![p(r, x + dx, y), p(r, x, y), p(r, x, y + dy)], s));
            }
            painter.rect_filled(Rect::from_min_max(p(r, 0.34, 0.41), p(r, 0.66, 0.59)), 1.0, c);
        }
        "trim" => {
            painter.line_segment([p(r, 0.5, 0.2), p(r, 0.5, 0.8)], s);
            painter.line_segment([p(r, 0.15, 0.5), p(r, 0.85, 0.5)], s);
            for (a, b, d) in [(0.15, 0.5, 1.0), (0.85, 0.5, -1.0)] {
                painter.add(Shape::convex_polygon(vec![p(r, a, b), p(r, a + 0.12 * d, b - 0.1), p(r, a + 0.12 * d, b + 0.1)], c, Stroke::NONE));
            }
        }
        "selector" => {
            painter.line_segment([p(r, 0.5, 0.2), p(r, 0.5, 0.8)], s);
            painter.line_segment([p(r, 0.4, 0.2), p(r, 0.6, 0.2)], s);
            painter.line_segment([p(r, 0.4, 0.8), p(r, 0.6, 0.8)], s);
            // Waveform hint on either side.
            for (i, h) in [0.1, 0.22, 0.14, 0.26, 0.12].iter().enumerate() {
                let x = 0.12 + i as f32 * 0.06;
                painter.line_segment([p(r, x, 0.5 - h), p(r, x, 0.5 + h)], Stroke::new(s.width * 0.7, c));
                let x2 = 0.88 - i as f32 * 0.06;
                painter.line_segment([p(r, x2, 0.5 - h), p(r, x2, 0.5 + h)], Stroke::new(s.width * 0.7, c));
            }
        }
        "grabber" => {
            // A stylised open hand: palm + four fingers + thumb.
            let palm = Rect::from_min_max(p(r, 0.3, 0.45), p(r, 0.72, 0.82));
            painter.rect_stroke(palm, 4.0, s, egui::StrokeKind::Middle);
            for (i, top) in [0.22, 0.16, 0.18, 0.26].iter().enumerate() {
                let x = 0.34 + i as f32 * 0.11;
                painter.line_segment([p(r, x, 0.47), p(r, x, *top)], s);
            }
            painter.line_segment([p(r, 0.3, 0.62), p(r, 0.18, 0.48)], s);
        }
        "scrubber" => {
            painter.add(Shape::convex_polygon(
                vec![p(r, 0.18, 0.4), p(r, 0.3, 0.4), p(r, 0.46, 0.24), p(r, 0.46, 0.76), p(r, 0.3, 0.6), p(r, 0.18, 0.6)],
                c,
                Stroke::NONE,
            ));
            for (k, rad) in [0.14f32, 0.26].iter().enumerate() {
                let n = 8;
                let pts: Vec<Pos2> = (0..=n)
                    .map(|i| {
                        let a = -0.8 + 1.6 * i as f32 / n as f32;
                        let cc = p(r, 0.46, 0.5);
                        pos2(cc.x + a.cos() * r.width() * rad, cc.y + a.sin() * r.height() * rad)
                    })
                    .collect();
                painter.add(Shape::line(pts, Stroke::new(s.width * (1.0 - k as f32 * 0.2), c)));
            }
        }
        "pencil" => {
            painter.line_segment([p(r, 0.25, 0.75), p(r, 0.72, 0.28)], Stroke::new(s.width * 2.2, c));
            painter.add(Shape::convex_polygon(vec![p(r, 0.18, 0.82), p(r, 0.22, 0.66), p(r, 0.34, 0.78)], c, Stroke::NONE));
        }
        "speaker" => {
            painter.add(Shape::convex_polygon(
                vec![p(r, 0.2, 0.4), p(r, 0.34, 0.4), p(r, 0.52, 0.22), p(r, 0.52, 0.78), p(r, 0.34, 0.6), p(r, 0.2, 0.6)],
                c,
                Stroke::NONE,
            ));
        }
        "click" => {
            painter.add(Shape::convex_polygon(vec![p(r, 0.5, 0.15), p(r, 0.78, 0.85), p(r, 0.22, 0.85)], Color32::TRANSPARENT, s));
            painter.line_segment([p(r, 0.5, 0.7), p(r, 0.7, 0.25)], s);
        }
        "link" => {
            painter.circle_stroke(p(r, 0.38, 0.5), r.width() * 0.16, s);
            painter.circle_stroke(p(r, 0.62, 0.5), r.width() * 0.16, s);
        }
        "plus" => {
            painter.line_segment([p(r, 0.5, 0.25), p(r, 0.5, 0.75)], s);
            painter.line_segment([p(r, 0.25, 0.5), p(r, 0.75, 0.5)], s);
        }
        "minus" => {
            painter.line_segment([p(r, 0.25, 0.5), p(r, 0.75, 0.5)], s);
        }
        "triangle_down" => {
            painter.add(Shape::convex_polygon(vec![p(r, 0.25, 0.35), p(r, 0.75, 0.35), p(r, 0.5, 0.68)], c, Stroke::NONE));
        }
        "note" => {
            painter.circle_filled(p(r, 0.38, 0.72), r.width() * 0.13, c);
            painter.line_segment([p(r, 0.5, 0.72), p(r, 0.5, 0.2)], s);
            painter.line_segment([p(r, 0.5, 0.2), p(r, 0.72, 0.32)], s);
        }
        "gear" => {
            painter.circle_stroke(r.center(), r.width() * 0.2, s);
            for i in 0..8 {
                let a = i as f32 * std::f32::consts::TAU / 8.0;
                let c0 = r.center() + vec2(a.cos(), a.sin()) * r.width() * 0.22;
                let c1 = r.center() + vec2(a.cos(), a.sin()) * r.width() * 0.34;
                painter.line_segment([c0, c1], Stroke::new(s.width * 1.4, c));
            }
        }
        "close" => {
            // A code-drawn close cross: no bundled UI font has U+2715 in every family.
            painter.line_segment([p(r, 0.3, 0.3), p(r, 0.7, 0.7)], s);
            painter.line_segment([p(r, 0.7, 0.3), p(r, 0.3, 0.7)], s);
        }
        "lock" => {
            painter.rect_filled(Rect::from_min_max(p(r, 0.22, 0.46), p(r, 0.78, 0.88)), r.width() * 0.08, c);
            let (x, y, w) = (r.center().x, r.min.y + r.height() * 0.46, r.width() * 0.19);
            let arc: Vec<Pos2> =
                (0..=12).map(|i| std::f32::consts::PI * i as f32 / 12.0).map(|a| pos2(x - w * a.cos(), y - w * 1.4 * a.sin())).collect();
            painter.add(Shape::line(arc, s));
        }
        "wave" => {
            let n = 24;
            let pts: Vec<Pos2> = (0..=n)
                .map(|i| {
                    let t = i as f32 / n as f32;
                    pos2(
                        r.min.x + r.width() * (0.12 + 0.76 * t),
                        r.center().y - (t * 18.0).sin() * r.height() * 0.28 * (1.0 - (t - 0.5).abs() * 1.6).max(0.1),
                    )
                })
                .collect();
            painter.add(Shape::line(pts, s));
        }
        "tab_transient" => {
            painter.line_segment([p(r, 0.08, 0.5), p(r, 0.3, 0.5)], s);
            painter.add(Shape::convex_polygon(vec![p(r, 0.42, 0.5), p(r, 0.28, 0.36), p(r, 0.28, 0.64)], c, Stroke::NONE));
            painter.line_segment([p(r, 0.5, 0.16), p(r, 0.5, 0.84)], s);
            for (x, h) in [(0.64, 0.32), (0.78, 0.2), (0.92, 0.1)] {
                painter.line_segment([p(r, x, 0.5 - h), p(r, x, 0.5 + h)], s);
            }
        }
        _ => {
            painter.rect_stroke(r.shrink(r.width() * 0.25), 1.0, s, egui::StrokeKind::Middle);
        }
    }
}
