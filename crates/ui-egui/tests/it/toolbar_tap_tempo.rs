//! The toolbar's Tap key sets the tempo from the clicks (#1483).

use egui::{Event, Modifiers, PointerButton, Pos2, RawInput, Rect, Shape, epaint::ClippedShape, vec2};
use serde_json::json;
use soundcraft_ui_egui::{Services, SoundApp};

struct Rig {
    ctx: egui::Context,
    app: SoundApp,
    shapes: Vec<ClippedShape>,
    time: f64,
}

impl Rig {
    fn new() -> Rig {
        let mut engine = soundcraft_engine::Engine::default();
        engine.execute("track.new", &json!({})).unwrap();
        let mut r = Rig { ctx: egui::Context::default(), app: SoundApp::new(engine, None, Services::default()), shapes: Vec::new(), time: 0.0 };
        r.frame(vec![]);
        r.frame(vec![]);
        r
    }

    fn frame(&mut self, events: Vec<Event>) {
        self.time += 0.05;
        let raw = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1600.0, 1000.0))),
            time: Some(self.time),
            events,
            ..Default::default()
        };
        let mut out = self.ctx.run_ui(raw, |ui| {
            self.app.logic(ui.ctx());
            self.app.ui(ui);
        });
        out.textures_delta.clear();
        self.shapes = out.shapes;
    }

    fn bpm(&self) -> f64 {
        self.app.engine.session().tempo.tempo_at_tick(0)
    }

    fn tap_key(&self) -> Pos2 {
        self.shapes
            .iter()
            .find_map(|c| match &c.shape {
                Shape::Text(t) if t.galley.text() == "Tap" => Some(t.galley.rect.translate(t.pos.to_vec2()).center()),
                _ => None,
            })
            .expect("no Tap key in the toolbar")
    }

    /// One click, then idle frames so the taps come half a second apart (120 BPM).
    fn click(&mut self, at: Pos2) {
        for pressed in [true, false] {
            self.frame(vec![
                Event::PointerMoved(at),
                Event::PointerButton { pos: at, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE },
            ]);
        }
        for _ in 0..8 {
            self.frame(vec![]);
        }
    }
}

#[test]
fn clicking_tap_sets_the_tempo_from_the_clicks_in_one_undo_step() {
    let mut r = Rig::new();
    r.app.run("event.tempo", json!({"bpm": 90.0})).unwrap();
    r.frame(vec![]);
    let at = r.tap_key();
    r.click(at);
    assert_eq!(r.bpm(), 90.0, "one tap alone must not change the tempo");
    for _ in 0..3 {
        r.click(at);
    }
    assert!((r.bpm() - 120.0).abs() < 1.0, "bpm {}", r.bpm());
    r.app.run("edit.undo", json!({})).unwrap();
    assert_eq!(r.bpm(), 90.0);
}

#[test]
fn a_tap_counts_when_the_button_goes_down_however_long_it_is_held() {
    let mut r = Rig::new();
    let at = r.tap_key();
    let button = |pressed| Event::PointerButton { pos: at, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE };
    // Presses half a second (ten frames) apart, the last one held much longer than the others.
    for hold in [1, 1, 1, 7] {
        r.frame(vec![Event::PointerMoved(at), button(true)]);
        for _ in 1..hold {
            r.frame(vec![]);
        }
        r.frame(vec![button(false)]);
        for _ in hold + 1..10 {
            r.frame(vec![]);
        }
    }
    assert!((r.bpm() - 120.0).abs() < 1.0, "bpm {}", r.bpm());
}

#[test]
fn a_click_pressed_and_released_in_one_frame_still_taps() {
    let mut r = Rig::new();
    r.app.run("event.tempo", json!({"bpm": 90.0})).unwrap();
    r.frame(vec![]);
    let at = r.tap_key();
    let button = |pressed| Event::PointerButton { pos: at, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE };
    for _ in 0..4 {
        r.frame(vec![Event::PointerMoved(at), button(true), button(false)]);
        for _ in 1..10 {
            r.frame(vec![]);
        }
    }
    assert!((r.bpm() - 120.0).abs() < 1.0, "bpm {}", r.bpm());
}
