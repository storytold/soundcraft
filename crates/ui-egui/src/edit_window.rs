//! The Edit window: toolbar, rulers, track headers and playlists.

use crate::dialogs::Dialog;
use crate::i18n::{tr, trf};
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

pub const HEADER_W: f32 = 136.0;
pub const COLUMN_W: f32 = 112.0;
/// Insert, send and instrument columns: Pro Tools' narrow stacks of slots.
const PLUGIN_COLUMN_W: f32 = 75.0;
/// The track colour strip at the left of each track header.
const STRIP_W: f32 = 14.0;
/// The row under the rulers that holds the column titles, when any column is shown.
const COLUMN_TITLES_H: f32 = 16.0;

/// An optional Edit-window column: view id, menu label, title and width.
type Column = (&'static str, &'static str, &'static str, f32);

const COLUMNS: [Column; 7] = [
    ("instruments", "Instruments", "INSTRUMENT", PLUGIN_COLUMN_W),
    ("inserts_ae", "Inserts A-E", "INSERTS A-E", PLUGIN_COLUMN_W),
    ("inserts_fj", "Inserts F-J", "INSERTS F-J", PLUGIN_COLUMN_W),
    ("sends_ae", "Sends A-E", "SENDS A-E", PLUGIN_COLUMN_W),
    ("sends_fj", "Sends F-J", "SENDS F-J", PLUGIN_COLUMN_W),
    ("io", "I/O", "I/O", 94.0),
    ("comments", "Comments", "COMMENTS", COLUMN_W),
];

/// Optional Edit-window columns (View › Edit Window Views) that are switched on, left to right.
pub fn edit_columns(s: &Session) -> Vec<Column> {
    let all = s.edit.flag("edit_view.all");
    COLUMNS.into_iter().filter(|c| all || s.edit.flag(&format!("edit_view.{}", c.0))).collect()
}

pub fn header_width(cols: &[Column]) -> f32 {
    HEADER_W + cols.iter().map(|c| c.3).sum::<f32>()
}
pub const RULER_H: f32 = 17.0;

/// Geometry of the last frame, for hit testing and agents (`ui.inspect`).
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct EditLayout {
    pub timeline: [f32; 4],
    pub rows: Vec<(u64, [f32; 4])>,
    pub scroll_y: f32,
    pub content_h: f32,
    /// While Zoom Toggle is on: the vertical scroll to go back to.
    zoom_back: Option<f32>,
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
    #[serde(skip)]
    pub scroll_carry: f64,
    /// The current swipe's movement per axis until either passes 6 points; it scrolls only along the larger.
    #[serde(skip)]
    swipe: egui::Vec2,
}

impl EditLayout {
    pub fn scroll_by(&mut self, dy: f32, view_h: f32) {
        self.scroll_y = (self.scroll_y + dy).clamp(0.0, (self.content_h - view_h + 40.0).max(0.0));
    }
}

pub fn rect([x0, y0, x1, y1]: [f32; 4]) -> Rect {
    Rect::from_min_max(pos2(x0, y0), pos2(x1, y1))
}

fn lane_rect(track: &Track, row: Rect, tl: Rect) -> Rect {
    Rect::from_min_max(pos2(tl.min.x, row.min.y), pos2(tl.max.x, row.min.y + track.height.points()))
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
    drive_drag(app, ui.ctx());
    egui::Panel::top("edit_toolbar").exact_size(toolbar::HEIGHT).frame(egui::Frame::NONE.fill(t.toolbar_bg)).show(ui, |ui| toolbar::show(app, ui));
    egui::Panel::bottom("edit_status").exact_size(22.0).frame(egui::Frame::NONE.fill(t.toolbar_bg)).show(ui, |ui| status_bar(app, ui));
    egui::Panel::bottom("lower_dock_tabs").exact_size(22.0).frame(egui::Frame::NONE.fill(t.panel_bg2)).show(ui, |ui| dock_tabs(app, ui));
    if app.ui.show_universe {
        egui::Panel::top("universe").exact_size(64.0).frame(egui::Frame::NONE.fill(Tokens::current().universe_bg)).show(ui, |ui| universe(app, ui));
    }
    crate::docking::show(app, ui, crate::MainWindow::Edit);
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
        (crate::i18n::tr("MIDI EDITOR"), app.ui.show_midi_editor, "window.midi_editor"),
        (crate::i18n::tr("MEMORY LOCATIONS"), app.ui.show_memory_locations, "window.memory_locations"),
        (crate::i18n::tr("UNDO HISTORY"), app.ui.show_undo_history, "window.undo_history"),
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
    let mut left = trf(
        "{} · {} Hz · {} · {} tracks · {}",
        &[
            &s.name,
            &s.sample_rate.hz(),
            &tr(match s.bit_depth {
                soundcraft_model::BitDepthSetting::Int16 => "16-bit",
                soundcraft_model::BitDepthSetting::Int24 => "24-bit",
                soundcraft_model::BitDepthSetting::Float32 => "32-bit float",
            }),
            &s.tracks.len(),
            &app.ui.status,
        ],
    );
    soundcraft_model::truncate_chars(&mut left, 4 * soundcraft_model::MAX_NAME_CHARS);
    ui.painter().text(pos2(r.min.x + 10.0, r.center().y), Align2::LEFT_CENTER, left, regular(11.0), t.text_dim);
    let undo = app.engine.undo_label().map_or(String::new(), |l| crate::i18n::trf("Undo {}", &[&tr(l)]));
    ui.painter().text(
        pos2(r.max.x - 10.0, r.center().y),
        Align2::RIGHT_CENTER,
        if app.frame_ms < 50.0 { crate::i18n::trf("{}   {} ms/frame", &[&undo, &format!("{:.1}", app.frame_ms)]) } else { undo },
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
        let resp = ui.interact(empty, ui.id().with("empty"), Sense::click());
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
        let resp = ui.interact(vs, ui.id().with("vscroll"), Sense::drag());
        if resp.dragged() {
            app.edit_layout.scroll_by(resp.drag_delta().y / frac, tl.height());
        }
    }
}

fn in_edit_window(s: &Session, t: &Track) -> bool {
    ops::shown(s, t)
}

/// Pro Tools' rule for track height: Alt reaches every shown track, Alt+Shift the selected ones too.
pub(crate) fn height_targets(s: &Session, m: egui::Modifiers, own: &[TrackId]) -> Vec<u64> {
    let extra = |t: &Track| if m.shift { s.edit.selected_tracks.contains(&t.id) } else { in_edit_window(s, t) };
    s.tracks.iter().filter(|t| own.contains(&t.id) || (m.alt && extra(t))).map(|t| t.id.0).collect()
}

/// How many folders this track sits in, for the name's indent (at most 6 shown).
fn folder_depth(s: &Session, track: &Track) -> usize {
    let mut current = track.folder;
    let mut depth = 0;
    while let Some(folder) = current.and_then(|folder| s.track(folder))
        && depth < 6
    {
        depth += 1;
        current = folder.folder;
    }
    depth
}

/// Pro Tools' track-name click: Shift adds the tracks up to the nearest selected one, Cmd/Ctrl adds or removes one.
pub(crate) fn click_track(app: &mut SoundApp, id: TrackId, mods: egui::Modifiers, shown: impl Fn(&Session, &Track) -> bool) {
    let s = app.engine.session();
    let order: Vec<TrackId> = s.tracks.iter().filter(|t| shown(s, t)).map(|t| t.id).collect();
    let row = |t: &TrackId| order.iter().position(|o| o == t);
    let mut tracks = s.edit.selected_tracks.clone();
    if mods.command {
        if tracks.contains(&id) {
            tracks.retain(|t| *t != id);
        } else {
            tracks.push(id);
        }
    } else if mods.shift
        && let Some(a) = row(&id)
        && let Some(b) = tracks.iter().filter_map(row).min_by_key(|b| b.abs_diff(a))
    {
        let span = order.get(a.min(b)..=a.max(b)).unwrap_or_default();
        tracks.extend(span.iter().filter(|t| !tracks.contains(t)).copied().collect::<Vec<_>>());
    } else {
        tracks = vec![id];
    }
    let ids: Vec<u64> = tracks.iter().map(|t| t.0).collect();
    let _ = app.run("edit.select", json!({"tracks": ids}));
}

fn scroll_and_zoom(app: &mut SoundApp, ui: &Ui, area: Rect, tl: Rect) {
    let (delta, zoom, pointer, scrolling) = ui.input(|i| (i.smooth_scroll_delta, i.zoom_delta(), i.pointer.hover_pos(), i.is_scrolling()));
    let mut swipe = if scrolling { app.edit_layout.swipe } else { egui::Vec2::ZERO };
    if swipe.max_elem() < 6.0 {
        swipe += delta.abs();
    }
    app.edit_layout.swipe = swipe;
    let Some(p) = pointer.filter(|_| tl.is_positive() && ui.rect_contains_pointer(area)) else { return };
    if zoom != 1.0 {
        let _ = app.engine.execute("view.zoom_by", &json!({"factor": zoom, "x": p.x.clamp(tl.min.x, tl.max.x) - tl.min.x}));
    }
    let horizontal = swipe.x > swipe.y;
    if horizontal && delta.x != 0.0 {
        pan(app, -f64::from(delta.x));
    }
    if !horizontal && delta.y != 0.0 {
        app.edit_layout.scroll_by(-delta.y, tl.height());
    }
}

fn pan(app: &mut SoundApp, px: f64) {
    let z = app.engine.session().edit.zoom;
    let want = (z.scroll as f64 + app.edit_layout.scroll_carry + px * z.samples_per_px).clamp(0.0, soundcraft_engine::cmd::MAX_POSITION as f64);
    if want.is_nan() {
        return;
    }
    let to = soundcraft_time::to_samples(want);
    app.edit_layout.scroll_carry = want - to as f64;
    if to != z.scroll {
        let _ = app.engine.execute("view.scroll", &json!({"to": to}));
    }
}

fn hscrollbar(app: &mut SoundApp, ui: &mut Ui, sb: Rect, tl: Rect) {
    let t = Tokens::current();
    ui.painter().rect_filled(sb, 0.0, t.panel_bg);
    let s = app.engine.session();
    let vis = f64::from(tl.width()) * s.edit.zoom.samples_per_px;
    let total = (s.content_end().saturating_add(s.sample_rate.samples(30.0)).max(1) as f64).max(s.edit.zoom.scroll as f64 + vis);
    let frac = (vis / total).clamp(0.02, 1.0) as f32;
    let off = (s.edit.zoom.scroll as f64 / total).clamp(0.0, 1.0) as f32;
    let thumb = Rect::from_min_size(pos2(sb.min.x + sb.width() * off, sb.min.y + 3.0), vec2((sb.width() * frac).max(24.0), sb.height() - 6.0));
    ui.painter().rect_filled(thumb, 3.0, t.button_hi);
    let resp = ui.interact(sb, ui.id().with("hscroll"), Sense::drag());
    if resp.dragged() {
        let dx = resp.drag_delta().x / sb.width();
        let by_px = f64::from(dx) * total / s.edit.zoom.samples_per_px;
        pan(app, by_px);
    }
}

/// Zoom Toggle sized to the track area on screen.
pub fn zoom_toggle(app: &mut SoundApp) {
    let [x0, y0, x1, y1] = app.edit_layout.timeline;
    let _ = app.run("view.zoom_toggle", json!({"width_px": x1 - x0, "height_px": y1 - y0}));
}

/// Zoom Toggle scrolls the first track it fitted to the top, and back when it turns off.
fn follow_zoom_toggle(app: &mut SoundApp) {
    let l = &mut app.edit_layout;
    match (&app.engine.session().edit.zoom_toggle, l.zoom_back) {
        (Some(z), None) => {
            l.zoom_back = Some(l.scroll_y);
            let content_top = l.rows.first().map_or(0.0, |(_, r)| r[1]);
            if let Some((_, r)) = l.rows.iter().find(|(id, _)| z.heights.iter().any(|(t, _)| t.0 == *id)) {
                l.scroll_y = r[1] - content_top;
            }
        }
        (None, Some(y)) => (l.scroll_y, l.zoom_back) = (y, None),
        _ => {}
    }
}

// ---- rulers ----------------------------------------------------------------------------------

fn ruler_label(id: &str) -> &'static str {
    match id {
        "bars_beats" => tr("Bars|Beats"),
        "min_secs" => tr("Min:Secs"),
        "timecode" => tr("Timecode"),
        "feet_frames" => tr("Feet+Frames"),
        "samples" => tr("Samples"),
        "tempo" => tr("Tempo"),
        "meter" => tr("Meter"),
        "markers" => tr("Markers"),
        "key" => tr("Key"),
        "chords" => tr("Chords"),
        "timecode2" => tr("Timecode 2"),
        "markers2" => tr("Markers 2"),
        "markers3" => tr("Markers 3"),
        "markers4" => tr("Markers 4"),
        "markers5" => tr("Markers 5"),
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
    let s = app.engine.session_arc();
    ui.painter().rect_filled(area, 0.0, t.ruler_bg);
    let view = Range::new(sample_at(&s, tl, tl.min.x), sample_at(&s, tl, tl.max.x));
    let sr = s.sample_rate;
    let mut y0 = area.min.y + 3.0;
    for (i, id) in rulers.iter().enumerate() {
        let h = ruler_height(id);
        let row = Rect::from_min_max(pos2(tl.min.x, y0), pos2(tl.max.x, y0 + h));
        let lab = Rect::from_min_max(pos2(area.min.x, y0), pos2(area.min.x + HEADER_W, y0 + h));
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
                    painter.text(
                        pos2(tl.min.x + 6.0, row.center().y),
                        Align2::LEFT_CENTER,
                        tr("C major"),
                        bold(11.0),
                        Color32::from_rgb(220, 214, 236),
                    );
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
                timebase_ruler(ui, &painter, &s, tl, row, view, TimeFormat::Timecode, alt, i + 1 == rulers.len());
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
                // Click on the marker ruler: recall a marker; double-click renames it, or adds one.
                let resp = ui.interact(row, ui.id().with(("marker_ruler", lane)), Sense::click());
                if let Some(p) = resp.interact_pointer_pos() {
                    let hit = s
                        .markers
                        .iter()
                        .filter(|m| m.ruler == lane || (lane == 1 && m.ruler == 0))
                        .find(|m| (x_of(&s, tl, m.start) - p.x).abs() < 8.0);
                    if resp.double_clicked() {
                        match hit {
                            Some(m) => app.dialogs.open_rename_marker(m.id, &m.name),
                            None => {
                                let _ = app.run("markers.add", json!({"at": sample_at(&s, tl, p.x).max(0), "ruler": lane}));
                            }
                        }
                    } else if resp.clicked()
                        && let Some(m) = hit
                    {
                        let _ = app.run("markers.recall", json!({"number": m.number}));
                    }
                }
            }
            _ => {
                if let Some(f) = fmt {
                    timebase_ruler(ui, &painter, &s, tl, row, view, f, s.frame_rate, i + 1 == rulers.len());
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

fn timebase_ruler(
    _ui: &Ui,
    painter: &egui::Painter,
    s: &Session,
    tl: Rect,
    row: Rect,
    view: Range,
    f: TimeFormat,
    rate: soundcraft_time::FrameRate,
    bottom: bool,
) {
    let t = Tokens::current();
    let sr = s.sample_rate;
    let spp = s.edit.zoom.samples_per_px;
    let mut marks: Vec<(Samples, bool)> = Vec::new();
    match f {
        TimeFormat::BarsBeats => {
            // Bars every n so labels are ≥ 60 px apart.
            let bar_px = {
                let b = GridValue::Note { value: NoteValue::Bar, dotted: false, triplet: false };
                b.step_samples(view.start.max(0), sr, &s.tempo, rate) as f64 / spp
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
                        format_position(at, f, sr, &s.tempo, rate, s.timecode_start, s.feet_frames_start())
                    }
                }
                _ => format_position(at, f, sr, &s.tempo, rate, s.timecode_start, s.feet_frames_start()),
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

/// Captions over the header columns, boxed in their own row under the rulers.
fn column_titles(ui: &Ui, rulers: Rect, cols: &[Column]) {
    if cols.is_empty() {
        return;
    }
    let t = Tokens::current();
    let line = Stroke::new(1.0, t.border);
    let mut x = rulers.min.x + HEADER_W;
    let end = rulers.min.x + header_width(cols);
    let row = Rect::from_min_max(pos2(x, rulers.max.y - COLUMN_TITLES_H - 3.0), pos2(end, rulers.max.y));
    ui.painter().rect_filled(row, 0.0, t.panel_bg2);
    ui.painter().hline(rulers.min.x..=end, row.min.y + 0.5, line);
    ui.painter().hline(rulers.min.x..=end, row.max.y - 0.5, line);
    ui.painter().vline(end - 0.5, row.y_range(), line);
    for &(_, _, title, w) in cols {
        ui.painter().vline(x + 0.5, row.y_range(), line);
        ui.painter().text(pos2(x + w / 2.0, row.center().y), Align2::CENTER_CENTER, tr(title), bold(10.0), t.section_label);
        x += w;
    }
}

/// The Edit window view selector: which columns sit beside the track headers.
fn views_button(app: &mut SoundApp, ui: &mut Ui, rulers: Rect) {
    let r = Rect::from_min_size(rulers.min + vec2(6.0, 5.0), vec2(50.0, 18.0));
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(r));
    let resp = crate::widgets::selector_box(&mut child, r.width(), r.height(), tr("Views"), Tokens::current().text);
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::ComboBox, child.is_enabled(), tr("Views")));
    let resp = resp.on_hover_text(tr("Columns beside the track headers: inserts, sends, I/O…"));
    egui::Popup::menu(&resp).close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside).show(|ui| {
        let shown = edit_columns(app.engine.session());
        for (id, label, _, _) in COLUMNS {
            if ui.add(egui::Button::new(tr(label)).selected(shown.iter().any(|c| c.0 == id))).clicked() {
                let _ = app.run(&format!("view.edit_{id}"), json!({}));
            }
        }
        ui.separator();
        if ui.button(tr("All")).clicked() {
            let _ = app.run("view.edit_all", json!({}));
        }
        if ui.button(tr("Minimal")).clicked() {
            let _ = app.run("view.edit_minimal", json!({}));
        }
    });
}

// ---- track rows ------------------------------------------------------------------------------

fn track_row(app: &mut SoundApp, ui: &mut Ui, id: TrackId, row: Rect, tl: Rect, cols: &[Column]) {
    let t = Tokens::current();
    let s = app.engine.session_arc();
    let Some(track) = s.track(id) else { return };
    let selected = s.edit.selected_tracks.contains(&id);
    let head = Rect::from_min_max(row.min, pos2(row.min.x + HEADER_W, row.max.y));
    let (head_bg, lane_bg) = crate::theme::track_backgrounds(track.color, selected, app.ui.track_tint);
    let mut x = head.max.x;
    for &(col, _, _, w) in cols {
        header_column(app, ui, track, col, Rect::from_min_size(pos2(x, row.min.y), vec2(w, row.height())), head_bg);
        x += w;
    }
    let main_h = track.height.points();
    let lane = lane_rect(track, row, tl);
    ui.painter().rect_filled(head, 0.0, head_bg);
    ui.painter().rect_filled(Rect::from_min_size(head.min, vec2(STRIP_W, head.height())), 0.0, rgb(track.color));
    ui.painter().line_segment([pos2(head.min.x, row.max.y + 0.5), pos2(tl.max.x, row.max.y + 0.5)], Stroke::new(1.0, t.border));
    ui.painter().hline(head.min.x + STRIP_W..=head.max.x, head.min.y + 0.5, Stroke::new(1.0, t.edge_light));
    track_header(app, ui, track, head, selected, head_bg);
    // Lane.
    ui.painter().vline(tl.min.x - 0.5, row.y_range(), Stroke::new(1.0, t.border));
    let painter = ui.painter().with_clip_rect(lane.intersect(ui.clip_rect()));
    painter.rect_filled(lane, 0.0, lane_bg);
    let s = app.engine.session_arc();
    // Shown grid lines go over the clips; otherwise the bar lines stay behind them.
    if !s.edit.grid_lines {
        grid_lines(&painter, &s, tl, lane);
    }
    let view = Range::new(sample_at(&s, tl, tl.min.x), sample_at(&s, tl, tl.max.x));
    let auto_view = AutoParam::parse(&track.view);
    for clip in track.clips() {
        if !clip.range().overlaps(&view) {
            continue;
        }
        draw_clip(app, &painter, &s, track, clip, tl, lane, auto_view.is_some());
    }
    draw_live_take(app, &painter, &s, track, tl, lane);
    crate::video_track::draw_lane(app, &painter, &s, track, tl, lane);
    if s.edit.grid_lines {
        grid_lines(&painter, &s, tl, lane);
    }
    if let Some(p) = &auto_view {
        draw_automation(&painter, &s, track, p, tl, lane);
    }
    if track.kind == TrackKind::Master || track.kind == TrackKind::Aux || track.kind == TrackKind::Vca {
        let p = auto_view.clone().unwrap_or(AutoParam::Volume);
        draw_automation(&painter, &s, track, &p, tl, lane);
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
    lane_interaction(app, ui, track, lane, tl);
    if track.view == "playlists" {
        playlist_lanes(app, ui, track, row, main_h, tl);
    }
}

/// Alternate playlists as lanes under the main one; clicking a clip promotes it (comping).
fn playlist_lanes(app: &mut SoundApp, ui: &mut Ui, track: &Track, row: Rect, main_h: f32, tl: Rect) {
    let t = Tokens::current();
    let s = app.engine.session_arc();
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
            draw_clip(app, &painter, &s, track, c, tl, lane, false);
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
        resp.on_hover_text(tr("Click a take (or the selection) to promote it to the main playlist"));
        y += LANE_H;
    }
}

fn track_header(app: &mut SoundApp, ui: &mut Ui, track: &Track, head: Rect, selected: bool, head_bg: Color32) {
    let t = Tokens::current();
    let id = track.id;
    // Drag the bottom edge to resize (snaps to the height presets).
    let edge = Rect::from_min_max(pos2(head.min.x, head.max.y - 4.0), pos2(head.max.x, head.max.y + 2.0));
    let eresp = ui.interact(edge, ui.id().with(("hedge", id.0)), Sense::drag());
    if eresp.hovered() || eresp.dragged() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeVertical);
    }
    if eresp.drag_started() {
        ui.data_mut(|d| d.insert_temp(eresp.id, track.height));
    }
    if eresp.dragged()
        && let Some(p) = ui.ctx().pointer_latest_pos()
        && let Some(origin) = ui.input(|i| i.pointer.press_origin()).filter(|o| o.y != p.y)
    {
        let start = ui.data(|d| d.get_temp::<TrackHeight>(eresp.id)).unwrap_or(track.height);
        let want = start.points() + p.y - origin.y;
        let m = ui.input(|i| i.modifiers);
        let best = if m.command {
            TrackHeight::from_points(want)
        } else {
            TrackHeight::ALL.into_iter().min_by_key(|h| ((h.points() - want).abs() * 10.0) as i64).unwrap_or(start)
        };
        let s = app.engine.session();
        let tracks = height_targets(s, m, &[id]);
        if tracks.iter().any(|t| s.track(TrackId(*t)).is_some_and(|tr| tr.height != best)) {
            let _ = app.engine.execute_merged("track.height", &json!({"tracks": tracks, "height": best.points()}), &format!("track_height:{}", id.0));
            ui.ctx().request_repaint();
        }
    }
    let h = head.height();
    // Click on header background selects the track.
    let bg = ui.interact(head, ui.id().with(("hdr", id.0)), Sense::click());
    if bg.clicked() {
        click_track(app, id, ui.input(|i| i.modifiers), in_edit_window);
    }
    bg.context_menu(|ui| track_context_menu(app, ui, id));
    let at = |x: f32, y: f32, w: f32, h: f32| Rect::from_min_size(pos2(head.min.x + x, head.min.y + y), vec2(w, h));
    let strip = rgb(track.color);
    let ink = shade(strip, -0.8);
    // The strip's round button opens a folder, or any other track's options.
    let round = at(2.0, 2.0, 10.0, 10.0);
    let rresp = ui.interact(round, ui.id().with(("options", id.0)), Sense::click());
    ui.painter().circle_filled(round.center(), 5.0, shade(strip, if rresp.hovered() { 0.6 } else { 0.45 }));
    arrow(ui.painter(), round.center(), !track.is_folder() || track.folder_open, ink);
    if track.is_folder() {
        if rresp.on_hover_text(tr(if track.folder_open { "Close folder" } else { "Open folder" })).clicked() {
            let _ = app.run("track.folder_toggle", json!({"track": id.0}));
        }
    } else {
        egui::Popup::menu(&rresp.on_hover_text(tr("Track options"))).show(|ui| track_context_menu(app, ui, id));
    }
    if h >= 40.0 && track.kind.has_playlist() {
        let ar = at(3.0, h - 12.0, 9.0, 9.0);
        let aresp = ui.interact(ar, ui.id().with(("auto_lane", id.0)), Sense::click());
        let showing = AutoParam::parse(&track.view).is_some();
        ui.painter().rect_filled(ar, 1.5, shade(strip, if aresp.hovered() || showing { 0.6 } else { 0.45 }));
        let (a, b) = (ar.min + vec2(2.5, 2.5), ar.max - vec2(2.5, 2.5));
        ui.painter().line_segment([a, b], Stroke::new(1.0, ink));
        ui.painter().circle_filled(a, 1.3, ink);
        ui.painter().circle_filled(b, 1.3, ink);
        if aresp.on_hover_text(tr(if showing { "Hide automation" } else { "Show volume automation" })).clicked() {
            let view = if !showing {
                "volume"
            } else if track.kind.is_midi() {
                "notes"
            } else {
                "waveform"
            };
            let _ = app.run("track.view", json!({"track": id.0, "view": view}));
        }
    }
    // Name, highlighted when the track is selected, and the playlist menu beside it.
    let indent = folder_depth(app.engine.session(), track) as f32 * 10.0;
    let name_r = at(16.0 + indent, 1.5, 97.0 - indent, 15.0);
    if selected {
        ui.painter().rect_filled(name_r, 2.0, t.name_field_sel);
    }
    let shown_name = if app.engine.session().edit.flag("edit_view.track_number") {
        let n = app.engine.session().track_index(id).map_or(0, |i| i + 1);
        format!("{n}  {}", track.name)
    } else {
        track.name.clone()
    };
    let name_color = if selected { t.text_dark } else { t.text.lerp_to_gamma(t.text_dim, 0.4) };
    ui.painter().with_clip_rect(name_r).text(pos2(name_r.min.x + 1.5, name_r.center().y), Align2::LEFT_CENTER, &shown_name, bold(12.5), name_color);
    let nresp = ui.interact(name_r, ui.id().with(("name", id.0)), Sense::click_and_drag());
    if nresp.double_clicked() {
        app.dialogs.open_rename_track(id, &track.name);
    } else if nresp.clicked() {
        click_track(app, id, ui.input(|i| i.modifiers), in_edit_window);
    }
    nresp.context_menu(|ui| track_context_menu(app, ui, id));
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
    let pl_r = at(114.0, 1.0, 11.0, 14.5);
    let presp = ui.interact(pl_r, ui.id().with(("pl", id.0)), Sense::click());
    ui.painter().rect_filled(pl_r, 1.5, shade(head_bg, if presp.hovered() { -0.2 } else { -0.35 }));
    arrow(ui.painter(), pos2(pl_r.center().x, pl_r.min.y + 5.75), true, t.text);
    egui::Popup::menu(&presp.on_hover_text(tr("Playlists"))).show(|ui| {
        if ui.button(tr("New...")).clicked() {
            let _ = app.run("track.playlist_new", json!({"track": id.0}));
        }
        if ui.button(tr("Duplicate...")).clicked() {
            let _ = app.run("track.playlist_duplicate", json!({"track": id.0}));
        }
        if ui.button(tr("Delete Unused...")).clicked() {
            let _ = app.run("track.playlist_delete_unused", json!({"track": id.0}));
        }
        ui.separator();
        for (i, p) in track.playlists.iter().enumerate() {
            if ui.selectable_label(i == track.active_playlist, &p.name).clicked() {
                let _ = app.run("track.playlist_select", json!({"track": id.0, "index": i}));
            }
        }
    });
    let m = &track.mixer;
    if h >= 37.0 {
        let (records, mixes) = (track.kind.has_playlist(), track.kind != TrackKind::Master);
        let implied = !m.solo && app.engine.session().tracks.iter().any(|t| t.mixer.solo);
        let keys = [
            (HeaderKey::Rec, m.record_arm, false, "mix.record_arm", records),
            (HeaderKey::Input, m.input_monitor, false, "mix.input_monitor", records),
            (HeaderKey::Solo, m.solo, false, "mix.solo", mixes),
            (HeaderKey::Mute, m.mute, implied, "mix.mute", mixes),
        ];
        for (i, (key, lit, implied, cmd, shown)) in keys.into_iter().enumerate() {
            if shown && header_toggle(ui, at(23.0 + i as f32 * 26.0, 20.0, 16.0, 16.0), id, key, lit, implied).clicked() {
                let _ = app.run(cmd, json!({"track": id.0}));
            }
        }
    }
    if h >= 58.0 {
        let vresp = header_selector(ui, at(17.0, 39.0, 105.0, 17.0), ui.id().with(("view", id.0)), &view_label(&track.view), t.text, true);
        egui::Popup::menu(&vresp).show(|ui| view_menu(app, ui, track));
    }
    if h >= 76.0 {
        if matches!(track.kind, TrackKind::Audio | TrackKind::Instrument | TrackKind::Aux) {
            let r = at(17.0, 57.0, 13.0, 17.0);
            let resp = ui.interact(r, ui.id().with(("freeze", id.0)), Sense::click());
            raised_key(ui.painter(), r, if track.frozen { t.slot_bypassed } else { t.slot_face }, resp.hovered());
            for a in [0.0f32, 60.0, 120.0] {
                let d = vec2(a.to_radians().sin(), a.to_radians().cos()) * 3.5;
                ui.painter().line_segment([r.center() - d, r.center() + d], Stroke::new(1.5, t.text));
            }
            if resp.on_hover_text(tr(if track.frozen { "Unfreeze" } else { "Freeze" })).clicked() {
                let _ = app.run("track.freeze", json!({"tracks": [id.0]}));
            }
        }
        if !matches!(track.kind, TrackKind::Master | TrackKind::Vca | TrackKind::Folder) {
            let (voice, color) = if track.inactive { ("off", t.text_dim) } else { ("dyn", t.text) };
            let resp = header_selector(ui, at(31.0, 57.0, 31.0, 17.0), ui.id().with(("voice", id.0)), voice, color, false);
            egui::Popup::menu(&resp.on_hover_text(tr("Voice: dyn plays the track, off makes it inactive"))).show(|ui| {
                for (label, inactive) in [("dyn", false), ("off", true)] {
                    if ui.selectable_label(track.inactive == inactive, label).clicked() {
                        let _ = app.run("track.make_inactive", json!({"tracks": [id.0], "inactive": inactive}));
                    }
                }
            });
        }
        let mode = m.automation_mode;
        let col = match mode {
            soundcraft_model::AutomationMode::Read => t.auto_read,
            soundcraft_model::AutomationMode::Write => t.auto_write,
            soundcraft_model::AutomationMode::Touch | soundcraft_model::AutomationMode::TouchLatch => t.auto_touch,
            soundcraft_model::AutomationMode::Latch => t.auto_latch,
            _ => t.text_dim,
        };
        let aresp = header_selector(ui, at(63.0, 57.0, 59.0, 17.0), ui.id().with(("auto_mode", id.0)), tr(mode.label()), col, true);
        egui::Popup::menu(&aresp).show(|ui| {
            for m in soundcraft_model::AutomationMode::ALL {
                if ui.selectable_label(m == mode, tr(m.label())).clicked() {
                    let _ = app.run("mix.automation_mode", json!({"track": id.0, "mode": m.label()}));
                }
            }
        });
    }
    if h >= 94.0 {
        if track.kind.has_playlist() {
            let r = at(17.0, 75.0, 13.0, 17.0);
            let resp = ui.interact(r, ui.id().with(("timebase", id.0)), Sense::click());
            raised_key(ui.painter(), r, t.slot_face, resp.hovered());
            let c = r.center();
            if track.ticks_timebase {
                let green = Color32::from_rgb(86, 172, 103);
                ui.painter().add(Shape::convex_polygon(
                    vec![c + vec2(-3.0, 3.25), c + vec2(3.0, 3.25), c + vec2(1.0, -3.25), c + vec2(-1.0, -3.25)],
                    green,
                    Stroke::NONE,
                ));
                ui.painter().line_segment([c + vec2(0.0, 2.0), c + vec2(2.0, -3.0)], Stroke::new(1.0, t.text));
            } else {
                ui.painter().circle_filled(c, 3.25, Color32::from_rgb(100, 184, 249));
                ui.painter().line_segment([c, c + vec2(0.0, -3.25)], Stroke::new(1.0, t.slot_face[1]));
                ui.painter().line_segment([c, c + vec2(3.25, 0.0)], Stroke::new(1.0, t.slot_face[1]));
            }
            let tip = tr(if track.ticks_timebase { "Timebase: ticks" } else { "Timebase: samples" });
            egui::Popup::menu(&resp.on_hover_text(tip)).show(|ui| {
                for (label, ticks) in [("Samples", false), ("Ticks", true)] {
                    if ui.selectable_label(track.ticks_timebase == ticks, tr(label)).clicked() {
                        let _ = app.run("track.timebase", json!({"tracks": [id.0], "ticks": ticks}));
                    }
                }
            });
        }
        let field = at(30.0, 75.0, 92.0, 17.0);
        if matches!(track.kind, TrackKind::Audio | TrackKind::Instrument) {
            let resp = ui.interact(field, ui.id().with(("field", id.0)), Sense::click());
            ui.painter().rect_filled(field, 2.0, shade(head_bg, if resp.hovered() { -0.02 } else { -0.1 }));
            if track.kind == TrackKind::Instrument {
                let name =
                    track.instrument.as_ref().and_then(|i| crate::mix_window::plugin_info(&i.plugin)).map_or(tr("no instrument"), |p| p.short_name);
                ui.painter().with_clip_rect(field).text(pos2(field.min.x + 5.0, field.center().y), Align2::LEFT_CENTER, name, bold(10.0), t.text);
                egui::Popup::menu(&resp.on_hover_text(tr("Instrument")))
                    .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
                    .show(|ui| crate::mix_window::instrument_menu(app, ui, id));
            } else {
                let c = pos2(field.min.x + 7.0, field.center().y);
                ui.painter().line_segment([c + vec2(0.0, -3.5), c + vec2(0.0, 1.0)], Stroke::new(1.0, t.text));
                ui.painter().add(Shape::convex_polygon(vec![c + vec2(0.0, 0.5), c + vec2(3.0, 3.5), c + vec2(-3.0, 3.5)], t.text, Stroke::NONE));
                if let Some(alg) = &track.elastic {
                    ui.painter().with_clip_rect(field).text(pos2(field.min.x + 14.0, field.center().y), Align2::LEFT_CENTER, alg, bold(10.0), t.text);
                }
                egui::Popup::menu(&resp.on_hover_text(tr("Elastic Audio"))).show(|ui| {
                    for alg in ["none", "polyphonic", "rhythmic", "monophonic", "varispeed", "x-form"] {
                        let current = track.elastic.as_deref().unwrap_or("none") == alg;
                        if ui.selectable_label(current, tr(alg)).clicked() {
                            let _ = app.run("track.elastic", json!({"tracks": [id.0], "algorithm": alg}));
                        }
                    }
                });
            }
        }
    }
    let md = app.meters.get(&id).copied().unwrap_or_default();
    header_meter(ui, Rect::from_min_max(pos2(head.max.x - 8.0, head.min.y + 1.0), pos2(head.max.x, head.max.y - 1.5)), &md, track.channels() >= 2);
}

/// A small menu arrow, pointing down, or right for a closed folder.
fn arrow(p: &egui::Painter, c: Pos2, down: bool, color: Color32) {
    let pts = if down {
        vec![c + vec2(-2.5, -1.75), c + vec2(2.5, -1.75), c + vec2(0.0, 1.75)]
    } else {
        vec![c + vec2(-1.75, -2.5), c + vec2(1.75, 0.0), c + vec2(-1.75, 2.5)]
    };
    p.add(Shape::convex_polygon(pts, color, Stroke::NONE));
}

/// A header selector (track view, automation mode…): the value centred, and for menus an arrow
/// in the top-right corner.
fn header_selector(ui: &Ui, r: Rect, id: egui::Id, text: &str, color: Color32, menu: bool) -> egui::Response {
    let resp = ui.interact(r, id, Sense::click());
    raised_key(ui.painter(), r, Tokens::current().slot_face, resp.hovered());
    let tr = if menu { r.with_max_x(r.max.x - 7.0) } else { r };
    ui.painter().with_clip_rect(tr.shrink(1.0)).text(tr.center(), Align2::CENTER_CENTER, text, bold(10.0), color);
    if menu {
        arrow(ui.painter(), pos2(r.max.x - 5.5, r.min.y + 5.75), true, Tokens::current().text);
    }
    resp
}

#[derive(Clone, Copy, PartialEq)]
enum HeaderKey {
    Rec,
    Input,
    Solo,
    Mute,
}

/// Record, input, solo or mute: `lit` gives the key its colour, `implied` a dark shade of it (a
/// mute that another track's solo implies). Record keys stay dark red when off, as in Pro Tools.
fn header_toggle(ui: &Ui, r: Rect, track: TrackId, key: HeaderKey, lit: bool, implied: bool) -> egui::Response {
    let t = Tokens::current();
    let (label, color) = match key {
        HeaderKey::Rec => ("Record enable", t.rec),
        HeaderKey::Input => ("Input monitoring", t.input),
        HeaderKey::Solo => ("Solo", t.solo),
        HeaderKey::Mute => ("Mute", t.mute),
    };
    let resp = ui.interact(r, ui.id().with((label, track.0)), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, ui.is_enabled(), lit, tr(label)));
    let rec = key == HeaderKey::Rec;
    let (tint, k) = if lit {
        (color, 0.85)
    } else if implied {
        (shade(color, -0.35), 0.55)
    } else if rec {
        (shade(color, -0.5), 0.35)
    } else {
        (color, 0.0)
    };
    let [top, bottom] = t.slot_face.map(|f| f.lerp_to_gamma(tint, k));
    raised_key(ui.painter(), r, [shade(top, 0.08), shade(bottom, -0.12)], resp.hovered());
    let ink = if lit {
        t.text_dark
    } else if implied {
        t.text.lerp_to_gamma(color, 0.3)
    } else {
        t.text
    };
    if rec {
        ui.painter().circle_filled(r.center(), 4.0, if lit { Color32::WHITE } else { t.text.lerp_to_gamma(color, 0.3) });
    } else {
        ui.painter().text(r.center(), Align2::CENTER_CENTER, label.get(..1).unwrap_or_default(), bold(11.0), ink);
    }
    resp.on_hover_text(tr(label))
}

/// The header's level meter: black-rimmed bars, dim where unlit, with a clip light on top.
fn header_meter(ui: &Ui, r: Rect, md: &crate::MeterDisplay, stereo: bool) {
    let t = Tokens::current();
    let p = ui.painter();
    p.rect_filled(r, 0.0, Color32::BLACK);
    let inner = r.shrink(1.0);
    let clip = inner.with_max_y(inner.min.y + 4.0);
    p.rect_filled(clip, 0.0, if md.clip { t.meter_red } else { t.meter_red.gamma_multiply(0.18) });
    let body = inner.with_min_y(clip.max.y + 1.0);
    let n = if stereo { 2 } else { 1 };
    let w = (body.width() - (n - 1) as f32) / n as f32;
    let (amber, red) = (meter_pos(-12.0), meter_pos(-3.0));
    for ch in 0..n {
        let bar = Rect::from_min_size(pos2(body.min.x + ch as f32 * (w + 1.0), body.min.y), vec2(w, body.height()));
        let y = |f: f32| bar.max.y - bar.height() * f;
        let lv = level_pos(md.level[ch]);
        for (lo, hi, col) in [(0.0, amber, t.meter_green), (amber, red, t.meter_yellow), (red, 1.0, t.meter_red)] {
            p.rect_filled(Rect::from_x_y_ranges(bar.x_range(), y(hi)..=y(lo)), 0.0, col.gamma_multiply(0.18));
            if lv > lo {
                p.rect_filled(Rect::from_x_y_ranges(bar.x_range(), y(lv.min(hi))..=y(lo)), 0.0, col);
            }
        }
        let hv = level_pos(md.hold[ch]);
        if hv > 0.01 {
            p.hline(bar.x_range(), y(hv), Stroke::new(1.0, if hv > red { t.meter_red } else { t.meter_yellow }));
        }
    }
}

/// A header column's own Ui, so its slots' ids can't collide with the track header's.
fn column_ui(ui: &mut Ui, col: &str, r: Rect) -> Ui {
    ui.new_child(egui::UiBuilder::new().id_salt(("edit_column", col)).max_rect(r))
}

/// One optional Edit-window column for a track (I/O, inserts, sends, comments).
fn header_column(app: &mut SoundApp, ui: &mut Ui, track: &Track, col: &str, r: Rect, head_bg: Color32) {
    match col {
        "io" => io_column(app, ui, track, r),
        "comments" => {
            ui.painter().rect_filled(r, 0.0, head_bg);
            ui.painter().vline(r.min.x, r.y_range(), Stroke::new(1.0, Tokens::current().border));
            let mut c = track.comments.clone();
            let mut child = ui.new_child(egui::UiBuilder::new().max_rect(r.shrink(3.0)));
            if child.add(egui::TextEdit::multiline(&mut c).desired_width(r.width() - 6.0).font(regular(10.0))).changed() {
                let _ =
                    app.engine.execute_merged("track.comments", &json!({"track": track.id.0, "comments": c}), &format!("comments:{}", track.id.0));
            }
        }
        _ => plugin_column(app, ui, track, col, r),
    }
}

/// The I/O column as Pro Tools draws it: input and output selectors, then volume and pan.
fn io_column(app: &mut SoundApp, ui: &mut Ui, track: &Track, r: Rect) {
    let t = Tokens::current();
    let id = track.id;
    ui.painter().rect_filled(r, 0.0, t.slot_well);
    let ui = column_ui(ui, "io", r);
    let row = |y: f32, h: f32| (r.height() >= y + h + 1.0).then_some(Rect::from_min_size(r.min + vec2(7.0, y), vec2(r.width() - 14.0, h)));
    if let Some(input) = row(4.0, 15.0) {
        let resp = header_selector(&ui, input, ui.id().with(("in", id.0)), &crate::mix_window::input_name(app, track), t.text, true);
        crate::mix_window::route_popup(app, &resp.on_hover_text(tr("Input")), id, true);
    }
    if let Some(out) = row(21.0, 17.0) {
        let (sel, fader) = (out.with_max_x(out.max.x - 11.0), out.with_min_x(out.max.x - 11.0));
        let resp = ui.interact(sel, ui.id().with(("out", id.0)), Sense::click());
        let fresp = ui.interact(fader, ui.id().with(("out_fader", id.0)), Sense::click());
        raised_key(ui.painter(), out, t.slot_face, resp.hovered() || fresp.hovered());
        groove(ui.painter(), fader.min.x - 0.5, out.min.y + 1.0..=out.max.y - 1.0, t.slot_face[0]);
        let name = crate::mix_window::route_name(app, &track.mixer.output);
        ui.painter().with_clip_rect(sel.shrink(1.0)).text(sel.center(), Align2::CENTER_CENTER, name, bold(10.0), t.text);
        let c = fader.center() + vec2(0.5, 0.0);
        ui.painter().vline(c.x, c.y - 5.0..=c.y + 5.0, Stroke::new(1.0, t.text));
        ui.painter().rect_filled(Rect::from_center_size(c + vec2(0.0, -1.5), vec2(5.0, 2.5)), 0.5, t.text);
        crate::mix_window::route_popup(app, &resp.on_hover_text(tr("Output")), id, false);
        if fresp.on_hover_text(tr("Show in the Mix window")).clicked() {
            let _ = app.run("edit.select", json!({"tracks": [id.0]}));
            let _ = app.run("window.mix", json!({}));
        }
    }
    if let Some(vr) = row(45.0, 30.0).map(|vr| vr.shrink2(vec2(1.0, 0.0))) {
        ui.painter().rect_filled(vr, 2.0, Color32::from_gray(15));
        ui.painter().rect_filled(vr.shrink(1.5), 1.5, Color32::BLACK);
        let m = &track.mixer;
        let pan = m.pan.first().map_or("-".into(), |p| crate::widgets::pan_text(*p));
        // The readout is black in both themes, so it takes the dark palette's green.
        let ink = Tokens::DARK.counter_text;
        for (y, label, value) in [(7.5, "vol", crate::widgets::db_text(m.volume_db)), (22.0, "pan", pan)] {
            ui.painter().text(pos2(vr.min.x + 7.0, vr.min.y + y), Align2::LEFT_CENTER, label, bold(10.0), ink);
            ui.painter().text(pos2(vr.max.x - 6.5, vr.min.y + y), Align2::RIGHT_CENTER, value, bold(10.0), ink);
        }
        ui.painter().hline(vr.min.x + 7.0..=vr.max.x - 6.0, vr.min.y + 14.5, Stroke::new(1.0, Color32::from_gray(35)));
    }
}

/// Inserts, sends or the instrument, laid out as Pro Tools does: 15-point cells every 18 points
/// on a neutral well, or on tracks too short for three of them, a row of squares.
fn plugin_column(app: &mut SoundApp, ui: &mut Ui, track: &Track, col: &str, r: Rect) {
    let t = Tokens::current();
    ui.painter().rect_filled(r, 0.0, t.slot_well);
    let rows = if r.height() < 16.0 { 0 } else { ((r.height() - 16.0) / 18.0) as usize + 1 };
    let cell = |row: usize| Rect::from_min_size(pos2(r.min.x + 4.0, r.min.y + 1.0 + row as f32 * 18.0), vec2(r.width() - 8.0, 15.0));
    let square = |i: usize, row: usize| Rect::from_min_size(cell(row).min + vec2(i as f32 * 14.0, 0.0), vec2(10.0, 15.0));
    let mut ui = column_ui(ui, col, r);
    if col == "instruments" {
        if rows > 0 && track.kind == TrackKind::Instrument {
            crate::mix_window::instrument_slot(app, &mut ui, track, cell(0));
        }
        return;
    }
    let off = if col.ends_with("ae") { 0 } else { 5 };
    let slot = if col.starts_with("inserts") { crate::mix_window::insert_slot } else { crate::mix_window::send_slot };
    let n = match rows {
        0 => 0,
        1 | 2 => 5,
        _ => rows.min(5),
    };
    for i in 0..n {
        slot(app, &mut ui, track, off + i, if rows < 3 { square(i, 0) } else { cell(i) });
    }
    if rows == 2 {
        for i in 0..5 {
            ui.painter().rect_filled(square(i, 1), 1.5, t.slot_empty);
        }
    }
}

fn view_label(v: &str) -> String {
    match AutoParam::parse(v) {
        Some(p) => auto_param_label(&p),
        None => tr(v).to_string(),
    }
}

/// [`AutoParam::label`] in the interface language.
pub fn auto_param_label(p: &AutoParam) -> String {
    use crate::i18n::trf;
    let slot_letter = |i: u8| char::from(b'A'.saturating_add(i.min(25)));
    match p {
        AutoParam::Volume => tr("volume").into(),
        AutoParam::Pan(0) => tr("pan").into(),
        AutoParam::Pan(i) => trf("pan {}", &[&(u16::from(*i) + 1)]),
        AutoParam::Mute => tr("mute").into(),
        AutoParam::SendLevel(i) => trf("send {} level", &[&slot_letter(*i)]),
        AutoParam::SendPan(i) => trf("send {} pan", &[&slot_letter(*i)]),
        AutoParam::SendMute(i) => trf("send {} mute", &[&slot_letter(*i)]),
        AutoParam::Plugin { slot, param } => trf("insert {} {}", &[&slot_letter(*slot), param]),
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

pub(crate) fn track_context_menu(app: &mut SoundApp, ui: &mut Ui, id: TrackId) {
    let tid = id.0;
    let items: [(&str, &str); 10] = [
        (tr("Rename..."), "ui.rename_track"),
        (tr("Duplicate..."), "track.duplicate"),
        (tr("Make Inactive"), "track.make_inactive"),
        (tr("Hide"), "track.hide"),
        (tr("Delete"), "track.delete"),
        (tr("Split into Mono"), "track.split_into_mono"),
        (tr("Commit..."), "track.commit"),
        (tr("Freeze"), "track.freeze"),
        (tr("Move to New Folder..."), "track.move_to_new_folder"),
        (tr("Group..."), "track.group"),
    ];
    let selection: Vec<u64> = {
        let selected = &app.engine.session().edit.selected_tracks;
        if selected.contains(&id) { selected.iter().map(|track| track.0).collect() } else { vec![tid] }
    };
    for (label, cmd) in items {
        if ui.button(label).clicked() {
            if cmd == "ui.rename_track" {
                let name = app.engine.session().track(id).map(|t| t.name.clone()).unwrap_or_default();
                app.dialogs.open_rename_track(id, &name);
            } else {
                let _ = app.run(cmd, json!({"tracks": selection}));
            }
        }
    }
    let folders: Vec<(u64, String)> = app
        .engine
        .session()
        .tracks
        .iter()
        .filter(|track| track.is_folder() && !selection.contains(&track.id.0))
        .map(|track| (track.id.0, track.name.clone()))
        .collect();
    if !folders.is_empty() {
        ui.menu_button(tr("Move to Folder"), |ui| {
            for (folder, name) in &folders {
                if ui.button(name).clicked() {
                    let _ = app.run("track.move_to_folder", json!({"tracks": selection, "folder": folder}));
                }
            }
        });
    }
    if app.engine.session().track(id).is_some_and(|track| track.folder.is_some()) && ui.button(tr("Remove from Folder")).clicked() {
        let _ = app.run("track.remove_from_folder", json!({"tracks": selection}));
    }
    ui.separator();
    ui.menu_button(tr("Track Height"), |ui| {
        for h in TrackHeight::ALL {
            if ui.button(tr(h.label())).clicked() {
                let own: Vec<TrackId> = selection.iter().map(|&t| TrackId(t)).collect();
                let tracks = height_targets(app.engine.session(), ui.input(|i| i.modifiers), &own);
                let _ = app.run("track.height", json!({"tracks": tracks, "height": h.label()}));
            }
        }
    });
    ui.menu_button(tr("Track Color"), |ui| {
        for (i, c) in soundcraft_model::TRACK_COLORS.iter().enumerate() {
            let (r, resp) = ui.allocate_exact_size(vec2(120.0, 14.0), Sense::click());
            ui.painter().rect_filled(r, 2.0, rgb(*c));
            if resp.clicked() {
                let _ = app.run("track.color", json!({"tracks": selection, "color": i}));
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
        let col = grid_veil(t.bar_line, t.playlist_bg);
        for at in bar.lines(view.start, view.end, sr, &s.tempo, s.frame_rate, 600) {
            let x = x_of(s, tl, at);
            painter.line_segment([pos2(x, lane.min.y), pos2(x, lane.max.y)], Stroke::new(1.0, col));
        }
    }
    if s.edit.grid_lines {
        let step_px = s.edit.grid.step_samples(view.start, sr, &s.tempo, s.frame_rate) as f64 / s.edit.zoom.samples_per_px;
        if step_px > 7.0 {
            let col = grid_veil(t.grid_line, t.playlist_bg);
            for at in s.edit.grid.lines(view.start, view.end, sr, &s.tempo, s.frame_rate, 2000) {
                let x = x_of(s, tl, at);
                painter.line_segment([pos2(x, lane.min.y), pos2(x, lane.max.y)], Stroke::new(1.0, col));
            }
        }
    }
}

fn clip_name_height(s: &Session, lane: Rect) -> f32 {
    if lane.height() >= 40.0 && s.edit.flag("view.clip.name") { 13.0 } else { 0.0 }
}

fn fade_curve_points(fade: soundcraft_model::Fade, fade_in: bool, a: f32, b: f32, body: Rect, pixels_per_point: f32) -> Vec<Pos2> {
    let scale = if pixels_per_point.is_finite() { pixels_per_point.clamp(0.5, 4.0) } else { 1.0 };
    let extent = (b - a).abs().max(body.height().abs()) * scale;
    let extent = if extent.is_finite() { extent.clamp(64.0, 4096.0) } else { 64.0 };
    // Physical-pixel sampling avoids visible corners, even for steep bends.
    // Keep an exact midpoint and cap allocations for extreme zoom/clip sizes.
    let segments = (extent.ceil() as usize).div_ceil(2) * 2;
    (0..=segments)
        .map(|i| {
            let k = i as f32 / segments as f32;
            pos2(a + (b - a) * k, body.max.y - body.height() * fade.gain(if fade_in { k } else { 1.0 - k }))
        })
        .collect()
}

fn fade_control_position(fade: soundcraft_model::Fade, a: f32, b: f32, body: Rect) -> Pos2 {
    let padding = body.height().clamp(0.0, 8.0) * 0.5;
    let y = body.max.y - body.height() * fade.gain(0.5);
    // Keep the whole handle inside the body when a bend approaches an edge.
    pos2((a + b) * 0.5, y.max(body.min.y + padding).min(body.max.y - padding))
}

fn clip_preview_fades(clip: &Clip, gesture: Option<&Gesture>) -> (soundcraft_model::Fade, soundcraft_model::Fade) {
    let (mut fade_in, mut fade_out) = (clip.fade_in, clip.fade_out);
    match gesture {
        Some(Gesture::Fade { clip: id, fade_in: side, to, .. }) if *id == clip.id => {
            if let Some(fade) = fade_drag_preview(clip, *side, *to) {
                if *side {
                    fade_in = fade;
                } else {
                    fade_out = fade;
                }
            }
        }
        Some(Gesture::FadeCurve { clip: id, fade_in: side, curvature, .. }) if *id == clip.id => {
            if *side {
                fade_in.curvature = *curvature;
            } else {
                fade_out.curvature = *curvature;
            }
        }
        _ => {}
    }
    (fade_in, fade_out)
}

/// The take `track` is recording, drawn in red as it grows, as in Pro Tools.
fn draw_live_take(app: &SoundApp, painter: &egui::Painter, s: &Session, track: &Track, tl: Rect, lane: Rect) {
    let Some(rec) = &app.recorder else { return };
    let Some(take) = rec.live_take(track.id) else { return };
    let block_len = soundcraft_playback::record::LIVE_BLOCK as f64 * s.sample_rate.as_f64() / f64::from(rec.sample_rate.max(1));
    let len = soundcraft_time::to_samples(take.first().map_or(0, Vec::len) as f64 * block_len);
    let sel = s.edit.selection;
    let (start, pass) = if s.edit.loop_record && !sel.is_empty() { (sel.start, Some(sel.len())) } else { (app.record_start, None) };
    let Some((origin, span)) = soundcraft_engine::io::live_span(start, pass, app.punch, len) else { return };
    let r = Rect::from_min_max(pos2(x_of(s, tl, span.start), lane.min.y + 1.0), pos2(x_of(s, tl, span.end), lane.max.y - 1.0));
    let t = Tokens::current();
    let (body, _, wave) = clip_colors([t.rec.r(), t.rec.g(), t.rec.b()], false);
    painter.rect(r, CornerRadius::same(2), body, Stroke::new(1.0, Color32::from_rgb(10, 10, 10)), StrokeKind::Inside);
    if track.view != "blocks" {
        crate::waveform::draw_live(&painter.with_clip_rect(r.intersect(lane)), s, tl, origin as f64, block_len, &take, r, wave);
    }
}

/// The translucent white or black that turns `bg` into `line`, so grid lines also show over clips.
fn grid_veil(line: Color32, bg: Color32) -> Color32 {
    let level = |c: Color32| (f32::from(c.r()) + f32::from(c.g()) + f32::from(c.b())) / 3.0;
    let (l, b) = (level(line), level(bg));
    if l > b {
        Color32::from_white_alpha(((l - b) / (255.0 - b) * 255.0).round() as u8)
    } else {
        Color32::from_black_alpha(((b - l) / b.max(1.0) * 255.0).round() as u8)
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
    let name_h = clip_name_height(s, lane);
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
    // The waveform and fade curve share the same temporary envelope every frame.
    let (fade_in, fade_out) = clip_preview_fades(clip, app.gesture.as_ref().filter(|_| !dim));
    match &clip.content {
        _ if track.view == "blocks" => {}
        ClipContent::Audio { source, .. } => {
            if let Some(audio) = s.pool.get(*source) {
                if vis.width() > 0.0 && vis.height() > 2.0 {
                    // Drawn with the fades being dragged, so the waveform follows them before release.
                    let preview;
                    let shown = if (fade_in, fade_out) == (clip.fade_in, clip.fade_out) {
                        clip
                    } else {
                        preview = Clip { fade_in, fade_out, ..clip.clone() };
                        &preview
                    };
                    crate::waveform::draw(&painter.with_clip_rect(vis), s, tl, shown, audio, track.channels(), body_r, wave);
                    let n = track.channels().min(audio.peaks.len()).max(1);
                    for k in 1..n {
                        let y = body_r.min.y + body_r.height() / n as f32 * k as f32;
                        painter.line_segment([pos2(vis.min.x, y), pos2(vis.max.x, y)], Stroke::new(1.0, body.gamma_multiply(0.7)));
                    }
                }
            } else {
                painter.text(body_r.center(), Align2::CENTER_CENTER, tr("media offline"), regular(10.0), Color32::from_rgb(220, 120, 120));
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
                let vcol = if crate::theme::is_light() {
                    Color32::from_rgb(20, 20 + n.velocity, 40)
                } else {
                    wave.gamma_multiply(0.5 + f32::from(n.velocity) / 254.0)
                };
                p.rect_filled(
                    Rect::from_min_max(pos2(nx0, y - nh * 0.5), pos2(nx1, y + nh * 0.5)),
                    1.0,
                    if selected { Color32::from_rgb(16, 16, 16) } else { vcol },
                );
            }
        }
    }
    let prev = crossfade_partner(track, clip);
    if s.edit.flag("waveform.overlapped_crossfades")
        && track.view != "blocks"
        && let Some(prev) = prev
        && let ClipContent::Audio { source, .. } = &prev.content
        && let Some(audio) = s.pool.get(*source)
    {
        let xf = Rect::from_min_max(pos2(x0, body_r.min.y), pos2(x_of(s, tl, prev.end()).min(x1), body_r.max.y)).intersect(vis);
        let tail = if prev.muted || dim {
            Color32::from_rgb(70, 70, 70)
        } else {
            clip_colors(prev.color.unwrap_or(track.color), false).2.gamma_multiply(0.6)
        };
        crate::waveform::draw(&painter.with_clip_rect(xf), s, tl, prev, audio, track.channels(), body_r, tail);
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
    let fade_col = Color32::from_rgba_unmultiplied(255, 255, 255, 90);
    for (side, fade) in [(true, fade_in), (false, fade_out)] {
        if fade.len <= 0 {
            continue;
        }
        let (a, b) =
            if side { (x0, x_of(s, tl, clip.start.saturating_add(fade.len))) } else { (x_of(s, tl, clip.end().saturating_sub(fade.len)), x1) };
        let pts = fade_curve_points(fade, side, a, b, body_r, painter.ctx().pixels_per_point());
        painter.with_clip_rect(vis).add(Shape::line(pts, Stroke::new(1.2, fade_col)));
        let midpoint = fade_control_position(fade, a, b, body_r);
        let active = matches!(app.gesture, Some(Gesture::FadeCurve { clip: id, fade_in, .. }) if id == clip.id && fade_in == side);
        let hovered = painter.ctx().input(|i| i.pointer.hover_pos()).is_some_and(|p| near_fade_midpoint(p, midpoint));
        if !dim && s.edit.tool == Tool::Smart && AutoParam::parse(&track.view).is_none() && b - a >= 32.0 && vis.contains(midpoint) {
            let col = Color32::from_rgba_unmultiplied(255, 255, 255, if active || hovered { 230 } else { 200 });
            painter.with_clip_rect(vis).circle(midpoint, 3.5, body, Stroke::new(1.0, col));
        }
    }
    // The fade-out of the clip this one crossfades with, drawn over the overlap.
    if let Some(c) = prev.filter(|c| c.fade_out.len > 0) {
        let (a, b) = (x_of(s, tl, c.end().saturating_sub(c.fade_out.len)), x_of(s, tl, c.end()));
        let pts = fade_curve_points(c.fade_out, false, a, b, body_r, painter.ctx().pixels_per_point());
        painter.with_clip_rect(vis).add(Shape::line(pts, Stroke::new(1.2, fade_col)));
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
        Some(s.format_position(clip.start, s.edit.main_counter))
    } else if s.edit.flag("view.clip.time.original") || s.edit.flag("view.clip.time.user") {
        clip.source().and_then(|src| s.source(src)).map(|src| {
            format_position(
                i64::try_from(src.time_reference).unwrap_or(0).saturating_add(clip.source_offset()).saturating_sub(s.timecode_start),
                TimeFormat::Timecode,
                s.sample_rate,
                &s.tempo,
                s.frame_rate,
                0,
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

/// The earlier clip of a crossfade pair: the one before `clip` in its playlist, overlapping its start.
fn crossfade_partner<'a>(track: &'a Track, clip: &Clip) -> Option<&'a Clip> {
    let (p, i) = track.playlists.iter().find_map(|p| {
        let i = p.clips.partition_point(|c| c.start < clip.start);
        p.clips.get(i).is_some_and(|c| c.id == clip.id).then_some((p, i))
    })?;
    p.clips.get(i.checked_sub(1)?).filter(|c| c.end() > clip.start)
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
    painter.text(pos2(lane.min.x + 6.0, lane.min.y + 9.0), Align2::LEFT_CENTER, auto_param_label(p), regular(10.0), t.text_dim);
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
    let on = s.edit.edit_mode.is_grid() || s.edit.flag(ops::SNAP_TO_GRID);
    if on { s.edit.grid.snap(at, s.sample_rate, &s.tempo, s.frame_rate) } else { at }
}

fn clip_at(track: &Track, at: Samples) -> Option<&Clip> {
    track.clips().iter().rev().find(|c| c.range().contains(at))
}

/// The unity-gain end of each existing fade, at the top of the clip body.
fn fade_handle_at(s: &Session, tl: Rect, lane: Rect, clip: &Clip, p: Pos2) -> Option<bool> {
    if clip.length <= 0 || clip.end() <= clip.start {
        return None;
    }
    let y = lane.min.y + 1.0 + clip_name_height(s, lane);
    [(true, clip.fade_in), (false, clip.fade_out)]
        .into_iter()
        .filter(|(_, fade)| fade.len > 0)
        .filter_map(|(fade_in, fade)| {
            let len = fade.len.min(clip.length);
            let at = if fade_in { clip.start.saturating_add(len).min(clip.end()) } else { clip.end().saturating_sub(len).max(clip.start) };
            let head = pos2(x_of(s, tl, at), y);
            let delta = p - head;
            (lane.contains(head) && delta.x.abs() <= 8.0 && delta.y.abs() <= 8.0).then_some((fade_in, delta.length_sq()))
        })
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(fade_in, _)| fade_in)
}

fn near_fade_midpoint(p: Pos2, midpoint: Pos2) -> bool {
    let delta = p - midpoint;
    delta.x.abs() <= 8.0 && delta.y.abs() <= 8.0
}

fn fade_midpoint(s: &Session, tl: Rect, lane: Rect, clip: &Clip, fade_in: bool) -> Option<Pos2> {
    let fade = if fade_in { clip.fade_in } else { clip.fade_out };
    if fade.len <= 0 || clip.length <= 0 {
        return None;
    }
    let len = fade.len.min(clip.length);
    let (a, b) = if fade_in { (clip.start, clip.start.saturating_add(len)) } else { (clip.end().saturating_sub(len), clip.end()) };
    let (a, b) = (x_of(s, tl, a), x_of(s, tl, b));
    if b - a < 32.0 {
        return None;
    }
    let body = Rect::from_min_max(pos2(a, lane.min.y + 1.0 + clip_name_height(s, lane)), pos2(b, lane.max.y - 1.0));
    let p = fade_control_position(fade, a, b, body);
    lane.contains(p).then_some(p)
}

fn fade_curve_handle_at(s: &Session, tl: Rect, lane: Rect, clip: &Clip, p: Pos2) -> Option<bool> {
    [true, false].into_iter().find(|side| fade_midpoint(s, tl, lane, clip, *side).is_some_and(|midpoint| near_fade_midpoint(p, midpoint)))
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
        let at = drag_snap(s, sample_at(s, tl, p.x).max(0), ui.input(|i| i.modifiers.command));
        let spot = Spot::source(s, soundcraft_model::SourceId(src.0), track.id, at);
        spot_or_run(app, spot);
    }
    if resp.dnd_hover_payload::<crate::DragSource>().is_some() {
        ui.painter().rect_stroke(lane, 0.0, Stroke::new(2.0, Tokens::current().accent), StrokeKind::Inside);
    }
    // Audio files dropped from outside onto an instrument track load its sampler.
    if track.kind == soundcraft_model::TrackKind::Instrument {
        crate::sampler_window::drop_target(app, ui.ctx(), lane, track.id);
    }
    let s = app.engine.session_arc();
    let tool = s.edit.tool;
    let mods = ui.input(|i| i.modifiers);
    let Some(p) = resp.interact_pointer_pos().or_else(|| resp.hover_pos()) else { return };
    let at = sample_at(&s, tl, p.x).max(0);
    let origin = if resp.drag_started() { ui.input(|i| i.pointer.press_origin()).unwrap_or(p) } else { p };
    let at_origin = sample_at(&s, tl, origin.x).max(0);
    let auto_view = AutoParam::parse(&track.view);
    let body_hit = clip_at(track, at_origin);
    // Check fade endpoints before clip bodies: a full-length fade can end exactly
    // at the next clip's start, where `clip_at` would otherwise choose that neighbour.
    let fade_handle = if tool == Tool::Smart && auto_view.is_none() {
        track
            .clips()
            .iter()
            .rev()
            .filter(|c| body_hit.is_none_or(|front| front.id == c.id || front.start == c.end()))
            .find_map(|c| fade_handle_at(&s, tl, lane, c, origin).map(|side| (c, side)))
    } else {
        None
    };
    let curve_handle = if tool == Tool::Smart && auto_view.is_none() && fade_handle.is_none() {
        body_hit.and_then(|c| fade_curve_handle_at(&s, tl, lane, c, origin).map(|side| (c, side)))
    } else {
        None
    };
    let hit = fade_handle.or(curve_handle).map(|(c, _)| c).or(body_hit).cloned();
    let upper = origin.y < lane.center().y;
    let near_edge = |c: &Clip| -> Option<bool> {
        let xs = x_of(&s, tl, c.start);
        let xe = x_of(&s, tl, c.end());
        if (origin.x - xs).abs() < 6.0 {
            Some(true)
        } else if (origin.x - xe).abs() < 6.0 {
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
    let fade_corner = if tool == Tool::Smart {
        fade_handle.map(|(_, side)| side).or_else(|| {
            hit.as_ref().and_then(|c| {
                let xs = x_of(&s, tl, c.start);
                let xe = x_of(&s, tl, c.end());
                let top = origin.y < lane.min.y + lane.height() * 0.28;
                if !top {
                    None
                } else if (origin.x - xs) > 0.0 && (origin.x - xs) < 14.0 {
                    Some(true)
                } else if (xe - origin.x) > 0.0 && (xe - origin.x) < 14.0 {
                    Some(false)
                } else {
                    None
                }
            })
        })
    } else {
        None
    };
    if fade_corner.is_some() && resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeNwSe);
    }
    if curve_handle.is_some() && resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeVertical);
    }
    let gain_icon_at = |q: Pos2| {
        let c = clip_at(track, sample_at(&s, tl, q.x).max(0)).filter(|_| auto_view.is_none())?;
        clip_gain_icon(&s, tl, lane, c).filter(|r| r.contains(q)).map(|_| c.clone())
    };
    if resp.hovered() && gain_icon_at(p).is_some() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeVertical);
    }
    let selector_press = eff == Tool::Selector && fade_corner.is_none() && curve_handle.is_none() && gain_icon_at(origin).is_none();
    if selector_press && resp.is_pointer_button_down_on() && ui.input(|i| i.pointer.primary_pressed()) {
        place_insertion(app, ui, &s, track.id, at, mods.shift);
    }
    let gain_drag = if resp.drag_started() { gain_icon_at(origin) } else { None };
    if let Some(c) = gain_drag {
        // `clip.gain` refuses to run with nothing selected.
        if s.edit.selected_tracks.is_empty() && s.edit.selected_clips.is_empty() {
            let _ = app.engine.execute("edit.select", &json!({"clips": [c.id.0]}));
        }
        let pos = crate::widgets::clip_gain_to_pos(c.gain_db);
        let screen = ui.ctx().content_rect();
        let y = (origin.y + CLIP_FADER_TRAVEL * pos).max(screen.min.y + CLIP_FADER_TRAVEL + 30.0).min(screen.max.y - 10.0);
        let fader_bottom = pos2(origin.x.max(screen.min.x + 26.0), y);
        let fine = mods.command || mods.ctrl;
        app.gesture = Some(Gesture::ClipGain { clip: c.id, track: track.id, pos, anchor: (origin.y, pos), fine, fader_bottom });
    } else if resp.drag_started()
        && let Some((c, fade_in)) = curve_handle
    {
        let fade = if fade_in { c.fade_in } else { c.fade_out };
        let anchor = (origin.y, fade.gain(0.5));
        let height = (lane.height() - 2.0 - clip_name_height(&s, lane)).max(1.0);
        app.gesture = Some(Gesture::FadeCurve { clip: c.id, fade_in, curvature: fade_curve_drag_value(fade, anchor, height, p.y), anchor, height });
        ui.ctx().request_repaint();
    } else if resp.drag_started()
        && let (Some(fade_in), Some(c)) = (fade_corner, hit.as_ref())
    {
        app.gesture = fade_drag_range(c, fade_in, at).map(|range| Gesture::Fade {
            clip: c.id,
            track: track.id,
            fade_in,
            to: if fade_in { range.end } else { range.start },
        });
        ui.ctx().request_repaint();
    } else if resp.drag_started() {
        app.gesture = match eff {
            Tool::Selector | Tool::Smart => Some(Gesture::Select { track: track.id, anchor: snap(&s, at_origin), tracks: vec![track.id] }),
            Tool::Grabber => hit.as_ref().map(|c| {
                let clips = if s.edit.selected_clips.contains(&c.id) { s.edit.selected_clips.clone() } else { vec![c.id] };
                Gesture::MoveClips { clips, grab_at: at_origin, delta: 0, from_track: track.id, to_track: track.id }
            }),
            Tool::Trim => hit.as_ref().map(|c| match near_edge(c) {
                Some(false) => Gesture::TrimEnd { clip: c.id, track: track.id, to: c.end() },
                _ if at_origin - c.start < c.end() - at_origin => Gesture::TrimStart { clip: c.id, track: track.id, to: c.start },
                _ => Gesture::TrimEnd { clip: c.id, track: track.id, to: c.end() },
            }),
            Tool::Scrubber => Some(Gesture::Scrub { last: at }),
            Tool::Pencil if auto_view.is_none() && track.kind.is_midi() => {
                hit.is_none().then_some(Gesture::MidiClip { track: track.id, from: at_origin, to: at_origin })
            }
            Tool::Pencil => Some(Gesture::Pencil { track: track.id, points: Vec::new() }),
            Tool::Zoom => Some(Gesture::ZoomBox { start: origin.x }),
        };
    }
    if let Some(Gesture::ClipGain { clip, track: t, pos, anchor, fine, fader_bottom }) = &mut app.gesture
        && *t == track.id
        && resp.dragged()
    {
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
        ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeVertical);
        clip_gain_popup(ui.ctx(), *fader_bottom, *pos);
    }
    if resp.clicked() && curve_handle.is_none() {
        match eff {
            Tool::Grabber => {
                if let Some(c) = &hit {
                    let mut ids = if mods.shift { s.edit.selected_clips.iter().map(|x| x.0).collect::<Vec<_>>() } else { Vec::new() };
                    ids.push(c.id.0);
                    let _ = app.run("edit.select", json!({"clips": ids}));
                    if s.edit.edit_mode == EditMode::Spot {
                        app.dialogs.open = Spot::clips(&s, vec![c.id], None, 0).map(Dialog::Spot);
                    }
                }
            }
            Tool::Trim if s.edit.edit_mode == EditMode::Spot && hit.is_some() => {
                if let Some(c) = &hit {
                    let start = at - c.start < c.end() - at;
                    app.dialogs.open = Spot::trim(&s, c.id, start, if start { c.start } else { c.end() }).map(Dialog::Spot);
                }
            }
            Tool::Zoom => {
                let _ = app.run(if mods.alt { "view.zoom_out" } else { "view.zoom_in" }, json!({}));
            }
            Tool::Pencil => {
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
                    let _ = app.run(
                        "automation.set_point",
                        json!({"track": track.id.0, "param": automation_id(param), "at": at, "value": lo + (hi - lo) * k}),
                    );
                } else if track.kind.is_midi() && hit.is_none() {
                    new_midi_clip(app, &s, track.id, at, at);
                }
            }
            _ if !selector_press => place_insertion(app, ui, &s, track.id, at, mods.shift),
            _ => {}
        }
    }
    if resp.double_clicked()
        && curve_handle.is_none()
        && eff != Tool::Pencil
        && let Some(c) = &hit
    {
        let _ = app.run("edit.select", json!({"clips": [c.id.0]}));
        if eff == Tool::Grabber && matches!(c.content, ClipContent::Midi { .. }) {
            app.ui.show_midi_editor = true;
        }
    }
}

fn place_insertion(app: &mut SoundApp, ui: &Ui, s: &Session, track: TrackId, at: Samples, extend: bool) {
    let at = snap(s, at);
    let sel = s.edit.selection;
    let params = if extend && s.edit.selected_tracks.contains(&track) {
        let r = if at < sel.start { Range::new(at, sel.end) } else { Range::new(sel.start, at) };
        json!({"start": r.start, "end": r.end})
    } else if extend && !s.edit.selected_tracks.is_empty() {
        let tracks: Vec<u64> = s.edit.selected_tracks.iter().chain([&track]).map(|t| t.0).collect();
        json!({"tracks": tracks, "start": sel.start.min(at), "end": sel.end.max(at)})
    } else {
        json!({"tracks": [track.0], "start": at})
    };
    let rev = app.engine.revision;
    let _ = app.run("edit.select", params);
    if app.engine.revision != rev {
        ui.ctx().request_discard("insertion point moved");
    }
}

/// The same bounded range drives the preview and the committed fade.
fn fade_drag_range(clip: &Clip, fade_in: bool, to: Samples) -> Option<Range> {
    let end = clip.end();
    if clip.length <= 0 || end <= clip.start {
        return None;
    }
    Some(if fade_in {
        Range::new(clip.start, to.clamp(clip.start.saturating_add(1), end))
    } else {
        Range::new(to.clamp(clip.start, end.saturating_sub(1)), end)
    })
}

fn fade_drag_preview(clip: &Clip, fade_in: bool, to: Samples) -> Option<soundcraft_model::Fade> {
    let range = fade_drag_range(clip, fade_in, to)?;
    let original = if fade_in { clip.fade_in } else { clip.fade_out };
    Some(soundcraft_model::Fade {
        len: range.end.saturating_sub(range.start),
        shape: if original.len > 0 { original.shape } else { soundcraft_model::FadeShape::SCurve },
        curvature: if original.len > 0 { original.curvature } else { 0.0 },
    })
}

fn fade_curve_drag_value(fade: soundcraft_model::Fade, anchor: (f32, f32), height: f32, y: f32) -> f32 {
    fade.curvature_for_midpoint_gain(anchor.1 + (anchor.0 - y) / height.max(1.0))
}

/// Runs before anything is painted, against the last frame's layout.
fn drive_drag(app: &mut SoundApp, ctx: &egui::Context) {
    if app.gesture.is_none() || matches!(app.gesture, Some(Gesture::Fader { .. })) {
        return;
    }
    let (s, tl) = (app.engine.session_arc(), rect(app.edit_layout.timeline));
    let p = ctx.pointer_latest_pos();
    if !ctx.input(|i| i.pointer.any_down()) {
        finish_drag(app, &s, tl, p);
        return;
    }
    let Some(p) = p else { return };
    let at = sample_at(&s, tl, p.x).max(0);
    let inside = pos2(p.x.max(tl.min.x).min(tl.max.x), p.y.max(tl.min.y).min(tl.max.y));
    let rows = &app.edit_layout.rows;
    let gap = |r: &[f32; 4]| (r[1] - inside.y).max(inside.y - r[3]).max(0.0);
    let to_track = rows.iter().min_by(|a, b| gap(&a.1).total_cmp(&gap(&b.1))).map(|(i, _)| TrackId(*i));
    let mut invalid_fade = false;
    match &mut app.gesture {
        Some(Gesture::Select { track, tracks, .. }) => {
            let row_of = |t: TrackId| rows.iter().position(|(i, _)| *i == t.0);
            if let (Some(a), Some(b)) = (row_of(*track), to_track.and_then(row_of)) {
                *tracks = rows.get(a.min(b)..=a.max(b)).unwrap_or_default().iter().map(|(i, _)| TrackId(*i)).collect();
            }
        }
        Some(Gesture::MoveClips { clips, grab_at, delta, from_track, to_track: tt }) => {
            if let Some(t2) = to_track {
                *tt = t2;
            }
            let plan = drag_plan(ctx, &s, app.engine.revision, clips, (*from_track, *tt), *grab_at);
            *delta = move_delta(&plan, &s, *from_track, at - *grab_at, ctx.input(|i| i.modifiers.command));
        }
        Some(Gesture::TrimStart { to, .. } | Gesture::TrimEnd { to, .. }) => *to = drag_snap(&s, at, ctx.input(|i| i.modifiers.command)),
        Some(Gesture::MidiClip { to, .. }) => *to = at,
        Some(Gesture::Fade { clip, fade_in, to, .. }) => {
            if let Some(range) = s.find_clip(*clip).and_then(|(_, c)| fade_drag_range(c, *fade_in, at)) {
                *to = if *fade_in { range.end } else { range.start };
            } else {
                invalid_fade = true;
            }
        }
        Some(Gesture::FadeCurve { clip, fade_in, curvature, anchor, height }) => {
            if let Some((_, c)) = s.find_clip(*clip).filter(|(_, c)| c.length > 0) {
                let fade = if *fade_in { c.fade_in } else { c.fade_out };
                if fade.len > 0 {
                    *curvature = fade_curve_drag_value(fade, *anchor, *height, p.y);
                } else {
                    invalid_fade = true;
                }
            } else {
                invalid_fade = true;
            }
        }
        Some(Gesture::Scrub { last }) => {
            *last = at;
            let _ = app.engine.execute("transport.locate", &json!({"at": at}));
        }
        Some(Gesture::Pencil { track, points }) => {
            if let Some(t) = s.track(*track)
                && let Some(param) = AutoParam::parse(&t.view)
                && let Some((_, row)) = rows.iter().find(|(i, _)| *i == t.id.0)
            {
                let lane = lane_rect(t, rect(*row), tl);
                let (lo, hi, _) = param.range();
                let (lo, hi) = if matches!(param, AutoParam::Volume | AutoParam::SendLevel(_)) {
                    (-60.0, 12.0)
                } else if matches!(param, AutoParam::Plugin { .. }) {
                    plugin_range(t, &param)
                } else {
                    (lo, hi)
                };
                let k = ((lane.max.y - 4.0 - p.y) / (lane.height() - 8.0)).clamp(0.0, 1.0);
                if points.last().is_none_or(|(a, _)| (at - a).abs() as f64 > s.edit.zoom.samples_per_px * 3.0) {
                    points.push((at, lo + (hi - lo) * k));
                }
            }
        }
        _ => {}
    }
    if invalid_fade {
        app.gesture = None;
    }
    if let Some(Gesture::Select { anchor, tracks, .. }) = &app.gesture {
        let r = Range::new(*anchor, snap(&s, at));
        if *tracks != s.edit.selected_tracks || r != s.edit.selection {
            let ids: Vec<u64> = tracks.iter().map(|x| x.0).collect();
            let _ = app.run("edit.select", json!({"tracks": ids, "start": r.start, "end": r.end}));
        }
    }
    let out = p - inside;
    if out != egui::Vec2::ZERO && matches!(app.gesture, Some(Gesture::Select { .. } | Gesture::MoveClips { .. })) {
        let k = 12.0 * ctx.input(|i| i.stable_dt).min(0.1);
        if out.x != 0.0 {
            let _ = app.engine.execute("view.scroll", &json!({"by_px": out.x * k}));
        }
        app.edit_layout.scroll_by(out.y * k, tl.height());
        ctx.request_repaint();
    }
}

fn finish_drag(app: &mut SoundApp, s: &Session, tl: Rect, p: Option<Pos2>) {
    match app.gesture.take() {
        Some(Gesture::MoveClips { clips, delta, from_track, to_track, .. }) => drop_clips(app, clips, delta, from_track, to_track),
        Some(Gesture::Fade { clip, fade_in, to, .. }) => {
            let to = p.map_or(to, |p| sample_at(s, tl, p.x).max(0));
            if let Some(fade) = s.find_clip(clip).and_then(|(_, c)| fade_drag_preview(c, fade_in, to)) {
                let _ = app.run(
                    "edit.fades_create",
                    json!({"clip": clip.0, "side": if fade_in { "in" } else { "out" }, "length": fade.len, "shape": fade.shape.label(), "curvature": fade.curvature}),
                );
            }
        }
        Some(Gesture::FadeCurve { clip, fade_in, curvature, anchor, height }) => {
            if let Some((_, c)) = s.find_clip(clip).filter(|(_, c)| c.length > 0) {
                let fade = if fade_in { c.fade_in } else { c.fade_out };
                let curvature = p.map_or(curvature, |p| fade_curve_drag_value(fade, anchor, height, p.y));
                if fade.len > 0 && (fade.curvature - curvature).abs() > 1e-6 {
                    let _ = app.run(
                        "edit.fades_create",
                        json!({"clip": clip.0, "side": if fade_in { "in" } else { "out" }, "length": fade.len, "shape": fade.shape.label(), "curvature": curvature}),
                    );
                }
            }
        }
        Some(Gesture::TrimStart { clip, to, .. }) => trim_clip(app, clip, to, true),
        Some(Gesture::TrimEnd { clip, to, .. }) => trim_clip(app, clip, to, false),
        Some(Gesture::MidiClip { track, from, to }) => new_midi_clip(app, s, track, from, to),
        Some(Gesture::Pencil { track, points }) => {
            if let Some(param) = s.track(track).and_then(|t| AutoParam::parse(&t.view)) {
                let pid = automation_id(&param);
                for (a, v) in points {
                    let _ = app.run("automation.set_point", json!({"track": track.0, "param": pid, "at": a, "value": v}));
                }
            }
        }
        Some(Gesture::ZoomBox { start }) => {
            let Some(p) = p else { return };
            let a = sample_at(s, tl, start.min(p.x));
            let b = sample_at(s, tl, start.max(p.x));
            if b > a {
                let spp = ((b - a) as f64 / f64::from(tl.width())).max(0.25);
                let _ = app.run("view.zoom_set", json!({"samples_per_px": spp}));
                let _ = app.run("view.scroll", json!({"to": a}));
            }
        }
        _ => {}
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

/// Grid snapping while dragging; holding Cmd/Ctrl swaps Grid and Slip.
fn drag_snap(s: &Session, at: Samples, flip: bool) -> Samples {
    if s.edit.edit_mode.is_grid() != flip { s.edit.grid.snap(at, s.sample_rate, &s.tempo, s.frame_rate) } else { at }
}

/// What a clip drag works out once per target track rather than every frame: where the tracks go
/// (None when the clips can't land there), each destination's earliest clip and Shuffle slots,
/// the clips that move (track, start, length) and the clip Grid snaps.
struct DragPlan {
    dest: Option<Vec<(TrackId, TrackId)>>,
    groups: Vec<(TrackId, Samples, Vec<Samples>)>,
    moving: Vec<(TrackId, Samples, Samples)>,
    grabbed: Option<Clip>,
}

/// The [`DragPlan`] for `clips` grabbed at `grab_at` on track `from` and dragged onto `to`, kept
/// until the document, the Edit mode or the target changes.
fn drag_plan(ctx: &egui::Context, s: &Session, revision: u64, clips: &[ClipId], (from, to): (TrackId, TrackId), grab_at: Samples) -> Arc<DragPlan> {
    type Key = (u64, EditMode, Vec<ClipId>, TrackId, TrackId, Samples);
    let id = egui::Id::new("clip_drag_plan");
    let key: Key = (revision, s.edit.edit_mode, clips.to_vec(), from, to, grab_at);
    if let Some((k, plan)) = ctx.data(|d| d.get_temp::<(Key, Arc<DragPlan>)>(id))
        && k == key
    {
        return plan;
    }
    let dest = if to == from { Some(Vec::new()) } else { ops::destinations(s, clips, from, to) };
    let groups = match &dest {
        Some(d) if s.edit.edit_mode == EditMode::Shuffle => {
            ops::shuffle_groups(s, clips, d).into_iter().map(|(t, first)| (t, first, ops::shuffle_slots(s, t, clips))).collect()
        }
        _ => Vec::new(),
    };
    let ids: HashSet<ClipId> = clips.iter().copied().collect();
    let movable = |c: &&Clip| ids.contains(&c.id) && !ops::locked(c);
    let moving = s.tracks.iter().flat_map(|t| t.clips().iter().filter(movable).map(|c| (t.id, c.start, c.length))).collect();
    let mine: Vec<&Clip> = s.track(from).map(|t| t.clips().iter().filter(movable).collect()).unwrap_or_default();
    let grabbed = mine.iter().rev().find(|c| c.range().contains(grab_at)).or_else(|| mine.iter().min_by_key(|c| c.start)).map(|c| (*c).clone());
    let plan = Arc::new(DragPlan { dest, groups, moving, grabbed });
    ctx.data_mut(|d| d.insert_temp(id, (key, Arc::clone(&plan))));
    plan
}

/// How far a Grabber drag by `raw` from track `from` takes the clips in the current Edit mode.
/// Shuffle lands the earliest clip bound for the target track on a slot there, as
/// `edit.move_clips` does; Grid snaps the grabbed clip.
fn move_delta(plan: &DragPlan, s: &Session, from: TrackId, raw: Samples, flip: bool) -> Samples {
    let mode = s.edit.edit_mode;
    if mode == EditMode::Shuffle {
        let to = plan.dest.as_ref().map_or(from, |d| ops::lands(d, from));
        plan.groups
            .iter()
            .find(|g| g.0 == to)
            .map_or(raw, |(_, first, slots)| ops::nearest_slot(slots, first.saturating_add(raw)).saturating_sub(*first))
    } else if mode.is_grid() != flip
        && let Some(c) = &plan.grabbed
    {
        ops::grid_delta(s, c, raw, mode == EditMode::GridRelative)
    } else {
        raw
    }
}

/// The dragged clips with one from the grabbed track first: `edit.move_clips` sends that track to
/// the target and the others along with it.
fn grabbed_first(s: &Session, clips: &[ClipId], from: TrackId) -> Vec<ClipId> {
    let mut v = clips.to_vec();
    v.sort_by_key(|c| s.find_clip(*c).is_none_or(|(t, _)| t != from));
    v
}

fn drop_clips(app: &mut SoundApp, clips: Vec<ClipId>, delta: Samples, from: TrackId, to: TrackId) {
    let s = app.engine.session();
    let spot = Spot::clips(s, grabbed_first(s, &clips, from), (to != from).then_some(to), delta);
    spot_or_run(app, spot);
}

fn trim_clip(app: &mut SoundApp, clip: ClipId, to: Samples, start: bool) {
    let spot = Spot::trim(app.engine.session(), clip, start, to);
    spot_or_run(app, spot);
}

/// Spot mode asks for exact positions first; the other modes make the edit straight away.
fn spot_or_run(app: &mut SoundApp, spot: Option<Spot>) {
    let Some(spot) = spot else { return };
    if app.engine.session().edit.edit_mode == EditMode::Spot {
        app.dialogs.open = Some(Dialog::Spot(spot));
    } else if let Some((id, params)) = spot.command(app.engine.session()) {
        let _ = app.run(&id, params);
    }
}

/// Whole bars from `from` to `to` (at least one), inside the gap between clips where `from` is.
fn midi_clip_span(s: &Session, track: TrackId, from: Samples, to: Samples) -> Option<Range> {
    let clips = s.track(track)?.clips();
    let bar = GridValue::Note { value: NoteValue::Bar, dotted: false, triplet: false };
    let start = bar.line_at_or_before(from.min(to), s.sample_rate, &s.tempo, s.frame_rate);
    let last = from.max(to);
    let mut end = bar.line_at_or_before(last, s.sample_rate, &s.tempo, s.frame_rate);
    if end < last || end <= start {
        end = bar.next_line(end, s.sample_rate, &s.tempo, s.frame_rate);
    }
    let prev = clips.iter().map(Clip::end).filter(|&e| e <= from).max().unwrap_or(start);
    let next = clips.iter().map(|c| c.start).filter(|&b| b > from).min().unwrap_or(end);
    let r = Range::new(start.max(prev), end.min(next));
    (!r.is_empty()).then_some(r)
}

fn new_midi_clip(app: &mut SoundApp, s: &Session, track: TrackId, from: Samples, to: Samples) {
    if let Some(r) = midi_clip_span(s, track, from, to)
        && let Ok(v) = app.run("midi.new_clip", json!({"track": track.0, "start": r.start, "end": r.end}))
    {
        let _ = app.run("edit.select", json!({"clips": [v["clip"]]}));
    }
}

fn clip_context_menu(app: &mut SoundApp, ui: &mut Ui, clip: Option<ClipId>) {
    let Some(c) = clip else {
        if ui.button(tr("Paste")).clicked() {
            let _ = app.run("edit.paste", json!({}));
        }
        return;
    };
    let ids = json!([c.0]);
    for (label, cmd) in [
        (tr("Mute/Unmute"), "edit.mute_clips"),
        (tr("Edit Lock/Unlock"), "clip.edit_lock"),
        (tr("Time Lock/Unlock"), "clip.time_lock"),
        (tr("Quantize to Grid"), "clip.quantize_to_grid"),
        (tr("Render Clip Gain"), "clip.gain_render"),
        (tr("Bring to Front"), "clip.bring_to_front"),
        (tr("Send to Back"), "clip.send_to_back"),
    ] {
        if ui.button(label).clicked() {
            let _ = app.run(cmd, json!({"clips": ids}));
        }
    }
    ui.separator();
    if ui.button(tr("Rename...")).clicked() {
        let name = app.engine.session().find_clip(c).map(|(_, x)| x.name.clone()).unwrap_or_default();
        app.dialogs.open_rename_clip(c, &name);
    }
    if ui.button(tr("Delete")).clicked() {
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
        let now = ui.input(|i| i.time);
        let (id, key) = (ui.id().with("insertion_blink"), (sel.start, s.edit.selected_tracks.clone()));
        let since = ui.ctx().data_mut(|d| {
            let (last, since) = d.get_temp_mut_or_insert_with(id, || (key.clone(), now));
            if *last != key {
                *last = key;
                *since = now;
            }
            *since
        });
        let phase = (now - since) * 2.0;
        if (phase as i64) % 2 == 0 {
            for (id, r) in &app.edit_layout.rows {
                if s.edit.selected_tracks.contains(&TrackId(*id)) {
                    painter.line_segment([pos2(x, r[1].max(tl.min.y)), pos2(x, r[3])], Stroke::new(1.0, t.insertion));
                }
            }
        }
        ui.ctx().request_repaint_after_secs(((1.0 - phase.fract()) / 2.0) as f32);
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
    if let Some(Gesture::MoveClips { clips, delta, from_track, to_track, grab_at }) = &app.gesture
        && let plan = drag_plan(ui.ctx(), s, app.engine.revision, clips, (*from_track, *to_track), *grab_at)
        && let Some(dest) = &plan.dest
    {
        for &(t, start, len) in &plan.moving {
            let d = ops::lands(dest, t);
            let start = plan
                .groups
                .iter()
                .find(|g| g.0 == d)
                .map_or(start + delta, |(_, first, slots)| ops::nearest_slot(slots, first + delta) + (start - first));
            if let Some((_, r)) = app.edit_layout.rows.iter().find(|(i, _)| *i == d.0) {
                let x0 = x_of(s, tl, start);
                let x1 = x_of(s, tl, start + len);
                painter.rect_stroke(
                    Rect::from_min_max(pos2(x0, r[1] + 1.0), pos2(x1, r[3] - 1.0)),
                    2.0,
                    Stroke::new(2.0, Color32::WHITE),
                    StrokeKind::Inside,
                );
            }
        }
    }
    if let Some(Gesture::MidiClip { track, from, to }) = &app.gesture
        && let Some(r) = midi_clip_span(s, *track, *from, *to)
        && let Some((_, row)) = app.edit_layout.rows.iter().find(|(i, _)| *i == track.0)
    {
        let ghost = Rect::from_min_max(pos2(x_of(s, tl, r.start), row[1] + 1.0), pos2(x_of(s, tl, r.end), row[3] - 1.0));
        painter.rect_stroke(ghost, 2.0, Stroke::new(2.0, Color32::WHITE), StrokeKind::Inside);
    }
    if let Some(Gesture::TrimStart { to, clip, .. } | Gesture::TrimEnd { to, clip, .. }) = &app.gesture
        && let Some((tid, _)) = s.find_clip(*clip)
        && let Some((_, r)) = app.edit_layout.rows.iter().find(|(i, _)| *i == tid.0)
    {
        let x = x_of(s, tl, *to);
        painter.line_segment([pos2(x, r[1]), pos2(x, r[3])], Stroke::new(2.0, Color32::WHITE));
    }
    if let Some(at) = scroll_to {
        let _ = app.engine.execute("view.scroll", &json!({"to": at}));
    }
    if matches!(app.gesture, Some(Gesture::Fade { .. })) {
        ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeNwSe);
    } else if matches!(app.gesture, Some(Gesture::FadeCurve { .. })) {
        ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeVertical);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::Harness;

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

    fn harness(n: usize) -> Harness<'static, SoundApp> {
        let mut engine = soundcraft_engine::Engine::default();
        engine.execute("track.new", &json!({"count": n})).unwrap();
        harness_with(engine)
    }

    fn harness_with(engine: soundcraft_engine::Engine) -> Harness<'static, SoundApp> {
        let app = SoundApp::new(engine, None, crate::Services::default());
        let builder = Harness::builder().with_size(vec2(1400.0, 900.0)).with_step_dt(1.0 / 60.0);
        let builder = if std::env::var_os("SOUNDCRAFT_FADE_SHOTS").is_some() { builder.wgpu() } else { builder };
        let mut h = builder.build_ui_state(
            |ui, app: &mut SoundApp| {
                let ctx = ui.ctx().clone();
                app.logic(&ctx);
                app.ui(ui);
            },
            app,
        );
        h.input_mut().max_texture_side = Some(8192);
        h.run_steps(4);
        h
    }

    #[test]
    fn a_file_time_stamp_at_the_end_of_the_range_draws() {
        let mut engine = soundcraft_engine::demo::demo_engine();
        let s = engine.session_mut();
        s.edit.flags.insert("view.clip.time.original".into());
        s.sources.iter_mut().for_each(|src| src.time_reference = i64::MAX as u64);
        harness_with(engine).run_steps(2);
    }

    fn smart_demo() -> Harness<'static, SoundApp> {
        let mut engine = soundcraft_engine::demo::demo_engine();
        engine.execute("edit.tool", &json!({"tool": "smart"})).unwrap();
        engine.execute("edit.mode", &json!({"mode": "slip"})).unwrap();
        harness_with(engine)
    }

    fn row(h: &Harness<SoundApp>, i: usize) -> Rect {
        rect(h.state().edit_layout.rows[i].1)
    }

    fn row_ids(h: &Harness<SoundApp>) -> Vec<TrackId> {
        h.state().edit_layout.rows.iter().map(|(i, _)| TrackId(*i)).collect()
    }

    fn timeline(h: &Harness<SoundApp>) -> Rect {
        rect(h.state().edit_layout.timeline)
    }

    fn lane_point(h: &Harness<SoundApp>, i: usize) -> Pos2 {
        pos2(timeline(h).min.x + 100.0, row(h, i).min.y + 10.0)
    }

    fn clip_on_screen(h: &Harness<SoundApp>, ok: impl Fn(f32, f32) -> bool) -> (Clip, Rect) {
        let (s, tl) = (h.state().engine.session(), timeline(h));
        h.state()
            .edit_layout
            .rows
            .iter()
            .filter_map(|(id, r)| s.track(TrackId(*id)).map(|t| (t, r)))
            .find_map(|(t, r)| {
                let c = t.clips().iter().find(|c| ok(x_of(s, tl, c.start), x_of(s, tl, c.end())))?;
                Some((c.clone(), lane_rect(t, rect(*r), tl)))
            })
            .expect("no such clip on screen")
    }

    fn start_drag(h: &mut Harness<SoundApp>, at: Pos2) {
        h.hover_at(at);
        h.drag_at(at);
        h.hover_at(at + vec2(20.0, 0.0));
        h.step();
    }

    fn drag(h: &mut Harness<SoundApp>, from: Pos2, to: Pos2) {
        h.hover_at(from);
        h.drag_at(from);
        for k in 1..=8 {
            h.hover_at(from + (to - from) * (k as f32 / 8.0));
        }
        h.drop_at(to);
        h.step();
    }

    fn quick_drag(h: &mut Harness<SoundApp>, at: Pos2, to: Pos2) {
        h.hover_at(at);
        h.drag_at(at);
        h.hover_at(at + (to - at) * 0.75);
        h.hover_at(to);
        h.drop_at(to);
        h.step();
    }

    fn click(h: &mut Harness<SoundApp>, at: Pos2) {
        h.hover_at(at);
        h.drag_at(at);
        h.drop_at(at);
        h.step();
    }

    fn click_with(h: &mut Harness<SoundApp>, at: Pos2, mods: egui::Modifiers) {
        h.event(egui::Event::ModifiersChanged(mods));
        click(h, at);
        h.event(egui::Event::ModifiersChanged(egui::Modifiers::NONE));
    }

    fn painted_rows(h: &Harness<SoundApp>, y_of: impl Fn(&Shape) -> Option<f32>) -> Vec<usize> {
        let rows = &h.state().edit_layout.rows;
        let mut out: Vec<usize> = h
            .output()
            .shapes
            .iter()
            .filter_map(|c| y_of(&c.shape))
            .filter_map(|y| rows.iter().position(|(_, r)| (r[1]..=r[3]).contains(&y)))
            .collect();
        out.sort_unstable();
        out.dedup();
        out
    }

    fn highlighted_rows(h: &Harness<SoundApp>) -> Vec<usize> {
        painted_rows(h, |shape| match shape {
            Shape::Rect(r) if r.fill == Tokens::current().selection => Some(r.rect.center().y),
            _ => None,
        })
    }

    fn insertion_rows(h: &Harness<SoundApp>) -> Vec<usize> {
        painted_rows(h, |shape| match shape {
            Shape::LineSegment { points, stroke } if stroke.color == Tokens::current().insertion => Some((points[0].y + points[1].y) / 2.0),
            _ => None,
        })
    }

    #[test]
    fn drag_selection_spans_the_rows_from_anchor_to_pointer() {
        let mut h = harness(4);
        let ids = row_ids(&h);
        for (from, to) in [(3, 2), (0, 1), (2, 0), (1, 3)] {
            let (a, b) = (lane_point(&h, from), lane_point(&h, to) + vec2(80.0, 0.0));
            drag(&mut h, a, b);
            let want = ids.get(from.min(to)..=from.max(to)).unwrap();
            assert_eq!(h.state().engine.session().edit.selected_tracks, want, "drag from row {from} to row {to}");
        }
    }

    #[test]
    fn the_frame_a_drag_reaches_a_row_paints_it_selected() {
        let mut h = harness(4);
        let (a, b) = (lane_point(&h, 3), lane_point(&h, 2) + vec2(60.0, 0.0));
        start_drag(&mut h, a);
        h.hover_at(b);
        h.step();
        assert_eq!(highlighted_rows(&h), [2, 3]);
        h.drop_at(b);
        h.step();
    }

    #[test]
    fn a_drag_paints_each_frame_once() {
        let mut h = harness(4);
        let (a, b) = (lane_point(&h, 3), lane_point(&h, 0) + vec2(300.0, 0.0));
        start_drag(&mut h, a);
        for k in 1..=10 {
            h.hover_at(a + (b - a) * (k as f32 / 10.0));
            h.step();
            assert_eq!(h.output().platform_output.num_completed_passes, 1, "move {k}");
        }
        h.drop_at(b);
        h.step();
    }

    #[test]
    fn a_drag_frame_changes_the_document_once() {
        let mut h = harness(4);
        let (a, b) = (lane_point(&h, 3), lane_point(&h, 2) + vec2(60.0, 0.0));
        start_drag(&mut h, a);
        let rev = h.state().engine.revision;
        h.hover_at(b);
        h.step();
        assert_eq!(h.state().engine.revision, rev + 1);
        h.drop_at(b);
        h.step();
    }

    #[test]
    fn a_drag_selection_starts_where_the_button_went_down() {
        let mut h = harness(4);
        let a = lane_point(&h, 1);
        drag(&mut h, a, a + vec2(200.0, 0.0));
        let s = h.state().engine.session();
        assert_eq!(s.edit.selection.start, sample_at(s, timeline(&h), a.x));
    }

    #[test]
    fn pressing_in_a_lane_places_the_insertion_point_before_release() {
        let mut h = harness(4);
        let a = lane_point(&h, 2);
        h.hover_at(a);
        h.drag_at(a);
        h.step();
        let s = h.state().engine.session();
        assert_eq!(s.edit.selection, Range::point(sample_at(s, timeline(&h), a.x)));
        assert_eq!(s.edit.selected_tracks, row_ids(&h)[2..3]);
        h.drop_at(a);
        h.step();
    }

    #[test]
    fn a_moved_insertion_point_shows_at_once_even_mid_blink() {
        let mut h = harness(4);
        let (a, b) = (lane_point(&h, 0), lane_point(&h, 2) + vec2(150.0, 0.0));
        click(&mut h, a);
        for _ in 0..90 {
            if insertion_rows(&h).is_empty() {
                break;
            }
            h.step();
        }
        assert!(insertion_rows(&h).is_empty(), "the insertion point never blinked off");
        click(&mut h, b);
        assert!(!insertion_rows(&h).is_empty());
    }

    #[test]
    fn the_insertion_point_shows_only_on_selected_tracks() {
        let mut h = harness(4);
        let a = lane_point(&h, 2);
        click(&mut h, a);
        assert_eq!(insertion_rows(&h), [2]);
    }

    #[test]
    fn shift_click_adds_another_track_to_the_selection() {
        let mut h = harness(4);
        let ids = row_ids(&h);
        let (a, b, c) = (lane_point(&h, 0), lane_point(&h, 2) + vec2(100.0, 0.0), lane_point(&h, 3) + vec2(300.0, 0.0));
        drag(&mut h, a, a + vec2(200.0, 0.0));
        let sel = h.state().engine.session().edit.selection;
        click_with(&mut h, b, egui::Modifiers::SHIFT);
        assert_eq!(h.state().engine.session().edit.selected_tracks, [ids[0], ids[2]]);
        assert_eq!(h.state().engine.session().edit.selection, sel);
        click_with(&mut h, c, egui::Modifiers::SHIFT);
        let s = h.state().engine.session();
        assert_eq!(s.edit.selected_tracks, [ids[0], ids[2], ids[3]]);
        assert_eq!(s.edit.selection, Range::new(sel.start, sample_at(s, timeline(&h), c.x)));
        assert_eq!(highlighted_rows(&h), [0, 2, 3]);
    }

    #[test]
    fn shift_and_cmd_click_on_track_names_extend_the_selection() {
        let mut h = harness(5);
        let ids = row_ids(&h);
        let (none, shift, cmd) = (egui::Modifiers::NONE, egui::Modifiers::SHIFT, egui::Modifiers::COMMAND);
        for (i, mods, want) in [(1, none, vec![1]), (3, shift, vec![1, 2, 3]), (2, cmd, vec![1, 3]), (0, cmd, vec![0, 1, 3]), (4, none, vec![4])] {
            let at = row(&h, i).min + vec2(60.0, 10.0);
            click_with(&mut h, at, mods);
            let want: Vec<TrackId> = want.into_iter().map(|k| ids[k]).collect();
            assert_eq!(h.state().engine.session().edit.selected_tracks, want, "row {i} clicked with {mods:?}");
        }
    }

    #[test]
    fn a_quick_drag_from_a_clip_edge_trims_it() {
        let mut h = smart_demo();
        let tl = timeline(&h);
        let (clip, lane) = clip_on_screen(&h, |xs, xe| xe > tl.min.x + 100.0 && xe < tl.max.x - 20.0 && xe - xs > 100.0);
        let press = pos2(x_of(h.state().engine.session(), tl, clip.end()) - 2.0, lane.min.y + lane.height() * 0.75);
        quick_drag(&mut h, press, press - vec2(40.0, 0.0));
        let after = h.state().engine.session().find_clip(clip.id).unwrap().1.clone();
        assert_eq!(after.start, clip.start, "the clip moved instead of being trimmed");
        assert!(after.end() < clip.end(), "the clip end did not move");
    }

    #[test]
    fn a_quick_clip_drag_keeps_the_clip_under_the_pointer() {
        let mut h = smart_demo();
        let tl = timeline(&h);
        let (clip, lane) = clip_on_screen(&h, |xs, xe| xe - xs > 120.0 && (xs + xe) / 2.0 > tl.min.x + 60.0 && (xs + xe) / 2.0 < tl.max.x - 60.0);
        let s = h.state().engine.session();
        let grab = pos2((x_of(s, tl, clip.start) + x_of(s, tl, clip.end())) / 2.0, lane.min.y + lane.height() * 0.75);
        let by = sample_at(s, tl, grab.x + 40.0) - sample_at(s, tl, grab.x);
        quick_drag(&mut h, grab, grab + vec2(40.0, 0.0));
        let after = h.state().engine.session().find_clip(clip.id).unwrap().1.clone();
        assert_eq!(after.start - clip.start, by, "the clip trails the pointer");
    }

    #[test]
    fn fade_drag_ranges_handle_empty_short_and_saturated_clips() {
        let mut c = Clip::audio(ClipId(1), "test", soundcraft_model::SourceId(1), 0, 100, 1);
        for side in [true, false] {
            for to in [i64::MIN, 100, 101, i64::MAX] {
                assert_eq!(fade_drag_range(&c, side, to), Some(Range::new(100, 101)));
            }
            c.length = 0;
            assert_eq!(fade_drag_range(&c, side, 100), None);
            c.length = -1;
            assert_eq!(fade_drag_range(&c, side, 100), None);
            c.length = 1;
        }
        c.start = i64::MAX - 1;
        c.length = 10;
        for side in [true, false] {
            assert_eq!(fade_drag_range(&c, side, i64::MAX), Some(Range::new(i64::MAX - 1, i64::MAX)));
            c.start = i64::MAX;
            assert_eq!(fade_drag_range(&c, side, i64::MAX), None);
            c.start = i64::MAX - 1;
        }
    }

    fn fade_shot(h: &mut Harness<SoundApp>, name: &str) {
        if let Some(dir) = std::env::var_os("SOUNDCRAFT_FADE_SHOTS") {
            let dir = std::path::PathBuf::from(dir);
            std::fs::create_dir_all(&dir).unwrap();
            h.render().unwrap().save(dir.join(format!("{name}.png"))).unwrap();
        }
    }

    fn fade_curves(h: &Harness<SoundApp>, clip_rect: Rect) -> Vec<(egui::epaint::PathShape, Rect)> {
        h.output()
            .shapes
            .iter()
            .filter_map(|shape| match &shape.shape {
                Shape::Path(path)
                    if path.stroke == egui::epaint::PathStroke::new(1.2, Color32::from_rgba_unmultiplied(255, 255, 255, 90))
                        && path.points.iter().all(|point| clip_rect.contains(*point)) =>
                {
                    Some((path.clone(), shape.clip_rect))
                }
                _ => None,
            })
            .collect()
    }

    fn waveform_geometry(h: &Harness<SoundApp>, clip_rect: Rect) -> Vec<Vec<Pos2>> {
        h.output()
            .shapes
            .iter()
            .filter_map(|shape| match &shape.shape {
                // The waveform's columns snap to physical pixels and fade out over half of one.
                Shape::Mesh(mesh) if !mesh.vertices.is_empty() && mesh.vertices.iter().all(|vertex| clip_rect.expand(1.0).contains(vertex.pos)) => {
                    Some(mesh.vertices.iter().map(|vertex| vertex.pos).collect())
                }
                _ => None,
            })
            .collect()
    }

    #[test]
    fn waveforms_follow_fade_drags_before_release_and_match_the_committed_waveform() {
        for channels in [1, 2] {
            for fade_in in [true, false] {
                for bend in [false, true] {
                    for mode in ["peak", "power", "rectified", "outlines"] {
                        let mut h = smart_demo();
                        let tl = timeline(&h);
                        let (clip, lane) = clip_on_screen(&h, |xs, xe| xs > tl.min.x + 30.0 && xe < tl.max.x - 100.0 && xe - xs > 100.0);
                        let (tid, _) = h.state().engine.session().find_clip(clip.id).unwrap();
                        let neighbour = h
                            .state()
                            .engine
                            .session()
                            .track(tid)
                            .unwrap()
                            .clips()
                            .iter()
                            .find(|c| c.id != clip.id && c.start >= clip.end())
                            .unwrap()
                            .clone();
                        let source = clip.source().unwrap();
                        if channels == 2 {
                            let audio = h.state().engine.session().pool.get(source).unwrap().clone();
                            let left = audio.buffer.channels[0].clone();
                            let right = left.iter().map(|sample| sample * 0.5).collect();
                            let audio = soundcraft_model::SourceAudio::new(soundcraft_audio_io::AudioBuffer {
                                sample_rate: audio.buffer.sample_rate,
                                channels: vec![left, right],
                            });
                            let s = h.state_mut().engine.session_mut();
                            s.pool.insert(source, std::sync::Arc::new(audio));
                            s.track_mut(tid).unwrap().format = soundcraft_model::ChannelFormat::Stereo;
                        }
                        if mode == "power" {
                            h.state_mut().engine.execute("view.waveform_power", &json!({})).unwrap();
                        }
                        h.state_mut().engine.execute("view.waveform_rectified", &json!({"value": mode == "rectified"})).unwrap();
                        h.state_mut().engine.execute("view.waveform_outlines", &json!({"value": mode == "outlines"})).unwrap();
                        h.state_mut().engine.execute("view.waveform_zoom", &json!({"value": 1.4})).unwrap();
                        let c = h.state_mut().engine.session_mut().find_clip_mut(clip.id).unwrap();
                        c.gain_db = -3.0;
                        c.gain_env = vec![(0, -3.0), (c.length, 3.0)];
                        let target = soundcraft_model::Fade {
                            len: if bend { c.length * 2 / 3 } else { c.length / 4 },
                            shape: soundcraft_model::FadeShape::SCurve,
                            curvature: 0.0,
                        };
                        let other = soundcraft_model::Fade { len: c.length / 8, shape: soundcraft_model::FadeShape::Linear, curvature: 0.2 };
                        (c.fade_in, c.fade_out) = if fade_in { (target, other) } else { (other, target) };
                        h.step();
                        let before = h.state().engine.session().clone();
                        let undo_len = h.state().engine.undo_history().len();
                        let audio = before.pool.get(source).unwrap().clone();
                        let (xs, xe) = (x_of(&before, tl, clip.start), x_of(&before, tl, clip.end()));
                        let clip_rect = Rect::from_min_max(pos2(xs, lane.min.y), pos2(xe, lane.max.y)).intersect(tl);
                        let neighbour_rect = Rect::from_min_max(
                            pos2(x_of(&before, tl, neighbour.start), lane.min.y),
                            pos2(x_of(&before, tl, neighbour.end()), lane.max.y),
                        )
                        .intersect(tl);
                        let original = waveform_geometry(&h, clip_rect);
                        let neighbour_wave = waveform_geometry(&h, neighbour_rect);
                        assert_eq!(original.len(), channels);
                        let side = if fade_in { "in" } else { "out" };
                        let operation = if bend { "bend" } else { "length" };
                        let screenshot = channels == 1 && mode == "peak";
                        if screenshot {
                            fade_shot(&mut h, &format!("waveform-{side}-{operation}-before"));
                        }
                        let (press, to) = if bend {
                            let c = before.find_clip(clip.id).unwrap().1;
                            let midpoint = fade_midpoint(&before, tl, lane, c, fade_in).unwrap();
                            (midpoint, midpoint - vec2(0.0, 16.0))
                        } else {
                            (
                                pos2(if fade_in { xs + 6.0 } else { xe - 6.0 }, lane.min.y + 4.0),
                                pos2(xs + (xe - xs) * if fade_in { 0.7 } else { 0.3 }, lane.min.y + 4.0),
                            )
                        };
                        h.hover_at(press);
                        h.drag_at(press);
                        h.hover_at(to);
                        h.step();
                        h.step();
                        assert!(if bend {
                            matches!(h.state().gesture, Some(Gesture::FadeCurve { .. }))
                        } else {
                            matches!(h.state().gesture, Some(Gesture::Fade { .. }))
                        });
                        let preview = waveform_geometry(&h, clip_rect);
                        assert_eq!(preview.len(), channels);
                        for channel in 0..channels {
                            assert_ne!(
                                preview[channel], original[channel],
                                "the waveform must respond while the button is held: {side}, {operation}, {mode}, channel={channel}"
                            );
                        }
                        assert_eq!(waveform_geometry(&h, neighbour_rect), neighbour_wave);
                        assert_eq!(h.state().engine.session(), &before);
                        assert_eq!(h.state().engine.undo_history().len(), undo_len);
                        assert!(
                            std::sync::Arc::ptr_eq(h.state().engine.session().pool.get(source).unwrap(), &audio),
                            "preview must retain cached media and peaks"
                        );
                        if screenshot {
                            fade_shot(&mut h, &format!("waveform-{side}-{operation}-preview"));
                        }
                        h.drop_at(to);
                        h.step();
                        assert!(h.state().gesture.is_none());
                        assert_eq!(waveform_geometry(&h, clip_rect), preview, "release must not cause a waveform jump");
                        assert_eq!(waveform_geometry(&h, neighbour_rect), neighbour_wave);
                        assert_eq!(h.state().engine.undo_history().len(), undo_len + 1);
                        assert!(std::sync::Arc::ptr_eq(h.state().engine.session().pool.get(source).unwrap(), &audio));
                        if screenshot {
                            fade_shot(&mut h, &format!("waveform-{side}-{operation}-released"));
                        }
                        assert!(h.state_mut().engine.undo());
                        assert_eq!(h.state().engine.session(), &before);
                        h.step();
                        assert_eq!(waveform_geometry(&h, clip_rect), original);
                    }
                }
            }
        }
    }

    #[test]
    fn fade_drag_previews_and_commits_stay_inside_the_clip() {
        for fade_in in [true, false] {
            for release_outside in [false, true] {
                let mut h = smart_demo();
                let tl = timeline(&h);
                let (clip, lane) = clip_on_screen(&h, |xs, xe| xs > tl.min.x + 30.0 && xe < tl.max.x - 100.0 && xe - xs > 100.0);
                let s = h.state().engine.session();
                let (xs, xe) = (x_of(s, tl, clip.start), x_of(s, tl, clip.end()));
                let corner = pos2(if fade_in { xs + 6.0 } else { xe - 6.0 }, lane.min.y + 4.0);
                let before = s.clone();
                let undo_len = h.state().engine.undo_history().len();
                start_drag(&mut h, corner);
                assert!(matches!(h.state().gesture, Some(Gesture::Fade { fade_in: side, .. }) if side == fade_in));
                let name = if fade_in { "fade-in" } else { "fade-out" };
                for (label, x) in [("left", xs - 60.0), ("right", xe + 60.0), ("inside", (xs + xe) / 2.0)] {
                    h.hover_at(pos2(x, corner.y));
                    h.step();
                    let s = h.state().engine.session();
                    let range = fade_drag_range(&clip, fade_in, sample_at(s, tl, x)).unwrap();
                    let curves = fade_curves(&h, Rect::from_min_max(pos2(xs, lane.min.y), pos2(xe, lane.max.y)));
                    assert_eq!(curves.len(), 1, "only one fade curve should be painted");
                    let (curve, clip_rect) = &curves[0];
                    let points = &curve.points;
                    assert_eq!(points.first().unwrap().x, x_of(s, tl, range.start));
                    assert_eq!(points.last().unwrap().x, x_of(s, tl, range.end));
                    assert!(points.iter().all(|p| p.x >= xs && p.x <= xe && lane.contains(*p)));
                    assert!(clip_rect.min.x >= xs && clip_rect.max.x <= xe);
                    assert_eq!(h.state().engine.session(), &before, "drag must only preview");
                    if !release_outside {
                        fade_shot(&mut h, &format!("{name}-{label}"));
                    }
                }
                // Releasing at a new point without an intervening move must use that point.
                let release = pos2(if release_outside { if fade_in { xe + 60.0 } else { xs - 60.0 } } else { xs + (xe - xs) * 0.3 }, corner.y);
                h.drop_at(release);
                h.step();
                assert!(h.state().gesture.is_none());
                let range = fade_drag_range(&clip, fade_in, sample_at(&before, tl, release.x)).unwrap();
                let mut expected = before.clone();
                let c = expected.find_clip_mut(clip.id).unwrap();
                let fade = soundcraft_model::Fade { len: range.end - range.start, shape: soundcraft_model::FadeShape::SCurve, curvature: 0.0 };
                if fade_in {
                    c.fade_in = fade;
                } else {
                    c.fade_out = fade;
                }
                assert_eq!(h.state().engine.session(), &expected);
                assert_eq!(h.state().engine.undo_history().len(), undo_len + 1);
                if release_outside {
                    assert_eq!(fade.len, clip.length);
                    fade_shot(&mut h, &format!("{name}-committed"));
                }
                assert!(h.state_mut().engine.undo());
                assert_eq!(h.state().engine.session(), &before);
                assert!(h.state_mut().engine.redo());
                assert_eq!(h.state().engine.session(), &expected);
            }
        }
    }

    #[test]
    fn a_fade_drag_replaces_the_old_curve_and_matches_the_released_curve() {
        for fade_in in [true, false] {
            for show_name in [true, false] {
                let mut h = smart_demo();
                h.state_mut().engine.execute("view.clip_name", &json!({"value": show_name})).unwrap();
                h.step();
                let tl = timeline(&h);
                let (clip, lane) = clip_on_screen(&h, |xs, xe| xs > tl.min.x + 30.0 && xe < tl.max.x - 100.0 && xe - xs > 100.0);
                let c = h.state_mut().engine.session_mut().find_clip_mut(clip.id).unwrap();
                c.fade_in = soundcraft_model::Fade { len: c.length / 8, shape: soundcraft_model::FadeShape::Linear, curvature: 0.0 };
                c.fade_out = soundcraft_model::Fade { len: c.length / 8, shape: soundcraft_model::FadeShape::EqualPower, curvature: 0.0 };
                h.step();
                let s = h.state().engine.session();
                let (xs, xe) = (x_of(s, tl, clip.start), x_of(s, tl, clip.end()));
                let clip_rect = Rect::from_min_max(pos2(xs, lane.min.y), pos2(xe, lane.max.y));
                let original = fade_curves(&h, clip_rect);
                assert_eq!(original.len(), 2);
                let (edited, opposite) = if fade_in { (0, 1) } else { (1, 0) };
                let before = s.clone();
                let corner = pos2(if fade_in { xs + 6.0 } else { xe - 6.0 }, lane.min.y + 4.0);
                start_drag(&mut h, corner);
                let to = pos2(if fade_in { xs + (xe - xs) * 0.65 } else { xs + (xe - xs) * 0.35 }, corner.y);
                h.hover_at(to);
                h.step();
                let preview = fade_curves(&h, clip_rect);
                assert_eq!(preview.len(), 2, "one curve per side, without the old curve underneath");
                assert_ne!(preview[edited], original[edited]);
                assert_eq!(preview[opposite], original[opposite]);
                assert_eq!(h.state().engine.session(), &before);
                assert!(!h.output().shapes.iter().any(|shape| {
                    matches!(&shape.shape, Shape::LineSegment { points, stroke } if stroke.width == 2.0 && stroke.color == Color32::WHITE && points.iter().all(|p| clip_rect.contains(*p)))
                }), "the straight preview must be gone");
                let side = if fade_in { "in" } else { "out" };
                if show_name {
                    fade_shot(&mut h, &format!("fade-{side}-curve-dragging"));
                }
                h.drop_at(to);
                h.step();
                assert_eq!(fade_curves(&h, clip_rect), preview, "curve geometry, thickness and colour must be identical after release");
                if show_name {
                    fade_shot(&mut h, &format!("fade-{side}-curve-released"));
                }
            }
        }
    }

    #[test]
    fn fade_curve_sampling_keeps_steep_bends_below_a_quarter_pixel_error() {
        for (width, height) in [(420.0, 180.0), (75.0, 320.0), (10_000.0, 80.0)] {
            let body = Rect::from_min_size(Pos2::ZERO, vec2(width, height));
            for scale in [1.0, 2.0, 4.0] {
                for shape in soundcraft_model::FadeShape::ALL {
                    for curvature in [-1.0, 1.0] {
                        for fade_in in [true, false] {
                            let fade = soundcraft_model::Fade { len: 1000, shape, curvature };
                            let points = fade_curve_points(fade, fade_in, 0.0, width, body, scale);
                            assert!(points.len() > 17 && points.len() <= 4097);
                            assert_eq!((points.len() - 1) % 2, 0);
                            let segments = (points.len() - 1) as f32;
                            for (i, pair) in points.windows(2).enumerate() {
                                let line = pair[1] - pair[0];
                                for fraction in [0.25, 0.5, 0.75] {
                                    let t = (i as f32 + fraction) / segments;
                                    let actual = pos2(width * t, height - height * fade.gain(if fade_in { t } else { 1.0 - t }));
                                    let along = ((actual - pair[0]).dot(line) / line.length_sq()).clamp(0.0, 1.0);
                                    let distance = actual.distance(pair[0] + line * along) * scale;
                                    assert!(
                                        distance < 0.25,
                                        "{width}x{height}, scale={scale}, {shape:?}, curve={curvature}, side={fade_in}, error={distance}"
                                    );
                                }
                            }
                        }
                    }
                }
            }
        }
        let body = Rect::from_min_size(Pos2::ZERO, vec2(1e20, 1e20));
        for scale in [f32::NAN, f32::INFINITY, 1.0] {
            let points = fade_curve_points(soundcraft_model::Fade::default(), true, 0.0, 1e20, body, scale);
            assert!(points.len() <= 4097);
        }
    }

    fn painted_fade_control(h: &Harness<SoundApp>, clip_rect: Rect) -> Pos2 {
        let controls: Vec<_> = h
            .output()
            .shapes
            .iter()
            .filter_map(|shape| match &shape.shape {
                Shape::Circle(circle) if circle.radius == 3.5 && clip_rect.contains(circle.center) => {
                    let radius = circle.radius + circle.stroke.width * 0.5;
                    for offset in [vec2(radius, 0.0), vec2(-radius, 0.0), vec2(0.0, radius), vec2(0.0, -radius)] {
                        assert!(shape.clip_rect.contains(circle.center + offset), "the complete control must remain visible");
                    }
                    Some(circle.center)
                }
                _ => None,
            })
            .collect();
        assert_eq!(controls.len(), 1, "the midpoint must stay visible without hover or a held mouse button");
        controls[0]
    }

    #[test]
    fn fade_midpoint_handles_remain_visible_and_can_be_grabbed_again() {
        for fade_in in [true, false] {
            for show_name in [true, false] {
                for shape in [soundcraft_model::FadeShape::SCurve, soundcraft_model::FadeShape::Exponential] {
                    for height in ["micro", "jumbo"] {
                        let mut h = smart_demo();
                        let tl = timeline(&h);
                        let (clip, _) = clip_on_screen(&h, |xs, xe| xs > tl.min.x + 30.0 && xe < tl.max.x - 100.0 && xe - xs > 100.0);
                        let (tid, _) = h.state().engine.session().find_clip(clip.id).unwrap();
                        let spp = h.state().engine.session().edit.zoom.samples_per_px * 0.5;
                        h.state_mut().engine.execute("view.clip_name", &json!({"value": show_name})).unwrap();
                        h.state_mut().engine.execute("track.height", &json!({"tracks": [tid.0], "height": height})).unwrap();
                        h.state_mut().engine.execute("view.zoom_set", &json!({"samples_per_px": spp})).unwrap();
                        h.state_mut().engine.execute("view.scroll", &json!({"to": clip.start - clip.length / 5})).unwrap();
                        let c = h.state_mut().engine.session_mut().find_clip_mut(clip.id).unwrap();
                        let fade = soundcraft_model::Fade { len: c.length * 2 / 3, shape, curvature: 0.0 };
                        if fade_in {
                            c.fade_in = fade;
                        } else {
                            c.fade_out = fade;
                        }
                        h.step();
                        let row = h.state().edit_layout.rows.iter().find(|(id, _)| *id == tid.0).unwrap().1;
                        let lane = lane_rect(h.state().engine.session().track(tid).unwrap(), rect(row), tl);
                        let s = h.state().engine.session();
                        let clip_rect = Rect::from_min_max(pos2(x_of(s, tl, clip.start), lane.min.y), pos2(x_of(s, tl, clip.end()), lane.max.y));
                        let control = painted_fade_control(&h, clip_rect);
                        let undo_len = h.state().engine.undo_history().len();
                        start_drag(&mut h, control);
                        let outside = control + vec2(0.0, 500.0);
                        h.hover_at(outside);
                        h.step();
                        h.drop_at(outside);
                        h.step();
                        let c = h.state().engine.session().find_clip(clip.id).unwrap().1;
                        assert_eq!(if fade_in { c.fade_in.curvature } else { c.fade_out.curvature }, -1.0);
                        assert!(h.state().gesture.is_none());
                        painted_fade_control(&h, clip_rect);
                        let side = if fade_in { "in" } else { "out" };
                        let screenshot = show_name && height == "jumbo" && shape == soundcraft_model::FadeShape::SCurve;
                        if screenshot {
                            fade_shot(&mut h, &format!("fade-{side}-smooth-released"));
                        }
                        for iteration in 0..2 {
                            let before = h.state().engine.session().clone();
                            let control = painted_fade_control(&h, clip_rect);
                            h.hover_at(control);
                            h.step();
                            assert_eq!(h.output().platform_output.cursor_icon, egui::CursorIcon::ResizeVertical);
                            start_drag(&mut h, control);
                            let to = control - vec2(0.0, (lane.height() - 2.0 - clip_name_height(&before, lane)) * 0.1);
                            h.hover_at(to);
                            h.step();
                            assert!(
                                matches!(h.state().gesture, Some(Gesture::FadeCurve { clip: id, fade_in: side, .. }) if id == clip.id && side == fade_in)
                            );
                            h.drop_at(to);
                            h.step();
                            let c = h.state().engine.session().find_clip(clip.id).unwrap().1;
                            let current = if fade_in { c.fade_in } else { c.fade_out };
                            let previous = before.find_clip(clip.id).unwrap().1;
                            let previous = if fade_in { previous.fade_in } else { previous.fade_out };
                            assert!(current.curvature > previous.curvature);
                            assert_eq!((current.len, current.shape), (previous.len, previous.shape));
                            painted_fade_control(&h, clip_rect);
                            assert_eq!(h.state().engine.undo_history().len(), undo_len + iteration + 2);
                            if screenshot && iteration == 1 {
                                fade_shot(&mut h, &format!("fade-{side}-smooth-regrabbed"));
                            }
                            let after = h.state().engine.session().clone();
                            assert!(h.state_mut().engine.undo());
                            assert_eq!(h.state().engine.session(), &before);
                            assert!(h.state_mut().engine.redo());
                            assert_eq!(h.state().engine.session(), &after);
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn fade_midpoints_drag_vertically_without_moving_the_endpoints() {
        for fade_in in [true, false] {
            for show_name in [true, false] {
                for dy in [-16.0, 16.0] {
                    let mut h = smart_demo();
                    h.state_mut().engine.execute("view.clip_name", &json!({"value": show_name})).unwrap();
                    h.step();
                    let tl = timeline(&h);
                    let (clip, lane) = clip_on_screen(&h, |xs, xe| xs > tl.min.x + 30.0 && xe < tl.max.x - 100.0 && xe - xs > 100.0);
                    let c = h.state_mut().engine.session_mut().find_clip_mut(clip.id).unwrap();
                    let target = soundcraft_model::Fade { len: c.length * 2 / 3, shape: soundcraft_model::FadeShape::SCurve, curvature: 0.0 };
                    let other = soundcraft_model::Fade { len: c.length / 8, shape: soundcraft_model::FadeShape::Linear, curvature: -0.2 };
                    (c.fade_in, c.fade_out) = if fade_in { (target, other) } else { (other, target) };
                    h.step();
                    let before = h.state().engine.session().clone();
                    let clip_rect =
                        Rect::from_min_max(pos2(x_of(&before, tl, clip.start), lane.min.y), pos2(x_of(&before, tl, clip.end()), lane.max.y));
                    let original = fade_curves(&h, clip_rect);
                    let (edited, opposite) = if fade_in { (0, 1) } else { (1, 0) };
                    let middle = original[edited].0.points.len() / 2;
                    let midpoint = original[edited].0.points[middle];
                    let undo_len = h.state().engine.undo_history().len();
                    h.hover_at(midpoint);
                    h.step();
                    assert_eq!(h.output().platform_output.cursor_icon, egui::CursorIcon::ResizeVertical);
                    click(&mut h, midpoint);
                    assert_eq!(h.state().engine.session(), &before, "clicking the curve control should not change the selection");
                    h.drag_at(midpoint);
                    let to = midpoint + vec2(0.0, dy);
                    h.hover_at(to);
                    h.step();
                    h.step(); // Paint the frame requested when the drag gesture starts.
                    let curvature = match h.state().gesture {
                        Some(Gesture::FadeCurve { clip: id, fade_in: side, curvature, .. }) if id == clip.id && side == fade_in => curvature,
                        _ => panic!("the midpoint must start a curve gesture"),
                    };
                    assert_eq!(h.output().platform_output.cursor_icon, egui::CursorIcon::ResizeVertical);
                    assert!(if dy < 0.0 { curvature > 0.0 } else { curvature < 0.0 });
                    let preview = fade_curves(&h, clip_rect);
                    assert_eq!(preview.len(), 2);
                    assert_eq!(preview[opposite], original[opposite]);
                    assert_eq!(preview[edited].0.points.first(), original[edited].0.points.first());
                    assert_eq!(preview[edited].0.points.last(), original[edited].0.points.last());
                    assert_eq!(preview[edited].0.points[middle].x, midpoint.x);
                    assert!(
                        (preview[edited].0.points[middle].y - (midpoint.y + dy)).abs() < 0.001,
                        "side={fade_in}, names={show_name}, dy={dy}, midpoint={midpoint:?}, drawn={:?}, gesture={:?}",
                        preview[edited].0.points[middle],
                        h.state().gesture
                    );
                    assert_eq!(h.state().engine.session(), &before);
                    let side = if fade_in { "in" } else { "out" };
                    let direction = if dy < 0.0 { "up" } else { "down" };
                    if show_name {
                        fade_shot(&mut h, &format!("fade-{side}-bend-{direction}-preview"));
                    }
                    h.drop_at(to);
                    h.step();
                    let mut expected = before.clone();
                    let c = expected.find_clip_mut(clip.id).unwrap();
                    if fade_in {
                        c.fade_in.curvature = curvature;
                    } else {
                        c.fade_out.curvature = curvature;
                    }
                    assert_eq!(h.state().engine.session(), &expected);
                    assert_eq!(fade_curves(&h, clip_rect), preview, "the released envelope must match the preview");
                    let rel = if fade_in { target.len / 2 } else { clip.length - target.len / 2 };
                    let previous_gain = before.find_clip(clip.id).unwrap().1.gain_at(rel);
                    let new_gain = expected.find_clip(clip.id).unwrap().1.gain_at(rel);
                    assert!(if dy < 0.0 { new_gain > previous_gain } else { new_gain < previous_gain });
                    assert_eq!(h.state().engine.undo_history().len(), undo_len + 1);
                    if show_name {
                        fade_shot(&mut h, &format!("fade-{side}-bend-{direction}-released"));
                    }
                    assert!(h.state_mut().engine.undo());
                    assert_eq!(h.state().engine.session(), &before);
                    assert!(h.state_mut().engine.redo());
                    assert_eq!(h.state().engine.session(), &expected);
                }
            }
        }
    }

    #[test]
    fn fade_curve_drags_cancel_safely_or_return_to_the_original_curve() {
        for invalidate in [0, 1, 2] {
            let mut h = smart_demo();
            let tl = timeline(&h);
            let (clip, lane) = clip_on_screen(&h, |xs, xe| xs > tl.min.x + 30.0 && xe < tl.max.x - 100.0 && xe - xs > 100.0);
            let c = h.state_mut().engine.session_mut().find_clip_mut(clip.id).unwrap();
            c.fade_out = soundcraft_model::Fade { len: c.length * 2 / 3, shape: soundcraft_model::FadeShape::SCurve, curvature: 0.0 };
            h.step();
            let midpoint =
                fade_midpoint(h.state().engine.session(), tl, lane, h.state().engine.session().find_clip(clip.id).unwrap().1, false).unwrap();
            let before = h.state().engine.session().clone();
            let undo_len = h.state().engine.undo_history().len();
            start_drag(&mut h, midpoint);
            h.hover_at(midpoint - vec2(0.0, 16.0));
            h.step();
            assert!(matches!(h.state().gesture, Some(Gesture::FadeCurve { .. })));
            if invalidate == 1 {
                h.state_mut().engine.session_mut().find_clip_mut(clip.id).unwrap().length = 0;
            } else if invalidate == 2 {
                let (tid, _) = before.find_clip(clip.id).unwrap();
                h.state_mut().engine.session_mut().track_mut(tid).unwrap().playlist_mut().unwrap().clips.retain(|c| c.id != clip.id);
            }
            h.hover_at(midpoint);
            h.step();
            h.drop_at(midpoint);
            h.step();
            assert!(h.state().gesture.is_none());
            assert_eq!(h.state().engine.undo_history().len(), undo_len);
            if invalidate == 0 {
                assert_eq!(h.state().engine.session(), &before);
            }
        }
    }

    #[test]
    fn existing_fades_drag_from_their_upper_curve_endpoints() {
        for fade_in in [true, false] {
            for show_name in [true, false] {
                for full_length in [false, true] {
                    let mut h = smart_demo();
                    h.state_mut().engine.execute("view.clip_name", &json!({"value": show_name})).unwrap();
                    h.step();
                    let tl = timeline(&h);
                    let (clip, lane) = clip_on_screen(&h, |xs, xe| xs > tl.min.x + 30.0 && xe < tl.max.x - 100.0 && xe - xs > 100.0);
                    let len = if full_length { clip.length } else { clip.length * 2 / 3 };
                    let c = h.state_mut().engine.session_mut().find_clip_mut(clip.id).unwrap();
                    let fade = soundcraft_model::Fade { len, shape: soundcraft_model::FadeShape::SCurve, curvature: 0.35 };
                    if fade_in {
                        c.fade_in = fade;
                    } else {
                        c.fade_out = fade;
                    }
                    h.step();
                    let s = h.state().engine.session();
                    let (xs, xe) = (x_of(s, tl, clip.start), x_of(s, tl, clip.end()));
                    let curves = fade_curves(&h, Rect::from_min_max(pos2(xs, lane.min.y), pos2(xe, lane.max.y)));
                    assert_eq!(curves.len(), 1);
                    let curve = &curves[0].0;
                    // Take the handle from the painted curve, rather than duplicating hit geometry.
                    let head = *if fade_in { curve.points.last() } else { curve.points.first() }.unwrap();
                    let press = head + vec2(3.0, 2.0);
                    if fade_in && full_length {
                        let (tid, _) = s.find_clip(clip.id).unwrap();
                        assert_ne!(
                            clip_at(s.track(tid).unwrap(), sample_at(s, tl, press.x)).unwrap().id,
                            clip.id,
                            "the pointer should be over the adjacent clip"
                        );
                    }
                    let before = s.clone();
                    let undo_len = h.state().engine.undo_history().len();
                    h.hover_at(press);
                    h.step();
                    assert_eq!(h.output().platform_output.cursor_icon, egui::CursorIcon::ResizeNwSe);
                    let side = if fade_in { "in" } else { "out" };
                    if show_name && !full_length {
                        fade_shot(&mut h, &format!("fade-{side}-endpoint-hover"));
                    }
                    h.drag_at(press);
                    let to = head + vec2(if fade_in { -35.0 } else { 35.0 }, 0.0);
                    h.hover_at(to);
                    h.step();
                    assert!(matches!(h.state().gesture, Some(Gesture::Fade { clip: id, fade_in: side, .. }) if id == clip.id && side == fade_in));
                    assert_eq!(h.output().platform_output.cursor_icon, egui::CursorIcon::ResizeNwSe);
                    assert_eq!(h.state().engine.session(), &before, "pressing the handle must not move the edit selection");
                    if show_name && !full_length {
                        fade_shot(&mut h, &format!("fade-{side}-endpoint-dragging"));
                    }
                    h.drop_at(to);
                    h.step();
                    let range = fade_drag_range(&clip, fade_in, sample_at(&before, tl, to.x)).unwrap();
                    let mut expected = before.clone();
                    let c = expected.find_clip_mut(clip.id).unwrap();
                    let fade = soundcraft_model::Fade { len: range.end - range.start, shape: soundcraft_model::FadeShape::SCurve, curvature: 0.35 };
                    assert!(fade.len < len);
                    if fade_in {
                        c.fade_in = fade;
                    } else {
                        c.fade_out = fade;
                    }
                    assert_eq!(h.state().engine.session(), &expected);
                    assert_eq!(h.state().engine.undo_history().len(), undo_len + 1);
                    assert!(h.state_mut().engine.undo());
                    assert_eq!(h.state().engine.session(), &before);
                }
            }
        }
    }

    #[test]
    fn existing_fade_handles_preserve_selector_and_other_smart_tool_zones() {
        let mut h = smart_demo();
        let tl = timeline(&h);
        let (clip, lane) = clip_on_screen(&h, |xs, xe| xs > tl.min.x + 30.0 && xe < tl.max.x - 100.0 && xe - xs > 100.0);
        let c = h.state_mut().engine.session_mut().find_clip_mut(clip.id).unwrap();
        c.fade_out = soundcraft_model::Fade { len: c.length / 2, shape: soundcraft_model::FadeShape::SCurve, curvature: 0.0 };
        h.step();
        let s = h.state().engine.session();
        let rect = Rect::from_min_max(pos2(x_of(s, tl, clip.start), lane.min.y), pos2(x_of(s, tl, clip.end()), lane.max.y));
        let curves = fade_curves(&h, rect);
        let head = *curves[0].0.points.first().unwrap();
        h.hover_at(head + vec2(0.0, 16.0));
        h.step();
        assert_eq!(h.output().platform_output.cursor_icon, egui::CursorIcon::Text);
        h.state_mut().engine.execute("edit.tool", &json!({"tool": "selector"})).unwrap();
        h.step();
        h.hover_at(head);
        h.step();
        assert_eq!(h.output().platform_output.cursor_icon, egui::CursorIcon::Text);
        start_drag(&mut h, head);
        assert!(matches!(h.state().gesture, Some(Gesture::Select { .. })));
        h.drop_at(head + vec2(20.0, 0.0));
        h.step();
    }

    #[test]
    fn a_fade_drag_handles_its_clip_shrinking_to_one_sample() {
        for fade_in in [true, false] {
            let mut h = smart_demo();
            let tl = timeline(&h);
            let (clip, lane) = clip_on_screen(&h, |xs, xe| xs > tl.min.x + 20.0 && xe < tl.max.x - 20.0 && xe - xs > 100.0);
            let s = h.state().engine.session();
            let corner = pos2(x_of(s, tl, if fade_in { clip.start } else { clip.end() }) + if fade_in { 6.0 } else { -6.0 }, lane.min.y + 4.0);
            start_drag(&mut h, corner);
            assert!(matches!(h.state().gesture, Some(Gesture::Fade { .. })));
            let c = h.state_mut().engine.session_mut().find_clip_mut(clip.id).unwrap();
            c.length = 1;
            c.clamp_fades();
            let undo_len = h.state().engine.undo_history().len();
            h.drop_at(corner + vec2(40.0, 0.0));
            h.step();
            let c = h.state().engine.session().find_clip(clip.id).unwrap().1;
            assert_eq!(if fade_in { c.fade_in.len } else { c.fade_out.len }, 1);
            assert_eq!(h.state().engine.undo_history().len(), undo_len + 1);
            assert!(h.state().gesture.is_none());
        }
    }

    #[test]
    fn a_fade_drag_cancels_if_its_clip_disappears() {
        let mut h = smart_demo();
        let tl = timeline(&h);
        let (clip, lane) = clip_on_screen(&h, |xs, xe| xs > tl.min.x + 20.0 && xe - xs > 100.0);
        let corner = pos2(x_of(h.state().engine.session(), tl, clip.start) + 6.0, lane.min.y + 4.0);
        start_drag(&mut h, corner);
        assert!(matches!(h.state().gesture, Some(Gesture::Fade { .. })));
        let (tid, _) = h.state().engine.session().find_clip(clip.id).unwrap();
        h.state_mut().engine.session_mut().track_mut(tid).unwrap().playlist_mut().unwrap().clips.retain(|c| c.id != clip.id);
        let undo_len = h.state().engine.undo_history().len();
        h.hover_at(corner + vec2(40.0, 0.0));
        h.step();
        h.drop_at(corner + vec2(40.0, 0.0));
        h.step();
        assert!(h.state().gesture.is_none());
        assert_eq!(h.state().engine.undo_history().len(), undo_len);
    }

    #[test]
    fn a_fade_drag_survives_its_clip_shrinking_to_nothing() {
        let mut h = smart_demo();
        let tl = timeline(&h);
        let (clip, lane) = clip_on_screen(&h, |xs, xe| xs > tl.min.x + 20.0 && xe - xs > 100.0);
        let corner = pos2(x_of(h.state().engine.session(), tl, clip.start) + 6.0, lane.min.y + 4.0);
        start_drag(&mut h, corner);
        assert!(matches!(h.state().gesture, Some(Gesture::Fade { .. })));
        if let Some(c) = h.state_mut().engine.session_mut().find_clip_mut(clip.id) {
            c.length = 0;
        }
        let undo_len = h.state().engine.undo_history().len();
        h.hover_at(corner + vec2(20.0, 0.0));
        h.step();
        assert!(h.state().gesture.is_none());
        h.drop_at(corner + vec2(20.0, 0.0));
        h.step();
        assert!(h.state().gesture.is_none());
        assert_eq!(h.state().engine.undo_history().len(), undo_len);
    }

    #[test]
    fn a_drag_keeps_going_when_its_row_scrolls_away() {
        let mut h = harness(20);
        let (ids, a) = (row_ids(&h), lane_point(&h, 0));
        start_drag(&mut h, a);
        h.state_mut().edit_layout.scroll_y = 500.0;
        h.step();
        assert!(row(&h, 0).max.y < timeline(&h).min.y, "the first row is still on screen");
        let b = lane_point(&h, 8);
        h.hover_at(b);
        h.drop_at(b);
        h.step();
        assert_eq!(h.state().engine.session().edit.selected_tracks, ids[..=8]);
        assert!(h.state().gesture.is_none(), "the drag outlived the release");
    }

    #[test]
    fn a_gesture_left_over_after_the_release_is_dropped() {
        let mut h = harness(1);
        h.state_mut().gesture = Some(Gesture::Scrub { last: 0 });
        h.step();
        assert!(h.state().gesture.is_none());
    }

    #[test]
    fn a_flick_past_the_last_track_selects_down_to_it() {
        let mut h = harness(4);
        let ids = row_ids(&h);
        let a = lane_point(&h, 1);
        let below = pos2(a.x + 60.0, row(&h, 3).max.y + 100.0);
        start_drag(&mut h, a);
        h.hover_at(below);
        h.drop_at(below);
        h.step();
        assert_eq!(h.state().engine.session().edit.selected_tracks, ids[1..]);
    }

    #[test]
    fn dragging_past_the_bottom_scrolls_and_keeps_selecting() {
        let mut h = harness(20);
        let first = row_ids(&h)[0];
        let a = lane_point(&h, 0);
        let below = pos2(a.x + 60.0, timeline(&h).max.y + 40.0);
        start_drag(&mut h, a);
        h.hover_at(below);
        h.step();
        h.run_steps(60);
        assert!(h.state().edit_layout.scroll_y > 300.0, "scrolled {} px", h.state().edit_layout.scroll_y);
        h.drop_at(below);
        h.step();
        let tl = timeline(&h);
        let last_seen = h.state().edit_layout.rows.iter().filter(|(_, r)| r[1] < tl.max.y).count();
        let sel = &h.state().engine.session().edit.selected_tracks;
        assert_eq!((sel.first(), sel.len()), (Some(&first), last_seen));
    }

    #[test]
    fn dragging_past_the_right_edge_scrolls_the_timeline() {
        let mut h = harness(2);
        let a = lane_point(&h, 0);
        let right = pos2(timeline(&h).max.x + 40.0, a.y);
        let s = h.state().engine.session();
        let (before, pointed) = (s.edit.zoom.scroll, sample_at(s, timeline(&h), right.x));
        start_drag(&mut h, a);
        h.hover_at(right);
        h.step();
        h.run_steps(30);
        h.drop_at(right);
        h.step();
        let s = h.state().engine.session();
        assert!(s.edit.zoom.scroll > before, "the timeline did not scroll");
        assert!(s.edit.selection.end > pointed, "the selection did not follow the scroll");
    }

    #[test]
    fn a_redrawn_frame_counts_once_in_the_frame_time() {
        let mut h = harness(4);
        let mut t = 1.0;
        let mut frame = |h: &mut Harness<SoundApp>| {
            t += 1.0 / 60.0;
            h.input_mut().time = Some(t);
            h.step();
        };
        for _ in 0..120 {
            frame(&mut h);
        }
        let button = |pos: Pos2, pressed: bool| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        for k in 0..30 {
            let at = lane_point(&h, k % 4) + vec2(k as f32 * 7.0, 0.0);
            h.hover_at(at);
            frame(&mut h);
            h.event(button(at, true));
            frame(&mut h);
            h.event(button(at, false));
            frame(&mut h);
        }
        let ms = h.state().frame_ms;
        assert!((ms - 1000.0 / 60.0).abs() < 0.5, "{ms} ms/frame");
    }

    #[test]
    fn dragging_a_name_plate_down_drops_it_where_released() {
        let mut h = harness(4);
        let order = |h: &Harness<SoundApp>| -> Vec<TrackId> { h.state().engine.session().tracks.iter().map(|t| t.id).collect() };
        let before = order(&h);
        let grab = row(&h, 0).min + vec2(40.0, 12.0);
        let release = pos2(grab.x, row(&h, 2).min.y);
        drag(&mut h, grab, release);
        let want: Vec<TrackId> = [1, 0, 2, 3].iter().map(|&i| before[i]).collect();
        assert_eq!(order(&h), want);
    }

    #[test]
    fn a_pencil_click_on_a_plugin_lane_uses_the_plugin_range() {
        let mut engine = soundcraft_engine::demo::demo_engine();
        engine.execute("track.view", &json!({"track": "Kick", "view": "plugin:a:input_gain"})).unwrap();
        engine.execute("edit.tool", &json!({"tool": "pencil"})).unwrap();
        let mut h = harness_with(engine);
        let s = h.state().engine.session_arc();
        let kick = s.track_by_name("Kick").unwrap();
        let i = h.state().edit_layout.rows.iter().position(|(id, _)| *id == kick.id.0).unwrap();
        let lane = lane_rect(kick, row(&h, i), timeline(&h));
        // A quarter up the lane: -12 dB in the Input range of -24..24 dB.
        let at = pos2(lane.min.x + 100.0, lane.max.y - 4.0 - (lane.height() - 8.0) * 0.25);
        click(&mut h, at);
        let s = h.state().engine.session_arc();
        let param = AutoParam::Plugin { slot: 0, param: "input_gain".into() };
        let points = s.track_by_name("Kick").and_then(|t| t.lane(&param)).map(|l| l.points.clone()).unwrap_or_default();
        assert_eq!(points.len(), 1, "status: {}", h.state().ui.status);
        assert!((points[0].value + 12.0).abs() < 0.01, "{}", points[0].value);
    }
}
