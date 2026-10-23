//! The MIDI Editor's controller lanes under the Velocity lane: CC, pitch bend, aftertouch and
//! program change, drawn as values held until the next event. Each MIDI track keeps its own list
//! of lanes (`midi.lanes`). The Pencil draws with the current shape (`edit.pencil_shape`), the
//! Grabber adds, selects and drags events, the Trimmer raises or lowers them, Alt-click deletes.

use crate::SoundApp;
use crate::midi_editor::{RollView, position_label, returned_indices, snap_tick, tooltip};
use crate::theme::{Tokens, regular};
use egui::{Align2, Color32, Pos2, Rect, Response, Sense, Stroke, StrokeKind, Ui, pos2, vec2};
use serde_json::{Value, json};
use soundcraft_midi::{CtrlKind, MAX_POINTS, Note, PencilShape, Sequence, ShapeStroke};
use soundcraft_model::{ClipId, Tool, TrackId};
use soundcraft_time::{TICKS_PER_QUARTER, TempoMap};
use std::collections::HashSet;

/// A controller lane's height, and the least a lane shrinks to when many are open.
pub const LANE_H: f32 = 56.0;
const LANE_MIN: f32 = 18.0;
pub const LANE_GAP: f32 = 4.0;
/// The Velocity lane's height, and the least it shrinks to.
pub const VEL_LANE_H: f32 = 62.0;
const VEL_MIN: f32 = 24.0;
/// How near the pointer (px) an event is hit.
const HIT: f32 = 5.0;
/// Space between a lane header and its lane, so a stroke started just left of a lane still
/// reaches the lane.
const HEADER_GAP: f32 = 12.0;
/// A stroke's event spacing, as `midi.ctrls_draw` uses: a 64th note.
const DRAW_STEP: i64 = TICKS_PER_QUARTER / 16;

/// Heights of the Velocity lane and of each of `n` controller lanes: together at most 60% of `avail`.
pub fn lane_heights(avail: f32, n: usize) -> (f32, f32) {
    if n == 0 {
        return (VEL_LANE_H, 0.0);
    }
    let want = VEL_LANE_H + n as f32 * (LANE_H + LANE_GAP);
    let cap = (avail * 0.6).max(0.0);
    if want <= cap {
        (VEL_LANE_H, LANE_H)
    } else {
        // The floors must not push the lanes past the cap: the editor gives the roll whatever is
        // left, and a non-positive roll returns before the headers are drawn, so a track with many
        // lanes could not remove any of them. The Velocity lane keeps its floor first, and the
        // lanes share what remains: they may shrink below LANE_MIN, because an empty lane still
        // shows its header and a header is what the user needs to remove it.
        let k = cap / want;
        let vel = (VEL_LANE_H * k).max(VEL_MIN).min(cap);
        let lane = ((cap - vel) / n as f32 - LANE_GAP).max(0.0).min((LANE_H * k).max(LANE_MIN));
        (vel, lane)
    }
}

/// One controller lane on screen.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LaneView {
    pub kind: CtrlKind,
    pub rect: Rect,
}

impl LaneView {
    /// Screen y of a value.
    pub fn y_of(&self, value: f64) -> f32 {
        let (lo, hi) = self.kind.range();
        let k = ((value - f64::from(lo)) / f64::from(hi - lo).max(1.0)) as f32;
        self.rect.max.y - 2.0 - k.clamp(0.0, 1.0) * (self.rect.height() - 4.0)
    }

    /// The whole value at screen y, within the kind's range.
    pub fn value_at(&self, y: f32) -> f64 {
        let (lo, hi) = self.kind.range();
        let k = f64::from((self.rect.max.y - 2.0 - y) / (self.rect.height() - 4.0).max(1.0));
        if k.is_finite() { (f64::from(lo) + k.clamp(0.0, 1.0) * f64::from(hi - lo)).round() } else { f64::from(self.kind.rest_value()) }
    }
}

/// A gesture in a lane, from press to release.
#[derive(Debug, Clone, PartialEq)]
pub enum LaneGesture {
    /// The Pencil: Free Hand collects `points`; the other shapes draw `stroke`.
    Draw { lane: usize, points: Vec<(i64, f64)>, stroke: ShapeStroke },
    /// Dragging the selected events (Grabber), or only their values (Trimmer).
    Move { lane: usize, start: Pos2, anchor_tick: i64, dt: i64, dv: i64, copy: bool, values_only: bool },
    /// Rubber-band selection.
    Band { lane: usize, start: Pos2 },
    /// A Pencil shape across the velocity stems.
    Velocity { stroke: ShapeStroke },
}

impl LaneGesture {
    fn lane(&self) -> Option<usize> {
        match self {
            LaneGesture::Draw { lane, .. } | LaneGesture::Move { lane, .. } | LaneGesture::Band { lane, .. } => Some(*lane),
            LaneGesture::Velocity { .. } => None,
        }
    }
}

/// What the lanes need from the editor for one frame.
pub struct LaneCtx<'a> {
    pub rv: RollView,
    pub full: Rect,
    pub cid: ClipId,
    pub track: TrackId,
    pub seq: &'a Sequence,
    pub color: Color32,
    pub base_tick: i64,
    pub len_ticks: i64,
    pub grid: i64,
    pub snapping: bool,
    pub tool: Tool,
    pub shape: PencilShape,
    pub tempo: &'a TempoMap,
    /// Screen x of the bar lines.
    pub bars: &'a [f32],
}

impl LaneCtx<'_> {
    fn snap(&self, tick: f64) -> i64 {
        if self.snapping { snap_tick(tick, self.base_tick, self.grid) } else { tick.round() as i64 }.max(0)
    }
}

/// How a value reads: pitch bend signed, a program change by its General MIDI name.
pub fn value_text(kind: CtrlKind, v: i64) -> String {
    match kind {
        CtrlKind::PitchBend => format!("{v:+}"),
        CtrlKind::Program => format!("{} {}", v + 1, soundcraft_midi::gm_program_name(v.clamp(0, 127) as u8)),
        _ => v.to_string(),
    }
}

/// A Free Hand stroke's points as `midi.ctrls_draw` keeps them: sorted, one per tick (the last).
fn free_hand(points: &[(i64, f64)]) -> Vec<(i64, f64)> {
    let mut p: Vec<(i64, f64)> = points.iter().map(|&(t, v)| (t.max(0), v)).collect();
    p.sort_by_key(|x| x.0);
    p.dedup_by(|later, kept| {
        let same = later.0 == kept.0;
        if same {
            *kept = *later;
        }
        same
    });
    p
}

fn stroke_points(points: &[(i64, f64)], stroke: &ShapeStroke) -> Vec<(i64, f64)> {
    if stroke.shape == PencilShape::FreeHand { free_hand(points) } else { stroke.points(DRAW_STEP) }
}

/// An event as the lane shows it.
#[derive(Debug, Clone, Copy)]
struct Shown {
    tick: i64,
    value: i32,
    /// Its index in the sequence (none for a stroke being drawn).
    index: Option<usize>,
    selected: bool,
}

/// The lane's events with the gesture in progress applied: moved, copied or drawn over.
fn shown_events(app: &SoundApp, cx: &LaneCtx, li: usize, lane: LaneView) -> Vec<Shown> {
    let sel = &app.midi.ctrl_selected;
    let g = app.midi.lane_gesture.as_ref().filter(|g| g.lane() == Some(li));
    let (lo, hi) = lane.kind.range();
    let drawn = match g {
        Some(LaneGesture::Draw { points, stroke, .. }) => stroke_points(points, stroke),
        _ => Vec::new(),
    };
    let span = drawn.first().zip(drawn.last()).map(|(a, b)| (a.0, b.0));
    let mut out = Vec::new();
    let mut moved = Vec::new();
    for (i, c) in cx.seq.ctrls.iter().enumerate().filter(|(_, c)| c.kind == lane.kind) {
        let selected = sel.contains(&i);
        if let Some(LaneGesture::Move { dt, dv, copy, .. }) = g
            && selected
        {
            let value = i64::from(c.value).saturating_add(*dv).clamp(i64::from(lo), i64::from(hi)) as i32;
            moved.push(Shown { tick: c.tick.saturating_add(*dt).max(0), value, index: Some(i), selected: true });
            if *copy {
                out.push(Shown { tick: c.tick, value: c.value, index: Some(i), selected: false });
            }
            continue;
        }
        if span.is_some_and(|(a, b)| (a..=b).contains(&c.tick)) {
            continue;
        }
        out.push(Shown { tick: c.tick, value: c.value, index: Some(i), selected });
    }
    out.retain(|o| !moved.iter().any(|m| m.tick == o.tick));
    out.extend(moved);
    out.extend(drawn.into_iter().map(|(tick, v)| Shown {
        tick,
        value: v.round().clamp(f64::from(lo), f64::from(hi)) as i32,
        index: None,
        selected: false,
    }));
    out.sort_by_key(|s| s.tick);
    out
}

fn lane_background(ui: &Ui, cx: &LaneCtx, r: Rect) {
    let t = Tokens::current();
    ui.painter().rect_filled(r, 0.0, t.playlist_bg.lerp_to_gamma(Color32::BLACK, 0.25));
    let painter = ui.painter().with_clip_rect(r);
    for &x in cx.bars {
        painter.line_segment([pos2(x, r.min.y), pos2(x, r.max.y)], Stroke::new(1.0, t.bar_line.gamma_multiply(0.6)));
    }
    let end_x = cx.rv.x_of(cx.len_ticks);
    if end_x < r.max.x {
        painter.rect_filled(Rect::from_min_max(pos2(end_x.max(r.min.x), r.min.y), r.max), 0.0, Color32::from_black_alpha(90));
    }
}

/// One controller lane: drawing, hit testing and the tools.
pub fn ctrl_lane(app: &mut SoundApp, ui: &mut Ui, cx: &LaneCtx, li: usize, lane: LaneView) {
    let t = Tokens::current();
    let r = lane.rect;
    lane_background(ui, cx, r);
    let painter = ui.painter().with_clip_rect(r);
    let (lo, _) = lane.kind.range();
    if lane.kind.rest_value() != lo {
        let y = lane.y_of(f64::from(lane.kind.rest_value()));
        painter.line_segment([pos2(r.min.x, y), pos2(r.max.x, y)], Stroke::new(1.0, t.grid_line));
    }
    // Held values: a step at each event, flat until the next one and to the clip's end.
    let shown = shown_events(app, cx, li, lane);
    let at = |s: &Shown| pos2(cx.rv.x_of(s.tick), lane.y_of(f64::from(s.value)));
    let line = Stroke::new(1.5, cx.color);
    for w in shown.windows(2) {
        if let [a, b] = w {
            let (pa, pb) = (at(a), at(b));
            painter.line_segment([pa, pos2(pb.x, pa.y)], line);
            painter.line_segment([pos2(pb.x, pa.y), pb], line);
        }
    }
    if let Some(last) = shown.last() {
        let p = at(last);
        painter.line_segment([p, pos2(cx.rv.x_of(cx.len_ticks).min(r.max.x), p.y)], line);
    }
    for s in &shown {
        let p = at(s);
        if !(r.min.x - HIT..=r.max.x + HIT).contains(&p.x) {
            continue;
        }
        let dot = Rect::from_center_size(p, vec2(5.0, 5.0));
        if s.selected {
            painter.rect(dot, 1.0, cx.color.lerp_to_gamma(Color32::WHITE, 0.65), Stroke::new(1.0, t.accent), StrokeKind::Outside);
        } else {
            painter.rect_filled(dot, 1.0, if s.index.is_some() { cx.color } else { cx.color.gamma_multiply(0.6) });
        }
    }
    let resp = ui.interact(r, ui.id().with(("cc_lane", cx.cid.0, li)), Sense::click_and_drag());
    let mods = ui.input(|i| i.modifiers);
    let pointer = ui.ctx().pointer_latest_pos();
    let hit = |p: Pos2| {
        shown
            .iter()
            .filter_map(|s| {
                let q = at(s);
                let d = (q.x - p.x).abs().max((q.y - p.y).abs());
                (d <= HIT).then_some((s.index?, d))
            })
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(i, _)| i)
    };
    let in_lane = |i: &usize| cx.seq.ctrls.get(*i).is_some_and(|c| c.kind == lane.kind);
    if resp.drag_started()
        && let Some(p) = ui.input(|i| i.pointer.press_origin()).or_else(|| resp.interact_pointer_pos())
    {
        app.midi.selected.clear();
        app.midi.ctrl_selected.retain(in_lane);
        app.midi.lane_gesture = Some(match cx.tool {
            Tool::Pencil => {
                let tick = cx.rv.tick_at(p.x);
                let tick = if cx.shape == PencilShape::FreeHand { (tick.round() as i64).max(0) } else { cx.snap(tick) };
                let from = (tick, lane.value_at(p.y));
                let seed = ui.input(|i| i.time).to_bits();
                LaneGesture::Draw { lane: li, points: vec![from], stroke: ShapeStroke { shape: cx.shape, from, to: from, period: cx.grid, seed } }
            }
            Tool::Trim => {
                // The selected events, or every event in the lane when none is.
                if app.midi.ctrl_selected.is_empty() {
                    app.midi.ctrl_selected = (0..cx.seq.ctrls.len()).filter(in_lane).collect();
                }
                LaneGesture::Move { lane: li, start: p, anchor_tick: 0, dt: 0, dv: 0, copy: false, values_only: true }
            }
            _ => match hit(p) {
                Some(i) => {
                    if !app.midi.ctrl_selected.contains(&i) {
                        if !mods.shift {
                            app.midi.ctrl_selected.clear();
                        }
                        app.midi.ctrl_selected.push(i);
                    }
                    let anchor_tick = cx.seq.ctrls.get(i).map_or(0, |c| c.tick);
                    LaneGesture::Move { lane: li, start: p, anchor_tick, dt: 0, dv: 0, copy: mods.alt, values_only: false }
                }
                None => LaneGesture::Band { lane: li, start: p },
            },
        });
    }
    let mut info: Option<String> = None;
    if let (Some(p), Some(g)) = (pointer, app.midi.lane_gesture.as_mut())
        && g.lane() == Some(li)
    {
        let value = lane.value_at(p.y);
        match g {
            LaneGesture::Draw { points, stroke, .. } => {
                let tick = cx.rv.tick_at(p.x);
                if stroke.shape == PencilShape::FreeHand {
                    let tick = (tick.round() as i64).max(0);
                    // Fill the gap since the last frame so a quick stroke leaves no holes.
                    if let Some(&(lt, lv)) = points.last() {
                        let n = (tick.saturating_sub(lt).abs() / DRAW_STEP).min(512);
                        for k in 1..n {
                            let f = k as f64 / n as f64;
                            points.push((lt + (tick.saturating_sub(lt) as f64 * f) as i64, lv + (value - lv) * f));
                        }
                    }
                    if points.len() < MAX_POINTS as usize {
                        points.push((tick, value));
                    }
                    info = Some(format!("{}  {}", value_text(lane.kind, value as i64), position_label(cx.tempo, cx.base_tick + tick)));
                } else {
                    stroke.to = (cx.snap(tick), value);
                    info = Some(format!("{} {}", stroke.shape.label(), value_text(lane.kind, value as i64)));
                }
            }
            LaneGesture::Move { start, anchor_tick, dt, dv, values_only, .. } => {
                if !*values_only {
                    let raw = f64::from(p.x - start.x) / f64::from(cx.rv.px_per_tick.max(f32::EPSILON));
                    *dt = if cx.snapping { snap_tick(*anchor_tick as f64 + raw, cx.base_tick, cx.grid) - *anchor_tick } else { raw.round() as i64 };
                }
                *dv = (value - lane.value_at(start.y)) as i64;
                info = Some(if *values_only {
                    format!("{:+}", *dv)
                } else {
                    format!("{:+}  {}", *dv, position_label(cx.tempo, cx.base_tick + (*anchor_tick + *dt).max(0)))
                });
            }
            LaneGesture::Band { start, .. } => {
                ui.painter().rect(
                    Rect::from_two_pos(*start, p).intersect(r),
                    0.0,
                    Color32::from_rgba_unmultiplied(120, 170, 255, 40),
                    Stroke::new(1.0, Color32::from_rgb(120, 170, 255)),
                    StrokeKind::Inside,
                );
            }
            LaneGesture::Velocity { .. } => {}
        }
    }
    let released = resp.drag_stopped() || !ui.input(|i| i.pointer.primary_down());
    if released && app.midi.lane_gesture.as_ref().is_some_and(|g| g.lane() == Some(li)) {
        let kind = lane.kind.id();
        match app.midi.lane_gesture.take() {
            Some(LaneGesture::Draw { points, stroke, .. }) => {
                let p = if stroke.shape == PencilShape::FreeHand {
                    let pts: Vec<Value> = free_hand(&points).iter().map(|&(t, v)| json!([t, v])).collect();
                    (pts.len() > 1).then(|| json!({"clip": cx.cid.0, "kind": kind, "points": pts}))
                } else {
                    (stroke.from != stroke.to).then(|| {
                        json!({"clip": cx.cid.0, "kind": kind, "shape": stroke.shape.id(), "from": [stroke.from.0, stroke.from.1],
                            "to": [stroke.to.0, stroke.to.1], "period": cx.grid, "seed": stroke.seed})
                    })
                };
                if let Some(p) = p {
                    let _ = app.run("midi.ctrls_draw", p);
                    app.midi.ctrl_selected.clear();
                }
            }
            Some(LaneGesture::Move { dt, dv, copy, .. }) if dt != 0 || dv != 0 => {
                let sel = app.midi.ctrl_selected.clone();
                let r = app.run("midi.ctrls_move", json!({"clip": cx.cid.0, "indices": sel, "ticks": dt, "value": dv, "copy": copy}));
                app.midi.ctrl_selected = returned_indices(r);
            }
            Some(LaneGesture::Band { start, .. }) => {
                if let Some(p) = pointer {
                    let band = Rect::from_two_pos(start, p);
                    let picked = shown.iter().filter(|s| band.contains(at(s))).filter_map(|s| s.index);
                    if mods.shift {
                        for i in picked {
                            if !app.midi.ctrl_selected.contains(&i) {
                                app.midi.ctrl_selected.push(i);
                            }
                        }
                    } else {
                        app.midi.ctrl_selected = picked.collect();
                    }
                }
            }
            _ => {}
        }
    }
    if resp.clicked()
        && let Some(p) = resp.interact_pointer_pos()
    {
        app.midi.selected.clear();
        app.midi.ctrl_selected.retain(in_lane);
        let add = json!({"clip": cx.cid.0, "kind": lane.kind.id(), "tick": cx.snap(cx.rv.tick_at(p.x)), "value": lane.value_at(p.y)});
        match (cx.tool, hit(p)) {
            (_, Some(i)) if mods.alt => {
                let _ = app.run("midi.ctrl_delete", json!({"clip": cx.cid.0, "indices": [i]}));
                app.midi.ctrl_selected.clear();
            }
            (Tool::Pencil, _) | (Tool::Grabber | Tool::Smart, None) => {
                let r = app.run("midi.ctrl_add", add);
                app.midi.ctrl_selected = r.ok().and_then(|v| v["index"].as_u64()).map(|i| vec![i as usize]).unwrap_or_default();
            }
            (_, Some(i)) => {
                if mods.shift {
                    if let Some(k) = app.midi.ctrl_selected.iter().position(|&x| x == i) {
                        app.midi.ctrl_selected.remove(k);
                    } else {
                        app.midi.ctrl_selected.push(i);
                    }
                } else {
                    app.midi.ctrl_selected = vec![i];
                }
            }
            (_, None) => app.midi.ctrl_selected.clear(),
        }
    }
    if info.is_none()
        && app.midi.lane_gesture.is_none()
        && let Some(p) = pointer.filter(|p| r.contains(*p))
    {
        info = Some(match hit(p).and_then(|i| cx.seq.ctrls.get(i)) {
            Some(c) => {
                format!("{}  {}  {}", lane.kind.label(), value_text(lane.kind, i64::from(c.value)), position_label(cx.tempo, cx.base_tick + c.tick))
            }
            None => format!("{}  {}", lane.kind.label(), value_text(lane.kind, lane.value_at(p.y) as i64)),
        });
    }
    if let (Some(text), Some(p)) = (info, pointer) {
        tooltip(ui, cx.full, p, text);
    }
}

/// The Pencil's shapes in the Velocity lane: a stroke sets each note it spans to the shape's value
/// at the note's start (only the selected notes when some are). Free Hand is the lane's own stroke.
pub fn velocity_shape(app: &mut SoundApp, ui: &mut Ui, cx: &LaneCtx, vresp: &Response, editable: &dyn Fn(usize) -> bool) {
    let rv = cx.rv;
    let vel = rv.vel;
    let pointer = ui.ctx().pointer_latest_pos();
    let at = |p: Pos2| ((rv.tick_at(p.x).round() as i64).max(0), rv.velocity_at(p.y) as f64);
    if vresp.drag_started()
        && let Some(p) = ui.input(|i| i.pointer.press_origin()).or_else(|| vresp.interact_pointer_pos())
    {
        let from = at(p);
        let seed = ui.input(|i| i.time).to_bits();
        app.midi.lane_gesture = Some(LaneGesture::Velocity { stroke: ShapeStroke { shape: cx.shape, from, to: from, period: cx.grid, seed } });
    }
    let Some(LaneGesture::Velocity { stroke }) = app.midi.lane_gesture.as_mut() else { return };
    if let Some(p) = pointer {
        stroke.to = at(p);
    }
    let stroke = *stroke;
    let y = |v: f64| vel.max.y - 2.0 - (vel.height() - 4.0) * (v as f32 / 127.0);
    let pts: Vec<Pos2> = stroke.points(DRAW_STEP).iter().map(|&(t, v)| pos2(rv.x_of(t), y(v))).collect();
    ui.painter().with_clip_rect(vel).add(egui::Shape::line(pts, Stroke::new(1.5, Tokens::current().accent)));
    if vresp.drag_stopped() || !ui.input(|i| i.pointer.primary_down()) {
        app.midi.lane_gesture = None;
        let (lo, hi) = stroke.span();
        let notes: &[Note] = &cx.seq.notes;
        for (i, n) in notes.iter().enumerate().filter(|(i, n)| editable(*i) && (lo..=hi).contains(&n.start)) {
            let v = stroke.value_at(n.start).round().clamp(1.0, 127.0) as i64;
            if i64::from(n.velocity) != v {
                let _ = app.engine.execute_merged(
                    "midi.note_edit",
                    &json!({"clip": cx.cid.0, "index": i, "velocity": v}),
                    &format!("velshape:{}", cx.cid.0),
                );
            }
        }
        app.engine.end_merge();
    }
}

/// The menu of controllers: the common ones, then every CC number. Kinds with events in the clip
/// are marked. Returns the one picked.
fn kind_menu(ui: &mut Ui, present: &HashSet<CtrlKind>, current: Option<CtrlKind>) -> Option<CtrlKind> {
    let mut pick = None;
    let label = |k: CtrlKind| if present.contains(&k) { format!("{}  •", k.label()) } else { k.label() };
    for k in CtrlKind::COMMON {
        if ui.selectable_label(current == Some(k), label(k)).clicked() {
            pick = Some(k);
            ui.close();
        }
    }
    ui.menu_button("Other Controllers", |ui| {
        for (a, b) in [(0u8, 31u8), (32, 63), (64, 95), (96, 127)] {
            ui.menu_button(format!("CC {a}–{b}"), |ui| {
                for n in a..=b {
                    let k = CtrlKind::Cc(n);
                    if ui.selectable_label(current == Some(k), label(k)).clicked() {
                        pick = Some(k);
                        ui.close();
                    }
                }
            });
        }
    });
    pick
}

fn set_lanes(app: &mut SoundApp, track: TrackId, lanes: &[CtrlKind]) {
    let ids: Vec<String> = lanes.iter().map(|k| k.id()).collect();
    let _ = app.run("midi.lanes", json!({"track": track.0, "lanes": ids}));
}

/// The lane headers in the keys column: the Velocity lane's menu adds lanes; a controller lane's
/// menu changes what it shows or removes it.
pub fn lane_headers(app: &mut SoundApp, ui: &mut Ui, cx: &LaneCtx, lanes: &[LaneView]) {
    let t = Tokens::current();
    let x0 = cx.full.min.x;
    let x1 = cx.rv.roll.min.x;
    let present: HashSet<CtrlKind> = cx.seq.ctrls.iter().map(|c| c.kind).collect();
    let kinds: Vec<CtrlKind> = lanes.iter().map(|l| l.kind).collect();
    let header = |ui: &mut Ui, r: Rect, name: &str, full_name: &str, salt: (u64, usize)| -> Response {
        let resp = ui.interact(r, ui.id().with(("lane_header", salt)), Sense::click());
        let label = format!("{full_name} lane");
        resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, &label));
        let fill = if resp.hovered() { t.panel_bg2.lerp_to_gamma(t.accent, 0.15) } else { t.panel_bg2 };
        ui.painter().rect_filled(r, 0.0, fill);
        let p = ui.painter().with_clip_rect(r);
        let rows = r.height() >= 26.0;
        p.text(pos2(r.min.x + 4.0, if rows { r.min.y + 9.0 } else { r.center().y }), Align2::LEFT_CENTER, name, regular(9.5), t.text);
        // A small down-pointing triangle under the name: the header opens a menu.
        if rows {
            let c = pos2(r.min.x + 8.0, r.min.y + 21.0);
            p.add(egui::Shape::convex_polygon(vec![c + vec2(-3.0, -1.5), c + vec2(3.0, -1.5), c + vec2(0.0, 2.0)], t.text_dim, Stroke::NONE));
        }
        resp
    };
    let vel_r = Rect::from_min_max(pos2(x0, cx.rv.vel.min.y), pos2(x1 - HEADER_GAP, cx.rv.vel.max.y));
    let vresp = header(ui, vel_r, "Velocity", "Velocity", (cx.cid.0, usize::MAX)).on_hover_text("Add a controller lane");
    egui::Popup::menu(&vresp).show(|ui| {
        ui.menu_button("Add Controller Lane", |ui| {
            if let Some(k) = kind_menu(ui, &present, None)
                && !kinds.contains(&k)
            {
                let mut v = kinds.clone();
                v.push(k);
                set_lanes(app, cx.track, &v);
            }
        });
        let missing: Vec<CtrlKind> = present.iter().copied().filter(|k| !kinds.contains(k)).collect();
        if ui.add_enabled(!missing.is_empty(), egui::Button::new("Show Lanes with Data")).clicked() {
            let mut v = kinds.clone();
            let mut more = missing;
            more.sort_by_key(|k| k.id());
            v.extend(more);
            set_lanes(app, cx.track, &v);
            ui.close();
        }
    });
    for (i, lane) in lanes.iter().enumerate() {
        let r = Rect::from_min_max(pos2(x0, lane.rect.min.y), pos2(x1 - HEADER_GAP, lane.rect.max.y));
        let resp = header(ui, r, &lane.kind.short(), &lane.kind.label(), (cx.cid.0, i)).on_hover_text(lane.kind.label());
        egui::Popup::menu(&resp).show(|ui| {
            if let Some(k) = kind_menu(ui, &present, Some(lane.kind)) {
                // This lane shows `k` now; another lane that showed it goes away.
                let v: Vec<CtrlKind> =
                    kinds.iter().enumerate().filter(|&(j, x)| j == i || *x != k).map(|(j, x)| if j == i { k } else { *x }).collect();
                set_lanes(app, cx.track, &v);
            }
            ui.separator();
            if ui.button("Remove Lane").clicked() {
                let v: Vec<CtrlKind> = kinds.iter().copied().filter(|x| *x != lane.kind).collect();
                set_lanes(app, cx.track, &v);
                ui.close();
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The lanes stay inside the 60% cap. `midi_editor::show` gives the roll what is left, and a
    /// roll with no height returns before the lane headers are drawn, so breaking the cap would
    /// leave no way to remove a lane.
    #[test]
    fn lane_heights_stay_inside_the_cap() {
        for avail in [300.0f32, 600.0, 1200.0] {
            let cap = avail * 0.6;
            // Beyond this the gaps alone fill the cap; `show` shrinks the band for that case.
            let max_n = ((cap - VEL_MIN) / LANE_GAP).floor().max(1.0) as usize;
            for n in [1, 2, 5, 12, 25, max_n].into_iter().filter(|n| *n <= max_n) {
                let (vel, lane) = lane_heights(avail, n);
                let band = vel + n as f32 * (lane + LANE_GAP);
                assert!(band <= cap + 0.01, "avail {avail} n {n}: band {band} over cap {cap}");
                assert!(lane <= LANE_H, "avail {avail} n {n}: lane {lane} over {LANE_H}");
                assert!(vel <= VEL_LANE_H, "avail {avail} n {n}: velocity {vel} over {VEL_LANE_H}");
            }
        }
    }

    /// The case the finding names: 600 px of editor and 25 lanes used to need more than the cap.
    #[test]
    fn twenty_five_lanes_fit_a_600px_editor() {
        let (vel, lane) = lane_heights(600.0, 25);
        let band = vel + 25.0 * (lane + LANE_GAP);
        assert!(band <= 360.0 + 0.01, "band {band} over the 360 px cap");
    }

    #[test]
    fn no_lanes_leaves_only_the_velocity_lane() {
        assert_eq!(lane_heights(600.0, 0), (VEL_LANE_H, 0.0));
    }
}
