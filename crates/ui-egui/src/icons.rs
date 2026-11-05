//! Icons drawn procedurally with the egui painter. Original SoundCraft artwork (MIT OR Apache-2.0);
//! no image files and nothing derived from any other product's icons.

use egui::{Color32, Painter, Pos2, Rect, Shape, Stroke, pos2, vec2};

mod toolbar;
pub(crate) use toolbar::draw as draw_toolbar;

fn p(r: Rect, x: f32, y: f32) -> Pos2 {
    pos2(r.min.x + r.width() * x, r.min.y + r.height() * y)
}

fn group_marker(painter: &Painter, r: Rect, c: Color32, active: bool) {
    let diamond = vec![p(r, 0.5, 0.1), p(r, 0.9, 0.5), p(r, 0.5, 0.9), p(r, 0.1, 0.5)];
    painter.add(Shape::convex_polygon(diamond, if active { c } else { Color32::TRANSPARENT }, Stroke::new(0.8, c)));
}

/// Draw icon `name` inside `r` with colour `c`.
pub fn draw(painter: &Painter, r: Rect, name: &str, c: Color32) {
    let s = Stroke::new((r.width() / 14.0).clamp(1.2, 2.4), c);
    match name {
        "arrow_left" | "arrow_right" => {
            let (tip, base) = if name == "arrow_left" { (0.3, 0.7) } else { (0.7, 0.3) };
            painter.add(Shape::convex_polygon(vec![p(r, tip, 0.5), p(r, base, 0.25), p(r, base, 0.75)], c, Stroke::NONE));
        }
        "zoom_fit" => {
            for (y, width) in [(0.36, 0.28), (0.5, 0.38), (0.64, 0.2)] {
                painter.line_segment([p(r, 0.5 - width, y), p(r, 0.5 + width, y)], s);
            }
            for (y, direction) in [(0.12, -1.0), (0.88, 1.0)] {
                painter.add(Shape::convex_polygon(
                    vec![p(r, 0.5, y + direction * 0.06), p(r, 0.38, y - direction * 0.04), p(r, 0.62, y - direction * 0.04)],
                    c,
                    Stroke::NONE,
                ));
            }
        }
        "link_timeline" => {
            painter.line_segment([p(r, 0.5, 0.18), p(r, 0.5, 0.82)], s);
            for y in [0.32, 0.5, 0.68] {
                painter.line_segment([p(r, 0.12, y), p(r, 0.88, y)], Stroke::new(s.width * 0.7, c));
            }
            for x in [0.08, 0.92] {
                painter.line_segment([p(r, x, 0.36), p(r, x, 0.64)], s);
            }
        }
        "insertion_playback" => {
            for x in [0.18, 0.8] {
                painter.line_segment([p(r, x, 0.18), p(r, x, 0.82)], s);
            }
            painter.add(Shape::convex_polygon(vec![p(r, 0.36, 0.27), p(r, 0.67, 0.5), p(r, 0.36, 0.73)], c, Stroke::NONE));
        }
        "automation_edit" => {
            painter.add(Shape::line(vec![p(r, 0.1, 0.72), p(r, 0.36, 0.35), p(r, 0.66, 0.65), p(r, 0.9, 0.25)], s));
            for (x, y) in [(0.1, 0.72), (0.36, 0.35), (0.66, 0.65), (0.9, 0.25)] {
                painter.circle_filled(p(r, x, y), r.width() * 0.07, c);
            }
        }
        "markers_edit" => {
            draw(painter, r.translate(vec2(-r.width() * 0.18, 0.0)), "marker", c);
            draw(painter, Rect::from_min_max(p(r, 0.64, 0.3), p(r, 0.98, 0.7)), "triangle_down", c);
        }
        "mirrored_midi" | "layered_edit" => {
            for (x, y) in [(0.12, 0.26), (0.58, 0.58)] {
                painter.rect_stroke(Rect::from_min_max(p(r, x, y), p(r, x + 0.32, y + 0.2)), 0.0, s, egui::StrokeKind::Inside);
            }
            painter.line_segment([p(r, 0.47, 0.5), p(r, 0.55, 0.5)], s);
            if name == "layered_edit" {
                painter.rect_stroke(Rect::from_min_max(p(r, 0.28, 0.14), p(r, 0.72, 0.86)), 0.0, s, egui::StrokeKind::Inside);
            }
        }
        "audio_status" => {
            painter.rect_filled(Rect::from_min_max(p(r, 0.08, 0.18), p(r, 0.92, 0.82)), 0.0, c);
            for x in [0.3, 0.7] {
                painter.vline(
                    r.min.x + r.width() * x,
                    r.min.y + r.height() * 0.3..=r.min.y + r.height() * 0.7,
                    Stroke::new(s.width * 0.7, crate::theme::Tokens::current().counter_bg),
                );
            }
        }
        "session_status" => {
            painter.rect_stroke(Rect::from_min_max(p(r, 0.2, 0.12), p(r, 0.78, 0.88)), 0.0, s, egui::StrokeKind::Inside);
            painter.line_segment([p(r, 0.3, 0.62), p(r, 0.65, 0.62)], s);
        }
        "clock_status" => {
            painter.circle_stroke(r.center(), r.width().min(r.height()) * 0.34, s);
            painter.add(Shape::line(vec![p(r, 0.5, 0.25), p(r, 0.5, 0.5), p(r, 0.7, 0.5)], s));
        }
        "loop_playback" => {
            draw(painter, r, "loop", c);
            painter.line_segment([p(r, 0.5, 0.32), p(r, 0.5, 0.68)], s);
        }
        "play_outline" | "loop_play" => {
            painter.add(Shape::closed_line(vec![p(r, 0.34, 0.2), p(r, 0.76, 0.5), p(r, 0.34, 0.8)], s));
            if name == "loop_play" {
                draw(painter, r, "loop", c);
            }
        }
        "record_outline" => {
            painter.circle_filled(r.center(), r.width() * 0.16, c);
            painter.circle_stroke(r.center(), r.width() * 0.31, s);
        }
        "input_quantize" => {
            painter.add(Shape::line(
                vec![
                    p(r, 0.15, 0.35),
                    p(r, 0.38, 0.35),
                    p(r, 0.38, 0.22),
                    p(r, 0.75, 0.22),
                    p(r, 0.75, 0.78),
                    p(r, 0.38, 0.78),
                    p(r, 0.38, 0.65),
                    p(r, 0.15, 0.65),
                ],
                s,
            ));
            painter.line_segment([p(r, 0.18, 0.5), p(r, 0.58, 0.5)], s);
        }
        "columns" => {
            painter.rect_stroke(Rect::from_min_max(p(r, 0.08, 0.12), p(r, 0.92, 0.88)), 0.0, s, egui::StrokeKind::Inside);
            for x in [0.36, 0.64] {
                painter.line_segment([p(r, x, 0.2), p(r, x, 0.8)], s);
            }
        }
        "group_active" | "group_inactive" => group_marker(painter, r, c, name == "group_active"),
        "rulers" => {
            for y in [0.28, 0.5, 0.72] {
                painter.line_segment([p(r, 0.08, y), p(r, 0.62, y)], s);
            }
            painter.add(Shape::convex_polygon(vec![p(r, 0.74, 0.4), p(r, 0.98, 0.4), p(r, 0.86, 0.62)], c, Stroke::NONE));
        }
        "marker" => {
            painter.add(Shape::convex_polygon(
                vec![p(r, 0.28, 0.18), p(r, 0.72, 0.18), p(r, 0.72, 0.82), p(r, 0.5, 0.64), p(r, 0.28, 0.82)],
                Color32::TRANSPARENT,
                s,
            ));
        }
        "wave_zoom" => {
            draw(painter, Rect::from_center_size(r.center(), vec2(r.width() * 0.7, r.height() * 0.45)), "wave", c);
            for (y, direction) in [(r.min.y + 3.0, -1.0), (r.max.y - 3.0, 1.0)] {
                painter.add(Shape::convex_polygon(
                    vec![
                        pos2(r.center().x, y + direction * 1.8),
                        pos2(r.center().x - 2.8, y - direction * 1.2),
                        pos2(r.center().x + 2.8, y - direction * 1.2),
                    ],
                    c,
                    Stroke::NONE,
                ));
            }
        }
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
            for (x, direction) in [(0.25, 1.0), (0.75, -1.0)] {
                painter.add(Shape::line(vec![p(r, x + direction * 0.15, 0.22), p(r, x, 0.22), p(r, x, 0.78), p(r, x + direction * 0.15, 0.78)], s));
            }
            painter.line_segment([p(r, 0.1, 0.5), p(r, 0.9, 0.5)], s);
            painter.add(Shape::convex_polygon(vec![p(r, 0.9, 0.5), p(r, 0.76, 0.4), p(r, 0.76, 0.6)], c, Stroke::NONE));
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
        "track_audio" => {
            painter.line_segment([p(r, 0.06, 0.5), p(r, 0.94, 0.5)], Stroke::new(0.7, c));
            for (x, height) in [(0.18, 0.18), (0.34, 0.36), (0.5, 0.24), (0.66, 0.3), (0.82, 0.12)] {
                painter.line_segment([p(r, x, 0.5 - height), p(r, x, 0.5 + height)], Stroke::new(1.5, c));
            }
        }
        "track_aux" => {
            // A generic input arrow, authored from simple geometry.
            painter.rect_filled(Rect::from_min_max(p(r, 0.36, 0.08), p(r, 0.64, 0.56)), 0.0, c);
            painter.add(Shape::convex_polygon(vec![p(r, 0.12, 0.52), p(r, 0.88, 0.52), p(r, 0.5, 0.92)], c, Stroke::NONE));
        }
        "track_master" | "track_vca" => {
            let xs: &[f32] = if name == "track_vca" { &[0.25, 0.75] } else { &[0.5] };
            for &x in xs {
                painter.line_segment([p(r, x, 0.1), p(r, x, 0.9)], Stroke::new(1.0, c));
                painter.rect_filled(Rect::from_min_max(p(r, x - 0.17, 0.35), p(r, x + 0.17, 0.58)), 0.0, c);
            }
        }
        "track_instrument" => {
            let keys = Rect::from_min_max(p(r, 0.08, 0.18), p(r, 0.92, 0.82));
            painter.rect_stroke(keys, 0.0, Stroke::new(1.0, c), egui::StrokeKind::Inside);
            for x in [0.36, 0.64] {
                painter.line_segment([p(r, x, 0.18), p(r, x, 0.82)], Stroke::new(1.0, c));
                painter.rect_filled(Rect::from_min_max(p(r, x - 0.07, 0.18), p(r, x + 0.07, 0.55)), 0.0, c);
            }
        }
        "track_folder" => {
            painter.add(Shape::closed_line(
                vec![p(r, 0.08, 0.82), p(r, 0.08, 0.18), p(r, 0.4, 0.18), p(r, 0.5, 0.32), p(r, 0.92, 0.32), p(r, 0.92, 0.82)],
                Stroke::new(1.0, c),
            ));
        }
        "track_video" => {
            painter.rect_stroke(Rect::from_min_max(p(r, 0.08, 0.16), p(r, 0.92, 0.84)), 0.0, Stroke::new(1.0, c), egui::StrokeKind::Inside);
            draw(painter, r.shrink2(r.size() * 0.2), "play", c);
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
        "link_track" => {
            // A link chain below two track rows, so the track link reads apart from the timeline link's plain chain.
            painter.line_segment([p(r, 0.12, 0.18), p(r, 0.88, 0.18)], s);
            painter.line_segment([p(r, 0.12, 0.32), p(r, 0.88, 0.32)], s);
            painter.circle_stroke(p(r, 0.38, 0.72), r.width() * 0.16, s);
            painter.circle_stroke(p(r, 0.62, 0.72), r.width() * 0.16, s);
        }
        "insertion_follows" => {
            // The insertion line, a lead, and the playhead arrow it follows.
            painter.line_segment([p(r, 0.3, 0.14), p(r, 0.3, 0.86)], s);
            painter.line_segment([p(r, 0.42, 0.5), p(r, 0.62, 0.5)], s);
            painter.add(Shape::convex_polygon(vec![p(r, 0.84, 0.5), p(r, 0.64, 0.34), p(r, 0.64, 0.66)], c, Stroke::NONE));
        }
        _ => {
            painter.rect_stroke(r.shrink(r.width() * 0.25), 1.0, s, egui::StrokeKind::Middle);
        }
    }
}
