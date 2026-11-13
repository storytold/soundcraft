use super::drag::fade_drag_preview;
use super::input::{gain_icon, near_fade_midpoint};
use super::layout::{sample_at, x_of};
use super::{
    Align2, AutoParam, Clip, ClipContent, ClipId, Color32, CornerRadius, Gesture, GridValue, NoteValue, Pos2, Range, Rect, Samples, Session, Shape,
    SoundApp, Stroke, StrokeKind, TimeFormat, Tokens, Tool, Track, bold, clip_colors, crossfade_cut, format_position, ops, pos2, regular, vec2,
};
use std::sync::Arc;

/// The outline of the Strip Silence rectangles.
pub fn strip_silence_stroke() -> Stroke {
    Stroke::new(1.5, Tokens::current().accent)
}

/// The Strip Silence rectangles on one lane, while the Strip Silence window is open: one around each
/// range Strip keeps.
pub(super) fn draw_strip_silence(painter: &egui::Painter, s: &Session, tl: Rect, lane: Rect, ranges: &[Range]) {
    let stroke = strip_silence_stroke();
    for r in ranges {
        let x0 = x_of(s, tl, r.start).max(lane.min.x);
        let x1 = x_of(s, tl, r.end).min(lane.max.x);
        if x1 > x0 {
            let rect = Rect::from_min_max(pos2(x0, lane.min.y + 1.0), pos2(x1, lane.max.y - 1.0));
            painter.rect(rect, 0.0, stroke.color.gamma_multiply(0.12), stroke, StrokeKind::Inside);
        }
    }
}

/// Grid positions computed once for the current viewport and shared by all visible lanes.
#[derive(Default)]
pub(super) struct GridLines {
    bars: Vec<f32>,
    grid: Vec<f32>,
}

impl GridLines {
    /// Collect and project visible lines before drawing any lanes in this frame.
    pub(super) fn new(s: &Session, tl: Rect) -> Self {
        let mut lines = Self::default();
        let view = Range::new(sample_at(s, tl, tl.min.x).max(0), sample_at(s, tl, tl.max.x));
        let sr = s.sample_rate;
        // Bar lines always; grid lines when enabled and dense enough.
        let bar = GridValue::Note { value: NoteValue::Bar, dotted: false, triplet: false };
        let bar_px = bar.step_samples(view.start, sr, &s.tempo, s.frame_rate) as f64 / s.edit.zoom.samples_per_px;
        if bar_px > 6.0 {
            lines.bars = bar.lines(view.start, view.end, sr, &s.tempo, s.frame_rate, 600).into_iter().map(|at| x_of(s, tl, at)).collect();
        }
        if s.edit.grid_lines {
            let step_px = s.edit.grid.step_samples(view.start, sr, &s.tempo, s.frame_rate) as f64 / s.edit.zoom.samples_per_px;
            if s.edit.grid_clips_markers || step_px > 7.0 {
                let visible_columns = tl.width().ceil().clamp(1.0, 16_384.0) as usize;
                lines.grid = s.grid_lines(view.start, view.end, visible_columns).into_iter().map(|at| x_of(s, tl, at)).collect();
            }
        }
        lines
    }

    /// Draw the prepared lines on one lane, preserving bar and grid colors and stacking order.
    pub(super) fn draw(&self, painter: &egui::Painter, lane: Rect) {
        let t = Tokens::current();
        for (positions, color) in [(&self.bars, t.bar_line), (&self.grid, t.grid_line)] {
            let col = grid_veil(color, t.playlist_bg);
            for &x in positions {
                painter.line_segment([pos2(x, lane.min.y), pos2(x, lane.max.y)], Stroke::new(1.0, col));
            }
        }
    }
}

/// Whether clips show their name (View › Clip › Name) in a lane tall enough to carry it.
pub(super) fn clip_name_shown(s: &Session, lane: Rect) -> bool {
    lane.height() >= 40.0 && s.edit.flag("view.clip.name")
}

pub(super) fn fade_curve_points(fade: soundcraft_model::Fade, fade_in: bool, a: f32, b: f32, body: Rect, pixels_per_point: f32) -> Vec<Pos2> {
    let scale = if pixels_per_point.is_finite() { pixels_per_point.clamp(0.5, 4.0) } else { 1.0 };
    let extent = (b - a).abs().max(body.height().abs()) * scale;
    let extent = if extent.is_finite() { extent.clamp(64.0, 4096.0) } else { 64.0 };
    // A narrow fade needs fewer: the curve between two samples stays inside a box `width / segments`
    // wide, so it is never further than that from its chord. Zoomed out on an edited drum take,
    // thousands of fades are a pixel or two wide.
    let narrow = ((b - a).abs() * scale * 4.0).ceil() + 1.0;
    // Physical-pixel sampling avoids visible corners, even for steep bends.
    // Keep an exact midpoint and cap allocations for extreme zoom/clip sizes.
    let segments = (extent.min(narrow).max(2.0).ceil() as usize).div_ceil(2) * 2;
    (0..=segments)
        .map(|i| {
            let k = i as f32 / segments as f32;
            pos2(a + (b - a) * k, body.max.y - body.height() * fade.gain(if fade_in { k } else { 1.0 - k }))
        })
        .collect()
}

pub(super) fn fade_control_position(fade: soundcraft_model::Fade, a: f32, b: f32, body: Rect) -> Pos2 {
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
pub(super) fn draw_live_take(app: &SoundApp, painter: &egui::Painter, s: &Session, track: &Track, tl: Rect, lane: Rect) {
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
        crate::waveform::draw_live(
            crate::waveform::Canvas { painter: &painter.with_clip_rect(r.intersect(lane)), s, tl },
            origin as f64,
            block_len,
            &take,
            r,
            wave,
        );
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

/// What `draw_clip` draws into: the app, painter and session, and the track lane the clips sit on.
#[derive(Clone, Copy)]
pub(super) struct ClipLane<'a> {
    pub(super) app: &'a SoundApp,
    pub(super) painter: &'a egui::Painter,
    pub(super) s: &'a Session,
    pub(super) track: &'a Track,
    /// Whether `track.clips()` is sorted by start (worked out once per track and frame).
    pub(super) sorted: bool,
    pub(super) tl: Rect,
    pub(super) lane: Rect,
}

/// Where and in what colours one clip is drawn, shared by the parts of `draw_clip`.
#[derive(Clone, Copy)]
struct ClipGeom<'a> {
    r: Rect,
    body_r: Rect,
    vis: Rect,
    /// The part of the clip that shows: up to or from the cut of a crossfade.
    x0: f32,
    x1: f32,
    /// The clip's own extent, which its fades run along.
    cx0: f32,
    cx1: f32,
    /// [`Self::x0`] and [`Self::x1`] as positions.
    shown: (Samples, Samples),
    body: Color32,
    wave: Color32,
    selected: bool,
    dim: bool,
    fade_in: soundcraft_model::Fade,
    fade_out: soundcraft_model::Fade,
    /// The clip's crossfade partners, before and after it (see [`ops::crossfade_partners`]).
    before: Option<&'a Clip>,
    after: Option<&'a Clip>,
    /// A clip this one is laid over without a crossfade: its tail shows through.
    layered: Option<&'a Clip>,
    /// How the clip meets neighbours it is butted to, outside a crossfade (see [`clip_cuts`]).
    cut: ClipCut,
}

/// Draw one clip of `clips` (its playlist), with `partners`, its [`ops::crossfade_partners`], and `cut`, how it meets the clips it is butted to (see [`clip_cuts`]). The two clips of a
/// crossfade are drawn butted at the cut, each only on its side of it; the zone laid over the cut is drawn with the
/// later clip.
pub(super) fn draw_clip<'a>(
    cl: &ClipLane<'_>,
    clips: &'a [Clip],
    partners: (Option<&'a Clip>, Option<&'a Clip>),
    cut: ClipCut,
    clip: &Clip,
    dim: bool,
) {
    let ClipLane { app, painter: outer, s, track, tl, lane, .. } = *cl;
    let (before, after) = partners;
    let cx0 = x_of(s, tl, clip.start);
    let cx1 = x_of(s, tl, clip.end()).max(cx0 + 1.0);
    let shown = shown_span(clip, before, after);
    let x0 = x_of(s, tl, shown.0);
    let x1 = x_of(s, tl, shown.1).max(x0 + 1.0);
    // The two clips of a crossfade show only on their side of the cut; any other clip keeps the lane as it was.
    let narrowed;
    let painter = if before.is_some() || after.is_some() {
        narrowed = outer.with_clip_rect(Rect::from_x_y_ranges(x0..=x1, lane.y_range()));
        &narrowed
    } else {
        outer
    };
    let inner = ClipLane { painter, ..*cl };
    let r = Rect::from_min_max(pos2(x0, lane.min.y + 1.0), pos2(x1, lane.max.y - 1.0));
    let selected = s.edit.selected_clips.contains(&clip.id)
        || (s.edit.selected_tracks.contains(&track.id)
            && !s.edit.selection.is_empty()
            && s.edit.selection.start <= clip.start
            && s.edit.selection.end >= clip.end());
    let color = clip.color.unwrap_or(track.color);
    let (mut body, _, mut wave) = clip_colors(color, selected);
    if clip.muted || dim {
        body = body.gamma_multiply(0.45);
        wave = Color32::from_rgb(70, 70, 70);
    }
    let body = if s.edit.flag("view.clip.transparency") { body.gamma_multiply(0.55) } else { body };
    let (corners, outline) =
        (ClipCut { left: cut.left || before.is_some(), right: cut.right || after.is_some() }.corners(), Stroke::new(1.0, CLIP_OUTLINE));
    if before.is_some() || cut.right || after.is_some() {
        // Where the clip is butted at a cut, or to its crossfade partner, its outline stops: the edge there is stroked
        // past the painter's clip, so no line shows (a cut's single line is drawn by the next clip; a crossfade's
        // cut is never drawn).
        painter.rect_filled(r, corners, body);
        let past = Rect::from_min_max(
            pos2(if before.is_some() { r.min.x - 2.0 } else { r.min.x }, r.min.y),
            pos2(if cut.right || after.is_some() { r.max.x + 2.0 } else { r.max.x }, r.max.y),
        );
        painter.with_clip_rect(painter.clip_rect().intersect(r)).rect_stroke(past, corners, outline, StrokeKind::Inside);
    } else {
        painter.rect(r, corners, body, outline, StrokeKind::Inside);
    }
    // The body is the whole clip: the name is a small pill drawn over it (see `draw_clip_pills`).
    let body_r = r;
    let vis = body_r.intersect(lane);
    // The waveform and fade curve share the same temporary envelope every frame.
    let (fade_in, fade_out) = clip_preview_fades(clip, app.gesture.as_ref().filter(|_| !dim));
    let layered = crossfade_partner(clips, clip).filter(|p| before.is_none_or(|a| a.id != p.id));
    let g = ClipGeom { r, body_r, vis, x0, x1, cx0, cx1, shown, body, wave, selected, dim, fade_in, fade_out, before, after, layered, cut };
    draw_clip_content(&inner, clip, &g);
    draw_crossfade_tail(&inner, outer, clip, &g);
    draw_gain_line(&inner, clip, &g);
    draw_fades(&inner, clip, &g);
    draw_clip_overlays(&inner, clip, &g);
    if let Some(earlier) = before {
        draw_zone(cl, earlier, clip, body_r.y_range(), dim);
    }
}

/// The clip body: waveform, MIDI notes or the offline notice.
fn draw_clip_content(cl: &ClipLane<'_>, clip: &Clip, g: &ClipGeom<'_>) {
    let ClipLane { painter, s, track, tl, .. } = *cl;
    let ClipGeom { body_r, vis, body, wave, selected, fade_in, fade_out, .. } = *g;
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
                    crate::waveform::draw(
                        crate::waveform::Canvas { painter: &painter.with_clip_rect(vis), s, tl },
                        shown,
                        audio,
                        track.channels(),
                        body_r,
                        wave,
                    );
                    let n = track.channels().min(audio.peaks.len()).max(1);
                    for k in 1..n {
                        let y = body_r.min.y + body_r.height() / n as f32 * k as f32;
                        painter.line_segment([pos2(vis.min.x, y), pos2(vis.max.x, y)], Stroke::new(1.0, body.gamma_multiply(0.7)));
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
}

/// The waveform of the clip this one crossfades with or is laid over, drawn over the overlap; in a crossfade, also
/// the clip's own audio left of the cut, drawn with `outer` (the lane's painter: the clip's own stops at the cut).
fn draw_crossfade_tail(cl: &ClipLane<'_>, outer: &egui::Painter, clip: &Clip, g: &ClipGeom<'_>) {
    let ClipLane { painter, s, track, tl, lane, .. } = *cl;
    let ClipGeom { body_r, vis, x0, x1, cx0, dim, before, layered, .. } = *g;
    let overlapped = s.edit.flag("waveform.overlapped_crossfades") && track.view != "blocks";
    if overlapped
        && let Some(prev) = before.or(layered)
        && let ClipContent::Audio { source, .. } = &prev.content
        && let Some(audio) = s.pool.get(*source)
    {
        let xf = Rect::from_min_max(pos2(x0, body_r.min.y), pos2(x_of(s, tl, prev.end()).min(x1), body_r.max.y)).intersect(vis);
        let tail = if prev.muted || dim {
            Color32::from_rgb(70, 70, 70)
        } else {
            clip_colors(prev.color.unwrap_or(track.color), false).2.gamma_multiply(0.6)
        };
        crate::waveform::draw(crate::waveform::Canvas { painter: &painter.with_clip_rect(xf), s, tl }, prev, audio, track.channels(), body_r, tail);
    }
    // In a crossfade both clips play across the whole zone: the later one's audio also shows left of the cut.
    if overlapped
        && before.is_some()
        && let ClipContent::Audio { source, .. } = &clip.content
        && let Some(audio) = s.pool.get(*source)
    {
        let own = Rect::from_min_max(pos2(cx0, body_r.min.y), pos2(x0, body_r.max.y));
        let tail = if clip.muted || dim {
            Color32::from_rgb(70, 70, 70)
        } else {
            clip_colors(clip.color.unwrap_or(track.color), false).2.gamma_multiply(0.6)
        };
        crate::waveform::draw(
            crate::waveform::Canvas { painter: &outer.with_clip_rect(own.intersect(lane)), s, tl },
            clip,
            audio,
            track.channels(),
            own,
            tail,
        );
    }
}

/// The clip gain line and its icon.
fn draw_gain_line(cl: &ClipLane<'_>, clip: &Clip, g: &ClipGeom<'_>) {
    let ClipLane { painter, s, track, tl, lane, .. } = *cl;
    let ClipGeom { body_r, x0, x1, shown, dim, .. } = *g;
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
    if !dim && let Some(icon) = gain_icon(s, tl, lane, clip, shown.0, shown.1) {
        let col = Color32::from_rgba_unmultiplied(255, 255, 255, 200);
        painter.line_segment([pos2(icon.center().x, icon.min.y), pos2(icon.center().x, icon.max.y)], Stroke::new(1.0, col));
        painter.rect_filled(Rect::from_center_size(pos2(icon.center().x, icon.center().y + 2.0), vec2(icon.width(), 3.0)), 1.0, col);
        painter.text(pos2(icon.max.x + 3.0, icon.center().y), Align2::LEFT_CENTER, format!("{:+.1} dB", clip.gain_db), regular(9.5), col);
    }
}

/// The fade curves, their midpoint handles and the fade-out of the clip this one is laid over.
fn draw_fades(cl: &ClipLane<'_>, clip: &Clip, g: &ClipGeom<'_>) {
    let ClipLane { painter, s, tl, .. } = *cl;
    let ClipGeom { body_r, vis, cx0, cx1, fade_in, fade_out, before, after, layered, .. } = *g;
    let fade_col = Color32::from_rgba_unmultiplied(255, 255, 255, 90);
    for (side, fade) in [(true, fade_in), (false, fade_out)] {
        if fade.len <= 0 {
            continue;
        }
        let (a, b) =
            if side { (cx0, x_of(s, tl, clip.start.saturating_add(fade.len))) } else { (x_of(s, tl, clip.end().saturating_sub(fade.len)), cx1) };
        let pts = fade_curve_points(fade, side, a, b, body_r, painter.ctx().pixels_per_point());
        // The fade that runs along a crossfade is drawn with the zone; only what lies past it is the clip's.
        let mut shown = vis;
        match (side, before, after) {
            (true, Some(prev), _) => shown.min.x = shown.min.x.max(x_of(s, tl, prev.end())),
            (false, _, Some(next)) => shown.max.x = shown.max.x.min(x_of(s, tl, next.start)),
            _ => {}
        }
        painter.with_clip_rect(shown).add(Shape::line(pts, Stroke::new(1.2, fade_col)));
        // The fades that run along a crossfade are the zone's: resized with its edges, not with a handle of their own.
        let hidden = if side { before.is_some() } else { after.is_some() };
        if !hidden {
            draw_fade_handle(cl, clip, g, (side, fade), (a, b));
        }
    }
    // The fade-out of the clip this one is laid over, drawn over the overlap.
    if let Some(c) = layered.filter(|c| c.fade_out.len > 0) {
        let (a, b) = (x_of(s, tl, c.end().saturating_sub(c.fade_out.len)), x_of(s, tl, c.end()));
        let pts = fade_curve_points(c.fade_out, false, a, b, body_r, painter.ctx().pixels_per_point());
        painter.with_clip_rect(vis).add(Shape::line(pts, Stroke::new(1.2, fade_col)));
    }
}

/// The midpoint handle of one fade (`side`: true for the fade-in), which runs from `a` to `b` on screen.
fn draw_fade_handle(cl: &ClipLane<'_>, clip: &Clip, g: &ClipGeom<'_>, fade: (bool, soundcraft_model::Fade), span: (f32, f32)) {
    let ClipLane { app, painter, s, track, .. } = *cl;
    let ClipGeom { body_r, vis, body, dim, .. } = *g;
    let ((side, fade), (a, b)) = (fade, span);
    let midpoint = fade_control_position(fade, a, b, body_r);
    let active = matches!(app.gesture, Some(Gesture::FadeCurve { clip: id, fade_in, .. }) if id == clip.id && fade_in == side);
    let hovered = painter.ctx().input(|i| i.pointer.hover_pos()).is_some_and(|p| near_fade_midpoint(p, midpoint));
    if !dim && s.edit.tool == Tool::Smart && AutoParam::parse(&track.view).is_none() && b - a >= 32.0 && vis.contains(midpoint) {
        let col = Color32::from_rgba_unmultiplied(255, 255, 255, if active || hovered { 230 } else { 200 });
        painter.with_clip_rect(vis).circle(midpoint, 3.5, body, Stroke::new(1.0, col));
    }
}

/// Optional clip overlays (View › Clip).
fn draw_clip_overlays(cl: &ClipLane<'_>, clip: &Clip, g: &ClipGeom<'_>) {
    let ClipLane { painter, s, track, sorted, tl, lane, .. } = *cl;
    let ClipGeom { r, cut, before, after, .. } = *g;
    // Optional clip overlays (View › Clip).
    let ov = Color32::from_rgb(16, 16, 16);
    if s.edit.flag("view.clip.sync_point") && clip.sync_point > 0 {
        let x = x_of(s, tl, clip.start + clip.sync_point);
        painter.add(Shape::convex_polygon(
            vec![pos2(x - 4.0, r.max.y - 1.0), pos2(x + 4.0, r.max.y - 1.0), pos2(x, r.max.y - 7.0)],
            Color32::WHITE,
            Stroke::NONE,
        ));
        painter.line_segment([pos2(x, r.min.y), pos2(x, r.max.y)], Stroke::new(1.0, Color32::from_rgba_unmultiplied(255, 255, 255, 90)));
    }
    let time_text = clip_time_text(s, clip);
    if let Some(tt) = time_text
        && r.height() > 30.0
    {
        painter.with_clip_rect(r.intersect(lane)).text(pos2(r.min.x.max(lane.min.x) + 4.0, r.max.y - 7.0), Align2::LEFT_CENTER, tt, regular(9.0), ov);
    }
    if s.edit.flag("view.clip.effects_status") && s.edit.values.keys().any(|k| k.starts_with(&format!("clip_fx.{}.", clip.id.0))) {
        painter.text(pos2(r.max.x - 6.0, r.max.y - 7.0), Align2::RIGHT_CENTER, "FX", bold(9.0), Color32::from_rgb(250, 230, 120));
    }
    if s.edit.flag("view.clip.overlap_shadows") {
        // Shade where a later clip covers this one (crossfades and layered edits). Only clips that
        // start inside this one can: on a sorted track, find them instead of checking every clip.
        let clips = track.clips();
        let later = if sorted {
            let from = clips.partition_point(|o| o.start <= clip.start);
            clips.get(from..clips.partition_point(|o| o.start < clip.end()).max(from)).unwrap_or_default()
        } else {
            clips
        };
        for o in later {
            if o.id != clip.id && o.start > clip.start && o.start < clip.end() && after.is_none_or(|b| b.id != o.id) {
                let x0 = x_of(s, tl, o.start);
                let x1 = x_of(s, tl, clip.end().min(o.end()));
                painter.rect_filled(Rect::from_min_max(pos2(x0, r.min.y), pos2(x1, r.max.y)), 0.0, Color32::from_black_alpha(70));
            }
        }
    }
    // Video clips get their pills from `video_track::draw_lane`, after their thumbnails.
    // The name of a clip of a crossfade keeps clear of the zone: the earlier clip's stops where the zone begins, the
    // later one's starts where it ends.
    if !clip.is_video() {
        let from = before.map_or(r.min.x, |a| x_of(s, tl, a.end()).max(r.min.x));
        let to = after.map_or(r.max.x, |b| x_of(s, tl, b.start).min(r.max.x));
        if to > from {
            draw_clip_labels_in(painter, s, lane, Rect::from_x_y_ranges(from..=to, r.y_range()), clip);
        }
    }
    if cut.left {
        // The cut's one line, on top of the waveform so it runs the full height.
        painter.rect_filled(Rect::from_min_size(r.min, vec2(1.0, r.height())), 0.0, CLIP_OUTLINE);
    }
}

/// The clip's start (or original) time label for the overlays, when one is shown.
fn clip_time_text(s: &Session, clip: &Clip) -> Option<String> {
    if s.edit.flag("view.clip.time.current") {
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
    }
}

/// The ink of a crossfade's zone: the theme's insertion colour, light on the dark clip bodies of the
/// dark palette and dark on the pale ones of the light palette.
fn zone_ink() -> Color32 {
    Tokens::current().insertion
}

/// A box over the zone of a crossfade, translucent so the clips and their waveforms show through. A
/// `ghost` is the outline of where a drag will leave it.
pub(super) fn zone_box(painter: &egui::Painter, zone: Rect, ghost: bool) {
    let ink = zone_ink();
    if zone.width() >= 3.0 {
        let (fill, line) = if ghost { (0.10, Stroke::new(2.0, ink)) } else { (0.16, Stroke::new(1.0, ink.gamma_multiply(0.7))) };
        painter.rect(zone, CornerRadius::ZERO, ink.gamma_multiply(fill), line, StrokeKind::Inside);
    }
}

/// The zone of the crossfade between `earlier` and `later` laid over their cut: a box across the whole
/// lane (the name band included), the outgoing fade of the earlier clip and the incoming fade of the
/// later one across all of it (the X). The incumbent marks no cut inside a crossfade, and neither do
/// we: it shows where the earlier clip's waveform gives way to the later one's. `body_y` is the height
/// the clips' fades span. Over another view of the track (`dim`, the automation view) the zone is only
/// the box, so the crossfade stays visible without competing with the curve being edited.
/// `cl` is the lane's: the zone spans the cut, past either clip's own painter.
fn draw_zone(cl: &ClipLane<'_>, earlier: &Clip, later: &Clip, body_y: egui::Rangef, dim: bool) {
    let ClipLane { app, painter, s, tl, lane, .. } = *cl;
    let (zs, ze) = (x_of(s, tl, later.start), x_of(s, tl, earlier.end()));
    let zone = Rect::from_x_y_ranges(zs..=ze, lane.y_range());
    zone_box(painter, zone, false);
    if !dim {
        let ppp = painter.ctx().pixels_per_point();
        let curves = painter.with_clip_rect(zone);
        let body = Rect::from_x_y_ranges(zs..=ze, body_y);
        let (_, fade_out) = clip_preview_fades(earlier, app.gesture.as_ref());
        let (fade_in, _) = clip_preview_fades(later, app.gesture.as_ref());
        for (fade, rising) in [(fade_out, false), (fade_in, true)] {
            if fade.len <= 0 {
                continue;
            }
            let (a, b) = if rising {
                (zs, x_of(s, tl, later.start.saturating_add(fade.len)))
            } else {
                (x_of(s, tl, earlier.end().saturating_sub(fade.len)), ze)
            };
            curves.add(Shape::line(fade_curve_points(fade, rising, a, b, body, ppp), Stroke::new(1.5, zone_ink())));
        }
    }
}

/// Where a clip sits in its lane: one point in from the lane's top and bottom, at least one point wide.
pub(super) fn clip_rect(s: &Session, tl: Rect, lane: Rect, clip: &Clip) -> Rect {
    let x0 = x_of(s, tl, clip.start);
    let x1 = x_of(s, tl, clip.end()).max(x0 + 1.0);
    Rect::from_min_max(pos2(x0, lane.min.y + 1.0), pos2(x1, lane.max.y - 1.0))
}

/// The pills of `clip` (name, marks). Drawn right after the clip itself, so clips and overlays drawn later cover them.
pub(crate) fn draw_clip_labels(painter: &egui::Painter, s: &Session, tl: Rect, lane: Rect, clip: &Clip) {
    draw_clip_labels_in(painter, s, lane, clip_rect(s, tl, lane, clip), clip);
}

/// [`draw_clip_labels`] in `r`, the part of the clip the pills may use.
fn draw_clip_labels_in(painter: &egui::Painter, s: &Session, lane: Rect, r: Rect, clip: &Clip) {
    if r.max.x < lane.min.x || r.min.x > lane.max.x {
        return;
    }
    let show_name = clip_name_shown(s, lane);
    let marks = clip_marks(clip, s.edit.flag("view.clip.rating"));
    draw_clip_pills(painter, lane, r, show_name.then_some(clip.name.as_str()), &marks);
}

/// The marks a clip carries at its top-right: its star rating and a lock when edit- or time-locked.
pub(super) fn clip_marks(clip: &Clip, show_rating: bool) -> String {
    let mut marks = if show_rating && clip.rating > 0 { "★".repeat(usize::from(clip.rating.min(5))) } else { String::new() };
    if clip.edit_locked || clip.time_locked {
        if !marks.is_empty() {
            marks.push(' ');
        }
        marks.push('🔒');
    }
    marks
}

/// Gap between a pill and the clip outline's inner edge (so the pill sits 1 pt inside the outline).
pub(super) const PILL_INSET: f32 = 2.0;
pub(super) const PILL_HEIGHT: f32 = 13.0;
/// Horizontal padding between a pill's edge and its text.
pub(super) const PILL_PAD: f32 = 3.0;
/// The narrowest name pill worth drawing: its padding plus about one letter.
pub(super) const PILL_MIN_WIDTH: f32 = 12.0;
/// Half-transparent black behind white text, readable on the light and the dark clip bodies alike.
pub(super) const PILL_BG: Color32 = Color32::from_black_alpha(125);

/// Width of the name pill: the text plus its padding, capped to the `room` the clip leaves it (the text is then
/// cut cleanly at the pill's edge, with no ellipsis). `None` when there is no room for anything useful.
pub(super) fn name_pill_width(room: f32, text_w: f32) -> Option<f32> {
    if room.is_nan() || room < PILL_MIN_WIDTH {
        return None;
    }
    Some((text_w.max(0.0) + 2.0 * PILL_PAD).min(room))
}

/// Left edge of the marks pill, which sits at the clip's top-right: `None` when the clip is too narrow to hold it
/// (it would veil the clip), where `left` is the leftmost x a pill may use.
pub(super) fn marks_pill_left(left: f32, clip_right: f32, w: f32) -> Option<f32> {
    let x = clip_right - PILL_INSET - w;
    (x >= left).then_some(x)
}

/// The clip's name as a small rounded pill at its top-left, and its marks (rating, lock) as a matching pill at the
/// top-right. The name keeps clear of the marks and floats to the left edge of the lane when the clip starts
/// off-screen, so it stays readable while scrolling.
fn draw_clip_pills(painter: &egui::Painter, lane: Rect, r: Rect, name: Option<&str>, marks: &str) {
    let p = painter.with_clip_rect(painter.clip_rect().intersect(r));
    let pill = |rect: Rect, galley: Arc<egui::Galley>| {
        p.rect_filled(rect, CornerRadius::same(3), PILL_BG);
        let at = pos2(rect.min.x + PILL_PAD, rect.center().y - galley.size().y * 0.5);
        p.with_clip_rect(p.clip_rect().intersect(rect)).galley(at, galley, Color32::WHITE);
    };
    let top = r.min.y + PILL_INSET;
    let left = r.min.x.max(lane.min.x) + PILL_INSET;
    let mut right = r.max.x.min(lane.max.x) - PILL_INSET;
    if !marks.is_empty() {
        let galley = p.layout_no_wrap(marks.to_owned(), regular(9.0), Color32::WHITE);
        let w = galley.size().x + 2.0 * PILL_PAD;
        if let Some(x) = marks_pill_left(left, r.max.x, w) {
            right = right.min(x - PILL_INSET);
            pill(Rect::from_min_size(pos2(x, top), vec2(w, PILL_HEIGHT)), galley);
        }
    }
    // No layout where no pill fits: zoomed out on a large session, most clips are too narrow.
    if let Some(name) = name.filter(|n| !n.is_empty() && right - left >= PILL_MIN_WIDTH) {
        let galley = p.layout_no_wrap(name.to_owned(), bold(10.0), Color32::WHITE);
        if let Some(w) = name_pill_width(right - left, galley.size().x) {
            pill(Rect::from_min_size(pos2(left, top), vec2(w, PILL_HEIGHT)), galley);
        }
    }
}

/// The 1 pt outline around a clip.
const CLIP_OUTLINE: Color32 = Color32::from_rgb(10, 10, 10);

/// How a clip meets its neighbours in the playlist. Two clips butted at a cut share ONE 1 pt line, as in
/// the incumbent: the later clip draws it (its left edge), the earlier one drops its right edge, and both
/// sides of the cut have square corners. A gap or an overlap leaves the clip drawn on its own.
/// Selection only recolours the body, so it never changes which edges are drawn.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct ClipCut {
    /// Starts exactly where the clip before it ends.
    pub(super) left: bool,
    /// Ends exactly where the clip after it starts.
    pub(super) right: bool,
}

impl ClipCut {
    /// The body's corners: square on a side that meets another clip, rounded otherwise.
    pub(super) fn corners(self) -> CornerRadius {
        let (nw, ne) = (if self.left { 0 } else { 2 }, if self.right { 0 } else { 2 });
        CornerRadius { nw, ne, sw: nw, se: ne }
    }
}

/// The cut of the clip spanning `start..end`, given the `(start, end)` of every other clip of its playlist, in any
/// order (a playlist's order is its stacking order, not its time order). A side is butted when another clip ends
/// (or starts) exactly there and no third clip covers that point.
pub(super) fn clip_cut(start: Samples, end: Samples, others: impl IntoIterator<Item = (Samples, Samples)>) -> ClipCut {
    // An empty clip has no span to share a line with.
    if end <= start {
        return ClipCut::default();
    }
    let (mut left, mut right, mut left_covered, mut right_covered) = (false, false, false, false);
    for (s, e) in others.into_iter().filter(|(s, e)| e > s) {
        left |= e == start;
        right |= s == end;
        left_covered |= s < start && start < e;
        right_covered |= s < end && end < e;
    }
    ClipCut { left: left && !left_covered, right: right && !right_covered }
}

/// [`clip_cut`] for `clip` among its playlist's `clips`, scanning them all.
pub(super) fn clip_cut_in(clips: &[Clip], clip: &Clip) -> ClipCut {
    clip_cut(clip.start, clip.end(), clips.iter().filter(|c| c.id != clip.id).map(|c| (c.start, c.end())))
}

/// [`clip_cut`] for each clip of a playlist. Sorted by start (the usual case), one pass over the playlist and two
/// binary searches per clip, so large sessions do no per-clip scan every frame; unsorted, a scan per clip.
pub(super) fn clip_cuts(clips: &[Clip]) -> impl Fn(&Clip) -> ClipCut + '_ {
    // reach[k]: the furthest end of the first k clips (empty clips reach nothing).
    let reach: Option<Vec<Samples>> = clips.is_sorted_by_key(|c| c.start).then(|| {
        std::iter::once(Samples::MIN)
            .chain(clips.iter().scan(Samples::MIN, |far, c| {
                if c.end() > c.start {
                    *far = (*far).max(c.end());
                }
                Some(*far)
            }))
            .collect()
    });
    move |clip| {
        let Some(reach) = &reach else { return clip_cut_in(clips, clip) };
        let (start, end) = (clip.start, clip.end());
        if end <= start {
            return ClipCut::default();
        }
        let reached = |k: usize| reach.get(k).copied().unwrap_or(Samples::MIN);
        // Butted and uncovered on the left: the clips starting before it reach exactly its start. On the right: the
        // clips starting before its end (itself included) reach exactly its end, and a clip starts there.
        let before = clips.partition_point(|c| c.start < start);
        let upto = clips.partition_point(|c| c.start < end);
        let next = clips.get(upto..).unwrap_or_default().iter().take_while(|c| c.start == end).any(|c| c.end() > c.start);
        ClipCut { left: reached(before) == start, right: next && reached(upto) == end }
    }
}

/// The audio crossfade whose zone holds `at`: its earlier and its later clip. Clips that merely overlap
/// (one laid over another, MIDI) are not a crossfade and are grabbed as clips.
pub(super) fn zone_at(clips: &[Clip], at: Samples) -> Option<(&Clip, &Clip)> {
    let top = clips.iter().rev().find(|c| c.range().contains(at))?;
    let earlier = ops::crossfade_earlier(clips, top).filter(|a| a.end() > at)?;
    Some((earlier, top))
}

/// The part of a clip that shows: the two clips of a crossfade are butted at the cut, so the earlier one
/// shows up to it and the later one from it. What lies under the zone belongs to the zone.
pub(super) fn shown_span(clip: &Clip, before: Option<&Clip>, after: Option<&Clip>) -> (Samples, Samples) {
    let span = ops::visible_span(clip, before, after);
    (span.start, span.end)
}

/// A crossfade as it is on screen, in points: its two clips and where its zone and its cut are.
struct ScreenZone<'a> {
    earlier: &'a Clip,
    later: &'a Clip,
    left: f32,
    right: f32,
    cut: f32,
}

impl ScreenZone<'_> {
    /// Narrower than the bands of its two edges and the band around its cut put together: the edges would
    /// leave the Grabber less than that whole band to slide it by.
    fn narrow(&self) -> bool {
        self.right - self.left < 2.0 * EDGE_GRAB + 2.0 * ZONE_CENTRE
    }

    /// What the Grabber takes, by pixels rather than samples (a 10 ms crossfade is a few points wide): the
    /// zone, and for a narrow one also the points within [`ZONE_CENTRE`] of its cut, a band the edges leave to it.
    fn grabs(&self, x: f32) -> bool {
        let (lo, hi) =
            if self.narrow() { (self.left.min(self.cut - ZONE_CENTRE), self.right.max(self.cut + ZONE_CENTRE)) } else { (self.left, self.right) };
        lo <= x && x <= hi
    }
}

/// The crossfades of `clips` whose zone is under `x` or within grabbing distance of it.
fn zones_near<'a>(s: &Session, tl: Rect, clips: &'a [Clip], x: f32) -> Vec<ScreenZone<'a>> {
    clips
        .iter()
        .filter(|b| x_of(s, tl, b.start) - EDGE_GRAB <= x && x <= x_of(s, tl, b.end()) + EDGE_GRAB)
        .filter_map(|later| {
            let earlier = ops::crossfade_earlier(clips, later)?;
            let (left, right) = (x_of(s, tl, later.start), x_of(s, tl, earlier.end()));
            Some(ScreenZone { earlier, later, left, right, cut: x_of(s, tl, crossfade_cut(earlier, later)) })
        })
        .filter(|z| z.left - EDGE_GRAB <= x && x <= z.right + EDGE_GRAB)
        .collect()
}

/// The audio crossfade the Grabber takes at `x`, by pixels (see [`ScreenZone::grabs`]): its earlier and its later
/// clip. Not one that can't slide (a clip edit-locked or stretched: the press is then the clips', as for any clip),
/// nor one with another clip laid over it at that point, which is grabbed as the clip it is.
pub(super) fn zone_under<'a>(s: &Session, tl: Rect, clips: &'a [Clip], x: f32) -> Option<(&'a Clip, &'a Clip)> {
    let top = clips.iter().rev().find(|c| c.range().contains(sample_at(s, tl, x).max(0)))?;
    zones_near(s, tl, clips, x)
        .into_iter()
        .filter(|z| z.grabs(x))
        .filter(|z| z.earlier.id == top.id || z.later.id == top.id)
        .filter(|z| ops::crossfade_slide(s, z.later.id, 0).is_some())
        .min_by(|p, q| (x - p.cut).abs().total_cmp(&(x - q.cut).abs()))
        .map(|z| (z.earlier, z.later))
}

/// An edge of a crossfade's zone that the Trim tool pulls.
pub(super) struct ZoneEdge {
    /// The later clip of the crossfade, the one its commands name.
    pub(super) later: ClipId,
    /// The left edge (the later clip's start), or the right one (the earlier clip's end).
    pub(super) start: bool,
    /// Where the edge is now.
    pub(super) at: Samples,
    /// The pointer is within grabbing distance of the edge, rather than somewhere in the zone.
    pub(super) near: bool,
    /// The pointer is in the band around the cut that a narrow zone keeps for sliding it.
    pub(super) centre: bool,
}

/// The zone edge nearest to the pointer at `x`, when it is in a zone or within grabbing distance of one.
pub(super) fn zone_edge_at(s: &Session, tl: Rect, clips: &[Clip], x: f32) -> Option<ZoneEdge> {
    zones_near(s, tl, clips, x)
        .into_iter()
        .flat_map(|z| {
            let centre = z.narrow() && (x - z.cut).abs() < ZONE_CENTRE;
            [(z.later.id, true, z.later.start, z.left, centre), (z.later.id, false, z.earlier.end(), z.right, centre)]
        })
        .map(|(later, start, at, edge_x, centre)| (ZoneEdge { later, start, at, near: false, centre }, (edge_x - x).abs()))
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(edge, dist)| ZoneEdge { near: dist < EDGE_GRAB && !edge.centre, ..edge })
}

/// How far from the cut of a narrow zone, in points, the Grabber still takes it.
const ZONE_CENTRE: f32 = 3.0;

/// How close the pointer has to be to a clip edge or a zone edge to trim it, in points.
pub(super) const EDGE_GRAB: f32 = 6.0;

/// The clip before `clip` in its playlist that overlaps its start, whatever the overlap is: a crossfade,
/// or a clip laid over another. The crossfade pairs are told apart with [`ops::crossfade_earlier`].
fn crossfade_partner<'a>(clips: &'a [Clip], clip: &Clip) -> Option<&'a Clip> {
    let i = clips.partition_point(|c| c.start < clip.start);
    clips.get(i).is_some_and(|c| c.id == clip.id).then_some(())?;
    clips.get(i.checked_sub(1)?).filter(|c| c.end() > clip.start)
}
