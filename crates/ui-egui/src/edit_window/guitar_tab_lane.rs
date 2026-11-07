//! The `guitar_tab` track view: a MIDI clip drawn as guitar tab on the timeline, string lines with each
//! note's fret number at its onset. Fingering comes from `soundcraft_engine::guitar_tab`, the
//! same as the Tab panel's.

use super::layout::{sample_at, x_of};
use egui::{Align2, Color32, Rect, Stroke, pos2};
use soundcraft_engine::guitar_tab::{self, TimedNote};
use soundcraft_model::{Clip, Session, Track};
use std::sync::{Arc, Weak};

/// String spacing below which fret numbers give way to dots.
const MIN_TEXT_GAP: f32 = 7.0;
/// Largest fret number font.
const MAX_FONT: f32 = 13.0;

#[derive(Clone)]
struct Cached {
    session: Weak<Session>,
    notes: Arc<Vec<TimedNote>>,
}

/// The track's notes on the fretboard, reused while the session is unchanged.
fn notes(ctx: &egui::Context, session: &Arc<Session>, track: &Track) -> Arc<Vec<TimedNote>> {
    let id = egui::Id::unique(("soundcraft-guitar-tab-lane", track.id));
    if let Some(c) = ctx.data(|d| d.get_temp::<Cached>(id)).filter(|c| c.session.upgrade().is_some_and(|s| Arc::ptr_eq(&s, session))) {
        return c.notes;
    }
    let notes = Arc::new(guitar_tab::timeline(session, track));
    ctx.data_mut(|d| d.insert_temp(id, Cached { session: Arc::downgrade(session), notes: Arc::clone(&notes) }));
    notes
}

/// Where the lane's strings run: the y of string 1 and the spacing.
pub(crate) fn strings_geometry(body: Rect, strings: usize) -> (f32, f32) {
    let gap = body.height() / strings.max(1) as f32;
    (body.min.y + gap * 0.5, gap)
}

/// Radius of the ring marking a note no string can play.
const UNPLAYABLE_R: f32 = 1.5;

/// The background of a fret number `w` wide whose onset is at `x`: numbers start at their onset,
/// so a note at the clip start is not cut by the clip edge.
pub(crate) fn number_rect(x: f32, y: f32, w: f32, h: f32) -> Rect {
    Rect::from_min_max(pos2(x - 1.0, y - h * 0.5), pos2(x + w + 1.0, y + h * 0.5))
}

/// How one note is shown at the current zoom and lane height.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Mark {
    Number,
    Dot,
    Hidden,
}

/// Per-string placement: a number when it fits after the previous one on that string, else a
/// dot (a repeated note, or no room), else nothing when even the dot would overlap.
pub(crate) struct Placer {
    text: bool,
    char_w: f32,
    right: Vec<f32>,
    dot: Vec<f32>,
    fret: Vec<Option<u8>>,
}

impl Placer {
    pub(crate) fn new(strings: usize, font: f32, text: bool) -> Self {
        Placer { text, char_w: font * 0.62, right: vec![f32::MIN; strings], dot: vec![f32::MIN; strings], fret: vec![None; strings] }
    }

    /// Width of `fret`'s number.
    pub(crate) fn width(&self, fret: u8) -> f32 {
        self.char_w * if fret >= 10 { 2.0 } else { 1.0 }
    }

    pub(crate) fn place(&mut self, string: usize, x: f32, fret: u8) -> Mark {
        let (Some(right), Some(dot), Some(last)) = (self.right.get(string).copied(), self.dot.get(string).copied(), self.fret.get(string).copied())
        else {
            return Mark::Hidden;
        };
        let w = self.width(fret);
        // A number spans `[x, x + w]` from its onset. A digit's width of air between numbers keeps
        // a dense passage readable.
        let fits = x >= right + self.char_w.max(2.0);
        let repeat = last == Some(fret) && x - right < 3.0 * w;
        let mark = if self.text && fits && !repeat {
            Mark::Number
        } else if x >= dot + 3.0 && x >= right {
            Mark::Dot
        } else {
            Mark::Hidden
        };
        match mark {
            Mark::Number => {
                self.right[string] = x + w;
                self.fret[string] = Some(fret);
            }
            Mark::Dot => self.dot[string] = x,
            Mark::Hidden => {}
        }
        if mark == Mark::Dot && !repeat {
            self.fret[string] = Some(fret);
        }
        mark
    }
}

/// Where and in what colours a clip's tab is drawn.
#[derive(Clone, Copy)]
pub(super) struct TabArea {
    pub(super) tl: Rect,
    pub(super) body: Rect,
    pub(super) vis: Rect,
    pub(super) ink: Color32,
    pub(super) bg: Color32,
}

/// Draw `clip`'s notes as tab inside the clip body (clipped to the visible part).
pub(super) fn draw(painter: &egui::Painter, session: &Arc<Session>, track: &Track, clip: &Clip, area: &TabArea) {
    let TabArea { tl, body, vis, ink, bg } = *area;
    let s: &Session = session;
    let tuning = guitar_tab::tuning_of(track);
    let n = tuning.len();
    if vis.width() <= 0.0 || body.height() < n as f32 * 1.5 {
        return;
    }
    let p = painter.with_clip_rect(vis);
    let (y0, gap) = strings_geometry(body, n);
    let line = Stroke::new(1.0, ink.gamma_multiply(0.45));
    for k in 0..n {
        let y = y0 + k as f32 * gap;
        p.line_segment([pos2(vis.min.x, y), pos2(vis.max.x, y)], line);
    }
    let text = gap >= MIN_TEXT_GAP;
    let font = (gap * 0.95).clamp(6.0, MAX_FONT);
    let notes = notes(painter.ctx(), session, track);
    let mut placer = Placer::new(n, font, text);
    // From a little before the visible area, so numbers that start off screen still crowd out.
    let from = sample_at(s, tl, vis.min.x - 3.0 * font);
    let to = sample_at(s, tl, vis.max.x + 1.0);
    let first = notes.partition_point(|t| t.at < from);
    for t in notes.get(first..).unwrap_or_default().iter().take_while(|t| t.at <= to).filter(|t| t.clip == clip.id) {
        let x = x_of(s, tl, t.at);
        let (Some(string), Some(fret)) = (t.note.string, t.note.fret) else {
            // A note no string can play: a mark under the lowest string, starting at the onset.
            let y = (y0 + (n as f32 - 0.5) * gap).min(body.max.y - 2.0);
            p.circle_stroke(pos2(x + UNPLAYABLE_R + 0.5, y), UNPLAYABLE_R, Stroke::new(1.0, Color32::from_rgb(230, 90, 80)));
            continue;
        };
        let k = usize::from(string.saturating_sub(1));
        let y = y0 + k as f32 * gap;
        match placer.place(k, x, fret) {
            Mark::Number => {
                p.rect_filled(number_rect(x, y, placer.width(fret), (font * 0.9).min(gap)), 1.0, bg);
                p.text(pos2(x, y), Align2::LEFT_CENTER, fret.to_string(), crate::theme::bold(font), ink);
            }
            Mark::Dot => {
                p.circle_filled(pos2(x, y), (gap * 0.18).clamp(1.0, 2.0), ink);
            }
            Mark::Hidden => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::vec2;

    #[test]
    fn numbers_give_way_to_dots_when_crowded() {
        let mut p = Placer::new(6, 10.0, true);
        assert_eq!(p.place(0, 100.0, 3), Mark::Number);
        // The number covers 100..106.2: a note inside it is hidden.
        assert_eq!(p.place(0, 104.0, 5), Mark::Hidden);
        // Past the number but too close for a second one: a dot.
        assert_eq!(p.place(0, 107.0, 5), Mark::Dot);
        // Too close even for a dot.
        assert_eq!(p.place(0, 108.0, 5), Mark::Hidden);
        // Other strings are independent.
        assert_eq!(p.place(1, 104.0, 5), Mark::Number);
        // Room again: a number.
        assert_eq!(p.place(0, 140.0, 7), Mark::Number);
        // A repeat of the same fret nearby is a dot even when a number would fit (tremolo).
        assert_eq!(p.place(0, 155.0, 7), Mark::Dot);
        assert_eq!(p.place(0, 200.0, 7), Mark::Number);
        // Out of range strings are ignored.
        assert_eq!(p.place(9, 0.0, 1), Mark::Hidden);
    }

    #[test]
    fn short_lanes_draw_dots_only() {
        let mut p = Placer::new(6, 6.0, false);
        assert_eq!(p.place(0, 10.0, 12), Mark::Dot);
        assert_eq!(p.place(0, 100.0, 12), Mark::Dot);
    }

    #[test]
    fn a_note_at_the_clip_start_stays_inside_the_body() {
        let body = Rect::from_min_size(pos2(50.0, 10.0), vec2(200.0, 60.0));
        let (y0, gap) = strings_geometry(body, 6);
        let font = (gap * 0.95).clamp(6.0, MAX_FONT);
        let mut p = Placer::new(6, font, true);
        let x = body.min.x;
        let y = y0 + 5.0 * gap;
        assert_eq!(p.place(5, x, 0), Mark::Number);
        // Only the 1-point padding before the onset reaches past the body (the clip edge cuts it);
        // a number centred on the onset lost half its width there.
        for fret in [0, 12] {
            let r = number_rect(x, y, p.width(fret), (font * 0.9).min(gap));
            assert!(body.expand(1.0).contains_rect(r), "{r:?} in {body:?}");
            assert!(r.min.x + 1.0 >= body.min.x && r.max.x > x + p.width(fret));
        }
    }

    #[test]
    fn strings_fill_the_body() {
        let (y0, gap) = strings_geometry(Rect::from_min_size(pos2(0.0, 10.0), vec2(100.0, 60.0)), 6);
        assert!((gap - 10.0).abs() < 1e-4 && (y0 - 15.0).abs() < 1e-4);
    }
}
