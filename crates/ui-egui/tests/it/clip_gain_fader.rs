use egui::{Event, PointerButton, Pos2, RawInput, Rect, pos2, vec2};
use serde_json::json;
use soundcraft_ui_egui::{Services, SoundApp, edit_window, widgets};

fn frame(ctx: &egui::Context, app: &mut SoundApp, events: Vec<Event>) {
    let raw = RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1600.0, 1000.0))), events, ..Default::default() };
    ctx.run_ui(raw, |ui| {
        app.logic(ui.ctx());
        app.ui(ui);
    })
    .textures_delta
    .clear();
}

fn button(pos: Pos2, pressed: bool) -> Event {
    Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers: Default::default() }
}

#[test]
fn the_clip_gain_icon_drags_with_the_pointer_in_one_undo_step() {
    let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
    app.engine.execute("view.clip_gain_info", &json!({"value": true})).unwrap();
    let ctx = egui::Context::default();
    frame(&ctx, &mut app, vec![]);
    frame(&ctx, &mut app, vec![]);
    let s = app.engine.session().clone();
    let kick = s.track_by_name("Kick").unwrap();
    let clip = kick.clips()[0].clone();
    let [x0, y0, x1, y1] = app.edit_layout.timeline;
    let tl = Rect::from_min_max(pos2(x0, y0), pos2(x1, y1));
    let row = app.edit_layout.rows.iter().find(|(id, _)| *id == kick.id.0).unwrap().1;
    let lane = Rect::from_min_max(pos2(tl.min.x, row[1]), pos2(tl.max.x, row[1] + kick.height.points()));
    let at = edit_window::clip_gain_icon(&s, tl, lane, &clip).unwrap().center();
    let gain = |app: &SoundApp| app.engine.session().find_clip(clip.id).unwrap().1.gain_db;
    let up = |dy: f32| widgets::clip_gain_from_pos(widgets::clip_gain_to_pos(0.0) + dy / edit_window::CLIP_FADER_TRAVEL);

    frame(&ctx, &mut app, vec![Event::PointerMoved(at)]);
    frame(&ctx, &mut app, vec![button(at, true)]);
    for dy in [3.0, 6.0, 10.0, 40.0] {
        frame(&ctx, &mut app, vec![Event::PointerMoved(at - vec2(0.0, dy))]);
    }
    assert!((gain(&app) - up(40.0)).abs() < 0.01, "{} vs {}", gain(&app), up(40.0));
    // Past the top end and back: the fader waits at +36 dB until the pointer returns.
    for dy in [300.0, 120.0, 20.0] {
        frame(&ctx, &mut app, vec![Event::PointerMoved(at - vec2(0.0, dy))]);
    }
    assert!((gain(&app) - up(20.0)).abs() < 0.01, "{} vs {}", gain(&app), up(20.0));
    frame(&ctx, &mut app, vec![button(at - vec2(0.0, 20.0), false)]);
    frame(&ctx, &mut app, vec![]);

    app.engine.execute("edit.undo", &json!({})).unwrap();
    assert_eq!(gain(&app), 0.0);
}
