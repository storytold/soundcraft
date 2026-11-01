//! Regression test for #264: the Edit toolbar clipped its tools in a narrow window; it now scrolls horizontally.

use egui::{Event, Pos2, Shape, vec2};
use serde_json::json;
use soundcraft_ui_egui::{Services, SoundApp};

const WIDTH: f32 = 700.0;

/// The toolbar's tempo text (the last group but one) and the clip rect it is painted in, once painted.
fn tempo(h: &egui_kittest::Harness<'_, SoundApp>) -> Option<(egui::Rect, egui::Rect)> {
    h.output().shapes.iter().find_map(|c| match &c.shape {
        Shape::Text(t) if t.galley.text() == "120.0000" => Some((t.galley.rect.translate(t.pos.to_vec2()), c.clip_rect)),
        _ => None,
    })
}

/// In a narrow window the toolbar scrolls horizontally far enough to reach the tempo readout.
#[test]
fn a_narrow_window_scrolls_the_toolbar_to_reach_the_tempo() {
    let mut engine = soundcraft_engine::Engine::default();
    engine.execute("track.new", &json!({})).unwrap();
    let app = SoundApp::new(engine, None, Services::default());
    let mut h = egui_kittest::Harness::builder().with_size(vec2(WIDTH, 700.0)).build_ui_state(
        |ui, app: &mut SoundApp| {
            app.logic(ui.ctx());
            app.ui(ui);
        },
        app,
    );
    for _ in 0..4 {
        h.step();
    }
    // Off the right edge, so egui culls it (or paints it beyond the window).
    assert!(tempo(&h).is_none_or(|(r, _)| r.min.x > WIDTH), "the tempo should start off-screen in a {WIDTH} pt window");
    let y = 40.0;

    // Wheel over the toolbar (a vertical wheel does not scroll a horizontal-only area, a horizontal one does).
    for _ in 0..12 {
        h.event(Event::PointerMoved(Pos2::new(WIDTH / 2.0, y)));
        h.event(Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: vec2(-300.0, 0.0),
            modifiers: egui::Modifiers::NONE,
            phase: egui::TouchPhase::Move,
            source: egui::MouseWheelSource::Unknown,
        });
        h.step();
    }
    let at = tempo(&h);
    assert!(
        at.is_some_and(|(r, clip)| r.min.x >= 0.0 && r.max.x <= WIDTH && clip.contains_rect(r)),
        "scrolling the toolbar must bring the whole tempo into the window, unclipped: (text, clip) = {at:?}"
    );
}
