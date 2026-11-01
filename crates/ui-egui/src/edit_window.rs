//! The Edit window: toolbar, rulers, track headers and playlists.

use crate::dialogs::Dialog;
use crate::spot::Spot;
use crate::theme::{Tokens, bold, clip_colors, regular, rgb};
use crate::widgets::{groove, level_pos, meter_pos, raised_key, shade};
use crate::{Gesture, SoundApp, toolbar};
use egui::{Align2, Color32, CornerRadius, Pos2, Rect, Sense, Shape, Stroke, StrokeKind, Ui, pos2, vec2};
use serde_json::json;
use soundcraft_engine::edit as ops;
use soundcraft_model::{AutoParam, Clip, ClipContent, ClipId, EditMode, Session, Tool, Track, TrackHeight, TrackId, TrackKind};
use soundcraft_time::{GridValue, NoteValue, Range, Samples, TimeFormat, format_position};
use std::collections::HashSet;
use std::sync::Arc;

mod automation;
mod clips;
mod drag;
mod header;
mod input;
mod layout;
mod overlay;
mod rulers;
mod scroll;
mod tracks;
mod universe;

#[cfg(test)]
use automation::automation_id;
pub use clips::strip_silence_stroke;
#[cfg(test)]
use clips::{clip_name_height, fade_curve_points};
use drag::drive_drag;
#[cfg(test)]
use drag::fade_drag_range;
#[cfg(test)]
use header::{header_column, header_selector};
pub use input::{CLIP_FADER_TRAVEL, clip_gain_icon};
#[cfg(test)]
use input::{clip_at, fade_midpoint};
use layout::COLUMN_TITLES_H;
#[cfg(test)]
use layout::lane_rect;
pub use layout::{COLUMN_W, EditLayout, HEADER_W, LANE_H, RULER_H, edit_columns, header_width, rect, sample_at, track_height, x_of};
use overlay::overlay;
pub use rulers::ruler_height;
use rulers::{column_titles, draw_rulers, views_button};
pub use scroll::zoom_toggle;
#[cfg(test)]
use scroll::{FollowFrame, follow_update};
use scroll::{follow_zoom_toggle, hscrollbar, scroll_and_zoom, scroll_into_view};
pub(crate) use tracks::{click_track, height_targets, track_context_menu};
use tracks::{in_edit_window, track_row};
use universe::universe;

pub fn show(app: &mut SoundApp, ui: &mut Ui) {
    let t = Tokens::current();
    drive_drag(app, ui.ctx());
    egui::Panel::top("edit_toolbar").exact_size(toolbar::HEIGHT).frame(egui::Frame::NONE.fill(t.toolbar_bg)).show(ui, |ui| toolbar::show(app, ui));
    egui::Panel::bottom("edit_status").exact_size(22.0).frame(egui::Frame::NONE.fill(t.toolbar_bg)).show(ui, |ui| status_bar(app, ui));
    egui::Panel::bottom("lower_dock_tabs").exact_size(22.0).frame(egui::Frame::NONE.fill(t.panel_bg2)).show(ui, |ui| dock_tabs(app, ui));
    if app.ui.show_universe {
        egui::Panel::top("universe").exact_size(64.0).frame(egui::Frame::NONE.fill(Tokens::current().universe_bg)).show(ui, |ui| universe(app, ui));
    }
    crate::docking::show(app, ui, crate::MainWindow::Edit);
}

/// Three dock toggle tabs for MIDI Editor, Memory Locations and Undo History.
fn dock_tabs(app: &mut SoundApp, ui: &mut Ui) {
    let t = Tokens::current();
    let r = ui.max_rect();
    let mut x = r.min.x + 10.0;
    for (label, on, id) in [
        ("MIDI EDITOR", app.ui.show_midi_editor, "window.midi_editor"),
        ("MEMORY LOCATIONS", app.ui.show_memory_locations, "window.memory_locations"),
        ("UNDO HISTORY", app.ui.show_undo_history, "window.undo_history"),
    ] {
        let g = ui.painter().layout_no_wrap(label.to_string(), bold(10.5), if on { Color32::WHITE } else { t.text_dim });
        let tr = Rect::from_min_size(pos2(x, r.min.y + 2.0), vec2(g.size().x + 16.0, r.height() - 4.0));
        if on {
            ui.painter().rect_filled(tr, 3.0, t.accent_dark);
        }
        ui.painter().galley(pos2(tr.min.x + 8.0, tr.center().y - g.size().y * 0.5), g, Color32::WHITE);
        if ui.interact(tr, ui.scope_id().with(("dock", label)), Sense::click()).clicked() {
            let _ = app.run(id, json!({}));
        }
        x = tr.max.x + 6.0;
    }
}

fn status_bar(app: &mut SoundApp, ui: &mut Ui) {
    let t = Tokens::current();
    let r = ui.max_rect();
    let s = app.engine.session();
    let mut left = format!(
        "{} · {} Hz · {} · {} tracks · {}",
        s.name,
        s.sample_rate.hz(),
        match s.bit_depth {
            soundcraft_model::BitDepthSetting::Int16 => "16-bit",
            soundcraft_model::BitDepthSetting::Int24 => "24-bit",
            soundcraft_model::BitDepthSetting::Float32 => "32-bit float",
        },
        s.tracks.len(),
        app.ui.status
    );
    soundcraft_model::truncate_chars(&mut left, 4 * soundcraft_model::MAX_NAME_CHARS);
    ui.painter().text(pos2(r.min.x + 10.0, r.center().y), Align2::LEFT_CENTER, left, regular(11.0), t.text_dim);
    let undo = app.engine.undo_label().map_or(String::new(), |l| format!("Undo {l}"));
    ui.painter().text(
        pos2(r.max.x - 10.0, r.center().y),
        Align2::RIGHT_CENTER,
        if app.frame_ms < 50.0 { format!("{undo}   {:.1} ms/frame", app.frame_ms) } else { undo },
        regular(11.0),
        t.text_dim,
    );
}

pub(crate) fn main_area(app: &mut SoundApp, ui: &mut Ui) {
    let session = app.engine.session();
    let mut ruler_height_total = session.edit.rulers.iter().map(|ruler| ruler_height(ruler)).sum::<f32>();
    if session.edit.flag("view.ruler.tempo_editor") && session.edit.rulers.iter().any(|ruler| ruler == "tempo") {
        ruler_height_total += ruler_height("tempo_editor");
    }
    let minimum = vec2(header_width(&edit_columns(session)) + 64.0, ruler_height_total + 100.0);
    crate::docking::minimum_body(ui, minimum, |ui| main_area_content(app, ui));
}

/// Lay out the Edit window: rulers, track headers and lanes, empty-area selection, overlays and both scrollbars.
fn main_area_content(app: &mut SoundApp, ui: &mut Ui) {
    let t = Tokens::current();
    let full = ui.max_rect();
    let rulers: Vec<String> = app.engine.session().edit.rulers.clone();
    let mut rulers = rulers;
    if app.engine.session().edit.flag("view.ruler.tempo_editor")
        && let Some(i) = rulers.iter().position(|r| r == "tempo")
    {
        rulers.insert(i + 1, "tempo_editor".into());
    }
    let cols = edit_columns(app.engine.session());
    let titles_h = if cols.is_empty() { 0.0 } else { COLUMN_TITLES_H };
    let rulers_h = rulers.iter().map(|r| ruler_height(r)).sum::<f32>() + 6.0 + titles_h;
    let hscroll_h = 14.0;
    let hw = header_width(&cols);
    let header = Rect::from_min_max(full.min, pos2(full.min.x + hw, full.max.y - hscroll_h));
    let tl = Rect::from_min_max(pos2(full.min.x + hw, full.min.y + rulers_h), pos2(full.max.x - 12.0, full.max.y - hscroll_h));
    let rulers_rect = Rect::from_min_max(pos2(full.min.x, full.min.y), pos2(full.max.x, full.min.y + rulers_h));
    app.edit_layout.timeline = [tl.min.x, tl.min.y, tl.max.x, tl.max.y];
    follow_zoom_toggle(app);
    ui.painter().rect_filled(full, 0.0, t.window_bg);
    scroll_and_zoom(app, ui, full, tl);
    draw_rulers(app, ui, rulers_rect, tl, &rulers);
    views_button(app, ui, rulers_rect);
    column_titles(ui, rulers_rect, &cols);
    if let Some(id) = app.edit_scroll_into_view.take() {
        scroll_into_view(app, id, tl.height());
    }
    // Tracks.
    let mut y = tl.min.y - app.edit_layout.scroll_y;
    app.edit_layout.rows.clear();
    let mut content_h = 0.0;
    for t in app.engine.session().tracks.iter().filter(|x| in_edit_window(app.engine.session(), x)) {
        let h = track_height(t);
        app.edit_layout.rows.push((t.id.0, [full.min.x, y, tl.max.x, y + h]));
        y += h + 1.0;
        content_h += h + 1.0;
    }
    app.edit_layout.content_h = content_h;
    let clip_rect = Rect::from_min_max(pos2(full.min.x, tl.min.y), pos2(full.max.x, tl.max.y));
    for (id, r) in app.edit_layout.rows.clone() {
        let row = rect(r);
        if row.max.y >= clip_rect.min.y && row.min.y <= clip_rect.max.y {
            let mut child = ui.new_child(egui::UiBuilder::new().max_rect(row));
            child.set_clip_rect(clip_rect);
            track_row(app, &mut child, TrackId(id), row, tl, &cols);
        }
    }
    // Below the tracks a click clears the selection and a double-click opens New Tracks.
    let empty = Rect::from_min_max(pos2(full.min.x, y.max(tl.min.y)), tl.max);
    if empty.height() > 4.0 {
        let resp = ui.interact(empty, ui.scope_id().with("empty"), Sense::click());
        if resp.double_clicked() {
            let _ = app.dialogs.open_for_command(&app.engine, "track.new");
        } else if resp.clicked() {
            let _ = app.run("edit.select_none", json!({}));
        }
    }
    overlay(app, ui, tl, Rect::from_min_max(pos2(tl.min.x, rulers_rect.min.y), tl.max));
    let _ = header;
    // Horizontal scrollbar.
    let sb = Rect::from_min_max(pos2(tl.min.x, full.max.y - hscroll_h), pos2(tl.max.x, full.max.y));
    hscrollbar(app, ui, sb, tl);
    // Vertical scrollbar gutter.
    let vs = Rect::from_min_max(pos2(tl.max.x, tl.min.y), pos2(full.max.x, tl.max.y));
    ui.painter().rect_filled(vs, 0.0, t.panel_bg);
    if content_h > tl.height() {
        let frac = tl.height() / content_h;
        let off = app.edit_layout.scroll_y / content_h;
        let thumb = Rect::from_min_size(pos2(vs.min.x + 2.0, vs.min.y + vs.height() * off), vec2(vs.width() - 4.0, (vs.height() * frac).max(20.0)));
        ui.painter().rect_filled(thumb, 3.0, t.button_hi);
        let resp = ui.interact(vs, ui.scope_id().with("vscroll"), Sense::drag());
        if resp.dragged() {
            app.edit_layout.scroll_by(resp.drag_delta().y / frac, tl.height());
        }
    }
}

#[cfg(test)]
mod tests;
