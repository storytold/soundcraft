//! Clicking the toolbar's Sub counter offers the time formats, as the Main counter does (#1374).

use egui::{Event, Modifiers, PointerButton, Pos2, RawInput, Rect, Shape, epaint::ClippedShape, vec2};
use soundcraft_time::TimeFormat;
use soundcraft_ui_egui::{Services, SoundApp};

struct Rig {
    ctx: egui::Context,
    app: SoundApp,
    shapes: Vec<ClippedShape>,
}

impl Rig {
    fn new() -> Rig {
        let mut r = Rig {
            ctx: egui::Context::default(),
            app: SoundApp::new(soundcraft_engine::Engine::default(), None, Services::default()),
            shapes: Vec::new(),
        };
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

    /// Centre of the last text drawn as `text` (a popup's items are painted over the window).
    fn text(&self, text: &str) -> Option<Pos2> {
        self.shapes.iter().rev().find_map(|c| match &c.shape {
            Shape::Text(t) if t.galley.text() == text => Some(t.galley.rect.translate(t.pos.to_vec2()).center()),
            _ => None,
        })
    }

    fn count(&self, text: &str) -> usize {
        self.shapes.iter().filter(|c| matches!(&c.shape, Shape::Text(t) if t.galley.text() == text)).count()
    }

    fn click(&mut self, at: Pos2) {
        for pressed in [true, false] {
            self.frame(vec![
                Event::PointerMoved(at),
                Event::PointerButton { pos: at, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE },
            ]);
        }
        self.frames(2);
    }
}

#[test]
fn the_sub_counter_menu_changes_its_format() {
    let mut r = Rig::new();
    let main = r.app.engine.session().edit.main_counter;
    assert_eq!(r.app.engine.session().edit.sub_counter, Some(TimeFormat::Samples));
    let sub = r.text("Sub").expect("the Sub counter is in the toolbar");
    let shown = r.count(TimeFormat::MinSecs.label());
    r.click(sub);
    assert_eq!(r.count(TimeFormat::MinSecs.label()), shown + 1, "clicking the Sub counter opens its format menu");
    let item = r.text(TimeFormat::MinSecs.label()).expect("clicking the Sub counter lists the formats");
    r.click(item);
    assert_eq!(r.app.engine.session().edit.sub_counter, Some(TimeFormat::MinSecs));
    assert_eq!(r.app.engine.session().edit.main_counter, main, "the Main counter keeps its format");
}
