//! The toolbar's tempo can be dragged or typed (#189).

use egui::{Event, Key, Modifiers, PointerButton, Pos2, RawInput, Rect, Shape, epaint::ClippedShape, vec2};
use serde_json::json;
use soundcraft_time::BarBeat;
use soundcraft_ui_egui::{Services, SoundApp};

struct Rig {
    ctx: egui::Context,
    app: SoundApp,
    shapes: Vec<ClippedShape>,
}

impl Rig {
    fn new() -> Rig {
        let mut engine = soundcraft_engine::Engine::default();
        engine.execute("track.new", &json!({})).unwrap();
        let mut r = Rig { ctx: egui::Context::default(), app: SoundApp::new(engine, None, Services::default()), shapes: Vec::new() };
        r.frames(4);
        r
    }

    fn frame(&mut self, events: Vec<Event>) {
        let raw = RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1600.0, 1000.0))), events, ..Default::default() };
        let mut out = self.ctx.run_ui(raw, |ui| {
            self.app.logic(ui.ctx());
            self.app.ui(ui);
        });
        out.textures_delta.clear();
        self.shapes = out.shapes;
    }

    fn frames(&mut self, n: usize) {
        for _ in 0..n {
            self.frame(vec![]);
        }
    }

    fn bpm(&self, tick: i64) -> f64 {
        self.app.engine.session().tempo.tempo_at_tick(tick)
    }

    /// Centre of the toolbar's tempo, which shows `bpm`.
    fn field(&self, bpm: f64) -> Pos2 {
        let text = format!("{bpm:.4}");
        self.shapes
            .iter()
            .find_map(|c| match &c.shape {
                Shape::Text(t) if t.galley.text() == text => Some(t.galley.rect.translate(t.pos.to_vec2()).center()),
                _ => None,
            })
            .expect("tempo not in the toolbar")
    }

    fn button(&mut self, at: Pos2, pressed: bool) {
        self.frame(vec![
            Event::PointerMoved(at),
            Event::PointerButton { pos: at, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE },
        ]);
    }

    fn type_into(&mut self, at: Pos2, text: &str) {
        self.button(at, true);
        self.button(at, false);
        self.frames(2);
        self.frame(vec![Event::Text(text.into())]);
        self.frame(vec![Event::Key { key: Key::Enter, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE }]);
        self.frames(2);
    }
}

#[test]
fn dragging_the_toolbar_tempo_changes_it_in_one_undo_step() {
    let mut r = Rig::new();
    let at = r.field(120.0);
    r.button(at, true);
    for k in 1..=10 {
        r.frame(vec![Event::PointerMoved(at - vec2(0.0, 6.0 * k as f32))]);
    }
    r.button(at - vec2(0.0, 60.0), false);
    assert!(r.bpm(0) > 123.0 && r.bpm(0) < 127.0, "bpm {}", r.bpm(0));
    r.app.run("edit.undo", json!({})).unwrap();
    assert_eq!(r.bpm(0), 120.0);
}

#[test]
fn clicking_the_toolbar_tempo_lets_you_type_it() {
    let mut r = Rig::new();
    let at = r.field(120.0);
    r.type_into(at, "98.5");
    assert_eq!(r.bpm(0), 98.5);
}

#[test]
fn the_toolbar_tempo_edits_the_tempo_change_in_effect_at_the_playhead() {
    let mut r = Rig::new();
    let bar5 = r.app.engine.session().tempo.tick_at_bar_beat(BarBeat { bar: 5, beat: 1, tick: 0 });
    let at = r.app.engine.session().tempo.tick_to_samples(bar5, r.app.engine.session().sample_rate);
    r.app.run("event.tempo", json!({"bpm": 140.0, "at": at})).unwrap();
    r.app.run("transport.locate", json!({"at": at + 48_000})).unwrap();
    r.frames(2);
    let field = r.field(140.0);
    r.type_into(field, "150");
    assert_eq!((r.bpm(0), r.bpm(bar5), r.app.engine.session().tempo.tempos().len()), (120.0, 150.0, 2));
}
