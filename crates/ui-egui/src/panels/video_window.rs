//! Window › Video (the picture at the playhead, sized to the movie's aspect, optional timecode
//! burn-in) and Window › Video Universe (the whole Video track as a strip; click to locate).

use crate::SoundApp;
use crate::theme::{mono, regular};
use crate::video_track::{MovieKey, movie_secs, online, picture_at};
use egui::{Align2, Color32, Rect, Sense, pos2, vec2};
use serde_json::json;
use soundcraft_model::{ClipContent, TrackKind};
use soundcraft_time::TimeFormat;

pub fn show(app: &mut SoundApp, ctx: &egui::Context) {
    if app.ui.show_video {
        video(app, ctx);
    }
    if app.ui.show_video_universe {
        universe(app, ctx);
    }
}

fn video_status_text(app: &SoundApp, pic: Option<&(MovieKey, Result<String, String>, f64, f64)>) -> String {
    match pic {
        Some((_, Err(why), ..)) => why.clone(),
        Some((k, ..)) => match app.video.info(*k) {
            Some(Ok(i)) => format!("{}x{}  {:.3} fps  {}", i.width, i.height, i.frame_rate, i.codec_detail),
            Some(Err(e)) => e.clone(),
            None => "opening…".into(),
        },
        None => String::new(),
    }
}

fn draw_video_surface(app: &SoundApp, painter: &egui::Painter, img: Rect, pic: Option<&(MovieKey, Result<String, String>, f64, f64)>, on: bool) {
    let msg = if !on {
        Some("Video Track Offline".to_string())
    } else {
        match pic {
            None => None,
            Some((_, Err(why), ..)) => Some(why.clone()),
            Some((k, ..)) => match (app.video.current(*k), app.video.current_error(*k)) {
                (Some(tex), _) => {
                    painter.image(tex.id(), img, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
                    None
                }
                (None, Some(e)) => Some(e.to_string()),
                (None, None) => match app.video.info(*k) {
                    Some(Err(e)) => Some(e.clone()),
                    _ => None,
                },
            },
        }
    };
    if let Some(m) = msg {
        painter.text(img.center(), Align2::CENTER_CENTER, m, regular(12.0), Color32::from_gray(170));
    }
}

fn video(app: &mut SoundApp, ctx: &egui::Context) {
    app.video.poll(ctx);
    let pos = app.position();
    let s = app.engine.session_arc();
    let pic = picture_at(&s, pos);
    let on = online(&s);
    if on && let Some((key, Ok(path), secs, fps)) = &pic {
        app.video.request_frame(ctx, *key, path, *secs, *fps);
    }
    let aspect = pic
        .as_ref()
        .and_then(|(k, ..)| app.video.info(*k).and_then(|i| i.as_ref().ok()).map(|i| i.display_aspect() as f32))
        .or_else(|| first_movie_aspect(&s))
        .unwrap_or(16.0 / 9.0)
        .clamp(0.2, 5.0);
    let mut open = true;
    let mut burn = app.ui.video_burn_in;
    egui::Window::new("Video").open(&mut open).default_size(vec2(480.0, 480.0 / aspect + 28.0)).resizable(true).show(ctx, |ui| {
        ui.horizontal(|ui| {
            crate::widgets::checkbox(ui, &mut burn, "Timecode burn-in");
            let text = video_status_text(app, pic.as_ref());
            ui.label(egui::RichText::new(text).font(regular(10.5)).color(Color32::from_gray(150)));
        });
        let avail = ui.available_rect_before_wrap();
        let (r, _) = ui.allocate_exact_size(avail.size().max(vec2(64.0, 36.0)), Sense::hover());
        let painter = ui.painter_at(r);
        painter.rect_filled(r, 0.0, Color32::BLACK);
        // Fit the picture inside the area, keeping its aspect.
        let (mut w, mut h) = (r.width(), r.width() / aspect);
        if h > r.height() {
            h = r.height();
            w = h * aspect;
        }
        let img = Rect::from_center_size(r.center(), vec2(w, h));
        draw_video_surface(app, &painter, img, pic.as_ref(), on);
        if burn {
            let tc = s.format_position(pos, TimeFormat::Timecode);
            let size = (h / 12.0).clamp(10.0, 36.0);
            let galley = painter.layout_no_wrap(tc, mono(size), Color32::WHITE);
            let at = pos2(img.center().x - galley.size().x / 2.0, img.max.y - galley.size().y - size * 0.6);
            painter.rect_filled(Rect::from_min_size(at, galley.size()).expand2(vec2(6.0, 2.0)), 2.0, Color32::from_black_alpha(200));
            painter.galley(at, galley, Color32::WHITE);
        }
    });
    app.ui.video_burn_in = burn;
    if !open {
        app.ui.show_video = false;
    }
}

/// Timeline position of strip coordinate `x` (the strip spans `left`..`left + width` and samples
/// 0..`end`), clamped to the start of the clip being drawn.
fn strip_position(x: f32, left: f32, width: f32, end: i64, clip_start: i64) -> i64 {
    (((x - left) / width * end as f32) as i64).max(clip_start)
}

fn first_movie_aspect(s: &soundcraft_model::Session) -> Option<f32> {
    s.videos.first().map(|v| v.aspect() as f32)
}

fn universe(app: &mut SoundApp, ctx: &egui::Context) {
    app.video.poll(ctx);
    let s = app.engine.session_arc();
    let pos = app.position();
    let mut open = true;
    let mut locate = None;
    egui::Window::new("Video Universe").open(&mut open).default_size(vec2(640.0, 90.0)).resizable(true).show(ctx, |ui| {
        let avail = ui.available_rect_before_wrap();
        let (r, resp) = ui.allocate_exact_size(vec2(avail.width().max(120.0), 64.0), Sense::click_and_drag());
        let painter = ui.painter_at(r);
        painter.rect_filled(r, 0.0, Color32::from_rgb(14, 14, 15));
        let end = s.content_end().max(s.sample_rate.samples(10.0)).max(1);
        let x_at = |at: i64| r.min.x + r.width() * (at as f32 / end as f32);
        let on = online(&s);
        let Some(track) = s.tracks.iter().find(|t| t.kind == TrackKind::Video) else {
            painter.text(r.center(), Align2::CENTER_CENTER, "No Video track — File › Import › Video…", regular(11.0), Color32::from_gray(150));
            return;
        };
        let ppp = ctx.pixels_per_point();
        for c in track.clips() {
            let ClipContent::Video { source, offset } = c.content else { continue };
            let Some(v) = s.video(source) else { continue };
            let key = MovieKey::new(source, &v.path);
            let cr = Rect::from_min_max(pos2(x_at(c.start), r.min.y + 4.0), pos2(x_at(c.end()).max(x_at(c.start) + 2.0), r.max.y - 4.0));
            painter.rect_filled(cr, 0.0, Color32::from_rgb(30, 30, 34));
            if !on || v.offline.is_some() {
                continue;
            }
            let tw = (cr.height() * v.aspect() as f32).max(8.0);
            let hpx = ((cr.height() * ppp / 16.0).ceil() * 16.0).clamp(16.0, 256.0) as u32;
            let mut x = cr.min.x;
            while x < cr.max.x {
                let secs = movie_secs(&s, c.start, offset, strip_position(x, r.min.x, r.width(), end, c.start)).max(0.0);
                if let Some(tex) = app.video.thumb(ctx, key, &v.path, secs, hpx) {
                    let tr = Rect::from_min_size(pos2(x, cr.min.y), vec2(tw, cr.height()));
                    let vis = tr.intersect(cr);
                    let uv = Rect::from_min_max(pos2(0.0, 0.0), pos2((vis.width() / tw).clamp(0.0, 1.0), 1.0));
                    painter.image(tex.id(), vis, uv, Color32::WHITE);
                }
                x += tw;
            }
        }
        // The Edit window's visible range and the playhead.
        let spp = s.edit.zoom.samples_per_px.max(1e-6);
        let vs = s.edit.zoom.scroll.max(0);
        let ve = vs.saturating_add((spp * f64::from(ui.ctx().content_rect().width())) as i64);
        let view = Rect::from_min_max(pos2(x_at(vs), r.min.y + 1.0), pos2(x_at(ve).min(r.max.x), r.max.y - 1.0));
        painter.rect_stroke(view, 0.0, egui::Stroke::new(1.0, Color32::from_rgb(230, 200, 80)), egui::StrokeKind::Inside);
        let px = x_at(pos);
        painter.line_segment([pos2(px, r.min.y), pos2(px, r.max.y)], egui::Stroke::new(1.5, Color32::from_rgb(240, 240, 240)));
        if (resp.clicked() || resp.dragged())
            && let Some(p) = resp.interact_pointer_pos()
        {
            locate = Some((((p.x - r.min.x) / r.width()).clamp(0.0, 1.0) * end as f32) as i64);
        }
    });
    if let Some(at) = locate {
        let _ = app.run("transport.locate", json!({"at": at}));
        let _ = app.run("edit.select", json!({"start": at, "end": at}));
    }
    if !open {
        app.ui.show_video_universe = false;
    }
}

#[cfg(test)]
mod tests {
    use super::strip_position;

    // A 1024 px strip over 1_024_000 samples: 1000 samples per point, exact in f32.
    const LEFT: f32 = 100.0;
    const WIDTH: f32 = 1024.0;
    const END: i64 = 1_024_000;

    #[test]
    fn strip_position_is_the_timeline_position_for_clips_after_zero() {
        // A clip at 48_000 has its left edge at x = 148.
        assert_eq!(strip_position(148.0, LEFT, WIDTH, END, 48_000), 48_000);
        assert_eq!(strip_position(248.0, LEFT, WIDTH, END, 48_000), 148_000);
        let s = soundcraft_model::Session::default();
        let at = strip_position(148.0, LEFT, WIDTH, END, 48_000);
        assert_eq!(crate::video_track::movie_secs(&s, 48_000, 0, at), 0.0, "the clip's left edge shows the movie's first picture");
    }

    #[test]
    fn strip_position_is_unchanged_for_clips_at_or_before_zero() {
        assert_eq!(strip_position(100.0, LEFT, WIDTH, END, 0), 0);
        assert_eq!(strip_position(200.0, LEFT, WIDTH, END, 0), 100_000);
        // A clip at -48_000 starts left of the strip; positions before it clamp to its start.
        assert_eq!(strip_position(0.0, LEFT, WIDTH, END, -48_000), -48_000);
        assert_eq!(strip_position(52.0, LEFT, WIDTH, END, -48_000), -48_000);
        assert_eq!(strip_position(200.0, LEFT, WIDTH, END, -48_000), 100_000);
    }
}
