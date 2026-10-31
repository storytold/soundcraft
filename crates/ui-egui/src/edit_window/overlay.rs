use super::drag::drag_plan;
use super::input::midi_clip_span;
use super::layout::x_of;
use super::scroll::{FollowFrame, follow_update};
use super::{Color32, Gesture, Rect, Samples, Shape, SoundApp, Stroke, StrokeKind, Tokens, TrackId, Ui, json, ops, pos2, rgb};

/// Selection overlay, insertion point and playhead across rulers and tracks.
/// While playing, auto-scroll keeps the playhead in view for the follow
/// modes (`page`/`continuous`/`center`) — but any outside view move (a manual
/// pan, scrollbar, universe jump, or programmatic scroll) pauses it, so the
/// timeline can be inspected anywhere during playback. Scrolling back to the
/// playhead, stopping/starting, or re-selecting a scrolling mode resumes the
/// follow. `none` and `after_playback` never scroll during playback.
pub(super) fn overlay(app: &mut SoundApp, ui: &mut Ui, tl: Rect, area: Rect) {
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
        let (id, key) = (ui.scope_id().with("insertion_blink"), (sel.start, s.edit.selected_tracks.clone()));
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
        let (hold, follow_to) = follow_update(FollowFrame {
            playing: true,
            mode_follow,
            playhead_visible: visible,
            cur,
            last_scroll: layout.last_scroll,
            last_follow_to: layout.last_follow_to,
            hold: layout.follow_hold,
            pos,
        });
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
