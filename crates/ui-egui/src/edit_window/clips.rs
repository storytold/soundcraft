use super::drag::fade_drag_preview;
use super::input::{clip_gain_icon, near_fade_midpoint};
use super::layout::{sample_at, x_of};
use super::{
    Align2, AutoParam, Clip, ClipContent, Color32, CornerRadius, Gesture, GridValue, NoteValue, Pos2, Range, Rect, Session, Shape, SoundApp, Stroke,
    StrokeKind, TimeFormat, Tokens, Tool, Track, bold, clip_colors, format_position, pos2, regular, vec2,
};

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

pub(super) fn grid_lines(painter: &egui::Painter, s: &Session, tl: Rect, lane: Rect) {
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

pub(super) fn clip_name_height(s: &Session, lane: Rect) -> f32 {
    if lane.height() >= 40.0 && s.edit.flag("view.clip.name") { 13.0 } else { 0.0 }
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
    x0: f32,
    x1: f32,
    name_h: f32,
    body: Color32,
    wave: Color32,
    selected: bool,
    dim: bool,
    fade_in: soundcraft_model::Fade,
    fade_out: soundcraft_model::Fade,
    prev: Option<&'a Clip>,
}

pub(super) fn draw_clip(cl: &ClipLane<'_>, clip: &Clip, dim: bool) {
    let ClipLane { app, painter, s, track, tl, lane, .. } = *cl;
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
        // Narrower than this, no glyph would show: the name starts 4 points in.
        if name_clip.width() > 6.0 {
            painter.with_clip_rect(name_clip).text(
                pos2(nb.min.x.max(lane.min.x) + 4.0, nb.center().y),
                Align2::LEFT_CENTER,
                &clip.name,
                bold(10.5),
                Color32::from_rgb(12, 12, 12),
            );
        }
    }
    let body_r = Rect::from_min_max(pos2(r.min.x, r.min.y + name_h), r.max);
    let vis = body_r.intersect(lane);
    // The waveform and fade curve share the same temporary envelope every frame.
    let (fade_in, fade_out) = clip_preview_fades(clip, app.gesture.as_ref().filter(|_| !dim));
    let prev = crossfade_partner(track, clip);
    let g = ClipGeom { r, body_r, vis, x0, x1, name_h, body, wave, selected, dim, fade_in, fade_out, prev };
    draw_clip_content(cl, clip, &g);
    draw_crossfade_tail(cl, &g);
    draw_gain_line(cl, clip, &g);
    draw_fades(cl, clip, &g);
    draw_clip_overlays(cl, clip, &g);
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

/// The waveform of the clip this one crossfades with, drawn over the overlap.
fn draw_crossfade_tail(cl: &ClipLane<'_>, g: &ClipGeom<'_>) {
    let ClipLane { painter, s, track, tl, .. } = *cl;
    let ClipGeom { body_r, vis, x0, x1, dim, prev, .. } = *g;
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
        crate::waveform::draw(crate::waveform::Canvas { painter: &painter.with_clip_rect(xf), s, tl }, prev, audio, track.channels(), body_r, tail);
    }
}

/// The clip gain line and its icon.
fn draw_gain_line(cl: &ClipLane<'_>, clip: &Clip, g: &ClipGeom<'_>) {
    let ClipLane { painter, s, track, tl, lane, .. } = *cl;
    let ClipGeom { body_r, x0, x1, dim, .. } = *g;
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
}

/// The fade curves, their midpoint handles and the crossfade partner's fade-out.
fn draw_fades(cl: &ClipLane<'_>, clip: &Clip, g: &ClipGeom<'_>) {
    let ClipLane { app, painter, s, track, tl, .. } = *cl;
    let ClipGeom { body_r, vis, x0, x1, body, dim, fade_in, fade_out, prev, .. } = *g;
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
}

/// Optional clip overlays (View › Clip).
fn draw_clip_overlays(cl: &ClipLane<'_>, clip: &Clip, g: &ClipGeom<'_>) {
    let ClipLane { painter, s, track, sorted, tl, lane, .. } = *cl;
    let ClipGeom { r, name_h, .. } = *g;
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

/// The earlier clip of a crossfade pair: the one before `clip` in its playlist, overlapping its start.
fn crossfade_partner<'a>(track: &'a Track, clip: &Clip) -> Option<&'a Clip> {
    let (p, i) = track.playlists.iter().find_map(|p| {
        let i = p.clips.partition_point(|c| c.start < clip.start);
        p.clips.get(i).is_some_and(|c| c.id == clip.id).then_some((p, i))
    })?;
    p.clips.get(i.checked_sub(1)?).filter(|c| c.end() > clip.start)
}
