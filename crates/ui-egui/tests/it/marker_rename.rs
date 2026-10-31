//! Double-clicking a marker in the ruler renames it; elsewhere it still adds one (#186).

use egui::{Event, Key, Modifiers, PointerButton, Pos2, RawInput, Rect, Shape, epaint::ClippedShape, pos2, vec2};
use serde_json::json;
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
        engine.execute("markers.add", &json!({"name": "Verse", "at": 96_000})).unwrap();
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

    /// The flag of the ruler marker named `name` (its label starts 8pt to the right).
    fn flag(&self, name: &str) -> Pos2 {
        self.shapes
            .iter()
            .find_map(|c| match &c.shape {
                Shape::Text(t) if t.galley.text() == name => Some(pos2(t.pos.x - 8.0, t.galley.rect.translate(t.pos.to_vec2()).center().y)),
                _ => None,
            })
            .expect("marker not on the ruler")
    }

    fn double_click(&mut self, at: Pos2) {
        for pressed in [true, false, true, false] {
            self.frame(vec![
                Event::PointerMoved(at),
                Event::PointerButton { pos: at, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE },
            ]);
        }
        self.frames(2);
    }

    fn names(&self) -> Vec<String> {
        self.app.engine.session().markers.iter().map(|m| m.name.clone()).collect()
    }
}

#[test]
fn double_clicking_a_ruler_marker_renames_it() {
    let mut r = Rig::new();
    let flag = r.flag("Verse");
    r.double_click(flag);
    assert_eq!(r.app.dialogs.open_name(), Some("rename_marker"));
    r.frame(vec![Event::Text("Chorus".into())]);
    r.frame(vec![Event::Key { key: Key::Enter, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE }]);
    r.frames(2);
    assert_eq!(r.names(), vec!["Chorus".to_string()]);
}

#[test]
fn double_clicking_the_empty_ruler_still_adds_a_marker() {
    let mut r = Rig::new();
    let flag = r.flag("Verse");
    r.double_click(flag + vec2(200.0, 0.0));
    assert_eq!(r.app.dialogs.open_name(), None);
    assert_eq!(r.names().len(), 2);
}
