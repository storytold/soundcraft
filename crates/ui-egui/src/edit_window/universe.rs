use super::{Color32, Rect, Samples, Sense, Session, SoundApp, Stroke, StrokeKind, Tokens, Track, Ui, json, pos2, rgb, vec2};

/// The Universe's clip blocks for one document revision and area: each track's clips merged into
/// runs of one colour. A drum edit holds tens of thousands of clips a pixel or less wide here;
/// painting one rect per clip every frame would cost more than all the rest of the window.
#[derive(Clone)]
struct UniverseRuns {
    revision: u64,
    area: Rect,
    runs: std::sync::Arc<Vec<(Rect, Color32)>>,
}

/// Each clip covers `[x0, max(x1, x0 + 1)]`; neighbours of one colour that touch or overlap are
/// painted as one rect, which covers exactly the same pixels in the same order.
fn universe_runs(s: &Session, r: Rect) -> Vec<(Rect, Color32)> {
    let total = (s.content_end() + s.sample_rate.samples(10.0)).max(1) as f32;
    let tracks: Vec<&Track> = s.tracks.iter().filter(|t| !t.hidden).collect();
    let n = tracks.len().max(1) as f32;
    let rh = (r.height() / n).max(1.0);
    let mut runs = Vec::new();
    for (i, t) in tracks.iter().enumerate() {
        let y = r.min.y + i as f32 * rh;
        let block = |x0: f32, x1: f32, color| (Rect::from_min_max(pos2(x0, y + 0.5), pos2(x1, y + rh - 0.5)), color);
        let mut run: Option<(f32, f32, Color32)> = None;
        for c in t.clips() {
            let x0 = r.min.x + c.start as f32 / total * r.width();
            let x1 = (r.min.x + c.end() as f32 / total * r.width()).max(x0 + 1.0);
            let color = rgb(c.color.unwrap_or(t.color));
            run = match run {
                Some((a, b, col)) if col == color && x0 >= a && x0 <= b => Some((a, b.max(x1), col)),
                Some((a, b, col)) => {
                    runs.push(block(a, b, col));
                    Some((x0, x1, color))
                }
                None => Some((x0, x1, color)),
            };
        }
        if let Some((a, b, col)) = run {
            runs.push(block(a, b, col));
        }
    }
    runs
}

/// [`Session::content_end`], worked out once per document revision: it walks every clip.
pub(super) fn content_end(app: &SoundApp, ctx: &egui::Context) -> Samples {
    let key = egui::Id::unique("content_end");
    let revision = app.engine.revision;
    if let Some((at, end)) = ctx.memory(|m| m.data.get_temp::<(u64, Samples)>(key))
        && at == revision
    {
        return end;
    }
    let end = app.engine.session().content_end();
    ctx.memory_mut(|m| m.data.insert_temp(key, (revision, end)));
    end
}

/// Universe: the whole session in miniature; the box is the visible area (drag or click to move).
pub(super) fn universe(app: &mut SoundApp, ui: &mut Ui) {
    let r = ui.max_rect().shrink2(vec2(8.0, 4.0));
    let key = egui::Id::unique("universe_runs");
    let revision = app.engine.revision;
    let runs = match ui.ctx().memory(|m| m.data.get_temp::<UniverseRuns>(key)).filter(|u| u.revision == revision && u.area == r) {
        Some(u) => u.runs,
        None => {
            let runs = std::sync::Arc::new(universe_runs(app.engine.session(), r));
            ui.ctx().memory_mut(|m| m.data.insert_temp(key, UniverseRuns { revision, area: r, runs: runs.clone() }));
            runs
        }
    };
    for &(rect, color) in runs.iter() {
        ui.painter().rect_filled(rect, 0.0, color);
    }
    let end = content_end(app, ui.ctx());
    let s = app.engine.session();
    let total = (end + s.sample_rate.samples(10.0)).max(1) as f32;
    let vis0 = s.edit.zoom.scroll as f32 / total;
    let tlw = app.edit_layout.timeline[2] - app.edit_layout.timeline[0];
    let vis1 = vis0 + (f64::from(tlw.max(100.0)) * s.edit.zoom.samples_per_px) as f32 / total;
    let vr = Rect::from_min_max(pos2(r.min.x + vis0 * r.width(), r.min.y - 2.0), pos2(r.min.x + vis1.min(1.0) * r.width(), r.max.y + 2.0));
    ui.painter().rect_stroke(vr, 2.0, Stroke::new(1.5, Tokens::current().universe_view), StrokeKind::Outside);
    if app.is_playing() {
        let x = r.min.x + app.position() as f32 / total * r.width();
        ui.painter().line_segment([pos2(x, r.min.y), pos2(x, r.max.y)], Stroke::new(1.0, Tokens::current().playhead));
    }
    let resp = ui.interact(r, ui.scope_id().with("universe"), Sense::click_and_drag());
    if let Some(p) = resp.interact_pointer_pos()
        && (resp.clicked() || resp.dragged())
    {
        let half = (vis1 - vis0) * 0.5;
        let k = ((p.x - r.min.x) / r.width() - half).clamp(0.0, 1.0);
        let _ = app.engine.execute("view.scroll", &json!({"to": (k * total) as i64}));
    }
}

#[cfg(test)]
mod universe_tests {
    use super::*;
    use soundcraft_model::{ChannelFormat, Clip, ClipId, SourceId, TrackKind};

    /// The pixel columns each block covers, per colour, as the old one-rect-per-clip painting did.
    fn covered(blocks: &[(Rect, Color32)]) -> Vec<(i32, Color32)> {
        let mut px: Vec<(i32, Color32)> = Vec::new();
        for (r, c) in blocks {
            for x in (r.min.x * 4.0).round() as i32..(r.max.x * 4.0).round() as i32 {
                px.retain(|p| p.0 != x);
                px.push((x, *c));
            }
        }
        px.sort_by_key(|p| p.0);
        px
    }

    fn per_clip(s: &Session, r: Rect) -> Vec<(Rect, Color32)> {
        let total = (s.content_end() + s.sample_rate.samples(10.0)).max(1) as f32;
        let t = &s.tracks[0];
        t.clips()
            .iter()
            .map(|c| {
                let x0 = r.min.x + c.start as f32 / total * r.width();
                let x1 = (r.min.x + c.end() as f32 / total * r.width()).max(x0 + 1.0);
                (Rect::from_min_max(pos2(x0, r.min.y + 0.5), pos2(x1, r.max.y - 0.5)), rgb(c.color.unwrap_or(t.color)))
            })
            .collect()
    }

    #[test]
    fn a_sliced_track_is_a_few_runs_covering_the_same_pixels() {
        let mut s = Session::default();
        let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, None);
        let mut clips = Vec::new();
        let mut at = 0;
        for i in 0..5000u64 {
            let mut c = Clip::audio(ClipId(i + 1), "hit", SourceId(1), 0, at, 200 + (i % 7) as i64 * 50);
            // A recoloured stretch, and a gap, in the middle.
            if (2000..2100).contains(&i) {
                c.color = Some([200, 40, 40]);
            }
            at = c.end() + if i == 3000 { 2_000_000 } else { 0 };
            clips.push(c);
        }
        s.track_mut(t).unwrap().playlist_mut().unwrap().clips = clips;
        let r = Rect::from_min_size(pos2(8.0, 4.0), vec2(900.0, 56.0));
        let runs = universe_runs(&s, r);
        assert!(runs.len() <= 6, "{} rects for 5000 clips", runs.len());
        assert_eq!(covered(&runs), covered(&per_clip(&s, r)));
    }
}
