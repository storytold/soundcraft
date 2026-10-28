use super::automation::{auto_value, automation_id, lane_range, plugin_range};
use super::input::{new_midi_clip, snap};
use super::layout::{lane_rect, rect, sample_at};
use super::{
    Arc, AutoParam, Clip, ClipId, Dialog, EditMode, Gesture, HashSet, Pos2, Range, Rect, Samples, Session, SoundApp, Spot, TrackId, json, ops, pos2,
};

/// The same bounded range drives the preview and the committed fade.
pub(super) fn fade_drag_range(clip: &Clip, fade_in: bool, to: Samples) -> Option<Range> {
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

pub(super) fn fade_drag_preview(clip: &Clip, fade_in: bool, to: Samples) -> Option<soundcraft_model::Fade> {
    let range = fade_drag_range(clip, fade_in, to)?;
    let original = if fade_in { clip.fade_in } else { clip.fade_out };
    Some(soundcraft_model::Fade {
        len: range.end.saturating_sub(range.start),
        shape: if original.len > 0 { original.shape } else { soundcraft_model::FadeShape::SCurve },
        curvature: if original.len > 0 { original.curvature } else { 0.0 },
    })
}

pub(super) fn fade_curve_drag_value(fade: soundcraft_model::Fade, anchor: (f32, f32), height: f32, y: f32) -> f32 {
    fade.curvature_for_midpoint_gain(anchor.1 + (anchor.0 - y) / height.max(1.0))
}

/// Runs before anything is painted, against the last frame's layout.
pub(super) fn drive_drag(app: &mut SoundApp, ctx: &egui::Context) {
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
        Some(Gesture::Breakpoint { track, param, at: from }) => {
            if let Some(t) = s.track(*track)
                && let Some((_, row)) = rows.iter().find(|(i, _)| *i == t.id.0)
                && let Some(points) = t.lane(param).map(|l| l.points.as_slice())
                && let Some(i) = points.iter().position(|pt| pt.at == *from)
            {
                let lane = lane_rect(t, rect(*row), tl);
                let after = i.checked_sub(1).and_then(|j| points.get(j)).map_or(0, |pt| pt.at.saturating_add(1));
                let before = points.get(i + 1).map_or(Samples::MAX, |pt| pt.at.saturating_sub(1));
                let to = snap(&s, at).clamp(after, before.max(after));
                let value = auto_value(lane, lane_range(t, param), p.y);
                if to != *from || points.get(i).is_some_and(|pt| (pt.value - value).abs() > 1e-6) {
                    let (key, pid) = (format!("breakpoint:{}", track.0), automation_id(param));
                    if to != *from {
                        let clear = json!({"track": track.0, "param": pid, "start": *from, "end": from.saturating_add(1)});
                        let _ = app.engine.execute_merged("automation.clear", &clear, &key);
                    }
                    let _ =
                        app.engine.execute_merged("automation.set_point", &json!({"track": track.0, "param": pid, "at": to, "value": value}), &key);
                    *from = to;
                }
            }
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

/// Grid snapping while dragging; holding Cmd/Ctrl swaps Grid and Slip.
pub(super) fn drag_snap(s: &Session, at: Samples, flip: bool) -> Samples {
    if s.edit.edit_mode.is_grid() != flip { s.edit.grid.snap(at, s.sample_rate, &s.tempo, s.frame_rate) } else { at }
}

/// What a clip drag works out once per target track rather than every frame: where the tracks go
/// (None when the clips can't land there), each destination's earliest clip and Shuffle slots,
/// the clips that move (track, start, length) and the clip Grid snaps.
pub(super) struct DragPlan {
    pub(super) dest: Option<Vec<(TrackId, TrackId)>>,
    pub(super) groups: Vec<(TrackId, Samples, Vec<Samples>)>,
    pub(super) moving: Vec<(TrackId, Samples, Samples)>,
    grabbed: Option<Clip>,
}

/// The [`DragPlan`] for `clips` grabbed at `grab_at` on track `from` and dragged onto `to`, kept
/// until the document, the Edit mode or the target changes.
pub(super) fn drag_plan(
    ctx: &egui::Context,
    s: &Session,
    revision: u64,
    clips: &[ClipId],
    (from, to): (TrackId, TrackId),
    grab_at: Samples,
) -> Arc<DragPlan> {
    type Key = (u64, EditMode, Vec<ClipId>, TrackId, TrackId, Samples);
    let id = egui::Id::unique("clip_drag_plan");
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
pub(super) fn spot_or_run(app: &mut SoundApp, spot: Option<Spot>) {
    let Some(spot) = spot else { return };
    if app.engine.session().edit.edit_mode == EditMode::Spot {
        app.dialogs.open = Some(Dialog::Spot(spot));
    } else if let Some((id, params)) = spot.command(app.engine.session()) {
        let _ = app.run(&id, params);
    }
}
