//! Score Editor: the selected MIDI clip (or track) as notation on a grand staff.
//!
//! A readable sketch rather than an engraving engine: note heads by duration (whole, half,
//! quarter, eighth with flags), stems, accidentals (sharps), ledger lines, bar lines from the
//! meter map, and the clef split at middle C.

use crate::SoundApp;
use crate::i18n::tr;
use crate::theme::{bold, regular};
use egui::{Align2, Color32, Pos2, Rect, Sense, Shape, Stroke, pos2, vec2};
use soundcraft_model::ClipContent;
use soundcraft_time::TICKS_PER_QUARTER;

const STEP: f32 = 6.0; // half the distance between staff lines

/// Diatonic step of a MIDI pitch (C4 = 60 → 0) and whether it needs a sharp.
fn diatonic(pitch: u8) -> (i32, bool) {
    let p = i32::from(pitch);
    let oct = p.div_euclid(12) - 5;
    let (step, sharp) = match p.rem_euclid(12) {
        0 => (0, false),
        1 => (0, true),
        2 => (1, false),
        3 => (1, true),
        4 => (2, false),
        5 => (3, false),
        6 => (3, true),
        7 => (4, false),
        8 => (4, true),
        9 => (5, false),
        10 => (5, true),
        _ => (6, false),
    };
    (oct * 7 + step, sharp)
}

pub fn show(app: &mut SoundApp, ctx: &egui::Context) {
    let mut open = app.ui.show_score;
    if !open {
        return;
    }
    egui::Window::new(tr("Score Editor")).id(egui::Id::new("Score Editor")).open(&mut open).default_size(vec2(900.0, 340.0)).show(ctx, |ui| {
        let Some(cid) = crate::midi_editor::target_clip(app) else {
            ui.label(tr("Select a MIDI clip or MIDI track."));
            return;
        };
        let s = app.engine.session();
        let Some((tid, clip)) = s.find_clip(cid).map(|(t, c)| (t, c.clone())) else { return };
        let ClipContent::Midi { sequence } = &clip.content else { return };
        let title = s.track(tid).map_or(String::new(), |t| t.name.clone());
        ui.label(egui::RichText::new(format!("{title} — {}", clip.name)).font(bold(13.0)));
        let len_ticks = (s.tempo.samples_to_ticks(clip.end(), s.sample_rate) - s.tempo.samples_to_ticks(clip.start, s.sample_rate)).max(1);
        let px_per_tick = 0.07f32;
        let width = (len_ticks as f32 * px_per_tick + 120.0).max(ui.available_width());
        egui::ScrollArea::horizontal().show(ui, |ui| {
            let (r, _) = ui.allocate_exact_size(vec2(width, 230.0), Sense::hover());
            let p = ui.painter_at(r);
            p.rect_filled(r, 4.0, Color32::from_rgb(246, 243, 236));
            let ink = Color32::from_rgb(20, 20, 24);
            let left = r.min.x + 70.0;
            // Treble staff: E4..F5 (steps 2..10 above C4); bass: G2..A3 (steps -10..-2).
            let c4_y = r.min.y + 115.0;
            let y_of = |step: i32| c4_y - step as f32 * STEP;
            for line in [2, 4, 6, 8, 10, -2, -4, -6, -8, -10] {
                let y = y_of(line);
                p.line_segment([pos2(r.min.x + 10.0, y), pos2(r.max.x - 10.0, y)], Stroke::new(1.0, ink));
            }
            p.text(pos2(r.min.x + 22.0, y_of(6)), Align2::CENTER_CENTER, "𝄞", regular(36.0), ink);
            // Bass clef, drawn: a curl from the F line plus two dots around it.
            let f = pos2(r.min.x + 18.0, y_of(-4));
            let curl: Vec<Pos2> = (0..=20)
                .map(|i| {
                    let a = -2.6 + i as f32 * 0.19;
                    let rad = 7.0 + i as f32 * 0.35;
                    pos2(f.x + a.cos() * rad * 0.9 + 2.0, f.y + a.sin() * rad + 4.0)
                })
                .collect();
            p.circle_filled(pos2(f.x - 4.0, f.y), 3.0, ink);
            p.add(Shape::line(curl, Stroke::new(2.2, ink)));
            p.circle_filled(pos2(f.x + 14.0, y_of(-3)), 1.8, ink);
            p.circle_filled(pos2(f.x + 14.0, y_of(-5)), 1.8, ink);
            p.line_segment([pos2(r.min.x + 10.0, y_of(10)), pos2(r.min.x + 10.0, y_of(-10))], Stroke::new(2.0, ink));
            // Bar lines.
            let base = s.tempo.samples_to_ticks(clip.start, s.sample_rate);
            let mut bar_tick = 0i64;
            let mut guard = 0;
            while bar_tick <= len_ticks && guard < 2000 {
                let x = left + bar_tick as f32 * px_per_tick;
                p.line_segment([pos2(x - 8.0, y_of(10)), pos2(x - 8.0, y_of(-10))], Stroke::new(1.0, ink));
                let m = s.tempo.meter_at_tick(base + bar_tick);
                let bb = s.tempo.bar_beat_at_tick(base + bar_tick);
                p.text(pos2(x - 6.0, y_of(12)), Align2::LEFT_BOTTOM, bb.bar.to_string(), regular(9.0), Color32::from_rgb(120, 110, 100));
                bar_tick += m.ticks_per_bar().max(1);
                guard += 1;
            }
            // Notes.
            for n in &sequence.notes {
                let x = left + n.start as f32 * px_per_tick;
                let (step, sharp) = diatonic(n.pitch);
                let y = y_of(step);
                // Ledger lines.
                let ledgers: Vec<i32> = if step >= 12 {
                    (12..=step).step_by(2).collect()
                } else if step <= -12 {
                    (step..=-12).filter(|s| s % 2 == 0).collect()
                } else if step == 0 || step == 1 {
                    vec![0]
                } else {
                    Vec::new()
                };
                for l in ledgers {
                    p.line_segment([pos2(x - 7.0, y_of(l)), pos2(x + 7.0, y_of(l))], Stroke::new(1.0, ink));
                }
                let q = n.length as f32 / TICKS_PER_QUARTER as f32;
                let hollow = q >= 1.75;
                let head = Rect::from_center_size(pos2(x, y), vec2(10.0, 7.5));
                if hollow {
                    p.add(Shape::ellipse_stroke(head.center(), head.size() * 0.5, Stroke::new(1.6, ink)));
                } else {
                    p.add(Shape::ellipse_filled(head.center(), head.size() * 0.5, ink));
                }
                if sharp {
                    p.text(pos2(x - 12.0, y), Align2::CENTER_CENTER, "♯", regular(12.0), ink);
                }
                if q < 3.5 {
                    // Stems up below the middle line of each staff, down above it.
                    let up = if step >= 0 { step < 6 } else { step < -6 };
                    let (sx, sy0, sy1): (f32, f32, f32) = if up { (x + 4.5, y, y - 26.0) } else { (x - 4.5, y, y + 26.0) };
                    p.line_segment([pos2(sx, sy0), pos2(sx, sy1)], Stroke::new(1.2, ink));
                    let flags = if q <= 0.3 {
                        2
                    } else if q <= 0.6 {
                        1
                    } else {
                        0
                    };
                    for f in 0..flags {
                        let fy = sy1 + if up { f as f32 * 6.0 } else { -(f as f32) * 6.0 };
                        let tip: Pos2 = pos2(sx + 8.0, fy + if up { 10.0 } else { -10.0 });
                        p.line_segment([pos2(sx, fy), tip], Stroke::new(1.6, ink));
                    }
                }
            }
        });
        ui.label(egui::RichText::new(tr("Notation view of the MIDI clip; edit notes in the MIDI Editor or Event List.")).small());
    });
    app.ui.show_score = open;
}

#[cfg(test)]
mod tests {
    #[test]
    fn diatonic_steps() {
        assert_eq!(super::diatonic(60), (0, false));
        assert_eq!(super::diatonic(61), (0, true));
        assert_eq!(super::diatonic(72), (7, false));
        assert_eq!(super::diatonic(48), (-7, false));
        assert_eq!(super::diatonic(64), (2, false));
    }
}
