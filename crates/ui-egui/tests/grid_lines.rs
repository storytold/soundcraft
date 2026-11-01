use egui::{Color32, FullOutput, Pos2, RawInput, Rect, Shape, pos2, vec2};
use serde_json::json;
use soundcraft_ui_egui::theme::Tokens;
use soundcraft_ui_egui::{Services, SoundApp, edit_window};

fn frame(ctx: &egui::Context, app: &mut SoundApp) -> FullOutput {
    let raw = RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1600.0, 1000.0))), ..Default::default() };
    let mut out = ctx.run_ui(raw, |ui| {
        app.logic(ui.ctx());
        app.ui(ui);
    });
    out.textures_delta.clear();
    out
}

fn over(top: Color32, bg: Color32) -> [i32; 3] {
    let keep = 255 - i32::from(top.a());
    [(top.r(), bg.r()), (top.g(), bg.g()), (top.b(), bg.b())].map(|(t, b)| i32::from(t) + i32::from(b) * keep / 255)
}

fn near(a: [i32; 3], b: Color32) -> bool {
    a.iter().zip([b.r(), b.g(), b.b()]).all(|(x, y)| (x - i32::from(y)).abs() <= 6)
}

/// Numeric Grid lines retain their stacking order over clips in both themes.
#[test]
fn grid_lines_are_drawn_over_clips_while_shown() {
    let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
    let ctx = egui::Context::default();
    for theme in ["dark", "light"] {
        for shown in [true, false] {
            app.run("ui.theme", json!({"mode": theme})).unwrap();
            app.run("view.grid_lines", json!({"value": shown})).unwrap();
            frame(&ctx, &mut app);
            let out = frame(&ctx, &mut app);
            let t = Tokens::current();
            let s = app.engine.session().clone();
            let kick = s.track_by_name("Kick").unwrap();
            let clip = kick.clips()[0].clone();
            let [x0, y0, x1, y1] = app.edit_layout.timeline;
            let tl = Rect::from_min_max(pos2(x0, y0), pos2(x1, y1));
            let row = app.edit_layout.rows.iter().find(|(id, _)| *id == kick.id.0).unwrap().1;
            let lane = Rect::from_min_max(pos2(tl.min.x, row[1]), pos2(tl.max.x, row[1] + kick.height.points()));
            let (cx0, cx1) = (edit_window::x_of(&s, tl, clip.start), edit_window::x_of(&s, tl, clip.end()));
            let body = out
                .shapes
                .iter()
                .position(|c| matches!(&c.shape, Shape::Rect(r) if r.rect.min.x == cx0 && r.rect.max.x == cx1 && r.rect.min.y == lane.min.y + 1.0))
                .unwrap();
            let lines: Vec<(usize, Color32)> = out
                .shapes
                .iter()
                .enumerate()
                .filter_map(|(i, c)| match &c.shape {
                    Shape::LineSegment { points: [a, b], stroke }
                        if a.x == b.x && a.x > cx0 + 1.0 && a.x < cx1 - 1.0 && a.y == lane.min.y && b.y == lane.max.y =>
                    {
                        Some((i, stroke.color))
                    }
                    _ => None,
                })
                .collect();
            assert!(lines.len() > 4, "{theme} {shown}: {} lines", lines.len());
            assert!(lines.iter().all(|(i, _)| (*i > body) == shown), "{theme} {shown}");
            for (_, col) in &lines {
                let look = over(*col, t.playlist_bg);
                assert!(near(look, t.grid_line) || near(look, t.bar_line), "{theme} {shown}: {look:?}");
            }
        }
    }
}

/// Clips/Markers grid lines reach the timeline at memory-location boundaries.
#[test]
fn clips_markers_grid_draws_a_line_at_a_memory_location() {
    let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
    let ctx = egui::Context::default();
    let kick = app.engine.session().track_by_name("Kick").unwrap();
    let clip = &kick.clips()[0];
    let marker = clip.start + clip.length / 3;
    app.run("markers.add", json!({"at": marker, "name": "Grid target"})).unwrap();
    app.run("view.grid", json!({"format": "clips_markers", "lines": true})).unwrap();

    frame(&ctx, &mut app);
    let out = frame(&ctx, &mut app);
    let session = app.engine.session();
    let kick = session.track_by_name("Kick").unwrap();
    let [x0, y0, x1, y1] = app.edit_layout.timeline;
    let timeline = Rect::from_min_max(pos2(x0, y0), pos2(x1, y1));
    let row = app.edit_layout.rows.iter().find(|(id, _)| *id == kick.id.0).unwrap().1;
    let lane = Rect::from_min_max(pos2(timeline.min.x, row[1]), pos2(timeline.max.x, row[1] + kick.height.points()));
    let expected_x = edit_window::x_of(session, timeline, marker);

    assert!(out.shapes.iter().any(|shape| {
        matches!(
            &shape.shape,
            Shape::LineSegment { points: [a, b], .. }
                if a.x == expected_x && b.x == expected_x && a.y == lane.min.y && b.y == lane.max.y
        )
    }));
}
