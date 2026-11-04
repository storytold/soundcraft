use soundcraft_ui_egui::{Services, SoundApp};
use std::time::Duration;

fn pass(ctx: &egui::Context, app: &mut SoundApp, shown: bool) -> Duration {
    let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
        app.logic(ui.ctx());
        if shown {
            app.ui(ui);
        }
    });
    out.textures_delta.clear();
    out.viewport_output[&egui::ViewportId::ROOT].repaint_delay
}

#[test]
fn only_a_shown_window_animates_and_a_hidden_one_wakes_once_a_second() {
    let ctx = egui::Context::default();
    let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
    for _ in 0..3 {
        pass(&ctx, &mut app, true);
    }
    assert_ne!(pass(&ctx, &mut app, true), Duration::ZERO);
    app.run("transport.play", serde_json::json!({})).unwrap();
    // Playing repaints at the meter rate (30 Hz in the Edit window, #207).
    assert!(pass(&ctx, &mut app, true) <= Duration::from_millis(34));
    pass(&ctx, &mut app, false);
    assert!((pass(&ctx, &mut app, false).as_secs_f64() - 1.0).abs() < 1e-3);
}
