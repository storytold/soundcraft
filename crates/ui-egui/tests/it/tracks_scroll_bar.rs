use egui::{Event, Modifiers, PointerButton, Pos2, Rect, Shape, pos2, vec2};
use egui_kittest::Harness;
use serde_json::json;
use soundcraft_ui_egui::{Services, SoundApp, theme::Tokens};

/// Feed one harness frame through the raw-input hook before stepping, so scripted input reaches the app.
fn step(h: &mut Harness<'_, SoundApp>) {
    let mut raw = std::mem::take(h.input_mut());
    let ctx = h.ctx.clone();
    h.state_mut().raw_input_hook(&ctx, &mut raw);
    *h.input_mut() = raw;
    h.step();
}

fn click(h: &mut Harness<'_, SoundApp>, pos: Pos2) {
    h.state_mut().synthetic.extend([
        Event::PointerMoved(pos),
        Event::PointerButton { pos, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE },
        Event::PointerButton { pos, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE },
    ]);
    for _ in 0..6 {
        step(h);
    }
}

/// The Tracks list's first row, the leftmost "Kick" painted.
fn first_row(h: &Harness<'_, SoundApp>) -> Rect {
    h.output()
        .shapes
        .iter()
        .filter_map(|shape| match &shape.shape {
            Shape::Text(text) if text.galley.job.text == "Kick" => Some(text.visual_bounding_rect()),
            _ => None,
        })
        .min_by(|a, b| a.min.x.total_cmp(&b.min.x))
        .unwrap()
}

#[test]
fn the_tracks_gutter_pages_a_long_list() {
    let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
    app.run("track.new", json!({"count": 60})).unwrap();
    let mut h = Harness::builder().with_size(vec2(1600.0, 1000.0)).build_ui_state(
        |ui, app: &mut SoundApp| {
            app.logic(ui.ctx());
            app.ui(ui);
        },
        app,
    );
    for _ in 0..6 {
        step(&mut h);
    }
    let thumb = h
        .output()
        .shapes
        .iter()
        .find_map(|shape| match &shape.shape {
            Shape::Rect(rect) if rect.fill == Tokens::DARK.button_hi && rect.rect.width() == 6.0 => Some(rect.rect),
            _ => None,
        })
        .expect("a list longer than its space shows a thumb");
    let before = first_row(&h);
    click(&mut h, pos2(thumb.center().x, thumb.max.y + 20.0));
    let after = first_row(&h);
    assert!(after.min.y < before.min.y - 100.0, "a click below the thumb didn't page the list: {before:?} -> {after:?}");
}
