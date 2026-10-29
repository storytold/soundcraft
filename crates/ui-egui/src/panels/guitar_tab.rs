//! The Tab panel: the target MIDI track as guitar tablature, with a cursor that follows the
//! playhead. The tab itself comes from `soundcraft_engine::guitar_tab`; this only draws it.

use crate::SoundApp;
use crate::theme::{Tokens, bold, mono, regular};
use egui::{Align2, Rect, Sense, Stroke, Ui, pos2, vec2};
use serde_json::json;
use soundcraft_engine::guitar_tab::{self, Tab};
use soundcraft_model::{Session, TrackId};
use std::sync::{Arc, Weak};

/// Width of one sixteenth.
const CELL: f32 = 16.0;
/// Space before the first and after the last sixteenth of a measure.
const PAD: f32 = 8.0;
/// Distance between string lines.
const LINE_GAP: f32 = 12.0;
/// Room for the string names left of each system.
const LABEL_W: f32 = 24.0;
/// Room above the top string (bar numbers) and below the bottom one.
const TOP: f32 = 16.0;
const BOTTOM: f32 = 12.0;

/// The tab of the last frame, reused while the session and the target track stay the same.
#[derive(Clone)]
struct Cached {
    session: Weak<Session>,
    track: TrackId,
    tab: Arc<Tab>,
}

fn cached_tab(app: &SoundApp, ui: &Ui, track: TrackId) -> Option<Arc<Tab>> {
    let id = egui::Id::unique("soundcraft-guitar-tab-cache");
    let session = app.engine.session_arc();
    let hit =
        ui.ctx().data(|d| d.get_temp::<Cached>(id)).filter(|c| c.track == track && c.session.upgrade().is_some_and(|s| Arc::ptr_eq(&s, &session)));
    if let Some(c) = hit {
        return Some(c.tab);
    }
    let tab = Arc::new(guitar_tab::layout(&session, track)?);
    ui.ctx().data_mut(|d| d.insert_temp(id, Cached { session: Arc::downgrade(&session), track, tab: Arc::clone(&tab) }));
    Some(tab)
}

/// Measures per system for `width`: (first measure index, x of each measure) per system.
fn systems(tab: &Tab, width: f32) -> Vec<Vec<(usize, f32)>> {
    let mut out: Vec<Vec<(usize, f32)>> = Vec::new();
    let mut x = LABEL_W;
    for (i, m) in tab.measures.iter().enumerate() {
        let w = measure_width(m.units);
        if out.is_empty() || (x + w > width && out.last().is_some_and(|s| !s.is_empty())) {
            out.push(Vec::new());
            x = LABEL_W;
        }
        if let Some(s) = out.last_mut() {
            s.push((i, x));
        }
        x += w;
    }
    out
}

fn measure_width(units: i64) -> f32 {
    units.clamp(1, 64) as f32 * CELL + 2.0 * PAD
}

/// The (measure index, sixteenth) a click `rel` points right of a system's left edge lands on.
fn clicked_unit(tab: &Tab, system: &[(usize, f32)], rel: f32) -> Option<(usize, f64)> {
    let (i, x) = system.iter().rev().find(|(_, x)| rel >= *x)?;
    let m = tab.measures.get(*i)?;
    let unit = (((rel - x - PAD) / CELL).floor() as i64).clamp(0, m.units.saturating_sub(1).max(0));
    Some((*i, unit as f64))
}

/// The session position of sixteenth `unit` of measure `i`.
fn unit_position(s: &Session, tab: &Tab, i: usize, unit: f64) -> Option<soundcraft_time::Samples> {
    let m = tab.measures.get(i)?;
    let tick = ((m.start as f64 + unit) as i64).saturating_mul(soundcraft_engine::score::UNIT);
    Some(s.tempo.tick_to_samples(tick, s.sample_rate))
}

pub fn guitar_tab_panel(app: &mut SoundApp, ui: &mut Ui) {
    let t = Tokens::current();
    ui.painter().rect_filled(ui.max_rect(), 0.0, t.panel_bg);
    let target = guitar_tab::target_track(app.engine.session());
    header(app, ui, target);
    let Some(track) = target else {
        ui.centered_and_justified(|ui| ui.label(egui::RichText::new("Select a MIDI or instrument track to see it as tab.").color(t.text_dim)));
        return;
    };
    let Some(tab) = cached_tab(app, ui, track) else { return };
    let playing = app.is_playing();
    let cursor = guitar_tab::locate(app.engine.session(), &tab, app.position());
    let strings = tab.tuning.len();
    let staff_h = (strings.max(1) - 1) as f32 * LINE_GAP;
    let system_h = TOP + staff_h + BOTTOM;
    let mut locate_to: Option<(usize, f64)> = None;
    egui::ScrollArea::vertical().id_salt("guitar_tab").auto_shrink([false, false]).show(ui, |ui| {
        let width = ui.available_width();
        let layout = systems(&tab, width);
        let follow_id = ui.scope_id().with("follow_system");
        for (si, system) in layout.iter().enumerate() {
            let (rect, resp) = ui.allocate_exact_size(vec2(width, system_h), Sense::click());
            if !ui.is_rect_visible(rect) && !playing {
                continue;
            }
            let painter = ui.painter_at(rect);
            let y0 = rect.min.y + TOP;
            let end_x = system.last().map_or(LABEL_W, |(i, x)| x + tab.measures.get(*i).map_or(0.0, |m| measure_width(m.units)));
            for (s, open) in tab.tuning.iter().enumerate() {
                let y = y0 + s as f32 * LINE_GAP;
                painter.text(pos2(rect.min.x + LABEL_W - 6.0, y), Align2::RIGHT_CENTER, guitar_tab::pitch_class(*open), mono(10.0), t.text_dim);
                painter.line_segment([pos2(rect.min.x + LABEL_W, y), pos2(rect.min.x + end_x, y)], Stroke::new(1.0, t.text_dim.gamma_multiply(0.45)));
            }
            for (i, x) in system {
                let Some(m) = tab.measures.get(*i) else { continue };
                let x = rect.min.x + x;
                let bar = Stroke::new(1.0, t.text_dim);
                painter.line_segment([pos2(x, y0), pos2(x, y0 + staff_h)], bar);
                let xe = x + measure_width(m.units);
                painter.line_segment([pos2(xe, y0), pos2(xe, y0 + staff_h)], bar);
                painter.text(pos2(x + 2.0, rect.min.y + 2.0), Align2::LEFT_TOP, m.number.to_string(), regular(9.0), t.text_dim);
                for e in &m.events {
                    let cx = x + PAD + (e.unit as f32 + 0.5) * CELL;
                    for n in &e.notes {
                        let (Some(string), Some(fret)) = (n.string, n.fret) else { continue };
                        let y = y0 + f32::from(string.saturating_sub(1)) * LINE_GAP;
                        let galley = painter.layout_no_wrap(fret.to_string(), bold(11.0), t.text);
                        let r = Rect::from_center_size(pos2(cx, y), galley.size() + vec2(2.0, -2.0));
                        painter.rect_filled(r, 0.0, t.panel_bg);
                        painter.galley(r.center() - galley.size() * 0.5, galley, t.text);
                    }
                    let lost = e.notes.iter().filter(|n| n.string.is_none()).count();
                    if lost > 0 {
                        painter.text(pos2(cx, y0 + staff_h + 2.0), Align2::CENTER_TOP, format!("?{lost}"), regular(9.0), t.playhead);
                    }
                }
                if let Some((ci, unit)) = cursor
                    && ci == *i
                {
                    let cx = x + PAD + unit as f32 * CELL;
                    let color = if playing { t.playhead } else { t.accent };
                    painter.line_segment([pos2(cx, y0 - 6.0), pos2(cx, y0 + staff_h + 6.0)], Stroke::new(1.5, color));
                    if playing && ui.ctx().data(|d| d.get_temp::<usize>(follow_id)) != Some(si) {
                        ui.ctx().data_mut(|d| d.insert_temp(follow_id, si));
                        ui.scroll_to_rect(rect, None);
                    }
                }
            }
            if resp.clicked()
                && let Some(p) = resp.interact_pointer_pos()
            {
                locate_to = clicked_unit(&tab, system, p.x - rect.min.x);
            }
        }
        if !playing {
            ui.ctx().data_mut(|d| d.remove::<usize>(follow_id));
        }
    });
    if let Some(at) = locate_to.and_then(|(i, unit)| unit_position(app.engine.session(), &tab, i, unit)) {
        let _ = app.run("edit.select", json!({"start": at, "end": at}));
    }
    if playing {
        ui.ctx().request_repaint();
    }
}

/// Title, track name and the tuning picker.
fn header(app: &mut SoundApp, ui: &mut Ui, track: Option<TrackId>) {
    let t = Tokens::current();
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 24.0), Sense::hover());
    ui.painter().rect_filled(r, 0.0, t.panel_bg2);
    ui.painter().text(pos2(r.min.x + 8.0, r.center().y), Align2::LEFT_CENTER, "TAB", bold(11.5), t.header_text);
    let Some(track) = track else { return };
    let Some((name, tuning)) = app.engine.session().track(track).map(|tr| (tr.name.clone(), guitar_tab::tuning_of(tr))) else { return };
    ui.painter().text(pos2(r.min.x + 44.0, r.center().y), Align2::LEFT_CENTER, &name, regular(11.0), t.text);
    let current = guitar_tab::preset_of(&tuning)
        .map_or_else(|| tuning.iter().rev().map(|p| guitar_tab::pitch_class(*p)).collect::<Vec<_>>().join(" "), |p| p.name.to_string());
    let combo = Rect::from_min_max(pos2((r.max.x - 190.0).max(r.min.x + 160.0), r.min.y + 1.0), pos2(r.max.x - 4.0, r.max.y - 1.0));
    let mut chosen: Option<&'static str> = None;
    ui.scope_builder(egui::UiBuilder::new().max_rect(combo), |ui| {
        craft_ui::ComboBox::from_id_salt("guitar_tab_tuning").selected_text(current).width(combo.width() - 8.0).show_ui(ui, |ui| {
            for p in guitar_tab::PRESETS {
                if ui.selectable_label(p.strings == tuning.as_slice(), p.name).clicked() {
                    chosen = Some(p.id);
                }
            }
        });
    });
    if let Some(id) = chosen
        && let Err(e) = app.run("track.tuning", json!({"track": track.0, "tuning": id}))
    {
        app.ui.status = e;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use soundcraft_engine::guitar_tab::TabMeasure;

    #[test]
    fn measures_wrap_into_systems() {
        let m = |number| TabMeasure { number, start: 0, units: 16, numerator: 4, denominator: 4, events: Vec::new() };
        let tab = Tab { track: TrackId(1), name: String::new(), tuning: guitar_tab::DEFAULT_TUNING.to_vec(), measures: (1..=5).map(m).collect() };
        let w = measure_width(16);
        let s = systems(&tab, LABEL_W + 2.0 * w + 1.0);
        assert_eq!(s.iter().map(Vec::len).collect::<Vec<_>>(), vec![2, 2, 1]);
        assert_eq!(s[1][1], (3, LABEL_W + w));
        // Narrower than one measure: still one per system.
        assert_eq!(systems(&tab, 10.0).len(), 5);
    }

    #[test]
    fn a_click_lands_on_the_sixteenth_under_it() {
        let m = |number: usize| TabMeasure { number, start: (number as i64 - 1) * 16, units: 16, numerator: 4, denominator: 4, events: Vec::new() };
        let tab = Tab { track: TrackId(1), name: String::new(), tuning: guitar_tab::DEFAULT_TUNING.to_vec(), measures: (1..=3).map(m).collect() };
        let layout = systems(&tab, 10_000.0);
        let system = &layout[0];
        let w = measure_width(16);
        // The middle of sixteenth 3 of bar 2.
        assert_eq!(clicked_unit(&tab, system, LABEL_W + w + PAD + 3.5 * CELL), Some((1, 3.0)));
        // Left of the first sixteenth, or past the last: clamped into the bar.
        assert_eq!(clicked_unit(&tab, system, LABEL_W + 1.0), Some((0, 0.0)));
        assert_eq!(clicked_unit(&tab, system, LABEL_W + 3.0 * w - 1.0), Some((2, 15.0)));
        // On the string labels: nothing.
        assert_eq!(clicked_unit(&tab, system, 2.0), None);
        let s = Session::default();
        let tick = 19 * soundcraft_engine::score::UNIT;
        assert_eq!(unit_position(&s, &tab, 1, 3.0), Some(s.tempo.tick_to_samples(tick, s.sample_rate)));
        assert_eq!(unit_position(&s, &tab, 9, 0.0), None);
    }
}
