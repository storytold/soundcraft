use super::layout::track_height;
use super::tracks::in_edit_window;
use super::universe::content_end;
use super::{Rect, Samples, Sense, SoundApp, Tokens, TrackId, Ui, json, pos2, vec2};

/// Scroll Into View: track `id` to the top, as far as the scroll bar reaches. A track the Edit
/// window doesn't show (hidden, or in a closed folder) leaves the view where it is.
pub(super) fn scroll_into_view(app: &mut SoundApp, id: TrackId, view_h: f32) {
    let s = app.engine.session();
    let (mut top, mut total, mut found) = (0.0, 0.0, false);
    for t in s.tracks.iter().filter(|t| in_edit_window(s, t)) {
        found |= t.id == id;
        if !found {
            top += track_height(t) + 1.0;
        }
        total += track_height(t) + 1.0;
    }
    if found {
        let l = &mut app.edit_layout;
        l.content_h = total;
        l.scroll_by(top - l.scroll_y, view_h);
    }
}

pub(super) fn scroll_and_zoom(app: &mut SoundApp, ui: &Ui, area: Rect, tl: Rect) {
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

/// Draw and drag the horizontal timeline scrollbar; its thumb reflects the visible sample range plus trailing space.
pub(super) fn hscrollbar(app: &mut SoundApp, ui: &mut Ui, sb: Rect, tl: Rect) {
    let t = Tokens::current();
    ui.painter().rect_filled(sb, 0.0, t.panel_bg);
    let end = content_end(app, ui.ctx());
    let s = app.engine.session();
    let vis = f64::from(tl.width()) * s.edit.zoom.samples_per_px;
    let total = (end.saturating_add(s.sample_rate.samples(30.0)).max(1) as f64).max(s.edit.zoom.scroll as f64 + vis);
    let frac = (vis / total).clamp(0.02, 1.0) as f32;
    let off = (s.edit.zoom.scroll as f64 / total).clamp(0.0, 1.0) as f32;
    let thumb = Rect::from_min_size(pos2(sb.min.x + sb.width() * off, sb.min.y + 3.0), vec2((sb.width() * frac).max(24.0), sb.height() - 6.0));
    ui.painter().rect_filled(thumb, 3.0, t.button_hi);
    let resp = ui.interact(sb, ui.scope_id().with("hscroll"), Sense::drag());
    if resp.dragged() {
        let dx = resp.drag_delta().x / sb.width();
        let by_px = f64::from(dx) * total / s.edit.zoom.samples_per_px;
        pan(app, by_px);
    }
}

/// Zoom Toggle sized to the track area on screen. With Alt held (an Alt-click on the button) it cancels Zoom Toggle
/// instead and keeps the current view.
pub fn zoom_toggle(app: &mut SoundApp) {
    if app.docking_context.as_ref().is_some_and(|c| c.input(|i| i.modifiers.alt)) {
        let _ = app.run("view.zoom_toggle_cancel", json!({}));
        return;
    }
    let [x0, y0, x1, y1] = app.edit_layout.timeline;
    let _ = app.run("view.zoom_toggle", json!({"width_px": x1 - x0, "height_px": y1 - y0}));
}

/// Zoom Toggle scrolls the first track it fitted to the top, and back when it turns off.
pub(super) fn follow_zoom_toggle(app: &mut SoundApp) {
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

/// One frame's inputs to [`follow_update`].
pub(super) struct FollowFrame {
    pub(super) playing: bool,
    pub(super) mode_follow: bool,
    pub(super) playhead_visible: bool,
    pub(super) cur: Samples,
    pub(super) last_scroll: Samples,
    pub(super) last_follow_to: Option<Samples>,
    pub(super) hold: bool,
    pub(super) pos: Samples,
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
pub(super) fn follow_update(frame: FollowFrame) -> (bool, Option<Samples>) {
    let FollowFrame { playing, mode_follow, playhead_visible, cur, last_scroll, last_follow_to, hold, pos } = frame;
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
