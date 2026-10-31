//! Dragging rows in the Tracks list reorders the tracks (#170); a click still selects.

use egui::{Event, PointerButton, Pos2, RawInput, Rect, Shape, epaint::ClippedShape, pos2, vec2};
use serde_json::json;
use soundcraft_model::TrackId;
use soundcraft_ui_egui::{Services, SoundApp};

struct Rig {
    ctx: egui::Context,
    app: SoundApp,
    shapes: Vec<ClippedShape>,
}

impl Rig {
    fn new() -> Rig {
        let mut engine = soundcraft_engine::Engine::default();
        engine.execute("track.new", &json!({"count": 6})).unwrap();
        let mut r = Rig { ctx: egui::Context::default(), app: SoundApp::new(engine, None, Services::default()), shapes: Vec::new() };
        for _ in 0..4 {
            r.frame(vec![]);
        }
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

    fn order(&self) -> Vec<TrackId> {
        self.app.engine.session().tracks.iter().map(|t| t.id).collect()
    }

    /// Centre of the Tracks list row naming the track at session index `i`.
    fn row(&self, i: usize) -> Pos2 {
        let name = &self.app.engine.session().tracks[i].name;
        self.shapes
            .iter()
            .find_map(|c| match &c.shape {
                Shape::Text(t) if t.galley.text() == name && t.pos.x < 150.0 => {
                    Some(pos2(t.pos.x + 20.0, t.galley.rect.translate(t.pos.to_vec2()).center().y))
                }
                _ => None,
            })
            .expect("row not in the Tracks list")
    }

    fn press(&mut self, at: Pos2, pressed: bool) {
        self.frame(vec![
            Event::PointerMoved(at),
            Event::PointerButton { pos: at, button: PointerButton::Primary, pressed, modifiers: Default::default() },
        ]);
    }

    fn drag(&mut self, from: Pos2, to: Pos2) {
        self.press(from, true);
        for k in 1..=6 {
            self.frame(vec![Event::PointerMoved(from.lerp(to, k as f32 / 6.0))]);
        }
        self.press(to, false);
        self.frame(vec![]);
    }
}

#[test]
fn dragging_a_track_list_row_moves_the_track_to_the_gap_it_is_dropped_in() {
    let mut r = Rig::new();
    let ids = r.order();
    let (from, lower_half_of_fifth) = (r.row(0), r.row(4) + vec2(0.0, 5.0));
    r.drag(from, lower_half_of_fifth);
    assert_eq!(r.order(), vec![ids[1], ids[2], ids[3], ids[4], ids[0], ids[5]]);
    let (from, upper_half_of_second) = (r.row(5), r.row(1) - vec2(0.0, 5.0));
    r.drag(from, upper_half_of_second);
    assert_eq!(r.order(), vec![ids[1], ids[5], ids[2], ids[3], ids[4], ids[0]]);
    let (from, own_lower_half) = (r.row(2), r.row(2) + vec2(0.0, 8.0));
    r.drag(from, own_lower_half);
    assert_eq!(r.order(), vec![ids[1], ids[5], ids[2], ids[3], ids[4], ids[0]]);
}

#[test]
fn clicking_a_track_list_row_still_selects_the_track() {
    let mut r = Rig::new();
    let at = r.row(2);
    r.press(at, true);
    r.press(at, false);
    assert_eq!(r.app.engine.session().edit.selected_tracks, vec![r.order()[2]]);
}
