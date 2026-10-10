//! The Edit window: toolbar, rulers, track headers and playlists.

use crate::theme::{Tokens, bold, clip_colors, regular, rgb};
use crate::widgets::{rec_toggle, selector_box, text_toggle};
use crate::{Gesture, SoundApp, icons, panels, toolbar};
use egui::{Align2, Color32, CornerRadius, Pos2, Rect, Sense, Shape, Stroke, StrokeKind, Ui, pos2, vec2};
use serde_json::json;
use soundcraft_model::{AutoParam, Clip, ClipContent, ClipId, Session, Tool, Track, TrackHeight, TrackId, TrackKind};
use soundcraft_time::{GridValue, NoteValue, Range, Samples, TimeFormat, format_position};

pub const HEADER_W: f32 = 236.0;
pub const COLUMN_W: f32 = 112.0;

/// Optional Edit-window columns (View › Edit Window Views) that are switched on.
pub fn edit_columns(s: &Session) -> Vec<&'static str> {
    let all = s.edit.flag("edit_view.all");
    ["io", "inserts_ae", "inserts_fj", "sends_ae", "sends_fj", "comments"]
        .into_iter()
        .filter(|c| all || s.edit.flag(&format!("edit_view.{c}")))
        .collect()
}

pub fn header_width(s: &Session) -> f32 {
    HEADER_W + COLUMN_W * edit_columns(s).len() as f32
}
pub const RULER_H: f32 = 17.0;

/// Geometry of the last frame, for hit testing and agents (`ui.inspect`).
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct EditLayout {
    pub timeline: [f32; 4],
    pub rows: Vec<(u64, [f32; 4])>,
    pub scroll_y: f32,
    pub content_h: f32,
    /// Follow-hold: the view was moved by something other than the playhead
    /// follow (a manual pan, scrollbar, universe jump, or programmatic
    /// scroll) during playback, so timeline auto-scroll stays paused until
    /// the playhead is scrolled back into view (or the transport
    /// stops/starts, or a scrolling mode is re-selected). Lets the timeline
    /// be inspected anywhere while playing.
    pub follow_hold: bool,
    /// Session `zoom.scroll` seen by the last overlay frame, for telling our
    /// own follow jumps apart from outside view moves.
    pub last_scroll: Samples,
    /// Follow target issued by the last overlay frame (`None` when none).
    pub last_follow_to: Option<Samples>,
}

pub fn x_of(s: &Session, tl: Rect, at: Samples) -> f32 {
    tl.min.x + ((at - s.edit.zoom.scroll) as f64 / s.edit.zoom.samples_per_px.max(0.01)) as f32
}

pub fn sample_at(s: &Session, tl: Rect, x: f32) -> Samples {
    s.edit.zoom.scroll + soundcraft_time::to_samples(f64::from(x - tl.min.x) * s.edit.zoom.samples_per_px)
}

pub const LANE_H: f32 = 46.0;

pub fn track_height(t: &Track) -> f32 {
    let alt = if t.view == "playlists" { t.playlists.len().saturating_sub(1) as f32 * LANE_H } else { 0.0 };
    t.height.points() + alt
}

pub fn show(app: &mut SoundApp, ui: &mut Ui) {
    let t = Tokens::current();
    egui::Panel::top("edit_toolbar").exact_size(toolbar::HEIGHT).frame(egui::Frame::NONE.fill(t.toolbar_bg)).show(ui, |ui| toolbar::show(app, ui));
    egui::Panel::bottom("edit_status").exact_size(22.0).frame(egui::Frame::NONE.fill(t.toolbar_bg)).show(ui, |ui| status_bar(app, ui));
    egui::Panel::bottom("lower_dock_tabs").exact_size(22.0).frame(egui::Frame::NONE.fill(t.panel_bg2)).show(ui, |ui| dock_tabs(app, ui));
    if app.ui.show_midi_editor {
        egui::Panel::bottom("midi_editor")
            .resizable(true)
            .default_size(320.0)
            .min_size(260.0)
            .frame(egui::Frame::NONE.fill(t.panel_bg))
            .show(ui, |ui| crate::midi_editor::show(app, ui));
    }
    if app.ui.show_universe {
        egui::Panel::top("universe").exact_size(64.0).frame(egui::Frame::NONE.fill(Tokens::current().universe_bg)).show(ui, |ui| universe(app, ui));
    }
    // Preserve a usable timeline when the main window is tiled narrowly.
    // These preferences remain enabled and the lists return when it grows.
    if app.ui.show_tracks_list && ui.available_width() >= HEADER_W + 76.0 + 168.0 {
        egui::Panel::left("tracks_list")
            .exact_size(168.0)
            .frame(egui::Frame::NONE.fill(t.panel_bg))
            .show(ui, |ui| panels::tracks_and_groups(app, ui));
    }
    if app.ui.show_clip_list && ui.available_width() >= HEADER_W + 76.0 + 230.0 {
        egui::Panel::right("clip_list").exact_size(230.0).frame(egui::Frame::NONE.fill(t.panel_bg)).show(ui, |ui| panels::clip_list(app, ui));
    }
    egui::CentralPanel::default().frame(egui::Frame::NONE.fill(t.window_bg)).show(ui, |ui| main_area(app, ui));
}

/// Universe: the whole session in miniature; the box is the visible area (drag or click to move).
fn universe(app: &mut SoundApp, ui: &mut Ui) {
    let r = ui.max_rect().shrink2(vec2(8.0, 4.0));
    let s = app.engine.session();
    let total = (s.content_end() + s.sample_rate.samples(10.0)).max(1) as f32;
    let tracks: Vec<&Track> = s.tracks.iter().filter(|t| !t.hidden).collect();
    let n = tracks.len().max(1) as f32;
    let rh = (r.height() / n).max(1.0);
    for (i, t) in tracks.iter().enumerate() {
        let y = r.min.y + i as f32 * rh;
        for c in t.clips() {
            let x0 = r.min.x + c.start as f32 / total * r.width();
            let x1 = r.min.x + c.end() as f32 / total * r.width();
            ui.painter().rect_filled(
                Rect::from_min_max(pos2(x0, y + 0.5), pos2(x1.max(x0 + 1.0), y + rh - 0.5)),
                0.0,
                rgb(c.color.unwrap_or(t.color)),
            );
        }
    }
    let vis0 = s.edit.zoom.scroll as f32 / total;
    let tlw = app.edit_layout.timeline[2] - app.edit_layout.timeline[0];
    let vis1 = vis0 + (f64::from(tlw.max(100.0)) * s.edit.zoom.samples_per_px) as f32 / total;
    let vr = Rect::from_min_max(pos2(r.min.x + vis0 * r.width(), r.min.y - 2.0), pos2(r.min.x + vis1.min(1.0) * r.width(), r.max.y + 2.0));
    ui.painter().rect_stroke(vr, 2.0, Stroke::new(1.5, Tokens::current().universe_view), StrokeKind::Outside);
    if app.is_playing() {
        let x = r.min.x + app.position() as f32 / total * r.width();
        ui.painter().line_segment([pos2(x, r.min.y), pos2(x, r.max.y)], Stroke::new(1.0, Tokens::current().playhead));
    }
    let resp = ui.interact(r, ui.id().with("universe"), Sense::click_and_drag());
    if let Some(p) = resp.interact_pointer_pos()
        && (resp.clicked() || resp.dragged())
    {
        let half = (vis1 - vis0) * 0.5;
        let k = ((p.x - r.min.x) / r.width() - half).clamp(0.0, 1.0);
        let _ = app.engine.execute("view.scroll", &json!({"to": (k * total) as i64}));
    }
}

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
        if ui.interact(tr, ui.id().with(("dock", label)), Sense::click()).clicked() {
            let _ = app.run(id, json!({}));
        }
        x = tr.max.x + 6.0;
    }
}

fn status_bar(app: &mut SoundApp, ui: &mut Ui) {
    let t = Tokens::current();
    let r = ui.max_rect();
    let s = app.engine.session();
    let left = format!(
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

fn main_area(app: &mut SoundApp, ui: &mut Ui) {
    let t = Tokens::current();
    let full = ui.max_rect();
    let rulers: Vec<String> = app.engine.session().edit.rulers.clone();
    let mut rulers = rulers;
    if app.engine.session().edit.flag("view.ruler.tempo_editor")
        && let Some(i) = rulers.iter().position(|r| r == "tempo")
    {
        rulers.insert(i + 1, "tempo_editor".into());
    }
    let rulers_h = rulers.iter().map(|r| ruler_height(r)).sum::<f32>() + 6.0;
    let hscroll_h = 14.0;
    let column_count = ((full.width() - HEADER_W - 76.0).max(0.0) / COLUMN_W).floor();
    let hw = (HEADER_W + COLUMN_W * column_count.min(edit_columns(app.engine.session()).len() as f32)).min((full.width() - 76.0).max(0.0));
    if full.height() <= rulers_h + hscroll_h || full.width() <= 12.0 {
        app.edit_layout.timeline = [full.min.x, full.min.y, full.min.x, full.min.y];
        app.edit_layout.rows.clear();
        ui.painter().text(full.center(), Align2::CENTER_CENTER, "Expand the Edit window to show the timeline.", regular(11.0), t.text_dim);
        return;
    }
    let header = Rect::from_min_max(full.min, pos2(full.min.x + hw, full.max.y - hscroll_h));
    let tl = Rect::from_min_max(pos2(full.min.x + hw, full.min.y + rulers_h), pos2(full.max.x - 12.0, full.max.y - hscroll_h));
    let rulers_rect = Rect::from_min_max(pos2(full.min.x, full.min.y), pos2(full.max.x, full.min.y + rulers_h));
    app.edit_layout.timeline = [tl.min.x, tl.min.y, tl.max.x, tl.max.y];
    ui.painter().rect_filled(full, 0.0, t.window_bg);
    // Wheel: vertical scroll tracks, shift/horizontal scroll timeline, cmd = zoom.
    let hovered = ui.rect_contains_pointer(Rect::from_min_max(pos2(full.min.x, tl.min.y), full.max));
    if hovered {
        let (delta, mods) = ui.input(|i| (i.smooth_scroll_delta, i.modifiers));
        if mods.command && delta.y.abs() > 0.0 {
            let id = if delta.y > 0.0 { "view.zoom_in" } else { "view.zoom_out" };
            let _ = app.run(id, json!({}));
        } else {
            let dx = if mods.shift { delta.y } else { delta.x };
            if dx.abs() > 0.0 {
                let _ = app.engine.execute("view.scroll", &json!({"by_px": -dx}));
            }
            if !mods.shift && delta.y.abs() > 0.0 {
                app.edit_layout.scroll_y = (app.edit_layout.scroll_y - delta.y).clamp(0.0, (app.edit_layout.content_h - tl.height() + 40.0).max(0.0));
            }
        }
    }
    draw_rulers(app, ui, rulers_rect, tl, &rulers);
    // Tracks.
    let ids: Vec<TrackId> =
        app.engine.session().tracks.iter().filter(|x| !x.hidden && !folder_collapsed(app.engine.session(), x)).map(|x| x.id).collect();
    let mut y = tl.min.y - app.edit_layout.scroll_y;
    app.edit_layout.rows.clear();
    let clip_rect = Rect::from_min_max(pos2(full.min.x, tl.min.y), pos2(full.max.x, tl.max.y));
    let mut content_h = 0.0;
    for id in ids {
        let Some(h) = app.engine.session().track(id).map(track_height) else { continue };
        let row = Rect::from_min_max(pos2(full.min.x, y), pos2(tl.max.x, y + h));
        if row.max.y >= clip_rect.min.y && row.min.y <= clip_rect.max.y {
            let mut child = ui.new_child(egui::UiBuilder::new().max_rect(row));
            child.set_clip_rect(clip_rect);
            track_row(app, &mut child, id, row, tl);
        }
        app.edit_layout.rows.push((id.0, [row.min.x, row.min.y, row.max.x, row.max.y]));
        y += h + 1.0;
        content_h += h + 1.0;
    }
    app.edit_layout.content_h = content_h;
    // Empty area click clears track selection.
    let empty = Rect::from_min_max(pos2(full.min.x, y.max(tl.min.y)), tl.max);
    if empty.height() > 4.0 && ui.interact(empty, ui.id().with("empty"), Sense::click()).clicked() {
        let _ = app.run("edit.select_none", json!({}));
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
        let resp = ui.interact(vs, ui.id().with("vscroll"), Sense::drag());
        if resp.dragged() {
            app.edit_layout.scroll_y = (app.edit_layout.scroll_y + resp.drag_delta().y / frac).clamp(0.0, (content_h - tl.height() + 40.0).max(0.0));
        }
    }
}

fn folder_collapsed(s: &Session, t: &Track) -> bool {
    t.folder.and_then(|f| s.track(f)).is_some_and(|f| !f.folder_open)
}

fn hscrollbar(app: &mut SoundApp, ui: &mut Ui, sb: Rect, tl: Rect) {
    let t = Tokens::current();
    ui.painter().rect_filled(sb, 0.0, t.panel_bg);
    let s = app.engine.session();
    let total = (s.content_end() + s.sample_rate.samples(30.0)).max(1) as f64;
    let vis = f64::from(tl.width()) * s.edit.zoom.samples_per_px;
    let frac = (vis / total).clamp(0.02, 1.0) as f32;
    let off = (s.edit.zoom.scroll as f64 / total).clamp(0.0, 1.0) as f32;
    let thumb = Rect::from_min_size(pos2(sb.min.x + sb.width() * off, sb.min.y + 3.0), vec2((sb.width() * frac).max(24.0), sb.height() - 6.0));
    ui.painter().rect_filled(thumb, 3.0, t.button_hi);
    let resp = ui.interact(sb, ui.id().with("hscroll"), Sense::drag());
    if resp.dragged() {
        let dx = resp.drag_delta().x / sb.width();
        let by_px = f64::from(dx) * total / s.edit.zoom.samples_per_px;
        let _ = app.engine.execute("view.scroll", &json!({"by_px": by_px}));
    }
}

// ---- rulers ----------------------------------------------------------------------------------

fn ruler_label(id: &str) -> &'static str {
    match id {
        "bars_beats" => "Bars|Beats",
        "min_secs" => "Min:Secs",
        "timecode" => "Timecode",
        "feet_frames" => "Feet+Frames",
        "samples" => "Samples",
        "tempo" => "Tempo",
        "meter" => "Meter",
        "markers" => "Markers",
        "key" => "Key",
        "chords" => "Chords",
        "timecode2" => "Timecode 2",
        "markers2" => "Markers 2",
        "markers3" => "Markers 3",
        "markers4" => "Markers 4",
        "markers5" => "Markers 5",
        "tempo_editor" => "",
        _ => "",
    }
}

pub fn ruler_height(id: &str) -> f32 {
    if id == "tempo_editor" { 56.0 } else { RULER_H }
}

/// The tempo editor: a graph of tempo over time. Click adds a change, Alt-click removes the nearest.
fn tempo_editor(app: &mut SoundApp, ui: &mut Ui, painter: &egui::Painter, s: &Session, tl: Rect, row: Rect) {
    let sr = s.sample_rate;
    painter.rect_filled(row, 0.0, Color32::from_rgb(24, 40, 32));
    let (lo, hi) = s.tempo.tempos().iter().fold((f64::MAX, f64::MIN), |(a, b), e| (a.min(e.bpm), b.max(e.bpm)));
    let (lo, hi) = ((lo - 20.0).max(20.0), (hi + 20.0).min(400.0));
    let y_of = |bpm: f64| row.max.y - 4.0 - ((bpm - lo) / (hi - lo).max(1.0)) as f32 * (row.height() - 8.0);
    let mut pts = Vec::new();
    let evs = s.tempo.tempos();
    for (k, e) in evs.iter().enumerate() {
        let x = x_of(s, tl, s.tempo.tick_to_samples(e.tick, sr)).max(tl.min.x);
        let x_next = evs.get(k + 1).map_or(tl.max.x, |n| x_of(s, tl, s.tempo.tick_to_samples(n.tick, sr)));
        pts.push(pos2(x, y_of(e.bpm)));
        pts.push(pos2(x_next.min(tl.max.x), y_of(e.bpm)));
        painter.circle_filled(pos2(x, y_of(e.bpm)), 3.0, Color32::from_rgb(140, 230, 160));
    }
    painter.add(Shape::line(pts, Stroke::new(1.5, Color32::from_rgb(140, 230, 160))));
    painter.text(pos2(tl.min.x + 4.0, row.min.y + 8.0), Align2::LEFT_CENTER, format!("{hi:.0}"), regular(9.0), Color32::from_rgb(120, 160, 130));
    painter.text(pos2(tl.min.x + 4.0, row.max.y - 8.0), Align2::LEFT_CENTER, format!("{lo:.0}"), regular(9.0), Color32::from_rgb(120, 160, 130));
    let resp = ui.interact(row, ui.id().with("tempo_editor"), Sense::click());
    if resp.clicked()
        && let Some(p) = resp.interact_pointer_pos()
    {
        let at = snap(s, sample_at(s, tl, p.x).max(0));
        if ui.input(|i| i.modifiers.alt) {
            let near = evs
                .iter()
                .filter(|e| e.tick > 0)
                .min_by_key(|e| ((x_of(s, tl, s.tempo.tick_to_samples(e.tick, sr)) - p.x).abs() * 10.0) as i64)
                .map(|e| s.tempo.tick_to_samples(e.tick, sr));
            if let Some(n) = near {
                let _ = app.run("event.tempo_remove", json!({"at": n}));
            }
        } else {
            let bpm = lo + f64::from((row.max.y - 4.0 - p.y) / (row.height() - 8.0)) * (hi - lo);
            let _ = app.run("event.tempo", json!({"bpm": (bpm * 10.0).round() / 10.0, "at": at}));
        }
    }
}

/// A "nice" step in samples for ruler labels at the current zoom.
fn nice_step(spp: f64, sr: f64, min_px: f64) -> i64 {
    let min = spp * min_px;
    let steps_s = [0.001, 0.005, 0.01, 0.05, 0.1, 0.25, 0.5, 1.0, 2.0, 5.0, 10.0, 15.0, 30.0, 60.0, 120.0, 300.0, 600.0, 1800.0, 3600.0];
    for st in steps_s {
        if st * sr >= min {
            return (st * sr) as i64;
        }
    }
    (3600.0 * sr) as i64
}

fn draw_rulers(app: &mut SoundApp, ui: &mut Ui, area: Rect, tl: Rect, rulers: &[String]) {
    let t = Tokens::current();
    let s = app.engine.session().clone();
    ui.painter().rect_filled(area, 0.0, t.ruler_bg);
    let view = Range::new(sample_at(&s, tl, tl.min.x), sample_at(&s, tl, tl.max.x));
    let sr = s.sample_rate;
    let mut y0 = area.min.y + 3.0;
    for (i, id) in rulers.iter().enumerate() {
        let h = ruler_height(id);
        let row = Rect::from_min_max(pos2(tl.min.x, y0), pos2(tl.max.x, y0 + h));
        let lab = Rect::from_min_max(pos2(area.min.x, y0), pos2(tl.min.x, y0 + h));
        y0 += h;
        let painter = ui.painter().with_clip_rect(row);
        ui.painter().text(pos2(lab.max.x - 12.0, lab.center().y), Align2::RIGHT_CENTER, ruler_label(id), bold(11.0), t.ruler_text);
        let fmt = TimeFormat::from_id(id);
        match id.as_str() {
            "tempo" => {
                painter.rect_filled(row.shrink2(vec2(0.0, 1.0)), 0.0, t.tempo_ruler);
                let tresp = ui.interact(row, ui.id().with("tempo_ruler"), Sense::click());
                if tresp.double_clicked()
                    && let Some(p) = tresp.interact_pointer_pos()
                {
                    let at = snap(&s, sample_at(&s, tl, p.x).max(0));
                    let bpm = s.tempo.tempo_at_tick(s.tempo.samples_to_ticks(at, sr));
                    app.dialogs.open = Some(crate::dialogs::Dialog::TempoChange { at, bpm });
                }
                let mut last_label = f32::MIN;
                for ev in s.tempo.tempos() {
                    let x = x_of(&s, tl, s.tempo.tick_to_samples(ev.tick, sr));
                    painter.add(Shape::convex_polygon(
                        vec![pos2(x, row.min.y + 3.0), pos2(x + 7.0, row.center().y), pos2(x, row.max.y - 3.0)],
                        Color32::from_rgb(220, 60, 50),
                        Stroke::NONE,
                    ));
                    if x - last_label > 46.0 {
                        last_label = x;
                        painter.text(
                            pos2(x + 10.0, row.center().y),
                            Align2::LEFT_CENTER,
                            format!("{}", (ev.bpm * 100.0).round() / 100.0),
                            bold(11.0),
                            Color32::from_rgb(230, 240, 230),
                        );
                    }
                }
            }
            "meter" => {
                painter.rect_filled(row.shrink2(vec2(0.0, 1.0)), 0.0, t.meter_ruler);
                for ev in s.tempo.meters() {
                    let x = x_of(&s, tl, s.tempo.tick_to_samples(ev.tick, sr));
                    painter.text(
                        pos2(x + 6.0, row.center().y),
                        Align2::LEFT_CENTER,
                        format!("{}/{}", ev.numerator, ev.denominator),
                        bold(11.0),
                        Color32::from_rgb(230, 236, 240),
                    );
                }
            }
            "tempo_editor" => {
                tempo_editor(app, ui, &painter, &s, tl, row);
            }
            "key" => {
                painter.rect_filled(row, 0.0, Color32::from_rgb(64, 52, 88));
                if s.key_signatures.is_empty() {
                    painter.text(pos2(tl.min.x + 6.0, row.center().y), Align2::LEFT_CENTER, "C major", bold(11.0), Color32::from_rgb(220, 214, 236));
                }
                for (at, k) in &s.key_signatures {
                    let x = x_of(&s, tl, *at);
                    painter.line_segment([pos2(x, row.min.y + 2.0), pos2(x, row.max.y - 2.0)], Stroke::new(2.0, Color32::from_rgb(200, 180, 240)));
                    painter.text(pos2(x + 5.0, row.center().y), Align2::LEFT_CENTER, k, bold(11.0), Color32::from_rgb(236, 230, 250));
                }
                let resp = ui.interact(row, ui.id().with("key_ruler"), Sense::click());
                if resp.double_clicked()
                    && let Some(p) = resp.interact_pointer_pos()
                {
                    let at = snap(&s, sample_at(&s, tl, p.x).max(0));
                    let _ = app.run("event.add_key_change", json!({"at": at, "key": "G major"}));
                }
            }
            "timecode2" => {
                // A second timecode ruler at 25 fps when the session runs at another rate (or 30 otherwise).
                let alt = if s.frame_rate == soundcraft_time::FrameRate::Fps25 {
                    soundcraft_time::FrameRate::Fps30
                } else {
                    soundcraft_time::FrameRate::Fps25
                };
                let mut s2 = s.clone();
                s2.frame_rate = alt;
                timebase_ruler(ui, &painter, &s2, tl, row, view, TimeFormat::Timecode, i + 1 == rulers.len());
            }
            "markers" | "markers2" | "markers3" | "markers4" | "markers5" | "chords" => {
                let lane: u8 = match id.as_str() {
                    "markers2" | "chords" => 2,
                    "markers3" => 3,
                    "markers4" => 4,
                    "markers5" => 5,
                    _ => 1,
                };
                painter.rect_filled(row, 0.0, if id == "chords" { Color32::from_rgb(40, 56, 60) } else { t.marker_ruler });
                for m in s.markers.iter().filter(|m| m.ruler == lane || (lane == 1 && m.ruler == 0)) {
                    let x = x_of(&s, tl, m.start);
                    let col = m.color.map_or(Color32::from_rgb(232, 200, 64), rgb);
                    if m.kind == soundcraft_model::MarkerKind::Selection {
                        let x1 = x_of(&s, tl, m.end);
                        painter.rect_filled(Rect::from_min_max(pos2(x, row.min.y + 3.0), pos2(x1, row.max.y - 3.0)), 2.0, col.gamma_multiply(0.5));
                    }
                    painter.add(Shape::convex_polygon(
                        vec![
                            pos2(x - 5.0, row.min.y + 2.0),
                            pos2(x + 5.0, row.min.y + 2.0),
                            pos2(x + 5.0, row.max.y - 6.0),
                            pos2(x, row.max.y - 2.0),
                            pos2(x - 5.0, row.max.y - 6.0),
                        ],
                        col,
                        Stroke::NONE,
                    ));
                    painter.text(pos2(x + 8.0, row.center().y), Align2::LEFT_CENTER, &m.name, bold(11.0), t.marker_text);
                }
                // Click on the marker ruler: recall a marker; double-click adds one.
                let resp = ui.interact(row, ui.id().with(("marker_ruler", lane)), Sense::click());
                if let Some(p) = resp.interact_pointer_pos() {
                    if resp.double_clicked() {
                        let at = sample_at(&s, tl, p.x).max(0);
                        let _ = app.run("markers.add", json!({"at": at, "ruler": lane}));
                    } else if resp.clicked()
                        && let Some(m) = s
                            .markers
                            .iter()
                            .filter(|m| m.ruler == lane || (lane == 1 && m.ruler == 0))
                            .find(|m| (x_of(&s, tl, m.start) - p.x).abs() < 8.0)
                    {
                        let _ = app.run("markers.recall", json!({"number": m.number}));
                    }
                }
            }
            _ => {
                if let Some(f) = fmt {
                    timebase_ruler(ui, &painter, &s, tl, row, view, f, i + 1 == rulers.len());
                    // Clicking a timebase ruler locates.
                    let resp = ui.interact(row, ui.id().with(("ruler", id.as_str())), Sense::click_and_drag());
                    if (resp.clicked() || resp.dragged())
                        && let Some(p) = resp.interact_pointer_pos()
                    {
                        let at = sample_at(&s, tl, p.x).max(0);
                        let _ = app.run("transport.locate", json!({"at": at}));
                    }
                }
            }
        }
        if id == "min_secs" || id == "bars_beats" || id == "timecode" || id == "samples" || id == "feet_frames" {
            // Main timebase marker (blue tick like a selected ruler).
            if Some(s.edit.main_counter) == fmt {
                ui.painter().rect_filled(Rect::from_min_size(pos2(lab.min.x + 6.0, lab.center().y - 3.0), vec2(6.0, 6.0)), 1.0, t.accent);
            }
        }
    }
    ui.painter().line_segment([pos2(area.min.x, area.max.y - 1.0), pos2(area.max.x, area.max.y - 1.0)], Stroke::new(1.0, t.border));
}

fn timebase_ruler(_ui: &Ui, painter: &egui::Painter, s: &Session, tl: Rect, row: Rect, view: Range, f: TimeFormat, bottom: bool) {
    let t = Tokens::current();
    let sr = s.sample_rate;
    let spp = s.edit.zoom.samples_per_px;
    let mut marks: Vec<(Samples, bool)> = Vec::new();
    match f {
        TimeFormat::BarsBeats => {
            // Bars every n so labels are ≥ 60 px apart.
            let bar_px = {
                let b = GridValue::Note { value: NoteValue::Bar, dotted: false, triplet: false };
                b.step_samples(view.start.max(0), sr, &s.tempo, s.frame_rate) as f64 / spp
            };
            let every = [1i64, 2, 4, 8, 16, 32, 64, 128].into_iter().find(|n| bar_px * *n as f64 >= 60.0).unwrap_or(256);
            let first = s.tempo.bar_beat_at(view.start.max(0), sr).bar;
            let mut bar = first - (first - 1).rem_euclid(every);
            let mut guard = 0;
            loop {
                let at = s.tempo.samples_at_bar_beat(soundcraft_time::BarBeat { bar, beat: 1, tick: 0 }, sr);
                if at > view.end || guard > 400 {
                    break;
                }
                marks.push((at, true));
                if bar_px * every as f64 >= 240.0 {
                    let m = s.tempo.meter_at_tick(s.tempo.samples_to_ticks(at, sr));
                    for beat in 2..=i64::from(m.numerator) {
                        marks.push((s.tempo.samples_at_bar_beat(soundcraft_time::BarBeat { bar, beat, tick: 0 }, sr), false));
                    }
                }
                bar += every;
                guard += 1;
            }
        }
        TimeFormat::Samples => {
            let step = {
                let min = spp * 90.0;
                let mut st = 1i64;
                while (st as f64) < min {
                    st *= if st.to_string().starts_with('1') {
                        2
                    } else if st.to_string().starts_with('2') {
                        5
                    } else {
                        2
                    };
                    if st > 10_000_000_000 {
                        break;
                    }
                }
                st
            };
            let mut at = view.start.max(0).div_euclid(step) * step;
            while at <= view.end && marks.len() < 400 {
                marks.push((at, true));
                at += step;
            }
        }
        _ => {
            let step = nice_step(spp, sr.as_f64(), 70.0).max(1);
            let mut at = view.start.max(0).div_euclid(step) * step;
            while at <= view.end && marks.len() < 400 {
                marks.push((at, true));
                let sub = step / 4;
                if sub > 0 && (sub as f64 / spp) > 8.0 {
                    for k in 1..4 {
                        marks.push((at + sub * k, false));
                    }
                }
                at += step;
            }
        }
    }
    for (at, major) in marks {
        let x = x_of(s, tl, at);
        if major {
            painter.line_segment([pos2(x, row.min.y + 2.0), pos2(x, row.max.y)], Stroke::new(1.0, t.ruler_tick));
            let label = match f {
                TimeFormat::BarsBeats => s.tempo.bar_beat_at(at, sr).bar.to_string(),
                TimeFormat::MinSecs => {
                    let secs = sr.seconds(at);
                    if secs.fract().abs() < 1e-9 {
                        format!("{}:{:02}", (secs / 60.0) as i64, (secs as i64) % 60)
                    } else {
                        format_position(at, f, sr, &s.tempo, s.frame_rate, s.timecode_start)
                    }
                }
                _ => format_position(at, f, sr, &s.tempo, s.frame_rate, s.timecode_start),
            };
            painter.text(pos2(x + 3.0, row.center().y), Align2::LEFT_CENTER, label, regular(11.0), t.ruler_text);
        } else {
            painter.line_segment([pos2(x, row.max.y - 4.0), pos2(x, row.max.y)], Stroke::new(1.0, t.ruler_tick));
        }
    }
    if bottom {
        painter.line_segment([pos2(tl.min.x, row.max.y), pos2(tl.max.x, row.max.y)], Stroke::new(1.0, t.border));
    }
}

// ---- track rows ------------------------------------------------------------------------------

fn track_row(app: &mut SoundApp, ui: &mut Ui, id: TrackId, row: Rect, tl: Rect) {
    let t = Tokens::current();
    let s = app.engine.session();
    let Some(track) = s.track(id).cloned() else { return };
    let selected = s.edit.selected_tracks.contains(&id);
    let head = Rect::from_min_max(row.min, pos2((row.min.x + HEADER_W).min(tl.min.x), row.max.y));
    let cols = edit_columns(app.engine.session());
    for (i, c) in cols.iter().enumerate() {
        let cr = Rect::from_min_size(pos2(head.max.x + i as f32 * COLUMN_W, row.min.y), vec2(COLUMN_W, row.height()));
        if cr.max.x > tl.min.x + 0.1 {
            break;
        }
        header_column(app, ui, &track, c, cr);
    }
    let main_h = track.height.points();
    let lane = Rect::from_min_max(pos2(tl.min.x, row.min.y), pos2(tl.max.x, row.min.y + main_h));
    // Header background.
    ui.painter().rect_filled(head, 0.0, if selected { t.header_selected } else { t.panel_bg2 });
    ui.painter().rect_filled(Rect::from_min_size(head.min, vec2(5.0, head.height())), 0.0, rgb(track.color));
    ui.painter().line_segment([pos2(head.min.x, row.max.y + 0.5), pos2(tl.max.x, row.max.y + 0.5)], Stroke::new(1.0, t.border));
    track_header(app, ui, &track, head, selected);
    // Lane.
    let painter = ui.painter().with_clip_rect(lane.intersect(ui.clip_rect()));
    painter.rect_filled(lane, 0.0, if selected { t.playlist_alt } else { t.playlist_bg });
    let s = app.engine.session().clone();
    grid_lines(&painter, &s, tl, lane);
    let view = Range::new(sample_at(&s, tl, tl.min.x), sample_at(&s, tl, tl.max.x));
    let auto_view = AutoParam::parse(&track.view);
    for clip in track.clips() {
        if !clip.range().overlaps(&view) {
            continue;
        }
        draw_clip(app, &painter, &s, &track, clip, tl, lane, auto_view.is_some());
    }
    crate::video_track::draw_lane(app, &painter, &s, &track, tl, lane);
    if let Some(p) = &auto_view {
        draw_automation(&painter, &s, &track, p, tl, lane);
    }
    if track.kind == TrackKind::Master || track.kind == TrackKind::Aux || track.kind == TrackKind::Vca {
        let p = auto_view.clone().unwrap_or(AutoParam::Volume);
        draw_automation(&painter, &s, &track, &p, tl, lane);
    }
    // Selection highlight on this track.
    let sel = s.edit.selection;
    if selected && !sel.is_empty() {
        let x0 = x_of(&s, tl, sel.start).max(lane.min.x);
        let x1 = x_of(&s, tl, sel.end).min(lane.max.x);
        if x1 > x0 {
            painter.rect_filled(Rect::from_min_max(pos2(x0, lane.min.y), pos2(x1, lane.max.y)), 0.0, t.selection);
        }
    }
    lane_interaction(app, ui, &track, lane, tl);
    if track.view == "playlists" {
        playlist_lanes(app, ui, &track, row, main_h, tl);
    }
}

/// Alternate playlists as lanes under the main one; clicking a clip promotes it (comping).
fn playlist_lanes(app: &mut SoundApp, ui: &mut Ui, track: &Track, row: Rect, main_h: f32, tl: Rect) {
    let t = Tokens::current();
    let s = app.engine.session().clone();
    let mut y = row.min.y + main_h;
    for (i, pl) in track.playlists.iter().enumerate() {
        if i == track.active_playlist {
            continue;
        }
        let lane = Rect::from_min_max(pos2(tl.min.x, y), pos2(tl.max.x, y + LANE_H));
        let head = Rect::from_min_max(pos2(row.min.x + 12.0, y), pos2(tl.min.x, y + LANE_H));
        ui.painter().rect_filled(lane, 0.0, t.playlist_lane);
        ui.painter().text(pos2(head.min.x + 4.0, head.center().y), Align2::LEFT_CENTER, &pl.name, regular(10.5), t.text_dim);
        let painter = ui.painter().with_clip_rect(lane.intersect(ui.clip_rect()));
        for c in &pl.clips {
            let r = Rect::from_min_max(pos2(x_of(&s, tl, c.start), lane.min.y + 3.0), pos2(x_of(&s, tl, c.end()), lane.max.y - 3.0));
            painter.rect(r, 2.0, rgb(track.color).gamma_multiply(0.45), Stroke::new(1.0, Color32::BLACK), StrokeKind::Inside);
            painter.text(pos2(r.min.x + 4.0, r.min.y + 7.0), Align2::LEFT_CENTER, &c.name, regular(9.5), t.text);
        }
        let resp = ui.interact(lane, ui.id().with(("pl_lane", track.id.0, i)), Sense::click());
        if resp.clicked()
            && let Some(p) = resp.interact_pointer_pos()
        {
            let at = sample_at(&s, tl, p.x);
            let sel = s.edit.selection;
            let range = if !sel.is_empty() && sel.contains(at) {
                Some((sel.start, sel.end))
            } else {
                pl.clips.iter().find(|c| c.range().contains(at)).map(|c| (c.start, c.end()))
            };
            if let Some((a, b)) = range {
                let _ = app.run("track.playlist_promote", json!({"track": track.id.0, "playlist": i, "start": a, "end": b}));
            }
        }
        resp.on_hover_text("Click a take (or the selection) to promote it to the main playlist");
        y += LANE_H;
    }
}

fn track_header(app: &mut SoundApp, ui: &mut Ui, track: &Track, head: Rect, selected: bool) {
    let t = Tokens::current();
    let id = track.id;
    // Drag the bottom edge to resize (snaps to the height presets).
    let edge = Rect::from_min_max(pos2(head.min.x, head.max.y - 4.0), pos2(head.max.x, head.max.y + 2.0));
    let eresp = ui.interact(edge, ui.id().with(("hedge", id.0)), Sense::drag());
    if eresp.hovered() || eresp.dragged() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeVertical);
    }
    if eresp.drag_stopped()
        && let Some(p) = ui.ctx().pointer_latest_pos()
    {
        let want = (p.y - head.min.y).max(10.0);
        let best = TrackHeight::ALL.into_iter().min_by_key(|h| ((h.points() - want).abs() * 10.0) as i64).unwrap_or(TrackHeight::Medium);
        if best != track.height {
            let _ = app.run("track.height", json!({"tracks": [id.0], "height": best.label()}));
        }
    }
    let x0 = head.min.x + 12.0;
    let h = head.height();
    // Click on header background selects the track.
    let bg = ui.interact(head, ui.id().with(("hdr", id.0)), Sense::click());
    if bg.clicked() {
        let add = ui.input(|i| i.modifiers.command || i.modifiers.shift);
        let mut tracks = if add { app.engine.session().edit.selected_tracks.clone() } else { Vec::new() };
        if add && tracks.contains(&id) {
            tracks.retain(|x| *x != id);
        } else {
            tracks.push(id);
        }
        let ids: Vec<u64> = tracks.iter().map(|x| x.0).collect();
        let _ = app.run("edit.select", json!({"tracks": ids}));
    }
    bg.context_menu(|ui| track_context_menu(app, ui, id));
    // Name field.
    let name_r = Rect::from_min_size(pos2(x0, head.min.y + 4.0), vec2(150.0, 17.0));
    ui.painter().rect(
        name_r,
        CornerRadius::same(2),
        if selected { t.name_field_sel } else { t.name_field },
        Stroke::new(1.0, Color32::from_rgb(20, 20, 20)),
        StrokeKind::Inside,
    );
    let shown_name = if app.engine.session().edit.flag("edit_view.track_number") {
        let n = app.engine.session().track_index(id).map_or(0, |i| i + 1);
        format!("{n}  {}", track.name)
    } else {
        track.name.clone()
    };
    ui.painter().with_clip_rect(name_r).text(name_r.center(), Align2::CENTER_CENTER, &shown_name, bold(12.0), t.text_dark);
    let nresp = ui.interact(name_r, ui.id().with(("name", id.0)), Sense::click_and_drag());
    if nresp.double_clicked() {
        app.dialogs.open_rename_track(id, &track.name);
    } else if nresp.clicked() {
        let _ = app.run("edit.select", json!({"tracks": [id.0]}));
    }
    // Drag the name plate to reorder tracks.
    if nresp.dragged()
        && let Some(p) = ui.ctx().pointer_latest_pos()
    {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
        let line_y = app
            .edit_layout
            .rows
            .iter()
            .map(|(_, r)| r[1])
            .chain(app.edit_layout.rows.last().map(|(_, r)| r[3]))
            .min_by_key(|y| ((y - p.y).abs() * 10.0) as i64)
            .unwrap_or(p.y);
        ui.ctx()
            .layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::new("reorder")))
            .line_segment([pos2(head.min.x, line_y), pos2(head.max.x + 600.0, line_y)], Stroke::new(3.0, Tokens::current().accent));
    }
    if nresp.drag_stopped()
        && let Some(p) = ui.ctx().pointer_latest_pos()
    {
        let rows = app.edit_layout.rows.clone();
        let target_row = rows.iter().position(|(_, r)| p.y < (r[1] + r[3]) * 0.5).unwrap_or(rows.len());
        // Convert the visible-row index to a session index.
        let s = app.engine.session();
        let to = rows.get(target_row).and_then(|(tid, _)| s.track_index(TrackId(*tid))).unwrap_or(s.tracks.len());
        let from = s.track_index(id).unwrap_or(0);
        let to = if to > from { to - 1 } else { to };
        if to != from {
            let _ = app.run("track.move", json!({"track": id.0, "to": to}));
        }
    }
    // Playlist selector arrow.
    let pl_r = Rect::from_min_size(pos2(name_r.max.x + 3.0, name_r.min.y), vec2(16.0, 17.0));
    icons::draw(ui.painter(), pl_r.shrink(3.0), "triangle_down", t.text);
    let presp = ui.interact(pl_r, ui.id().with(("pl", id.0)), Sense::click());
    egui::Popup::menu(&presp).show(|ui| {
        if ui.button("New...").clicked() {
            let _ = app.run("track.playlist_new", json!({"track": id.0}));
        }
        if ui.button("Duplicate...").clicked() {
            let _ = app.run("track.playlist_duplicate", json!({"track": id.0}));
        }
        if ui.button("Delete Unused...").clicked() {
            let _ = app.run("track.playlist_delete_unused", json!({"track": id.0}));
        }
        ui.separator();
        for (i, p) in track.playlists.iter().enumerate() {
            if ui.selectable_label(i == track.active_playlist, &p.name).clicked() {
                let _ = app.run("track.playlist_select", json!({"track": id.0, "index": i}));
            }
        }
    });
    if track.is_folder() {
        let fr = Rect::from_min_size(pos2(head.min.x + 6.0, name_r.min.y), vec2(10.0, 17.0));
        icons::draw(ui.painter(), fr, if track.folder_open { "triangle_down" } else { "triangle_right" }, t.text);
        if ui.interact(fr, ui.id().with(("fold", id.0)), Sense::click()).clicked() {
            let _ = app.run("track.folder_toggle", json!({"track": id.0}));
        }
    }
    let kind_label = match track.kind {
        TrackKind::Audio => "",
        TrackKind::Aux => "AUX",
        TrackKind::Master => "MASTER",
        TrackKind::Midi => "MIDI",
        TrackKind::Instrument => "INST",
        TrackKind::Vca => "VCA",
        TrackKind::Folder => "FOLDER",
        TrackKind::Video => "VIDEO",
    };
    if !kind_label.is_empty() {
        let kr = ui.painter().text(pos2(pl_r.max.x + 4.0, name_r.center().y), Align2::LEFT_CENTER, kind_label, bold(9.0), t.text_dim);
        if track.kind == TrackKind::Instrument {
            // The instrument picker (and the instrument's editor) behind the INST label.
            let name = track.instrument.as_ref().and_then(|i| crate::mix_window::plugin_info(&i.plugin)).map_or("none", |p| p.name);
            let resp = ui
                .interact(kr.expand(3.0), ui.id().with(("instrument", id.0)), Sense::click())
                .on_hover_text(format!("Instrument: {name} (click to change or open its editor)"));
            egui::Popup::menu(&resp).show(|ui| crate::mix_window::instrument_menu(app, ui, id));
        }
    }
    if h < 30.0 {
        return;
    }
    // Buttons row.
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(Rect::from_min_size(pos2(x0, name_r.max.y + 5.0), vec2(170.0, 20.0)))
            .layout(egui::Layout::left_to_right(egui::Align::Min)),
    );
    child.spacing_mut().item_spacing.x = 4.0;
    let m = &track.mixer;
    let bs = vec2(26.0, 18.0);
    if track.kind.has_playlist() {
        if rec_toggle(&mut child, bs, m.record_arm, "Record enable").clicked() {
            let _ = app.run("mix.record_arm", json!({"track": id.0}));
        }
        if text_toggle(&mut child, bs, "I", m.input_monitor, t.input, "Input monitoring").clicked() {
            let _ = app.run("mix.input_monitor", json!({"track": id.0}));
        }
    } else {
        child.add_space(bs.x * 2.0 + 4.0);
    }
    if track.kind != TrackKind::Master {
        if text_toggle(&mut child, bs, "S", m.solo, t.solo, "Solo").clicked() {
            let _ = app.run("mix.solo", json!({"track": id.0}));
        }
        if text_toggle(&mut child, bs, "M", m.mute, t.mute, "Mute").clicked() {
            let _ = app.run("mix.mute", json!({"track": id.0}));
        }
    }
    if h < 60.0 {
        return;
    }
    // View selector.
    let view_r = Rect::from_min_size(pos2(x0, head.min.y + 49.0), vec2(118.0, 16.0));
    let mut c2 = ui.new_child(egui::UiBuilder::new().max_rect(view_r));
    let vresp = selector_box(&mut c2, view_r.width(), view_r.height(), &view_label(&track.view), t.text);
    egui::Popup::menu(&vresp).show(|ui| view_menu(app, ui, track));
    // Automation mode.
    let am_r = Rect::from_min_size(pos2(x0, head.min.y + 68.0), vec2(118.0, 16.0));
    let mode = m.automation_mode;
    let col = match mode {
        soundcraft_model::AutomationMode::Read => t.auto_read,
        soundcraft_model::AutomationMode::Write => t.auto_write,
        soundcraft_model::AutomationMode::Touch | soundcraft_model::AutomationMode::TouchLatch => t.auto_touch,
        soundcraft_model::AutomationMode::Latch => t.auto_latch,
        _ => t.text_dim,
    };
    let mut c3 = ui.new_child(egui::UiBuilder::new().max_rect(am_r));
    let aresp = selector_box(&mut c3, am_r.width(), am_r.height(), mode.label(), col);
    egui::Popup::menu(&aresp).show(|ui| {
        for m in soundcraft_model::AutomationMode::ALL {
            if ui.selectable_label(m == mode, m.label()).clicked() {
                let _ = app.run("mix.automation_mode", json!({"track": id.0, "mode": m.label()}));
            }
        }
    });
    // Inline header meter.
    let md = app.meters.get(&id).copied().unwrap_or_default();
    let mr = Rect::from_min_max(pos2(head.max.x - 16.0, head.min.y + 4.0), pos2(head.max.x - 6.0, head.max.y - 4.0));
    if track.channels() >= 2 {
        let w = mr.width() / 2.0 - 0.5;
        crate::widgets::meter(ui, Rect::from_min_size(mr.min, vec2(w, mr.height())), md.level[0], md.hold[0], md.clip);
        crate::widgets::meter(ui, Rect::from_min_size(pos2(mr.min.x + w + 1.0, mr.min.y), vec2(w, mr.height())), md.level[1], md.hold[1], md.clip);
    } else {
        crate::widgets::meter(ui, mr, md.level[0], md.hold[0], md.clip);
    }
    // Volume readout and inserts summary.
    let info = format!("vol {}  pan {}", crate::widgets::db_text(m.volume_db), m.pan.first().map_or("-".into(), |p| crate::widgets::pan_text(*p)));
    ui.painter().text(pos2(view_r.max.x + 8.0, view_r.center().y), Align2::LEFT_CENTER, info, regular(10.0), t.text_dim);
    let ins: Vec<&str> = m.inserts.iter().flatten().filter_map(|i| soundcraft_dsp::plugin_info(&i.plugin).map(|p| p.short_name)).collect();
    if !ins.is_empty() {
        ui.painter().with_clip_rect(Rect::from_min_max(pos2(am_r.max.x + 6.0, am_r.min.y), pos2(head.max.x - 20.0, am_r.max.y))).text(
            pos2(am_r.max.x + 8.0, am_r.center().y),
            Align2::LEFT_CENTER,
            ins.join(" · "),
            regular(10.0),
            t.text_dim,
        );
    }
}

/// One optional Edit-window column for a track (I/O, inserts, sends, comments).
fn header_column(app: &mut SoundApp, ui: &mut Ui, track: &Track, col: &str, r: Rect) {
    let t = Tokens::current();
    ui.painter().rect_filled(r, 0.0, t.panel_bg);
    ui.painter().line_segment([pos2(r.min.x, r.min.y), pos2(r.min.x, r.max.y)], Stroke::new(1.0, t.border));
    let line_h = 15.0;
    let rows = ((r.height() - 4.0) / line_h).floor().max(1.0) as usize;
    let item = |i: usize| Rect::from_min_size(pos2(r.min.x + 4.0, r.min.y + 3.0 + i as f32 * line_h), vec2(r.width() - 8.0, line_h - 2.0));
    let text = |ui: &Ui, rr: Rect, s: &str, c: Color32| {
        ui.painter().rect_filled(rr, 2.0, t.slot_bg);
        ui.painter().with_clip_rect(rr).text(pos2(rr.min.x + 4.0, rr.center().y), Align2::LEFT_CENTER, s, regular(10.0), c);
    };
    let route = |r: &soundcraft_model::Route| match r {
        soundcraft_model::Route::None => "none".to_string(),
        soundcraft_model::Route::Main => "Out 1-2".to_string(),
        soundcraft_model::Route::Bus(b) => app.engine.session().bus(*b).map_or("bus".into(), |b| b.name.clone()),
        soundcraft_model::Route::Hardware(h) => h.clone(),
    };
    match col {
        "io" => {
            if rows >= 1 {
                text(ui, item(0), &format!("in: {}", route(&track.mixer.input)), t.text);
            }
            if rows >= 2 {
                text(ui, item(1), &format!("out: {}", route(&track.mixer.output)), t.text);
            }
        }
        "inserts_ae" | "inserts_fj" => {
            let off = if col == "inserts_ae" { 0 } else { 5 };
            for i in 0..5.min(rows) {
                let name = track
                    .mixer
                    .inserts
                    .get(off + i)
                    .cloned()
                    .flatten()
                    .and_then(|x| soundcraft_dsp::plugin_info(&x.plugin).map(|p| p.name))
                    .unwrap_or("");
                text(ui, item(i), name, t.text);
            }
        }
        "sends_ae" | "sends_fj" => {
            let off = if col == "sends_ae" { 0 } else { 5 };
            for i in 0..5.min(rows) {
                let s = track
                    .mixer
                    .sends
                    .get(off + i)
                    .cloned()
                    .flatten()
                    .map(|x| format!("{} {:.0}", route(&x.target), x.level_db.max(-99.0)))
                    .unwrap_or_default();
                text(ui, item(i), &s, t.text);
            }
        }
        _ => {
            let mut c = track.comments.clone();
            let mut child = ui.new_child(egui::UiBuilder::new().max_rect(r.shrink(3.0)));
            if child.add(egui::TextEdit::multiline(&mut c).desired_width(r.width() - 6.0).font(regular(10.0))).changed() {
                let _ =
                    app.engine.execute_merged("track.comments", &json!({"track": track.id.0, "comments": c}), &format!("comments:{}", track.id.0));
            }
        }
    }
}

fn view_label(v: &str) -> String {
    match AutoParam::parse(v) {
        Some(p) => p.label(),
        None => v.to_string(),
    }
}

fn view_menu(app: &mut SoundApp, ui: &mut Ui, track: &Track) {
    let id = track.id.0;
    let mut opts: Vec<&str> = Vec::new();
    if track.kind.is_midi() {
        opts.extend(["notes", "regions", "blocks"]);
    } else if track.kind.has_playlist() {
        opts.extend(["waveform", "blocks", "clip_gain", "playlists"]);
    }
    opts.extend(["volume", "pan", "mute"]);
    for o in opts {
        if ui.selectable_label(track.view == o, view_label(o)).clicked() {
            let _ = app.run("track.view", json!({"track": id, "view": o}));
        }
    }
    for (slot, ins) in track.mixer.inserts.iter().enumerate() {
        let Some(ins) = ins else { continue };
        let Some(info) = soundcraft_dsp::plugin_info(&ins.plugin) else { continue };
        ui.menu_button(info.name, |ui| {
            for p in info.params {
                let v = format!("plugin:{}:{}", (b'a' + slot as u8) as char, p.id);
                if ui.button(p.name).clicked() {
                    let _ = app.run("track.view", json!({"track": id, "view": v}));
                }
            }
        });
    }
}

fn track_context_menu(app: &mut SoundApp, ui: &mut Ui, id: TrackId) {
    let tid = id.0;
    let items: [(&str, &str); 10] = [
        ("Rename...", "ui.rename_track"),
        ("Duplicate...", "track.duplicate"),
        ("Make Inactive", "track.make_inactive"),
        ("Hide", "track.hide"),
        ("Delete", "track.delete"),
        ("Split into Mono", "track.split_into_mono"),
        ("Commit...", "track.commit"),
        ("Freeze", "track.freeze"),
        ("Move to New Folder...", "track.move_to_new_folder"),
        ("Group...", "track.group"),
    ];
    for (label, cmd) in items {
        if ui.button(label).clicked() {
            if cmd == "ui.rename_track" {
                let name = app.engine.session().track(id).map(|t| t.name.clone()).unwrap_or_default();
                app.dialogs.open_rename_track(id, &name);
            } else {
                let _ = app.run(cmd, json!({"tracks": [tid]}));
            }
        }
    }
    ui.separator();
    ui.menu_button("Track Height", |ui| {
        for h in TrackHeight::ALL {
            if ui.button(h.label()).clicked() {
                let _ = app.run("track.height", json!({"tracks": [tid], "height": h.label()}));
            }
        }
    });
    ui.menu_button("Track Color", |ui| {
        for (i, c) in soundcraft_model::TRACK_COLORS.iter().enumerate() {
            let (r, resp) = ui.allocate_exact_size(vec2(120.0, 14.0), Sense::click());
            ui.painter().rect_filled(r, 2.0, rgb(*c));
            if resp.clicked() {
                let _ = app.run("track.color", json!({"tracks": [tid], "color": i}));
            }
        }
    });
}

fn grid_lines(painter: &egui::Painter, s: &Session, tl: Rect, lane: Rect) {
    let t = Tokens::current();
    let view = Range::new(sample_at(s, tl, tl.min.x).max(0), sample_at(s, tl, tl.max.x));
    let sr = s.sample_rate;
    // Bar lines always; grid lines when enabled and dense enough.
    let bar = GridValue::Note { value: NoteValue::Bar, dotted: false, triplet: false };
    let bar_px = bar.step_samples(view.start, sr, &s.tempo, s.frame_rate) as f64 / s.edit.zoom.samples_per_px;
    if bar_px > 6.0 {
        for at in bar.lines(view.start, view.end, sr, &s.tempo, s.frame_rate, 600) {
            let x = x_of(s, tl, at);
            painter.line_segment([pos2(x, lane.min.y), pos2(x, lane.max.y)], Stroke::new(1.0, t.bar_line));
        }
    }
    if s.edit.grid_lines {
        let step_px = s.edit.grid.step_samples(view.start, sr, &s.tempo, s.frame_rate) as f64 / s.edit.zoom.samples_per_px;
        if step_px > 7.0 {
            for at in s.edit.grid.lines(view.start, view.end, sr, &s.tempo, s.frame_rate, 2000) {
                let x = x_of(s, tl, at);
                painter.line_segment([pos2(x, lane.min.y), pos2(x, lane.max.y)], Stroke::new(1.0, t.grid_line));
            }
        }
    }
}

fn draw_clip(app: &SoundApp, painter: &egui::Painter, s: &Session, track: &Track, clip: &Clip, tl: Rect, lane: Rect, dim: bool) {
    let x0 = x_of(s, tl, clip.start);
    let x1 = x_of(s, tl, clip.end()).max(x0 + 1.0);
    let r = Rect::from_min_max(pos2(x0, lane.min.y + 1.0), pos2(x1, lane.max.y - 1.0));
    let selected = s.edit.selected_clips.contains(&clip.id)
        || (s.edit.selected_tracks.contains(&track.id)
            && !s.edit.selection.is_empty()
            && s.edit.selection.start <= clip.start
            && s.edit.selection.end >= clip.end());
    let color = clip.color.unwrap_or(track.color);
    let (mut body, bar, mut wave) = clip_colors(color, selected);
    if clip.muted || dim {
        body = body.gamma_multiply(0.45);
        wave = Color32::from_rgb(70, 70, 70);
    }
    let body = if s.edit.flag("view.clip.transparency") { body.gamma_multiply(0.55) } else { body };
    painter.rect(r, CornerRadius::same(2), body, Stroke::new(1.0, Color32::from_rgb(10, 10, 10)), StrokeKind::Inside);
    let name_h = if lane.height() >= 40.0 && s.edit.flag("view.clip.name") { 13.0 } else { 0.0 };
    if name_h > 0.0 {
        let nb = Rect::from_min_max(r.min, pos2(r.max.x, r.min.y + name_h));
        painter.rect_filled(nb, CornerRadius { nw: 2, ne: 2, sw: 0, se: 0 }, if clip.muted { Color32::from_rgb(70, 70, 70) } else { bar });
        let name_clip = nb.intersect(lane);
        painter.with_clip_rect(name_clip).text(
            pos2(nb.min.x.max(lane.min.x) + 4.0, nb.center().y),
            Align2::LEFT_CENTER,
            &clip.name,
            bold(10.5),
            Color32::from_rgb(12, 12, 12),
        );
    }
    let body_r = Rect::from_min_max(pos2(r.min.x, r.min.y + name_h), r.max);
    let vis = body_r.intersect(lane);
    match &clip.content {
        ClipContent::Audio { source, offset } => {
            if let Some(audio) = s.pool.get(*source) {
                let nch = track.channels().min(audio.peaks.len().max(1)).max(1);
                let ch_h = body_r.height() / nch as f32;
                let zoom = s.edit.zoom.waveform_zoom;
                let rectified = s.edit.flag("waveform.rectified");
                let power = s.edit.flag("waveform.power");
                let outlines = s.edit.flag("waveform.outlines");
                let cols = vis.width().max(0.0) as usize;
                if cols > 0 && vis.height() > 2.0 {
                    let a = (offset + sample_at(s, tl, vis.min.x) - clip.start) as f64;
                    let b = (offset + sample_at(s, tl, vis.max.x) - clip.start) as f64;
                    for ch in 0..nch {
                        let Some(pk) = audio.peaks.get(ch % audio.peaks.len().max(1)) else { continue };
                        let cy = body_r.min.y + ch_h * (ch as f32 + 0.5);
                        let half = ch_h * 0.46;
                        let colsv = pk.columns(a, b, cols);
                        let mut mesh = egui::Mesh::default();
                        for (i, (mn, mx)) in colsv.iter().enumerate() {
                            let x = vis.min.x + i as f32;
                            let rel = sample_at(s, tl, x) - clip.start;
                            let g = clip.gain_at(rel.clamp(0, clip.length));
                            let (mut top, mut bot) = if rectified {
                                let a = (mx.abs().max(mn.abs()) * g * zoom).clamp(0.0, 1.0);
                                let b = cy + half;
                                (b - a * half * 2.0, b)
                            } else {
                                (cy - (mx * g * zoom).clamp(-1.0, 1.0) * half, cy - (mn * g * zoom).clamp(-1.0, 1.0) * half)
                            };
                            if power {
                                // Power view: compress peaks logarithmically around the centre.
                                let k = |y: f32| cy + (y - cy).signum() * ((y - cy).abs() / half).sqrt() * half;
                                top = k(top);
                                bot = k(bot);
                            }
                            if outlines {
                                bot = top + 1.0;
                            }
                            let bot = if (bot - top) < 1.0 { top + 1.0 } else { bot };
                            let idx = mesh.vertices.len() as u32;
                            mesh.colored_vertex(pos2(x, top), wave);
                            mesh.colored_vertex(pos2(x + 1.0, top), wave);
                            mesh.colored_vertex(pos2(x + 1.0, bot), wave);
                            mesh.colored_vertex(pos2(x, bot), wave);
                            mesh.add_triangle(idx, idx + 1, idx + 2);
                            mesh.add_triangle(idx, idx + 2, idx + 3);
                        }
                        painter.with_clip_rect(vis).add(Shape::mesh(mesh));
                        if nch > 1 && ch + 1 < nch {
                            let y = body_r.min.y + ch_h * (ch as f32 + 1.0);
                            painter.line_segment([pos2(vis.min.x, y), pos2(vis.max.x, y)], Stroke::new(1.0, body.gamma_multiply(0.7)));
                        }
                    }
                }
            } else {
                painter.text(body_r.center(), Align2::CENTER_CENTER, "media offline", regular(10.0), Color32::from_rgb(220, 120, 120));
            }
        }
        ClipContent::Video { .. } => {} // thumbnails: video_track::draw_lane
        ClipContent::Midi { sequence } => {
            let (lo, hi) = sequence.notes.iter().fold((127u8, 0u8), |(lo, hi), n| (lo.min(n.pitch), hi.max(n.pitch)));
            let (lo, hi) = if lo > hi { (48, 72) } else { (lo.saturating_sub(2), hi.saturating_add(2)) };
            let span = f32::from(hi.saturating_sub(lo).max(1));
            let base = s.tempo.samples_to_ticks(clip.start, s.sample_rate);
            let p = painter.with_clip_rect(vis);
            for n in &sequence.notes {
                let a = s.tempo.tick_to_samples(base + n.start, s.sample_rate);
                let b = s.tempo.tick_to_samples(base + n.start + n.length, s.sample_rate).min(clip.end());
                let nx0 = x_of(s, tl, a);
                let nx1 = x_of(s, tl, b).max(nx0 + 2.0);
                if nx1 < vis.min.x || nx0 > vis.max.x {
                    continue;
                }
                let y = body_r.max.y - 3.0 - (f32::from(n.pitch.saturating_sub(lo)) / span) * (body_r.height() - 6.0);
                let nh = ((body_r.height() - 6.0) / span).clamp(1.5, 6.0);
                let vcol = Color32::from_rgb(20, 20 + n.velocity, 40);
                p.rect_filled(
                    Rect::from_min_max(pos2(nx0, y - nh * 0.5), pos2(nx1, y + nh * 0.5)),
                    1.0,
                    if selected { Color32::from_rgb(16, 16, 16) } else { vcol },
                );
            }
        }
    }
    // Clip gain line.
    let show_gain = s.edit.flag("view.clip.gain_line") && (!clip.gain_env.is_empty() || clip.gain_db.abs() > 0.01 || track.view == "clip_gain");
    if show_gain && body_r.height() > 12.0 {
        let gy = |db: f32| body_r.max.y - 3.0 - ((db.clamp(-36.0, 36.0) + 36.0) / 72.0) * (body_r.height() - 6.0);
        let col = Color32::from_rgba_unmultiplied(255, 255, 255, 170);
        if clip.gain_env.is_empty() {
            painter.line_segment([pos2(x0, gy(clip.gain_db)), pos2(x1, gy(clip.gain_db))], Stroke::new(1.0, col));
        } else {
            let mut pts = vec![pos2(x0, gy(clip.gain_db + clip.gain_env.first().map_or(0.0, |p| p.1)))];
            for (o, db) in &clip.gain_env {
                let p = pos2(x_of(s, tl, clip.start + o), gy(clip.gain_db + db));
                pts.push(p);
                painter.circle_filled(p, 2.5, col);
            }
            pts.push(pos2(x1, gy(clip.gain_db + clip.gain_env.last().map_or(0.0, |p| p.1))));
            painter.add(Shape::line(pts, Stroke::new(1.0, col)));
        }
    }
    if !dim && let Some(icon) = clip_gain_icon(s, tl, lane, clip) {
        let col = Color32::from_rgba_unmultiplied(255, 255, 255, 200);
        painter.line_segment([pos2(icon.center().x, icon.min.y), pos2(icon.center().x, icon.max.y)], Stroke::new(1.0, col));
        painter.rect_filled(Rect::from_center_size(pos2(icon.center().x, icon.center().y + 2.0), vec2(icon.width(), 3.0)), 1.0, col);
        painter.text(pos2(icon.max.x + 3.0, icon.center().y), Align2::LEFT_CENTER, format!("{:+.1} dB", clip.gain_db), regular(9.5), col);
    }
    // Fades.
    let fade_col = Color32::from_rgba_unmultiplied(255, 255, 255, 90);
    if clip.fade_in.len > 0 {
        let fx = x_of(s, tl, clip.start + clip.fade_in.len);
        let pts: Vec<Pos2> = (0..=16)
            .map(|i| {
                let k = i as f32 / 16.0;
                pos2(x0 + (fx - x0) * k, body_r.max.y - body_r.height() * clip.fade_in.shape.gain(k))
            })
            .collect();
        painter.add(Shape::line(pts, Stroke::new(1.2, fade_col)));
        painter.line_segment([pos2(x0, body_r.min.y), pos2(fx, body_r.min.y)], Stroke::new(1.0, fade_col));
    }
    if clip.fade_out.len > 0 {
        let fx = x_of(s, tl, clip.end() - clip.fade_out.len);
        let pts: Vec<Pos2> = (0..=16)
            .map(|i| {
                let k = i as f32 / 16.0;
                pos2(fx + (x1 - fx) * k, body_r.max.y - body_r.height() * clip.fade_out.shape.gain(1.0 - k))
            })
            .collect();
        painter.add(Shape::line(pts, Stroke::new(1.2, fade_col)));
    }
    // Optional clip overlays (View › Clip).
    let ov = Color32::from_rgb(16, 16, 16);
    if s.edit.flag("view.clip.rating") && clip.rating > 0 && name_h > 0.0 {
        painter.text(pos2(r.max.x - 18.0, r.min.y + 7.0), Align2::RIGHT_CENTER, "★".repeat(usize::from(clip.rating.min(5))), regular(9.0), ov);
    }
    if s.edit.flag("view.clip.sync_point") && clip.sync_point > 0 {
        let x = x_of(s, tl, clip.start + clip.sync_point);
        painter.add(Shape::convex_polygon(
            vec![pos2(x - 4.0, r.max.y - 1.0), pos2(x + 4.0, r.max.y - 1.0), pos2(x, r.max.y - 7.0)],
            Color32::WHITE,
            Stroke::NONE,
        ));
        painter.line_segment([pos2(x, r.min.y + name_h), pos2(x, r.max.y)], Stroke::new(1.0, Color32::from_rgba_unmultiplied(255, 255, 255, 90)));
    }
    let time_text = if s.edit.flag("view.clip.time.current") {
        Some(format_position(clip.start, s.edit.main_counter, s.sample_rate, &s.tempo, s.frame_rate, s.timecode_start))
    } else if s.edit.flag("view.clip.time.original") || s.edit.flag("view.clip.time.user") {
        clip.source().and_then(|src| s.source(src)).map(|src| {
            format_position(
                i64::try_from(src.time_reference).unwrap_or(0) + clip.source_offset(),
                TimeFormat::Timecode,
                s.sample_rate,
                &s.tempo,
                s.frame_rate,
                0,
            )
        })
    } else {
        None
    };
    if let Some(tt) = time_text
        && r.height() > 30.0
    {
        painter.with_clip_rect(r.intersect(lane)).text(pos2(r.min.x.max(lane.min.x) + 4.0, r.max.y - 7.0), Align2::LEFT_CENTER, tt, regular(9.0), ov);
    }
    if s.edit.flag("view.clip.effects_status") && s.edit.values.keys().any(|k| k.starts_with(&format!("clip_fx.{}.", clip.id.0))) {
        painter.text(pos2(r.max.x - 6.0, r.max.y - 7.0), Align2::RIGHT_CENTER, "FX", bold(9.0), Color32::from_rgb(250, 230, 120));
    }
    if s.edit.flag("view.clip.overlap_shadows") {
        // Shade where a later clip covers this one (crossfades and layered edits).
        for o in track.clips() {
            if o.id != clip.id && o.start > clip.start && o.start < clip.end() {
                let x0 = x_of(s, tl, o.start);
                let x1 = x_of(s, tl, clip.end().min(o.end()));
                painter.rect_filled(Rect::from_min_max(pos2(x0, r.min.y), pos2(x1, r.max.y)), 0.0, Color32::from_black_alpha(70));
            }
        }
    }
    if clip.edit_locked || clip.time_locked {
        painter.text(pos2(r.max.x - 10.0, r.min.y + 7.0), Align2::CENTER_CENTER, "🔒", regular(9.0), Color32::WHITE);
    }
    let _ = app;
}

fn draw_automation(painter: &egui::Painter, s: &Session, track: &Track, p: &AutoParam, tl: Rect, lane: Rect) {
    let t = Tokens::current();
    let (lo, hi, _) = p.range();
    let (lo, hi) = if matches!(p, AutoParam::Volume | AutoParam::SendLevel(_)) {
        (-60.0f32, 12.0f32)
    } else if matches!(p, AutoParam::Plugin { .. }) {
        plugin_range(track, p)
    } else {
        (lo, hi)
    };
    let def = match p {
        AutoParam::Volume => track.mixer.volume_db,
        AutoParam::Pan(i) => track.mixer.pan.get(usize::from(*i)).copied().unwrap_or(0.0),
        _ => 0.0,
    };
    let yv = |v: f32| -> f32 {
        let k = ((v.clamp(lo, hi) - lo) / (hi - lo).max(1e-6)).clamp(0.0, 1.0);
        lane.max.y - 4.0 - k * (lane.height() - 8.0)
    };
    let lanes = track.lane(p);
    let pts: Vec<(f32, f32)> = match lanes {
        Some(l) if !l.points.is_empty() => {
            let mut v: Vec<(f32, f32)> = Vec::new();
            v.push((lane.min.x, yv(l.value_at(sample_at(s, tl, lane.min.x), def))));
            for pt in &l.points {
                let x = x_of(s, tl, pt.at);
                if x > lane.min.x && x < lane.max.x {
                    v.push((x, yv(pt.value)));
                }
            }
            v.push((lane.max.x, yv(l.value_at(sample_at(s, tl, lane.max.x), def))));
            v
        }
        _ => vec![(lane.min.x, yv(def)), (lane.max.x, yv(def))],
    };
    let line: Vec<Pos2> = pts.iter().map(|(x, y)| pos2(*x, *y)).collect();
    painter.add(Shape::line(line, Stroke::new(1.5, t.automation_line)));
    if let Some(l) = lanes {
        for pt in &l.points {
            let x = x_of(s, tl, pt.at);
            if x > lane.min.x - 4.0 && x < lane.max.x + 4.0 {
                painter.circle(pos2(x, yv(pt.value)), 3.0, Color32::from_rgb(40, 40, 40), Stroke::new(1.5, t.automation_line));
            }
        }
    }
    painter.text(pos2(lane.min.x + 6.0, lane.min.y + 9.0), Align2::LEFT_CENTER, p.label(), regular(10.0), t.text_dim);
}

fn plugin_range(track: &Track, p: &AutoParam) -> (f32, f32) {
    if let AutoParam::Plugin { slot, param } = p
        && let Some(Some(ins)) = track.mixer.inserts.get(usize::from(*slot))
        && let Some(info) = soundcraft_dsp::plugin_info(&ins.plugin)
        && let Some(pi) = info.param(param)
    {
        return (pi.min, pi.max);
    }
    (0.0, 1.0)
}

// ---- interaction -----------------------------------------------------------------------------

fn snap(s: &Session, at: Samples) -> Samples {
    if s.edit.edit_mode.is_grid() { s.edit.grid.snap(at, s.sample_rate, &s.tempo, s.frame_rate) } else { at }
}

fn clip_at(track: &Track, at: Samples) -> Option<&Clip> {
    track.clips().iter().rev().find(|c| c.range().contains(at))
}

pub const CLIP_FADER_TRAVEL: f32 = 200.0;

/// Bottom-left of an audio clip's visible part, when Clip Gain Info is on or the clip has gain and it fits.
pub fn clip_gain_icon(s: &Session, tl: Rect, lane: Rect, clip: &Clip) -> Option<Rect> {
    let x0 = x_of(s, tl, clip.start).max(lane.min.x);
    let x1 = x_of(s, tl, clip.end()).min(lane.max.x);
    let shown = matches!(clip.content, ClipContent::Audio { .. }) && (s.edit.flag("view.clip.gain_info") || clip.gain_db.abs() > 0.01);
    (shown && x1 - x0 >= 48.0 && lane.height() >= 30.0).then(|| Rect::from_min_size(pos2(x0 + 4.0, lane.max.y - 16.0), vec2(9.0, 13.0)))
}

fn clip_gain_popup(ctx: &egui::Context, fader_bottom: Pos2, pos: f32) {
    let t = Tokens::current();
    let p = ctx.layer_painter(egui::LayerId::new(egui::Order::Tooltip, egui::Id::new("clip_gain_fader")));
    let track = Rect::from_min_max(pos2(fader_bottom.x - 2.0, fader_bottom.y - CLIP_FADER_TRAVEL), pos2(fader_bottom.x + 2.0, fader_bottom.y));
    let panel = Rect::from_min_max(pos2(track.min.x - 22.0, track.min.y - 26.0), pos2(track.max.x + 22.0, track.max.y + 8.0));
    p.rect(panel, 3.0, Color32::from_rgb(38, 38, 40), Stroke::new(1.0, Color32::from_rgb(10, 10, 10)), StrokeKind::Inside);
    p.rect_filled(track, 2.0, t.fader_track);
    let zero = track.max.y - CLIP_FADER_TRAVEL * crate::widgets::clip_gain_to_pos(0.0);
    p.line_segment([pos2(track.min.x - 8.0, zero), pos2(track.min.x - 2.0, zero)], Stroke::new(1.0, Color32::from_rgb(150, 150, 150)));
    let cap = Rect::from_center_size(pos2(track.center().x, track.max.y - CLIP_FADER_TRAVEL * pos), vec2(22.0, 9.0));
    p.rect(cap, 2.0, Color32::from_rgb(210, 210, 210), Stroke::new(1.0, Color32::from_rgb(20, 20, 20)), StrokeKind::Inside);
    let db = crate::widgets::clip_gain_from_pos(pos);
    p.text(pos2(panel.center().x, panel.min.y + 12.0), Align2::CENTER_CENTER, format!("{db:+.1}"), regular(10.5), t.counter_text);
}

fn lane_interaction(app: &mut SoundApp, ui: &mut Ui, track: &Track, lane: Rect, tl: Rect) {
    let id = ui.id().with(("lane", track.id.0));
    let resp = ui.interact(lane, id, Sense::click_and_drag());
    // The clip menu is attached before the early return below: once the menu is open the pointer
    // is over the popup, the lane has no hover position, and the rest of this function bails out.
    // The clip is remembered at the right-click so the menu keeps its target while it is used.
    let menu_clip_id = id.with("menu_clip");
    if resp.secondary_clicked() {
        let clip = resp.interact_pointer_pos().and_then(|p| {
            let s = app.engine.session();
            clip_at(track, sample_at(s, tl, p.x).max(0)).map(|c| c.id)
        });
        ui.ctx().data_mut(|d| d.insert_temp(menu_clip_id, clip));
    }
    // The clip may have been removed or moved since the right-click; only use it if it still exists.
    let menu_clip = ui.ctx().data(|d| d.get_temp::<Option<ClipId>>(menu_clip_id)).flatten().filter(|c| track.clips().iter().any(|x| x.id == *c));
    resp.context_menu(|ui| clip_context_menu(app, ui, menu_clip));
    // Audio files dropped from the Clip List.
    if let Some(src) = resp.dnd_release_payload::<crate::DragSource>()
        && let Some(p) = ui.ctx().pointer_latest_pos()
    {
        let s = app.engine.session();
        let at = snap(s, sample_at(s, tl, p.x).max(0));
        let _ = app.run("clip.place_source", json!({"source": src.0, "track": track.id.0, "at": at}));
    }
    if resp.dnd_hover_payload::<crate::DragSource>().is_some() {
        ui.painter().rect_stroke(lane, 0.0, Stroke::new(2.0, Tokens::current().accent), StrokeKind::Inside);
    }
    let s = app.engine.session().clone();
    let tool = s.edit.tool;
    let mods = ui.input(|i| i.modifiers);
    let Some(p) = resp.interact_pointer_pos().or_else(|| resp.hover_pos()) else { return };
    let at = sample_at(&s, tl, p.x).max(0);
    let hit = clip_at(track, at).cloned();
    let upper = p.y < lane.center().y;
    let near_edge = |c: &Clip| -> Option<bool> {
        let xs = x_of(&s, tl, c.start);
        let xe = x_of(&s, tl, c.end());
        if (p.x - xs).abs() < 6.0 {
            Some(true)
        } else if (p.x - xe).abs() < 6.0 {
            Some(false)
        } else {
            None
        }
    };
    // Effective tool under the Smart tool.
    let eff = match tool {
        Tool::Smart => {
            if let Some(c) = &hit {
                if near_edge(c).is_some() {
                    Tool::Trim
                } else if upper {
                    Tool::Selector
                } else {
                    Tool::Grabber
                }
            } else {
                Tool::Selector
            }
        }
        other => other,
    };
    if resp.hovered() {
        let icon = match eff {
            Tool::Trim => egui::CursorIcon::ResizeHorizontal,
            Tool::Grabber => egui::CursorIcon::Grab,
            Tool::Selector => egui::CursorIcon::Text,
            Tool::Zoom => egui::CursorIcon::ZoomIn,
            Tool::Pencil => egui::CursorIcon::Crosshair,
            _ => egui::CursorIcon::Default,
        };
        ui.ctx().set_cursor_icon(icon);
    }
    let auto_view = AutoParam::parse(&track.view);
    let fade_corner = if tool == Tool::Smart {
        hit.as_ref().and_then(|c| {
            let xs = x_of(&s, tl, c.start);
            let xe = x_of(&s, tl, c.end());
            let top = p.y < lane.min.y + lane.height() * 0.28;
            if !top {
                None
            } else if (p.x - xs) > 0.0 && (p.x - xs) < 14.0 {
                Some(true)
            } else if (xe - p.x) > 0.0 && (xe - p.x) < 14.0 {
                Some(false)
            } else {
                None
            }
        })
    } else {
        None
    };
    if fade_corner.is_some() && resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeNwSe);
    }
    let gain_icon_at = |q: Pos2| {
        let c = clip_at(track, sample_at(&s, tl, q.x).max(0)).filter(|_| auto_view.is_none())?;
        clip_gain_icon(&s, tl, lane, c).filter(|r| r.contains(q)).map(|_| c.clone())
    };
    if resp.hovered() && gain_icon_at(p).is_some() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeVertical);
    }
    let press = ui.input(|i| i.pointer.press_origin()).unwrap_or(p);
    let gain_drag = if resp.drag_started() { gain_icon_at(press) } else { None };
    if let Some(c) = gain_drag {
        // `clip.gain` refuses to run with nothing selected.
        if s.edit.selected_tracks.is_empty() && s.edit.selected_clips.is_empty() {
            let _ = app.engine.execute("edit.select", &json!({"clips": [c.id.0]}));
        }
        let pos = crate::widgets::clip_gain_to_pos(c.gain_db);
        let screen = ui.ctx().content_rect();
        let y = (press.y + CLIP_FADER_TRAVEL * pos).max(screen.min.y + CLIP_FADER_TRAVEL + 30.0).min(screen.max.y - 10.0);
        let fader_bottom = pos2(press.x.max(screen.min.x + 26.0), y);
        let fine = mods.command || mods.ctrl;
        app.gesture = Some(Gesture::ClipGain { clip: c.id, track: track.id, pos, anchor: (press.y, pos), fine, fader_bottom });
    } else if resp.drag_started()
        && let (Some(fade_in), Some(c)) = (fade_corner, hit.as_ref())
    {
        app.gesture = Some(Gesture::Fade { clip: c.id, track: track.id, fade_in, to: at });
    } else if resp.drag_started() {
        app.gesture = match eff {
            Tool::Selector | Tool::Smart => Some(Gesture::Select { track: track.id, anchor: snap(&s, at), tracks: vec![track.id] }),
            Tool::Grabber => hit.as_ref().map(|c| {
                let clips = if s.edit.selected_clips.contains(&c.id) { s.edit.selected_clips.clone() } else { vec![c.id] };
                Gesture::MoveClips { clips, grab_at: at, delta: 0, from_track: track.id, to_track: track.id }
            }),
            Tool::Trim => hit.as_ref().map(|c| match near_edge(c) {
                Some(false) => Gesture::TrimEnd { clip: c.id, track: track.id, to: c.end() },
                _ if at - c.start < c.end() - at => Gesture::TrimStart { clip: c.id, track: track.id, to: c.start },
                _ => Gesture::TrimEnd { clip: c.id, track: track.id, to: c.end() },
            }),
            Tool::Scrubber => Some(Gesture::Scrub { last: at }),
            Tool::Pencil => Some(Gesture::Pencil { track: track.id, points: Vec::new() }),
            Tool::Zoom => Some(Gesture::ZoomBox { start: p.x }),
        };
    }
    if resp.dragged() {
        let to_track = app.edit_layout.rows.iter().find(|(_, r)| p.y >= r[1] && p.y <= r[3]).map(|(i, _)| TrackId(*i));
        match &mut app.gesture {
            Some(Gesture::Select { tracks, .. }) => {
                if let Some(t2) = to_track {
                    // Extend across the tracks between the anchor row and the pointer row.
                    let order: Vec<TrackId> = app.edit_layout.rows.iter().map(|(i, _)| TrackId(*i)).collect();
                    let a = order.iter().position(|x| *x == track.id).unwrap_or(0);
                    let b = order.iter().position(|x| *x == t2).unwrap_or(a);
                    let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
                    *tracks = order.get(lo..=hi).map(<[TrackId]>::to_vec).unwrap_or_default();
                }
            }
            Some(Gesture::MoveClips { grab_at, delta, to_track: tt, .. }) => {
                let raw = at - *grab_at;
                *delta = if s.edit.edit_mode.is_grid() {
                    snap(&s, hit.as_ref().map_or(*grab_at, |c| c.start) + raw) - hit.as_ref().map_or(*grab_at, |c| c.start)
                } else {
                    raw
                };
                if s.edit.edit_mode == soundcraft_model::EditMode::Spot {
                    *delta = 0;
                }
                if let Some(t2) = to_track {
                    *tt = t2;
                }
            }
            Some(Gesture::TrimStart { to, .. } | Gesture::TrimEnd { to, .. }) => *to = snap(&s, at),
            Some(Gesture::Fade { to, .. }) => *to = at,
            Some(Gesture::ClipGain { clip, pos, anchor, fine, .. }) => {
                // The fader follows the pointer from where it was pressed; ⌘ re-anchors for fine moves.
                if *fine != (mods.command || mods.ctrl) {
                    *fine = !*fine;
                    *anchor = (p.y, *pos);
                }
                let scale = if *fine { 0.1 } else { 1.0 };
                *pos = (anchor.1 + (anchor.0 - p.y) / CLIP_FADER_TRAVEL * scale).clamp(0.0, 1.0);
                let db = crate::widgets::clip_gain_from_pos(*pos);
                if s.find_clip(*clip).is_some_and(|(_, c)| (c.gain_db - db).abs() > 0.001) {
                    let _ = app.engine.execute_merged("clip.gain", &json!({"clips": [clip.0], "db": db}), &format!("clip_gain:{}", clip.0));
                }
            }
            Some(Gesture::Scrub { last }) => {
                *last = at;
                let _ = app.engine.execute("transport.locate", &json!({"at": at}));
            }
            Some(Gesture::Pencil { points, .. }) => {
                if let Some(param) = &auto_view {
                    let (lo, hi, _) = param.range();
                    let (lo, hi) = if matches!(param, AutoParam::Volume | AutoParam::SendLevel(_)) {
                        (-60.0, 12.0)
                    } else if matches!(param, AutoParam::Plugin { .. }) {
                        plugin_range(track, param)
                    } else {
                        (lo, hi)
                    };
                    let k = ((lane.max.y - 4.0 - p.y) / (lane.height() - 8.0)).clamp(0.0, 1.0);
                    let v = lo + (hi - lo) * k;
                    if points.last().is_none_or(|(a, _)| (at - a).abs() as f64 > s.edit.zoom.samples_per_px * 3.0) {
                        points.push((at, v));
                    }
                }
            }
            _ => {}
        }
        // Live selection feedback.
        if let Some(Gesture::Select { anchor, tracks, .. }) = &app.gesture {
            let r = Range::new(*anchor, snap(&s, at));
            let ids: Vec<u64> = tracks.iter().map(|x| x.0).collect();
            let _ = app.engine.execute("edit.select", &json!({"tracks": ids, "start": r.start, "end": r.end}));
        }
    }
    if let Some(Gesture::ClipGain { track: t, pos, fader_bottom, .. }) = &app.gesture
        && *t == track.id
        && resp.dragged()
    {
        ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeVertical);
        clip_gain_popup(ui.ctx(), *fader_bottom, *pos);
    }
    if resp.drag_stopped() {
        let g = app.gesture.take();
        match g {
            Some(Gesture::MoveClips { clips, delta, from_track, to_track, .. }) => {
                let ids: Vec<u64> = clips.iter().map(|c| c.0).collect();
                let mut params = json!({"clips": ids, "by": delta});
                if to_track != from_track {
                    params["track"] = json!(to_track.0);
                }
                if delta != 0 || to_track != from_track {
                    let _ = app.run("edit.move_clips", params);
                }
            }
            Some(Gesture::Fade { clip, track, fade_in, to }) => {
                if let Some((_, c)) = app.engine.session().find_clip(clip).map(|(t, c)| (t, c.clone())) {
                    let (a, b) = if fade_in { (c.start, to.clamp(c.start + 1, c.end())) } else { (to.clamp(c.start, c.end() - 1), c.end()) };
                    let _ = app.run("edit.fades_create", json!({"tracks": [track.0], "start": a, "end": b, "shape": "s-curve"}));
                }
            }
            Some(Gesture::TrimStart { clip, track, to }) => trim_clip(app, clip, track, to, true),
            Some(Gesture::TrimEnd { clip, track, to }) => trim_clip(app, clip, track, to, false),
            Some(Gesture::Pencil { track, points }) => {
                if let Some(param) = &auto_view {
                    let pid = automation_id(param);
                    for (a, v) in points {
                        let _ = app.run("automation.set_point", json!({"track": track.0, "param": pid, "at": a, "value": v}));
                    }
                }
            }
            Some(Gesture::ZoomBox { start }) => {
                let a = sample_at(&s, tl, start.min(p.x));
                let b = sample_at(&s, tl, start.max(p.x));
                if b > a {
                    let spp = ((b - a) as f64 / f64::from(tl.width())).max(0.25);
                    let _ = app.run("view.zoom_set", json!({"samples_per_px": spp}));
                    let _ = app.run("view.scroll", json!({"to": a}));
                }
            }
            _ => {}
        }
    }
    if resp.clicked() {
        match eff {
            Tool::Grabber => {
                if let Some(c) = &hit {
                    let mut ids = if mods.shift { s.edit.selected_clips.iter().map(|x| x.0).collect::<Vec<_>>() } else { Vec::new() };
                    ids.push(c.id.0);
                    let _ = app.run("edit.select", json!({"clips": ids}));
                }
            }
            Tool::Zoom => {
                let _ = app.run(if mods.alt { "view.zoom_out" } else { "view.zoom_in" }, json!({}));
            }
            Tool::Pencil => {
                if let Some(param) = &auto_view {
                    let (lo, hi, _) = param.range();
                    let (lo, hi) = if matches!(param, AutoParam::Volume | AutoParam::SendLevel(_)) { (-60.0, 12.0) } else { (lo, hi) };
                    let k = ((lane.max.y - 4.0 - p.y) / (lane.height() - 8.0)).clamp(0.0, 1.0);
                    let _ = app.run(
                        "automation.set_point",
                        json!({"track": track.id.0, "param": automation_id(param), "at": at, "value": lo + (hi - lo) * k}),
                    );
                }
            }
            _ => {
                let at = snap(&s, at);
                if mods.shift && s.edit.selected_tracks.contains(&track.id) {
                    let sel = s.edit.selection;
                    let r = if at < sel.start { Range::new(at, sel.end) } else { Range::new(sel.start, at) };
                    let _ = app.run("edit.select", json!({"start": r.start, "end": r.end}));
                } else {
                    let _ = app.run("edit.select", json!({"tracks": [track.id.0], "start": at}));
                }
            }
        }
    }
    if resp.double_clicked()
        && eff != Tool::Pencil
        && let Some(c) = &hit
    {
        let _ = app.run("edit.select", json!({"clips": [c.id.0]}));
    }
}

fn automation_id(p: &AutoParam) -> String {
    match p {
        AutoParam::Volume => "volume".into(),
        AutoParam::Pan(0) => "pan".into(),
        AutoParam::Pan(_) => "pan2".into(),
        AutoParam::Mute => "mute".into(),
        AutoParam::SendLevel(i) => format!("send_{}_level", (b'a' + i) as char),
        AutoParam::SendPan(i) => format!("send_{}_pan", (b'a' + i) as char),
        AutoParam::SendMute(i) => format!("send_{}_mute", (b'a' + i) as char),
        AutoParam::Plugin { slot, param } => format!("plugin:{}:{param}", (b'a' + slot) as char),
    }
}

fn trim_clip(app: &mut SoundApp, clip: ClipId, track: TrackId, to: Samples, start: bool) {
    let cmd = if start { "edit.trim_start_to_insertion" } else { "edit.trim_end_to_insertion" };
    // Trims through the command layer: select the clip's track and position the insertion.
    let sel_before = app.engine.session().edit.selection;
    let tracks_before: Vec<u64> = app.engine.session().edit.selected_tracks.iter().map(|x| x.0).collect();
    let c = app.engine.session().find_clip(clip).map(|(_, c)| c.clone());
    if let Some(c) = c {
        if start && to < c.start || !start && to > c.end() {
            // Extending: use trim to fill selection with handles.
            let r = if start { Range::new(to, c.end()) } else { Range::new(c.start, to) };
            let _ = app.run("edit.trim_to_fill_selection", json!({"clips": [clip.0], "start": r.start, "end": r.end}));
        } else {
            let _ = app.run(cmd, json!({"tracks": [track.0], "at": to}));
        }
    }
    let _ = app.engine.execute("edit.select", &json!({"tracks": tracks_before, "start": sel_before.start, "end": sel_before.end}));
}

fn clip_context_menu(app: &mut SoundApp, ui: &mut Ui, clip: Option<ClipId>) {
    let Some(c) = clip else {
        if ui.button("Paste").clicked() {
            let _ = app.run("edit.paste", json!({}));
        }
        return;
    };
    let ids = json!([c.0]);
    for (label, cmd) in [
        ("Mute/Unmute", "edit.mute_clips"),
        ("Edit Lock/Unlock", "clip.edit_lock"),
        ("Time Lock/Unlock", "clip.time_lock"),
        ("Quantize to Grid", "clip.quantize_to_grid"),
        ("Render Clip Gain", "clip.gain_render"),
        ("Bring to Front", "clip.bring_to_front"),
        ("Send to Back", "clip.send_to_back"),
    ] {
        if ui.button(label).clicked() {
            let _ = app.run(cmd, json!({"clips": ids}));
        }
    }
    ui.separator();
    if ui.button("Rename...").clicked() {
        let name = app.engine.session().find_clip(c).map(|(_, x)| x.name.clone()).unwrap_or_default();
        app.dialogs.open_rename_clip(c, &name);
    }
    if ui.button("Delete").clicked() {
        let _ = app.run("edit.select", json!({"clips": ids}));
        let _ = app.run("edit.clear", json!({}));
    }
}

/// Next follow state after observing one frame: `(hold, follow_to)`.
///
/// The view is ours to move only while nothing else moved it: when the
/// session scroll differs from both the last seen value and our own last
/// follow target, something else (a manual pan, scrollbar, universe jump,
/// or programmatic scroll) took over, so the follow pauses (`hold`) and no
/// jump is issued. Stopping, or scrolling back to the playhead, clears the
/// hold. Only `page`/`continuous`/`center` ever issue follow jumps while
/// playing; `none`/`after_playback` leave the view alone.
fn follow_update(
    playing: bool,
    mode_follow: bool,
    playhead_visible: bool,
    cur: Samples,
    last_scroll: Samples,
    last_follow_to: Option<Samples>,
    hold: bool,
    pos: Samples,
) -> (bool, Option<Samples>) {
    if !playing || playhead_visible {
        return (false, None);
    }
    if cur != last_scroll && Some(cur) != last_follow_to {
        return (mode_follow, None);
    }
    if hold || !mode_follow {
        return (hold, None);
    }
    (false, Some(pos))
}

/// Selection overlay, insertion point and playhead across rulers and tracks.
/// While playing, auto-scroll keeps the playhead in view for the follow
/// modes (`page`/`continuous`/`center`) — but any outside view move (a manual
/// pan, scrollbar, universe jump, or programmatic scroll) pauses it, so the
/// timeline can be inspected anywhere during playback. Scrolling back to the
/// playhead, stopping/starting, or re-selecting a scrolling mode resumes the
/// follow. `none` and `after_playback` never scroll during playback.
fn overlay(app: &mut SoundApp, ui: &mut Ui, tl: Rect, area: Rect) {
    let t = Tokens::current();
    let mut scroll_to: Option<Samples> = None;
    let s = app.engine.session();
    let painter = ui.painter().with_clip_rect(area);
    if s.edit.flag("view.marker.ruler_lines") {
        for m in &s.markers {
            let x = x_of(s, tl, m.start);
            if x > tl.min.x && x < tl.max.x {
                painter.line_segment(
                    [pos2(x, tl.min.y), pos2(x, tl.max.y)],
                    Stroke::new(1.0, m.color.map_or(Color32::from_rgba_unmultiplied(232, 200, 64, 70), |c| rgb(c).gamma_multiply(0.4))),
                );
            }
        }
    }
    let sel = s.edit.selection;
    // Timeline selection markers on the main ruler.
    if !sel.is_empty() {
        for x in [x_of(s, tl, sel.start), x_of(s, tl, sel.end)] {
            painter.add(Shape::convex_polygon(
                vec![pos2(x - 5.0, area.min.y + 2.0), pos2(x + 5.0, area.min.y + 2.0), pos2(x, area.min.y + 9.0)],
                t.accent,
                Stroke::NONE,
            ));
        }
    } else if !app.is_playing() {
        let x = x_of(s, tl, sel.start);
        let blink = (ui.input(|i| i.time) * 2.0) as i64 % 2 == 0;
        if blink {
            for (_, r) in &app.edit_layout.rows {
                if s.edit.selected_tracks.iter().any(|_| true) {
                    painter.line_segment([pos2(x, r[1].max(tl.min.y)), pos2(x, r[3])], Stroke::new(1.0, t.insertion));
                }
            }
        }
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(500));
    }
    if app.is_playing() {
        let x = x_of(s, tl, app.position());
        painter.line_segment([pos2(x, area.min.y), pos2(x, area.max.y)], Stroke::new(1.5, t.playhead));
        // Session reads first (owned copies), then the layout mutation: the
        // two borrow disjoint fields.
        let pos = app.position();
        let cur = s.edit.zoom.scroll;
        let mode_follow = matches!(s.edit.scrolling.as_str(), "page" | "continuous" | "center");
        let visible = x >= tl.min.x && x <= tl.max.x;
        let layout = &mut app.edit_layout;
        let (hold, follow_to) = follow_update(true, mode_follow, visible, cur, layout.last_scroll, layout.last_follow_to, layout.follow_hold, pos);
        layout.follow_hold = hold;
        layout.last_scroll = cur;
        layout.last_follow_to = follow_to;
        scroll_to = follow_to;
    }
    // Ghost of a clip move.
    if let Some(Gesture::MoveClips { clips, delta, to_track, .. }) = &app.gesture {
        let row = app.edit_layout.rows.iter().find(|(i, _)| *i == to_track.0).map(|(_, r)| *r);
        for c in clips {
            if let (Some((_, clip)), Some(r)) = (s.find_clip(*c), row) {
                let x0 = x_of(s, tl, clip.start + delta);
                let x1 = x_of(s, tl, clip.end() + delta);
                painter.rect_stroke(
                    Rect::from_min_max(pos2(x0, r[1] + 1.0), pos2(x1, r[3] - 1.0)),
                    2.0,
                    Stroke::new(2.0, Color32::WHITE),
                    StrokeKind::Inside,
                );
            }
        }
    }
    if let Some(Gesture::TrimStart { to, clip, .. } | Gesture::TrimEnd { to, clip, .. }) = &app.gesture
        && let Some((tid, _)) = s.find_clip(*clip)
        && let Some((_, r)) = app.edit_layout.rows.iter().find(|(i, _)| *i == tid.0)
    {
        let x = x_of(s, tl, *to);
        painter.line_segment([pos2(x, r[1]), pos2(x, r[3])], Stroke::new(2.0, Color32::WHITE));
    }
    if let Some(Gesture::Fade { clip, fade_in, to, .. }) = &app.gesture
        && let Some((tid, c)) = s.find_clip(*clip)
        && let Some((_, r)) = app.edit_layout.rows.iter().find(|(i, _)| *i == tid.0)
    {
        let (a, b) = if *fade_in { (c.start, *to) } else { (*to, c.end()) };
        let (x0, x1) = (x_of(s, tl, a), x_of(s, tl, b));
        let (y0, y1) = (r[1] + 2.0, r[3] - 2.0);
        let line = if *fade_in { [pos2(x0, y1), pos2(x1, y0)] } else { [pos2(x0, y0), pos2(x1, y1)] };
        painter.line_segment(line, Stroke::new(2.0, Color32::WHITE));
    }
    if let Some(at) = scroll_to {
        let _ = app.engine.execute("view.scroll", &json!({"to": at}));
    }
}

#[cfg(test)]
mod tests {
    use super::follow_update;

    #[test]
    fn follow_jumps_to_offscreen_playhead_while_playing() {
        // Steady view, follow mode, playhead left the screen: jump to it.
        assert_eq!(follow_update(true, true, false, 1_000, 1_000, None, false, 96_000), (false, Some(96_000)));
    }

    #[test]
    fn follow_leaves_onscreen_playhead_alone() {
        assert_eq!(follow_update(true, true, true, 1_000, 1_000, None, false, 96_000), (false, None));
    }

    #[test]
    fn follow_never_scrolls_when_stopped_nor_in_free_modes() {
        assert_eq!(follow_update(false, true, false, 1_000, 1_000, None, false, 96_000), (false, None));
        // `none`/`after_playback` never follow, even with a steady view.
        assert_eq!(follow_update(true, false, false, 1_000, 1_000, None, false, 96_000), (false, None));
        assert_eq!(follow_update(true, false, false, 9_000, 1_000, None, false, 96_000), (false, None));
    }

    #[test]
    fn outside_view_move_pauses_the_follow() {
        // The view moved and it was not our own jump: pause, no jump.
        assert_eq!(follow_update(true, true, false, 9_000, 1_000, None, false, 96_000), (true, None));
        // A held view stays held while it sits still elsewhere.
        assert_eq!(follow_update(true, true, false, 9_000, 9_000, None, true, 96_000), (true, None));
    }

    #[test]
    fn own_follow_jump_is_not_mistaken_for_a_takeover() {
        // The scroll equals the target we issued last frame: keep following.
        assert_eq!(follow_update(true, true, false, 96_000, 1_000, Some(96_000), false, 97_000), (false, Some(97_000)));
    }

    #[test]
    fn hold_clears_when_playhead_returns_or_transport_stops() {
        assert_eq!(follow_update(true, true, true, 9_000, 9_000, None, true, 96_000), (false, None));
        assert_eq!(follow_update(false, true, false, 9_000, 9_000, None, true, 96_000), (false, None));
    }
}
