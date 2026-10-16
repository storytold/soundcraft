//! A turn of the mouse wheel on a fader or knob is one undo step, however many notches it takes;
//! a second turn after the wheel has rested is another.

use egui::{Event, Modifiers, MouseWheelUnit, Pos2, RawInput, Rect, TouchPhase, vec2};
use serde_json::json;
use soundcraft_ui_egui::{Services, SoundApp, WHEEL_GESTURE_GAP};

fn frame(ctx: &egui::Context, app: &mut SoundApp, time: f64, events: Vec<Event>) {
    let raw = RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1600.0, 1000.0))), time: Some(time), events, ..Default::default() };
    ctx.run_ui(raw, |ui| app.logic(ui.ctx())).textures_delta.clear();
}

fn notch() -> Vec<Event> {
    vec![Event::MouseWheel { unit: MouseWheelUnit::Line, delta: vec2(0.0, 1.0), phase: TouchPhase::Move, modifiers: Modifiers::NONE }]
}

/// What a fader does with each notch (see `mix_window::strip`): a merged `mix.volume`.
fn ride(app: &mut SoundApp, track: u64, db: f32) {
    app.engine.execute_merged("mix.volume", &json!({"track": track, "db": db}), &format!("fader:{track}")).unwrap();
}

#[test]
fn one_turn_of_the_wheel_is_one_undo_step() {
    let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
    let ctx = egui::Context::default();
    let kick = app.engine.session().track_by_name("Kick").unwrap().id.0;
    let vol = |app: &SoundApp| app.engine.session().track_by_name("Kick").unwrap().mixer.volume_db;
    let start = vol(&app);
    let mut t = 1.0;
    frame(&ctx, &mut app, t, vec![]);

    // Three notches, a frame apart, no button down: one gesture.
    for i in 1..=3 {
        t += 1.0 / 60.0;
        frame(&ctx, &mut app, t, notch());
        ride(&mut app, kick, start + i as f32);
    }
    // The wheel rests; frames go by until the gesture ends.
    t += WHEEL_GESTURE_GAP + 0.1;
    frame(&ctx, &mut app, t, vec![]);
    // A second turn.
    t += 1.0 / 60.0;
    frame(&ctx, &mut app, t, notch());
    ride(&mut app, kick, start + 4.0);
    t += WHEEL_GESTURE_GAP + 0.1;
    frame(&ctx, &mut app, t, vec![]);

    assert_eq!(vol(&app), start + 4.0);
    app.engine.execute("edit.undo", &json!({})).unwrap();
    assert_eq!(vol(&app), start + 3.0, "undo takes back the second turn only");
    app.engine.execute("edit.undo", &json!({})).unwrap();
    assert_eq!(vol(&app), start, "and the whole first turn in one step");
}
