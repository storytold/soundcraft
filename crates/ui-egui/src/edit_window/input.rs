use super::automation::{automation_id, breakpoint_at, plugin_range, shown_automation};
use super::clips::{clip_name_height, fade_control_position};
use super::drag::{drag_snap, fade_curve_drag_value, fade_drag_range, spot_or_run};
use super::layout::{sample_at, x_of};
use super::{
    Align2, AutoParam, Clip, ClipContent, ClipId, Color32, Dialog, EditMode, Gesture, GridValue, NoteValue, Pos2, Range, Rect, Samples, Sense,
    Session, SoundApp, Spot, Stroke, StrokeKind, Tokens, Tool, Track, TrackId, Ui, json, ops, pos2, regular, vec2,
};

// ---- interaction -----------------------------------------------------------------------------

pub(super) fn snap(s: &Session, at: Samples) -> Samples {
    let on = s.edit.edit_mode.is_grid() || s.edit.flag(ops::SNAP_TO_GRID);
    if on { s.edit.grid.snap(at, s.sample_rate, &s.tempo, s.frame_rate) } else { at }
}

pub(super) fn clip_at(track: &Track, at: Samples) -> Option<&Clip> {
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

pub(super) fn near_fade_midpoint(p: Pos2, midpoint: Pos2) -> bool {
    let delta = p - midpoint;
    delta.x.abs() <= 8.0 && delta.y.abs() <= 8.0
}

pub(super) fn fade_midpoint(s: &Session, tl: Rect, lane: Rect, clip: &Clip, fade_in: bool) -> Option<Pos2> {
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

/// Draw the read-only clip-gain fader tooltip at `pos`.
fn clip_gain_popup(ctx: &egui::Context, fader_bottom: Pos2, pos: f32) {
    let t = Tokens::current();
    let p = ctx.layer_painter(egui::LayerId::new(egui::Order::Tooltip, egui::Id::unique("clip_gain_fader")));
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

/// What a lane interaction knows before it hit-tests: the lane, the session and the press point.
struct LaneInput<'a> {
    track: &'a Track,
    lane: Rect,
    tl: Rect,
    s: &'a Session,
    tool: Tool,
    origin: Pos2,
    at_origin: Samples,
    auto_view: Option<AutoParam>,
}

/// What is under the pointer: a breakpoint, fade handles and the clip body.
struct LaneHits<'a> {
    point_hit: Option<(AutoParam, Samples)>,
    fade_handle: Option<(&'a Clip, bool)>,
    curve_handle: Option<(&'a Clip, bool)>,
    hit: Option<Clip>,
}

/// Everything the lane's gesture, click and cursor handling reads.
struct LaneCtx<'a> {
    inp: LaneInput<'a>,
    hits: LaneHits<'a>,
    mods: egui::Modifiers,
    p: Pos2,
    at: Samples,
    eff: Tool,
    fade_corner: Option<bool>,
    selector_press: bool,
}

/// The lane's response, with its clip context menu and the drops it accepts.
fn lane_menu_and_drops(app: &mut SoundApp, ui: &mut Ui, track: &Track, lane: Rect, tl: Rect) -> egui::Response {
    let id = ui.scope_id().with(("lane", track.id.0));
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
    resp
}

/// Hit-test the press point against breakpoints, fade handles and clip bodies.
fn hit_test<'a>(inp: &LaneInput<'a>) -> LaneHits<'a> {
    let (track, lane, tl, s, tool, origin, at_origin) = (inp.track, inp.lane, inp.tl, inp.s, inp.tool, inp.origin, inp.at_origin);
    let auto_view = &inp.auto_view;
    let point_hit = if matches!(tool, Tool::Smart | Tool::Grabber | Tool::Pencil) {
        shown_automation(track).and_then(|param| breakpoint_at(s, tl, lane, track, &param, origin).map(|at| (param, at)))
    } else {
        None
    };
    let body_hit = clip_at(track, at_origin);
    // Check fade endpoints before clip bodies: a full-length fade can end exactly
    // at the next clip's start, where `clip_at` would otherwise choose that neighbour.
    let fade_handle = if tool == Tool::Smart && auto_view.is_none() {
        track
            .clips()
            .iter()
            .rev()
            .filter(|c| body_hit.is_none_or(|front| front.id == c.id || front.start == c.end()))
            .find_map(|c| fade_handle_at(s, tl, lane, c, origin).map(|side| (c, side)))
    } else {
        None
    };
    let curve_handle = if tool == Tool::Smart && auto_view.is_none() && fade_handle.is_none() {
        body_hit.and_then(|c| fade_curve_handle_at(s, tl, lane, c, origin).map(|side| (c, side)))
    } else {
        None
    };
    let hit = fade_handle.or(curve_handle).map(|(c, _)| c).or(body_hit).cloned();
    LaneHits { point_hit, fade_handle, curve_handle, hit }
}

/// Which edge of `c` the press point is within a few points of: `Some(true)` the start, `Some(false)` the end.
fn near_edge(inp: &LaneInput<'_>, c: &Clip) -> Option<bool> {
    let xs = x_of(inp.s, inp.tl, c.start);
    let xe = x_of(inp.s, inp.tl, c.end());
    if (inp.origin.x - xs).abs() < 6.0 {
        Some(true)
    } else if (inp.origin.x - xe).abs() < 6.0 {
        Some(false)
    } else {
        None
    }
}

/// Effective tool under the Smart tool; on a breakpoint every tool that edits one grabs it.
fn effective_tool(inp: &LaneInput<'_>, hits: &LaneHits<'_>) -> Tool {
    let upper = inp.origin.y < inp.lane.center().y;
    match inp.tool {
        _ if hits.point_hit.is_some() => Tool::Grabber,
        Tool::Smart => {
            if let Some(c) = &hits.hit {
                if near_edge(inp, c).is_some() {
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
    }
}

/// The fade corner (`true` for the fade-in) the Smart tool is pressing, if any.
fn fade_corner_at(inp: &LaneInput<'_>, hits: &LaneHits<'_>) -> Option<bool> {
    let (s, tl, lane, origin) = (inp.s, inp.tl, inp.lane, inp.origin);
    let (tool, point_hit, fade_handle, hit) = (inp.tool, &hits.point_hit, hits.fade_handle, &hits.hit);
    if tool == Tool::Smart && point_hit.is_none() {
        fade_handle.map(|(_, side)| side).or_else(|| {
            hit.as_ref().and_then(|c| {
                let xs = x_of(s, tl, c.start);
                let xe = x_of(s, tl, c.end());
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
    }
}

/// Start the gesture a press begins: a breakpoint, clip gain, a fade or the tool's own.
fn start_gesture(app: &mut SoundApp, ui: &mut Ui, c: &LaneCtx<'_>, resp: &egui::Response, gain_drag: Option<Clip>) {
    let LaneCtx { inp, hits, mods, p, at, eff, fade_corner, .. } = c;
    let (track, lane, s, origin, at_origin, auto_view) = (inp.track, inp.lane, inp.s, inp.origin, inp.at_origin, &inp.auto_view);
    let (point_hit, curve_handle, hit) = (&hits.point_hit, hits.curve_handle, &hits.hit);
    let (mods, p, at, eff, fade_corner) = (*mods, *p, *at, *eff, *fade_corner);
    if resp.drag_started()
        && let Some((param, at)) = &point_hit
    {
        app.gesture = Some(Gesture::Breakpoint { track: track.id, param: param.clone(), at: *at });
    } else if let Some(c) = gain_drag {
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
        let height = (lane.height() - 2.0 - clip_name_height(s, lane)).max(1.0);
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
            Tool::Selector | Tool::Smart => Some(Gesture::Select { track: track.id, anchor: snap(s, at_origin), tracks: vec![track.id] }),
            Tool::Grabber => hit.as_ref().map(|c| {
                let clips = if s.edit.selected_clips.contains(&c.id) { s.edit.selected_clips.clone() } else { vec![c.id] };
                Gesture::MoveClips { clips, grab_at: at_origin, delta: 0, from_track: track.id, to_track: track.id }
            }),
            Tool::Trim => hit.as_ref().map(|c| match near_edge(inp, c) {
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
}

/// While a clip-gain gesture is dragged, the fader follows the pointer.
fn drag_clip_gain(app: &mut SoundApp, ui: &mut Ui, c: &LaneCtx<'_>, resp: &egui::Response) {
    let (track, s, mods, p) = (c.inp.track, c.inp.s, c.mods, c.p);
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
}

/// Click handling for the lane: breakpoints, clip selection, spot dialogs, zoom and pencil points.
fn lane_click(app: &mut SoundApp, ui: &mut Ui, c: &LaneCtx<'_>, resp: &egui::Response) {
    let (track, lane, s, auto_view) = (c.inp.track, c.inp.lane, c.inp.s, &c.inp.auto_view);
    let (point_hit, curve_handle, hit) = (&c.hits.point_hit, c.hits.curve_handle, &c.hits.hit);
    let (mods, p, at, eff, selector_press) = (c.mods, c.p, c.at, c.eff, c.selector_press);
    if resp.clicked()
        && let Some((param, at)) = &point_hit
    {
        // Alt-click deletes a breakpoint, as in Pro Tools.
        if mods.alt {
            let _ =
                app.run("automation.clear", json!({"track": track.id.0, "param": automation_id(param), "start": at, "end": at.saturating_add(1)}));
        }
    } else if resp.clicked() && curve_handle.is_none() {
        match eff {
            Tool::Grabber => {
                if let Some(c) = &hit {
                    let mut ids = if mods.shift { s.edit.selected_clips.iter().map(|x| x.0).collect::<Vec<_>>() } else { Vec::new() };
                    ids.push(c.id.0);
                    let _ = app.run("edit.select", json!({"clips": ids}));
                    if s.edit.edit_mode == EditMode::Spot {
                        app.dialogs.open = Spot::clips(s, vec![c.id], None, 0).map(Dialog::Spot);
                    }
                }
            }
            Tool::Trim if s.edit.edit_mode == EditMode::Spot && hit.is_some() => {
                if let Some(c) = &hit {
                    let start = at - c.start < c.end() - at;
                    app.dialogs.open = Spot::trim(s, c.id, start, if start { c.start } else { c.end() }).map(Dialog::Spot);
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
                } else if track.kind.is_midi() {
                    match &hit {
                        Some(c) => {
                            let _ = app.run("edit.select", json!({"clips": [c.id.0]}));
                        }
                        None => new_midi_clip(app, s, track.id, at, at),
                    }
                }
            }
            _ if !selector_press => place_insertion(app, ui, s, track.id, at, mods.shift),
            _ => {}
        }
    }
    if resp.double_clicked()
        && curve_handle.is_none()
        && point_hit.is_none()
        && eff != Tool::Pencil
        && let Some(c) = &hit
    {
        let _ = app.run("edit.select", json!({"clips": [c.id.0]}));
        if eff == Tool::Grabber && matches!(c.content, ClipContent::Midi { .. }) {
            app.ui.show_midi_editor = true;
        }
    }
}

/// Handle one track lane: clip context menus, drag-and-drop sources, sampler drops, and the active tool's selection,
/// trim, fade, automation and clip editing.
pub(super) fn lane_interaction(app: &mut SoundApp, ui: &mut Ui, track: &Track, lane: Rect, tl: Rect) {
    let resp = lane_menu_and_drops(app, ui, track, lane, tl);
    let s = app.engine.session_arc();
    let tool = s.edit.tool;
    let mods = ui.input(|i| i.modifiers);
    let Some(p) = resp.interact_pointer_pos().or_else(|| resp.hover_pos()) else { return };
    let at = sample_at(&s, tl, p.x).max(0);
    let origin = if resp.drag_started() { ui.input(|i| i.pointer.press_origin()).unwrap_or(p) } else { p };
    let at_origin = sample_at(&s, tl, origin.x).max(0);
    let auto_view = AutoParam::parse(&track.view);
    let inp = LaneInput { track, lane, tl, s: &s, tool, origin, at_origin, auto_view };
    let hits = hit_test(&inp);
    let eff = effective_tool(&inp, &hits);
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
    let fade_corner = fade_corner_at(&inp, &hits);
    let curve_handle = hits.curve_handle;
    if fade_corner.is_some() && resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeNwSe);
    }
    if curve_handle.is_some() && resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeVertical);
    }
    let gain_icon_at = |q: Pos2| {
        let c = clip_at(track, sample_at(&s, tl, q.x).max(0)).filter(|_| inp.auto_view.is_none())?;
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
    let cx = LaneCtx { inp, hits, mods, p, at, eff, fade_corner, selector_press };
    start_gesture(app, ui, &cx, &resp, gain_drag);
    drag_clip_gain(app, ui, &cx, &resp);
    lane_click(app, ui, &cx, &resp);
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

/// Whole bars from `from` to `to` (at least one), inside the gap between clips where `from` is.
pub(super) fn midi_clip_span(s: &Session, track: TrackId, from: Samples, to: Samples) -> Option<Range> {
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

pub(super) fn new_midi_clip(app: &mut SoundApp, s: &Session, track: TrackId, from: Samples, to: Samples) {
    if let Some(r) = midi_clip_span(s, track, from, to)
        && let Ok(v) = app.run("midi.new_clip", json!({"track": track.0, "start": r.start, "end": r.end}))
    {
        let _ = app.run("edit.select", json!({"clips": [v["clip"]]}));
    }
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
