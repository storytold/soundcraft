//! The MIDI Editor: a piano roll with a ruler and a velocity lane, docked under the Edit window.

use crate::SoundApp;
use crate::theme::{Tokens, bold, regular};
use egui::{Align2, Color32, CursorIcon, Pos2, Rect, Sense, Stroke, StrokeKind, Ui, pos2, vec2};
use serde_json::json;
use soundcraft_midi::Note;
use soundcraft_model::{ClipContent, ClipId, EditMode, Tool, TrackId};
use soundcraft_time::{TICKS_PER_QUARTER, TempoMap};

const KEY_W: f32 = 48.0;
const RULER_H: f32 = 18.0;
const VEL_H: f32 = 64.0;
/// Default, smallest and largest key row height (the vertical zoom).
const ROW_H: f32 = 9.0;
const ROW_MIN: f32 = 4.0;
const ROW_MAX: f32 = 28.0;
/// Widest note-edge trim handle; notes narrower than `TRIM_MIN_W` can only be moved.
const EDGE_W: f32 = 6.0;
const TRIM_MIN_W: f32 = 10.0;
/// Closest horizontal zoom: a sixteenth note 120 px wide.
const MAX_PX_PER_TICK: f32 = 1.0;

#[derive(Debug, Clone, Default)]
pub struct MidiEditorState {
    pub selected: Vec<usize>,
    pub drag: Option<NoteDrag>,
    /// Vertical scroll in pixels from the top row (pitch 127).
    pub scroll: f32,
    pub clip: Option<ClipId>,
    /// Rubber-band selection start (screen position).
    pub band: Option<egui::Pos2>,
    /// The note sounding on the track's instrument while a key or note is held.
    pub audition: Option<(TrackId, u8)>,
    /// Horizontal zoom in pixels per tick (0: fit the clip to the width on the next frame).
    pub px_per_tick: f32,
    /// The tick at the roll's left edge, counted from the clip start.
    pub view_tick: f64,
    /// Key row height (0: the default).
    pub row_h: f32,
    /// A note being drawn with the Pencil.
    pub draft: Option<NoteDraft>,
    /// The last pointer position of a stroke in the velocity lane.
    pub vel_last: Option<Pos2>,
    /// Where everything was drawn on the last frame.
    pub view: Option<RollView>,
}

impl MidiEditorState {
    /// Start editing `id`. Note indices only mean something inside one clip, so the selection,
    /// any gesture in progress and the view (zoom and scroll) are reset when the clip changes.
    fn switch_clip(&mut self, id: ClipId) {
        if self.clip != Some(id) {
            self.selected.clear();
            self.drag = None;
            self.band = None;
            self.draft = None;
            self.vel_last = None;
            self.px_per_tick = 0.0;
            self.view_tick = 0.0;
        }
        self.clip = Some(id);
    }
}

/// What a note drag changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DragKind {
    Move,
    TrimStart,
    TrimEnd,
}

#[derive(Debug, Clone)]
pub struct NoteDrag {
    pub index: usize,
    pub kind: DragKind,
    pub start: egui::Pos2,
    pub dx_ticks: i64,
    pub dy_semi: i64,
    /// Alt (Option) was held when the drag started: the moved notes are copies.
    pub copy: bool,
}

/// A note the Pencil is drawing: its pitch and clip-relative start and end ticks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NoteDraft {
    pub pitch: u8,
    pub start: i64,
    pub end: i64,
}

/// The piano roll's screen geometry and its tick/pitch mapping, for one frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RollView {
    pub ruler: Rect,
    pub roll: Rect,
    pub keys: Rect,
    pub vel: Rect,
    pub view_tick: f64,
    pub px_per_tick: f32,
    pub row_h: f32,
    pub scroll: f32,
}

impl RollView {
    /// Screen x of a clip-relative tick.
    pub fn x_of(&self, tick: i64) -> f32 {
        self.roll.min.x + ((tick as f64 - self.view_tick) * f64::from(self.px_per_tick)) as f32
    }
    /// Clip-relative tick at screen x.
    pub fn tick_at(&self, x: f32) -> f64 {
        self.view_tick + f64::from(x - self.roll.min.x) / f64::from(self.px_per_tick.max(f32::EPSILON))
    }
    /// Top of the row for `pitch`.
    pub fn y_of(&self, pitch: i32) -> f32 {
        self.roll.min.y + (127 - pitch) as f32 * self.row_h - self.scroll
    }
    pub fn pitch_at(&self, y: f32) -> i32 {
        127 - ((y - self.roll.min.y + self.scroll) / self.row_h.max(1.0)).floor() as i32
    }
    /// The note's rectangle (at least 3 px wide so short notes stay visible).
    pub fn note_rect(&self, start: i64, length: i64, pitch: i32) -> Rect {
        let x0 = self.x_of(start);
        let y = self.y_of(pitch);
        Rect::from_min_max(pos2(x0, y + 1.0), pos2(self.x_of(start.saturating_add(length)).max(x0 + 3.0), y + self.row_h - 1.0))
    }
    /// The note under `pos` and the part of it: its edges trim, its body moves.
    pub fn hit(&self, notes: &[Note], pos: Pos2) -> Option<(usize, DragKind)> {
        notes.iter().enumerate().rev().find_map(|(i, n)| {
            let r = self.note_rect(n.start, n.length, i32::from(n.pitch)).expand2(vec2(0.0, 1.0));
            r.contains(pos).then(|| (i, edge_kind(r, pos.x)))
        })
    }
    /// The velocity lane's value at screen y.
    pub fn velocity_at(&self, y: f32) -> i64 {
        let v = (self.vel.max.y - 2.0 - y) / (self.vel.height() - 4.0).max(1.0) * 127.0;
        if v.is_finite() { v.round().clamp(1.0, 127.0) as i64 } else { 1 }
    }
}

/// Which part of a note rectangle `x` falls on.
fn edge_kind(r: Rect, x: f32) -> DragKind {
    if r.width() < TRIM_MIN_W {
        return DragKind::Move;
    }
    let edge = EDGE_W.min(r.width() / 4.0);
    if x >= r.max.x - edge {
        DragKind::TrimEnd
    } else if x <= r.min.x + edge {
        DragKind::TrimStart
    } else {
        DragKind::Move
    }
}

/// The nearest grid line to a clip-relative tick. Lines are counted from the session start, so
/// notes land on the bars and beats of the timeline wherever the clip starts.
fn snap_tick(tick: f64, base: i64, grid: i64) -> i64 {
    let g = grid.max(1) as f64;
    let abs = tick + base as f64;
    if !abs.is_finite() {
        return 0;
    }
    ((abs / g).round() * g) as i64 - base
}

/// The grid line at or before a clip-relative tick.
fn floor_tick(tick: f64, base: i64, grid: i64) -> i64 {
    let g = grid.max(1) as f64;
    let abs = tick + base as f64;
    if !abs.is_finite() {
        return 0;
    }
    ((abs / g).floor() * g) as i64 - base
}

/// Whether drags snap to the grid: in Grid modes, unless the suspend modifier is held (and the
/// other way round in Slip, Shuffle and Spot).
fn snaps(mode: EditMode, suspend: bool) -> bool {
    matches!(mode, EditMode::Grid | EditMode::GridRelative) != suspend
}

/// A note length as bars|beats|ticks.
fn length_label(map: &TempoMap, at: i64, len: i64) -> String {
    let m = map.meter_at_tick(at);
    let (bar, beat) = (m.ticks_per_bar().max(1), m.ticks_per_beat().max(1));
    format!("{}|{}|{:03}", len / bar, len % bar / beat, len % beat)
}

/// A position as bars|beats|ticks.
fn position_label(map: &TempoMap, at: i64) -> String {
    let bb = map.bar_beat_at_tick(at);
    format!("{}|{}|{:03}", bb.bar, bb.beat, bb.tick)
}

/// The MIDI clip being edited: the first selected MIDI clip, else a MIDI clip under the insertion
/// point on a selected track.
pub fn target_clip(app: &SoundApp) -> Option<ClipId> {
    let s = app.engine.session();
    for id in &s.edit.selected_clips {
        if let Some((_, c)) = s.find_clip(*id)
            && matches!(c.content, ClipContent::Midi { .. })
        {
            return Some(*id);
        }
    }
    let at = s.edit.selection.start;
    s.tracks
        .iter()
        .filter(|t| s.edit.selected_tracks.contains(&t.id) && t.kind.is_midi())
        .flat_map(|t| t.clips().iter())
        .find(|c| c.range().contains(at) || c.range().overlaps(&s.edit.selection))
        .or_else(|| s.tracks.iter().filter(|t| s.edit.selected_tracks.contains(&t.id) && t.kind.is_midi()).flat_map(|t| t.clips().iter()).next())
        .map(|c| c.id)
}

fn is_black(p: i32) -> bool {
    matches!(p.rem_euclid(12), 1 | 3 | 6 | 8 | 10)
}

/// Sound `want` on the instrument, releasing the note that was sounding.
fn audition(app: &mut SoundApp, want: Option<(TrackId, u8, u8)>) {
    if app.midi.audition == want.map(|(t, p, _)| (t, p)) {
        return;
    }
    if let Some(player) = &app.player {
        if let Some((t, p)) = app.midi.audition {
            player.note(t, p, 0);
        }
        if let Some((t, p, v)) = want {
            player.note(t, p, v);
        }
    }
    app.midi.audition = want.map(|(t, p, _)| (t, p));
}

/// Indices returned by a note command, for keeping the touched notes selected.
fn returned_indices(r: Result<serde_json::Value, String>) -> Vec<usize> {
    r.ok().and_then(|v| serde_json::from_value(v["indices"].clone()).ok()).unwrap_or_default()
}

pub fn show(app: &mut SoundApp, ui: &mut Ui) {
    if !ui.input(|i| i.pointer.primary_down()) {
        audition(app, None);
    }
    // Painter-only contents do not advance the layout. Claim the resized
    // space, including the empty state, so the dock keeps its chosen height.
    ui.take_available_space();
    let t = Tokens::current();
    let full = ui.max_rect();
    ui.painter().rect_filled(full, 0.0, t.panel_bg);
    let header = Rect::from_min_size(full.min, vec2(full.width(), 22.0));
    ui.painter().rect_filled(header, 0.0, t.panel_bg2);
    ui.painter().text(pos2(header.min.x + 8.0, header.center().y), Align2::LEFT_CENTER, "MIDI EDITOR", bold(11.5), t.header_text);
    let Some(cid) = target_clip(app) else {
        app.midi.view = None;
        let track = {
            let s = app.engine.session();
            s.tracks.iter().find(|t| t.kind.is_midi() && s.edit.selected_tracks.contains(&t.id)).map(|t| (t.id, t.name.clone()))
        };
        let msg = match &track {
            Some((_, name)) => format!("Click to add a MIDI clip to {name} at the insertion point."),
            None => "Select a MIDI clip (or a MIDI track) to edit its notes.".into(),
        };
        ui.painter().text(full.center(), Align2::CENTER_CENTER, msg, regular(13.0), t.text_dim);
        let body = Rect::from_min_max(pos2(full.min.x, header.max.y), full.max);
        if let Some((id, _)) = track
            && ui.interact(body, ui.id().with("new_clip"), Sense::click()).clicked()
        {
            let sel = app.engine.session().edit.selection;
            if let Ok(v) = app.run("midi.new_clip", json!({"track": id.0, "start": sel.start, "end": sel.end})) {
                let _ = app.run("edit.select", json!({"clips": [v["clip"]]}));
            }
        }
        return;
    };
    let Some((track, clip)) = app.engine.session().find_clip(cid).map(|(t, c)| (t, c.clone())) else { return };
    let ClipContent::Midi { sequence } = &clip.content else { return };
    let track_color = app.engine.session().track(track).map_or([120, 120, 200], |t| t.color);
    let cmd_key = if ui.ctx().os().is_mac() { "Cmd" } else { "Ctrl" };
    ui.painter().text(
        pos2(header.min.x + 110.0, header.center().y),
        Align2::LEFT_CENTER,
        format!(
            "{} · {} notes · {} selected · {cmd_key}+wheel: zoom · Alt+wheel: height · Alt-drag: copy · {cmd_key}-drag: snap on/off",
            clip.name,
            sequence.notes.len(),
            app.midi.selected.len()
        ),
        regular(11.0),
        t.text_dim,
    );
    let grid_ticks = match app.engine.session().edit.grid {
        soundcraft_time::GridValue::Note { value, dotted, triplet } => {
            let mut g = value.ticks();
            if dotted {
                g = g * 3 / 2;
            }
            if triplet {
                g = g * 2 / 3;
            }
            g.max(1)
        }
        _ => TICKS_PER_QUARTER / 4,
    };
    // Header toolbar: whole-clip MIDI operations.
    let bar = Rect::from_min_max(pos2(header.max.x - 330.0, header.min.y + 1.0), pos2(header.max.x - 6.0, header.max.y - 1.0));
    let mut tb = ui.new_child(egui::UiBuilder::new().max_rect(bar).layout(egui::Layout::right_to_left(egui::Align::Center)));
    tb.spacing_mut().item_spacing.x = 4.0;
    let mut op: Option<(&str, serde_json::Value)> = None;
    if tb.small_button("Legato").clicked() {
        op = Some(("event.change_duration", json!({"legato": 0})));
    }
    if tb.small_button("Vel −10").clicked() {
        op = Some(("event.change_velocity", json!({"add": -10})));
    }
    if tb.small_button("Vel +10").clicked() {
        op = Some(("event.change_velocity", json!({"add": 10})));
    }
    if tb.small_button("Quantize").on_hover_text("Quantize to the grid").clicked() {
        op = Some(("event.quantize", json!({"grid": grid_ticks, "strength": 100})));
    }
    if let Some((id, mut p)) = op {
        p["clips"] = json!([cid.0]);
        let _ = app.run(id, p);
    }
    let s = app.engine.session_arc();
    let sr = s.sample_rate;
    let base_tick = s.tempo.samples_to_ticks(clip.start, sr);
    let len_ticks = (s.tempo.samples_to_ticks(clip.end(), sr) - base_tick).max(1);
    let ruler = Rect::from_min_max(pos2(full.min.x + KEY_W, header.max.y), pos2(full.max.x - 4.0, header.max.y + RULER_H));
    let roll = Rect::from_min_max(pos2(ruler.min.x, ruler.max.y), pos2(ruler.max.x, full.max.y - VEL_H - 4.0));
    let keys = Rect::from_min_max(pos2(full.min.x, roll.min.y), pos2(roll.min.x, roll.max.y));
    let vel = Rect::from_min_max(pos2(roll.min.x, roll.max.y + 4.0), pos2(roll.max.x, full.max.y - 2.0));
    if !roll.is_positive() {
        return;
    }
    if app.midi.row_h <= 0.0 {
        app.midi.row_h = ROW_H;
    }
    // A new clip: fit it to the width and centre the rows on its notes.
    if app.midi.clip != Some(cid) || app.midi.px_per_tick <= 0.0 {
        let rows = (roll.height() / app.midi.row_h).floor() as i32;
        let hi = sequence.notes.iter().map(|n| i32::from(n.pitch)).max().unwrap_or(72);
        let lo = sequence.notes.iter().map(|n| i32::from(n.pitch)).min().unwrap_or(48);
        if app.midi.clip != Some(cid) {
            app.midi.scroll = (127 - (hi + lo) / 2 - rows / 2) as f32 * app.midi.row_h;
        }
        app.midi.switch_clip(cid);
        app.midi.px_per_tick = roll.width() / len_ticks as f32;
        app.midi.view_tick = 0.0;
    }
    // Wheel: scroll the rows (Shift: sideways), Ctrl/Cmd zooms the time, Alt the row height.
    let body = Rect::from_min_max(pos2(full.min.x, ruler.min.y), full.max);
    let min_ppt = (roll.width() / (len_ticks as f32 * 2.0)).min(MAX_PX_PER_TICK);
    if ui.rect_contains_pointer(body) {
        let (delta, zoom, mods, hover) = ui.input(|i| (i.smooth_scroll_delta, i.zoom_delta(), i.modifiers, i.pointer.hover_pos()));
        let m = &mut app.midi;
        if zoom != 1.0 {
            let x = hover.map_or(roll.center().x, |p| p.x.clamp(roll.min.x, roll.max.x));
            let anchor = m.view_tick + f64::from(x - roll.min.x) / f64::from(m.px_per_tick);
            m.px_per_tick = (m.px_per_tick * zoom).clamp(min_ppt, MAX_PX_PER_TICK);
            m.view_tick = anchor - f64::from(x - roll.min.x) / f64::from(m.px_per_tick);
        } else if mods.alt && delta.y != 0.0 {
            let y = hover.map_or(roll.center().y, |p| p.y.clamp(roll.min.y, roll.max.y));
            let rows_above = (y - roll.min.y + m.scroll) / m.row_h;
            m.row_h = (m.row_h * (delta.y * 0.01).exp()).clamp(ROW_MIN, ROW_MAX);
            m.scroll = rows_above * m.row_h - (y - roll.min.y);
        } else {
            let (dx, dy) = if mods.shift && delta.x == 0.0 { (delta.y, 0.0) } else { (delta.x, delta.y) };
            m.view_tick -= f64::from(dx) / f64::from(m.px_per_tick);
            m.scroll -= dy;
        }
    }
    {
        let m = &mut app.midi;
        m.px_per_tick = m.px_per_tick.clamp(min_ppt, MAX_PX_PER_TICK);
        let visible = f64::from(roll.width() / m.px_per_tick);
        m.view_tick = m.view_tick.clamp(0.0, (len_ticks as f64 - visible * 0.5).max(0.0));
        m.scroll = m.scroll.clamp(0.0, (128.0 * m.row_h - roll.height()).max(0.0));
    }
    // Follow the playhead a page at a time while it plays through the clip.
    let playing = app.is_playing();
    let play_tick = s.tempo.samples_to_ticks_f(app.position(), sr) - base_tick as f64;
    if playing {
        ui.ctx().request_repaint();
        let visible = f64::from(roll.width() / app.midi.px_per_tick);
        let follow = matches!(s.edit.scrolling.as_str(), "page" | "continuous" | "center");
        if follow && app.midi.drag.is_none() && (0.0..len_ticks as f64).contains(&play_tick) {
            let v = app.midi.view_tick;
            if play_tick < v || play_tick > v + visible {
                app.midi.view_tick = play_tick.clamp(0.0, (len_ticks as f64 - visible * 0.5).max(0.0));
            }
        }
    }
    let rv = RollView {
        ruler,
        roll,
        keys,
        vel,
        view_tick: app.midi.view_tick,
        px_per_tick: app.midi.px_per_tick,
        row_h: app.midi.row_h,
        scroll: app.midi.scroll,
    };
    app.midi.view = Some(rv);
    let row_h = rv.row_h;
    let tick_lo = rv.tick_at(roll.min.x).floor() as i64;
    let tick_hi = rv.tick_at(roll.max.x).ceil() as i64;
    let painter = ui.painter().with_clip_rect(roll.union(keys));
    // Background rows and keys.
    let hover = ui.ctx().pointer_latest_pos().filter(|p| roll.union(keys).contains(*p));
    let hover_pitch = hover.map(|p| rv.pitch_at(p.y));
    let top = rv.pitch_at(roll.min.y);
    let rows = (roll.height() / row_h).ceil() as i32;
    let (white_row, black_row) = (t.playlist_alt, t.playlist_bg.lerp_to_gamma(Color32::BLACK, 0.2));
    for r in 0..=rows + 1 {
        let pitch = top - r;
        if !(0..=127).contains(&pitch) {
            continue;
        }
        let y = rv.y_of(pitch);
        let row = Rect::from_min_size(pos2(roll.min.x, y), vec2(roll.width(), row_h));
        painter.rect_filled(row, 0.0, if is_black(pitch) { black_row } else { white_row });
        let key = Rect::from_min_size(pos2(keys.min.x, y), vec2(KEY_W - 2.0, (row_h - 1.0).max(1.0)));
        let key_fill = if is_black(pitch) { Color32::from_rgb(20, 20, 20) } else { Color32::from_rgb(225, 225, 225) };
        let hovered = hover_pitch == Some(pitch);
        painter.rect_filled(key, 1.0, if hovered { key_fill.lerp_to_gamma(t.accent, 0.5) } else { key_fill });
        if (pitch.rem_euclid(12) == 0 || hovered) && row_h >= 7.0 {
            let ink = if is_black(pitch) && !hovered { Color32::from_rgb(220, 220, 220) } else { Color32::from_rgb(40, 40, 40) };
            painter.text(pos2(key.max.x - 3.0, key.center().y), Align2::RIGHT_CENTER, soundcraft_midi::note_name(pitch as u8), regular(8.0), ink);
        }
        if pitch.rem_euclid(12) == 0 {
            painter.line_segment([pos2(roll.min.x, y + row_h), pos2(roll.max.x, y + row_h)], Stroke::new(1.0, t.bar_line));
        }
    }
    // Ruler, grid, beat and bar lines, all on the session's timeline.
    ui.painter().rect_filled(Rect::from_min_max(pos2(full.min.x, ruler.min.y), ruler.max), 0.0, t.ruler_bg);
    let rp = ui.painter().with_clip_rect(ruler);
    if grid_ticks as f32 * rv.px_per_tick > 6.0 {
        let mut g = floor_tick(tick_lo as f64, base_tick, grid_ticks);
        while g <= tick_hi {
            let x = rv.x_of(g);
            painter.line_segment([pos2(x, roll.min.y), pos2(x, roll.max.y)], Stroke::new(1.0, t.grid_line));
            g += grid_ticks;
        }
    }
    let first_bar = s.tempo.bar_beat_at_tick(base_tick.saturating_add(tick_lo).max(0)).bar;
    let label_every = {
        let m = s.tempo.meter_at_tick(base_tick);
        let px = m.ticks_per_bar() as f32 * rv.px_per_tick;
        (40.0 / px.max(0.01)).ceil().max(1.0) as i64
    };
    for bar in (first_bar..).take(4096) {
        let at = s.tempo.bar_start_tick(bar);
        if at - base_tick > tick_hi {
            break;
        }
        let m = s.tempo.meter_at_tick(at);
        let beat = m.ticks_per_beat();
        if beat as f32 * rv.px_per_tick > 10.0 {
            for k in 1..i64::from(m.numerator.max(1)) {
                let x = rv.x_of(at + k * beat - base_tick);
                painter.line_segment([pos2(x, roll.min.y), pos2(x, roll.max.y)], Stroke::new(1.0, t.bar_line.gamma_multiply(0.7)));
                rp.line_segment([pos2(x, ruler.max.y - 4.0), pos2(x, ruler.max.y)], Stroke::new(1.0, t.ruler_tick));
            }
        }
        let x = rv.x_of(at - base_tick);
        painter.line_segment([pos2(x, roll.min.y), pos2(x, roll.max.y)], Stroke::new(1.0, t.bar_line));
        rp.line_segment([pos2(x, ruler.min.y + 3.0), pos2(x, ruler.max.y)], Stroke::new(1.0, t.ruler_tick));
        if bar.rem_euclid(label_every) == 1 % label_every {
            rp.text(pos2(x + 3.0, ruler.center().y - 1.0), Align2::LEFT_CENTER, bar.to_string(), regular(10.0), t.ruler_text);
        }
    }
    // Past the clip's end: shaded.
    let end_x = rv.x_of(len_ticks);
    if end_x < roll.max.x {
        painter.rect_filled(Rect::from_min_max(pos2(end_x.max(roll.min.x), roll.min.y), roll.max), 0.0, Color32::from_black_alpha(90));
    }
    // Clicking the ruler moves the playhead there.
    let ruler_resp = ui.interact(ruler, ui.id().with(("ruler", cid.0)), Sense::click());
    if ruler_resp.clicked()
        && let Some(p) = ruler_resp.interact_pointer_pos()
    {
        let tick = base_tick.saturating_add(rv.tick_at(p.x).round() as i64).max(0);
        let _ = app.run("transport.locate", json!({"at": s.tempo.tick_to_samples(tick, sr)}));
    }
    // Interaction state for this frame.
    let resp = ui.interact(roll, ui.id().with(("roll", cid.0)), Sense::click_and_drag());
    let mods = ui.input(|i| i.modifiers);
    let snapping = snaps(s.edit.edit_mode, mods.command);
    let relative = s.edit.edit_mode == EditMode::GridRelative;
    // Notes, with the drag in progress previewed.
    let base = egui::Color32::from_rgb(track_color[0], track_color[1], track_color[2]);
    let drag = app.midi.drag.clone();
    let preview = |i: usize, n: &Note| -> (i64, i64, i32) {
        let (st, ln, p) = (n.start, n.length, i32::from(n.pitch));
        let Some(d) = drag.as_ref().filter(|_| app.midi.selected.contains(&i)) else { return (st, ln, p) };
        match d.kind {
            DragKind::Move => ((st + d.dx_ticks).max(0), ln, (p + d.dy_semi as i32).clamp(0, 127)),
            DragKind::TrimEnd => (st, (ln + d.dx_ticks).max(1), p),
            DragKind::TrimStart => {
                let end = st + ln;
                let s2 = (st + d.dx_ticks).clamp(0, end - 1);
                (s2, end - s2, p)
            }
        }
    };
    let fill_of = |n: &Note| {
        let k = 0.45 + 0.55 * f32::from(n.velocity) / 127.0;
        Color32::from_rgb((f32::from(base.r()) * k) as u8, (f32::from(base.g()) * k) as u8, (f32::from(base.b()) * k) as u8)
    };
    let copying = drag.as_ref().is_some_and(|d| d.copy && d.kind == DragKind::Move);
    for (i, n) in sequence.notes.iter().enumerate() {
        let sel = app.midi.selected.contains(&i);
        let (st, ln, p) = if copying { (n.start, n.length, i32::from(n.pitch)) } else { preview(i, n) };
        let r = rv.note_rect(st, ln, p);
        if r.max.x < roll.min.x || r.min.x > roll.max.x {
            continue;
        }
        let fill = if sel { fill_of(n).lerp_to_gamma(Color32::WHITE, 0.65) } else { fill_of(n) };
        let stroke = if sel { Stroke::new(1.5, t.accent) } else { Stroke::new(1.0, Color32::BLACK) };
        painter.rect(r, 2.0, fill, stroke, StrokeKind::Inside);
    }
    if copying {
        for (i, n) in sequence.notes.iter().enumerate().filter(|(i, _)| app.midi.selected.contains(i)) {
            let (st, ln, p) = preview(i, n);
            painter.rect(rv.note_rect(st, ln, p), 2.0, fill_of(n).gamma_multiply(0.7), Stroke::new(1.5, t.accent), StrokeKind::Inside);
        }
    }
    if let Some(d) = app.midi.draft {
        painter.rect(rv.note_rect(d.start, d.end - d.start, i32::from(d.pitch)), 2.0, base, Stroke::new(1.5, t.accent), StrokeKind::Inside);
    }
    // Playhead (while playing) or the insertion point.
    let px = rv.x_of(play_tick.round() as i64);
    if (roll.min.x..=roll.max.x).contains(&px) {
        let (col, w) = if playing { (t.playhead, 1.5) } else { (t.insertion, 1.0) };
        ui.painter().line_segment([pos2(px, ruler.min.y), pos2(px, roll.max.y)], Stroke::new(w, col));
    }
    // Interaction.
    let hit = |pos: Pos2| rv.hit(&sequence.notes, pos);
    if let Some(p) = hover.filter(|p| roll.contains(*p))
        && app.midi.drag.is_none()
        && matches!(hit(p), Some((_, DragKind::TrimStart | DragKind::TrimEnd)))
    {
        ui.ctx().set_cursor_icon(CursorIcon::ResizeHorizontal);
    }
    // Audition: a held piano key plays its pitch (sliding across keys plays each one); a held
    // note plays at its velocity, following the pitch it is dragged to; the Pencil plays the note
    // it is drawing.
    let key_resp = ui.interact(keys, ui.id().with(("keys", cid.0)), Sense::click_and_drag());
    let pointer = ui.ctx().pointer_latest_pos();
    let origin = ui.input(|i| i.pointer.press_origin());
    let want = if key_resp.is_pointer_button_down_on() {
        pointer.filter(|p| keys.contains(*p)).map(|p| (rv.pitch_at(p.y).clamp(0, 127) as u8, 100))
    } else if resp.is_pointer_button_down_on() {
        match origin.and_then(hit) {
            Some((i, _)) => sequence.notes.get(i).map(|n| {
                let dy = app.midi.drag.as_ref().filter(|d| d.kind == DragKind::Move).map_or(0, |d| d.dy_semi);
                ((i64::from(n.pitch) + dy).clamp(0, 127) as u8, n.velocity.max(1))
            }),
            None if s.edit.tool == Tool::Pencil => {
                app.midi.draft.map(|d| d.pitch).or_else(|| origin.map(|o| rv.pitch_at(o.y).clamp(0, 127) as u8)).map(|p| (p, 100))
            }
            None => None,
        }
    } else {
        None
    };
    audition(app, want.map(|(p, v)| (track, p, v)));
    if resp.drag_started()
        && let Some(p) = origin.or_else(|| resp.interact_pointer_pos())
    {
        match hit(p) {
            Some((i, kind)) => {
                if !app.midi.selected.contains(&i) {
                    if mods.shift {
                        app.midi.selected.push(i);
                    } else {
                        app.midi.selected = vec![i];
                    }
                }
                let copy = kind == DragKind::Move && mods.alt;
                app.midi.drag = Some(NoteDrag { index: i, kind, start: p, dx_ticks: 0, dy_semi: 0, copy });
            }
            None if s.edit.tool == Tool::Pencil => {
                let at = rv.tick_at(p.x);
                let start = if snapping { floor_tick(at, base_tick, grid_ticks) } else { at.round() as i64 }.max(0);
                let end = start + if snapping { grid_ticks } else { 1 };
                app.midi.draft = Some(NoteDraft { pitch: rv.pitch_at(p.y).clamp(0, 127) as u8, start, end });
            }
            None => app.midi.band = Some(p),
        }
    }
    // Rubber-band selection on empty space.
    if let (Some(a), Some(b)) = (app.midi.band, pointer) {
        let band = Rect::from_two_pos(a, b);
        ui.painter().rect(
            band,
            0.0,
            Color32::from_rgba_unmultiplied(120, 170, 255, 40),
            Stroke::new(1.0, Color32::from_rgb(120, 170, 255)),
            StrokeKind::Inside,
        );
        if resp.drag_stopped() || !ui.input(|i| i.pointer.primary_down()) {
            let picked = sequence
                .notes
                .iter()
                .enumerate()
                .filter(|(_, n)| rv.note_rect(n.start, n.length, i32::from(n.pitch)).intersects(band))
                .map(|(i, _)| i);
            if mods.shift {
                for i in picked {
                    if !app.midi.selected.contains(&i) {
                        app.midi.selected.push(i);
                    }
                }
            } else {
                app.midi.selected = picked.collect();
            }
            app.midi.band = None;
        }
    }
    // Drag in progress: snapped so the grabbed note's edge lands on the grid (Grid), or moved by
    // whole grid steps keeping its offset (Relative Grid).
    let mut info: Option<String> = None;
    if let (Some(p), Some(d)) = (pointer, app.midi.drag.as_mut())
        && let Some(n) = sequence.notes.get(d.index)
    {
        let raw = f64::from(p.x - d.start.x) / f64::from(rv.px_per_tick);
        let edge = match d.kind {
            DragKind::TrimEnd => n.end(),
            DragKind::Move | DragKind::TrimStart => n.start,
        };
        d.dx_ticks = if !snapping {
            raw.round() as i64
        } else if relative {
            (raw / grid_ticks as f64).round() as i64 * grid_ticks
        } else {
            snap_tick(edge as f64 + raw, base_tick, grid_ticks) - edge
        };
        if d.kind == DragKind::Move {
            d.dy_semi = -((p.y - d.start.y) / row_h).round() as i64;
        }
        let pitch = (i64::from(n.pitch) + d.dy_semi).clamp(0, 127) as u8;
        info = Some(match d.kind {
            DragKind::Move => {
                format!("{}  {}", soundcraft_midi::note_name(pitch), position_label(&s.tempo, base_tick + (n.start + d.dx_ticks).max(0)))
            }
            DragKind::TrimEnd => format!(
                "{}  length {}",
                soundcraft_midi::note_name(pitch),
                length_label(&s.tempo, base_tick + n.start, (n.length + d.dx_ticks).max(1))
            ),
            DragKind::TrimStart => {
                let st = (n.start + d.dx_ticks).clamp(0, n.end() - 1);
                format!("{}  start {}", soundcraft_midi::note_name(pitch), position_label(&s.tempo, base_tick + st))
            }
        });
    }
    if let (Some(p), Some(d)) = (pointer, app.midi.draft.as_mut()) {
        let at = rv.tick_at(p.x);
        let min = if snapping { grid_ticks } else { 1 };
        let end = if snapping { snap_tick(at, base_tick, grid_ticks) } else { at.round() as i64 };
        d.end = end.max(d.start + min);
        info = Some(format!("{}  length {}", soundcraft_midi::note_name(d.pitch), length_label(&s.tempo, base_tick + d.start, d.end - d.start)));
    }
    let released = resp.drag_stopped() || !ui.input(|i| i.pointer.primary_down());
    if released && let Some(d) = app.midi.drag.take() {
        let sel = app.midi.selected.clone();
        match d.kind {
            DragKind::Move if d.dx_ticks != 0 || d.dy_semi != 0 => {
                let r =
                    app.run("midi.notes_move", json!({"clip": cid.0, "indices": sel, "ticks": d.dx_ticks, "semitones": d.dy_semi, "copy": d.copy}));
                app.midi.selected = returned_indices(r);
            }
            DragKind::TrimEnd | DragKind::TrimStart if d.dx_ticks != 0 => {
                let mut p = json!({"clip": cid.0, "indices": sel});
                p[if d.kind == DragKind::TrimEnd { "end" } else { "start" }] = json!(d.dx_ticks);
                let r = app.run("midi.notes_trim", p);
                app.midi.selected = returned_indices(r);
            }
            _ => {}
        }
    }
    if released && let Some(d) = app.midi.draft.take() {
        let _ = app
            .run("midi.note_add", json!({"clip": cid.0, "pitch": d.pitch, "start_ticks": d.start, "length_ticks": d.end - d.start, "velocity": 100}));
        app.midi.selected.clear();
    }
    if resp.clicked()
        && let Some(p) = resp.interact_pointer_pos()
    {
        match hit(p) {
            Some((i, _)) => {
                if mods.shift {
                    if let Some(k) = app.midi.selected.iter().position(|&x| x == i) {
                        app.midi.selected.remove(k);
                    } else {
                        app.midi.selected.push(i);
                    }
                } else {
                    app.midi.selected = vec![i];
                }
            }
            None if s.edit.tool == Tool::Pencil => {
                let pitch = rv.pitch_at(p.y).clamp(0, 127);
                let at = rv.tick_at(p.x);
                let start = if snapping { floor_tick(at, base_tick, grid_ticks) } else { at.round() as i64 };
                let _ = app.run(
                    "midi.note_add",
                    json!({"clip": cid.0, "pitch": pitch, "start_ticks": start.max(0), "length_ticks": grid_ticks, "velocity": 100}),
                );
                app.midi.selected.clear();
            }
            None => app.midi.selected.clear(),
        }
    }
    // The note under the pointer, when nothing is being dragged.
    if info.is_none()
        && let Some(p) = hover.filter(|p| roll.contains(*p))
    {
        info = Some(match hit(p).and_then(|(i, _)| sequence.notes.get(i)) {
            Some(n) => format!(
                "{}  vel {}  {}  length {}",
                soundcraft_midi::note_name(n.pitch),
                n.velocity,
                position_label(&s.tempo, base_tick + n.start),
                length_label(&s.tempo, base_tick + n.start, n.length)
            ),
            None => soundcraft_midi::note_name(rv.pitch_at(p.y).clamp(0, 127) as u8).to_string(),
        });
    }
    if let (Some(text), Some(p)) = (info, pointer) {
        let tip = ui.painter().with_clip_rect(full);
        let galley = tip.layout_no_wrap(text, regular(10.5), t.text);
        let at = pos2((p.x + 14.0).min(full.max.x - galley.size().x - 6.0), (p.y - 20.0).max(full.min.y + 2.0));
        tip.rect_filled(Rect::from_min_size(at, galley.size()).expand(3.0), 3.0, t.panel_bg2.gamma_multiply(0.95));
        tip.galley(at, galley, t.text);
    }
    // Keyboard: Cmd+A select all; arrows transpose (Shift = octave) and move by the grid.
    if ui.rect_contains_pointer(roll) && !ui.ctx().egui_wants_keyboard_input() {
        let (all, up, down, left, right, shift) = ui.input(|i| {
            let arrows = !i.modifiers.alt && !i.modifiers.command && !i.modifiers.ctrl;
            (
                i.modifiers.command && i.key_pressed(egui::Key::A),
                arrows && i.key_pressed(egui::Key::ArrowUp),
                arrows && i.key_pressed(egui::Key::ArrowDown),
                arrows && !i.modifiers.shift && i.key_pressed(egui::Key::ArrowLeft),
                arrows && !i.modifiers.shift && i.key_pressed(egui::Key::ArrowRight),
                i.modifiers.shift,
            )
        });
        if all {
            app.midi.selected = (0..sequence.notes.len()).collect();
        }
        let semi = if up {
            1
        } else if down {
            -1
        } else {
            0
        } * if shift { 12 } else { 1 };
        let ticks = if right {
            grid_ticks
        } else if left {
            -grid_ticks
        } else {
            0
        };
        if (semi != 0 || ticks != 0) && !app.midi.selected.is_empty() {
            let moved = app.run("midi.notes_move", json!({"clip": cid.0, "indices": app.midi.selected, "ticks": ticks, "semitones": semi}));
            // The engine re-sorts the notes: follow the moved ones to their new indices, so the
            // selection never points at other notes and the arrows can keep nudging it.
            app.midi.selected = returned_indices(moved);
        }
    }
    if !app.midi.selected.is_empty()
        && !ui.ctx().egui_wants_keyboard_input()
        && ui.input(|i| i.key_pressed(egui::Key::Delete) || i.key_pressed(egui::Key::Backspace))
    {
        let _ = app.run("midi.note_delete", json!({"clip": cid.0, "indices": app.midi.selected}));
        app.midi.selected.clear();
    }
    velocity_lane(app, ui, &rv, cid, &sequence.notes, base, full);
}

/// The velocity lane: one stem per note. Dragging draws the velocity of every stem the pointer
/// passes; with notes selected only theirs change, so one note of a chord can be set on its own.
fn velocity_lane(app: &mut SoundApp, ui: &mut Ui, rv: &RollView, cid: ClipId, notes: &[Note], base: Color32, full: Rect) {
    let t = Tokens::current();
    let vel = rv.vel;
    ui.painter().rect_filled(vel, 0.0, t.playlist_bg.lerp_to_gamma(Color32::BLACK, 0.25));
    ui.painter().text(pos2(full.min.x + 6.0, vel.center().y), Align2::LEFT_CENTER, "Velocity", regular(10.0), t.text_dim);
    let painter = ui.painter().with_clip_rect(vel);
    let selected = app.midi.selected.clone();
    let editable = |i: usize| selected.is_empty() || selected.contains(&i);
    for (i, n) in notes.iter().enumerate() {
        let x = rv.x_of(n.start) + 1.0;
        let h = (vel.height() - 4.0) * f32::from(n.velocity) / 127.0;
        let col = if selected.contains(&i) {
            base.lerp_to_gamma(Color32::WHITE, 0.65)
        } else if editable(i) {
            base
        } else {
            base.gamma_multiply(0.35)
        };
        painter.line_segment([pos2(x, vel.max.y - 2.0), pos2(x, vel.max.y - 2.0 - h)], Stroke::new(3.0, col));
        painter.circle_filled(pos2(x, vel.max.y - 2.0 - h), 2.5, col);
    }
    let vresp = ui.interact(vel, ui.id().with(("vel", cid.0)), Sense::click_and_drag());
    let down = vresp.is_pointer_button_down_on() || vresp.clicked();
    let Some(p) = vresp.interact_pointer_pos().filter(|_| down) else {
        if app.midi.vel_last.take().is_some() {
            app.engine.end_merge();
        }
        return;
    };
    // The segment since the last frame, so a quick stroke still sets every stem it crossed.
    let a = app.midi.vel_last.unwrap_or(p);
    let (lo, hi) = (a.x.min(p.x) - 3.0, a.x.max(p.x) + 3.0);
    let mut targets: Vec<(usize, i64)> = Vec::new();
    for (i, n) in notes.iter().enumerate() {
        let x = rv.x_of(n.start) + 1.0;
        if editable(i) && (lo..=hi).contains(&x) {
            let f = if (p.x - a.x).abs() < 0.5 { 1.0 } else { ((x - a.x) / (p.x - a.x)).clamp(0.0, 1.0) };
            targets.push((i, rv.velocity_at(a.y + (p.y - a.y) * f)));
        }
    }
    // A click beside the stems still picks the nearest one.
    if targets.is_empty()
        && app.midi.vel_last.is_none()
        && let Some((i, _)) = notes
            .iter()
            .enumerate()
            .filter(|(i, _)| editable(*i))
            .map(|(i, n)| (i, (rv.x_of(n.start) - p.x).abs()))
            .filter(|(_, d)| *d < 8.0)
            .min_by(|a, b| a.1.total_cmp(&b.1))
    {
        targets.push((i, rv.velocity_at(p.y)));
    }
    app.midi.vel_last = Some(p);
    for (i, v) in targets {
        if notes.get(i).is_some_and(|n| i64::from(n.velocity) != v) {
            let _ = app.engine.execute_merged("midi.note_edit", &json!({"clip": cid.0, "index": i, "velocity": v}), &format!("vel:{}", cid.0));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn switching_clip_drops_selection_and_gestures() {
        let mut st = MidiEditorState { selected: vec![0, 2], clip: Some(ClipId(1)), band: Some(pos2(1.0, 2.0)), ..Default::default() };
        st.switch_clip(ClipId(1));
        assert_eq!(st.selected, vec![0, 2]);
        st.switch_clip(ClipId(2));
        assert!(st.selected.is_empty());
        assert!(st.drag.is_none() && st.band.is_none());
        assert_eq!(st.clip, Some(ClipId(2)));
    }

    #[test]
    fn snapping_counts_grid_lines_from_the_session_start() {
        // A clip starting 100 ticks after a line: its tick 0 is not on the grid.
        assert_eq!(snap_tick(0.0, 100, 240), -100);
        assert_eq!(snap_tick(150.0, 100, 240), 140);
        assert_eq!(floor_tick(379.0, 100, 240), 140);
        assert_eq!(snap_tick(f64::NAN, 0, 240), 0);
    }

    #[test]
    fn the_suspend_modifier_flips_snapping() {
        assert!(snaps(EditMode::Grid, false));
        assert!(!snaps(EditMode::Grid, true));
        assert!(snaps(EditMode::GridRelative, false));
        assert!(!snaps(EditMode::Slip, false));
        assert!(snaps(EditMode::Slip, true));
    }

    #[test]
    fn narrow_notes_only_move_and_wide_ones_trim_at_their_edges() {
        let narrow = Rect::from_min_size(pos2(0.0, 0.0), vec2(8.0, 8.0));
        assert_eq!(edge_kind(narrow, 7.5), DragKind::Move);
        let wide = Rect::from_min_size(pos2(0.0, 0.0), vec2(100.0, 8.0));
        assert_eq!(edge_kind(wide, 97.0), DragKind::TrimEnd);
        assert_eq!(edge_kind(wide, 2.0), DragKind::TrimStart);
        assert_eq!(edge_kind(wide, 50.0), DragKind::Move);
    }

    #[test]
    fn lengths_and_positions_read_as_bars_beats_ticks() {
        let map = TempoMap::default();
        assert_eq!(length_label(&map, 0, TICKS_PER_QUARTER * 5 + 120), "1|1|120");
        assert_eq!(position_label(&map, TICKS_PER_QUARTER * 4), "2|1|000");
    }
}
