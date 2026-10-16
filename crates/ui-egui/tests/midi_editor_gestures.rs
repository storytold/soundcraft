//! MIDI Editor mouse gestures: drawing, copying, trimming, velocity strokes and zoom.
use egui::{Event, Modifiers, PointerButton, Pos2, RawInput, Rect, pos2, vec2};
use serde_json::{Value, json};
use soundcraft_ui_egui::midi_editor::RollView;
use soundcraft_ui_egui::{Services, SoundApp};

struct Rig {
    app: SoundApp,
    ctx: egui::Context,
    clip: Value,
}

impl Rig {
    /// A MIDI track with a 16-beat clip open in the MIDI Editor, Grid mode on a 1/16 grid.
    fn new(notes: &[(u8, i64, i64)]) -> Self {
        let mut app = SoundApp::new(soundcraft_engine::Engine::default(), None, Services::default());
        app.run("track.new", json!({"name": "Keys", "kind": "midi"})).unwrap();
        let q = app.engine.session().sample_rate.samples(0.5);
        let clip = app.run("midi.new_clip", json!({"track": "Keys", "start": 0, "end": 16 * q})).unwrap()["clip"].clone();
        for &(pitch, start, length) in notes {
            app.run("midi.note_add", json!({"clip": clip, "pitch": pitch, "start_ticks": start, "length_ticks": length})).unwrap();
        }
        app.run("edit.select", json!({"clips": [clip]})).unwrap();
        app.run("view.grid", json!({"value": "1/16"})).unwrap();
        app.run("edit.mode", json!({"mode": "grid"})).unwrap();
        app.ui.show_midi_editor = true;
        let mut r = Rig { app, ctx: egui::Context::default(), clip };
        r.frames(6, Modifiers::NONE);
        r
    }

    fn frame(&mut self, events: Vec<Event>, modifiers: Modifiers) {
        let events = std::iter::once(Event::ModifiersChanged(modifiers)).chain(events).collect();
        let raw = RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1600.0, 1000.0))), events, ..Default::default() };
        let app = &mut self.app;
        self.ctx
            .run_ui(raw, |ui| {
                app.logic(ui.ctx());
                app.ui(ui);
            })
            .textures_delta
            .clear();
    }

    fn frames(&mut self, n: usize, modifiers: Modifiers) {
        for _ in 0..n {
            self.frame(vec![], modifiers);
        }
    }

    fn view(&self) -> RollView {
        self.app.midi.view.unwrap()
    }

    /// The centre of `pitch`'s row at a clip tick.
    fn at(&self, tick: i64, pitch: i32) -> Pos2 {
        let v = self.view();
        pos2(v.x_of(tick), v.y_of(pitch) + v.row_h / 2.0)
    }

    fn drag(&mut self, from: Pos2, to: Pos2, modifiers: Modifiers) {
        let button = |pos, pressed| Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers };
        self.frame(vec![Event::PointerMoved(from)], modifiers);
        self.frame(vec![button(from, true)], modifiers);
        for step in 1..=8 {
            self.frame(vec![Event::PointerMoved(from.lerp(to, step as f32 / 8.0))], modifiers);
        }
        self.frame(vec![button(to, false)], modifiers);
        self.frames(2, Modifiers::NONE);
    }

    fn click(&mut self, at: Pos2, modifiers: Modifiers) {
        self.frame(vec![Event::PointerMoved(at)], modifiers);
        self.frame(vec![Event::PointerButton { pos: at, button: PointerButton::Primary, pressed: true, modifiers }], modifiers);
        self.frame(vec![Event::PointerButton { pos: at, button: PointerButton::Primary, pressed: false, modifiers }], modifiers);
        self.frames(2, Modifiers::NONE);
    }

    /// (start, pitch, length, velocity) of every note.
    fn notes(&mut self) -> Vec<(i64, i64, i64, i64)> {
        let n = self.app.run("midi.notes", json!({"clip": self.clip})).unwrap();
        n["notes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|n| (n["start"].as_i64().unwrap(), n["pitch"].as_i64().unwrap(), n["length"].as_i64().unwrap(), n["velocity"].as_i64().unwrap()))
            .collect()
    }
}

#[test]
fn a_pencil_drag_draws_a_note_as_long_as_the_drag() {
    let mut r = Rig::new(&[]);
    r.app.run("edit.tool", json!({"tool": "pencil"})).unwrap();
    let (from, to) = (r.at(490, 60), r.at(1440, 60));
    r.drag(from, to, Modifiers::NONE);
    // The start snaps back to its sixteenth, the end to the nearest one.
    assert_eq!(r.notes(), vec![(480, 60, 960, 100)]);
}

#[test]
fn alt_dragging_notes_copies_them_and_selects_the_copies() {
    let mut r = Rig::new(&[(60, 0, 480), (64, 0, 480)]);
    r.app.midi.selected = vec![0, 1];
    let (from, to) = (r.at(240, 60), r.at(240 + 960, 62));
    r.drag(from, to, Modifiers::ALT);
    assert_eq!(r.notes(), vec![(0, 60, 480, 100), (0, 64, 480, 100), (960, 62, 480, 100), (960, 66, 480, 100)]);
    assert_eq!(r.app.midi.selected, vec![2, 3]);
    r.app.run("edit.undo", json!({})).unwrap();
    assert_eq!(r.notes().len(), 2, "the copy is one undo step");
}

#[test]
fn moving_notes_past_others_keeps_them_selected() {
    let mut r = Rig::new(&[(60, 0, 480), (62, 480, 480), (64, 960, 480)]);
    r.app.midi.selected = vec![0];
    let (from, to) = (r.at(240, 60), r.at(240 + 1440, 60));
    r.drag(from, to, Modifiers::NONE);
    assert_eq!(r.notes()[2], (1440, 60, 480, 100));
    assert_eq!(r.app.midi.selected, vec![2]);
}

#[test]
fn trimming_one_selected_note_trims_them_all() {
    let mut r = Rig::new(&[(60, 0, 480), (64, 960, 960)]);
    r.app.midi.selected = vec![0, 1];
    let edge = r.at(480, 60) - vec2(2.0, 0.0);
    r.drag(edge, edge + vec2(r.view().px_per_tick * 240.0, 0.0), Modifiers::NONE);
    assert_eq!(r.notes(), vec![(0, 60, 720, 100), (960, 64, 1200, 100)]);
    assert_eq!(r.app.midi.selected, vec![0, 1]);
    // The start edge, with the grid suspended: the end stays put.
    let start = r.at(960, 64) + vec2(2.0, 0.0);
    r.drag(start, start + vec2(r.view().px_per_tick * 100.0, 0.0), Modifiers::COMMAND);
    let n = r.notes();
    assert!(n[0].0 > 0 && n[0].0 < 120 && n[0].0 + n[0].2 == 720, "{n:?}");
    assert!(n[1].0 > 960 && n[1].0 + n[1].2 == 2160, "{n:?}");
}

#[test]
fn with_one_chord_note_selected_the_velocity_lane_sets_only_that_note() {
    let mut r = Rig::new(&[(60, 0, 480), (64, 0, 480), (67, 0, 480)]);
    r.app.midi.selected = vec![1];
    let v = r.view();
    r.click(pos2(v.x_of(0) + 1.0, v.vel.min.y + 2.0), Modifiers::NONE);
    let vel: Vec<i64> = r.notes().iter().map(|n| n.3).collect();
    assert_eq!(vel, vec![100, 127, 100]);
}

#[test]
fn a_velocity_stroke_sets_every_note_it_crosses_in_one_undo_step() {
    let mut r = Rig::new(&[(60, 0, 240), (60, 480, 240), (60, 960, 240), (60, 1440, 240)]);
    let v = r.view();
    let y = v.vel.max.y - 2.0 - (v.vel.height() - 4.0) * 0.5;
    r.drag(pos2(v.x_of(0) - 4.0, y), pos2(v.x_of(1440) + 4.0, y), Modifiers::NONE);
    assert!(r.notes().iter().all(|n| (63..=65).contains(&n.3)), "{:?}", r.notes());
    r.app.run("edit.undo", json!({})).unwrap();
    assert!(r.notes().iter().all(|n| n.3 == 100), "{:?}", r.notes());
}

#[test]
fn zooming_keeps_the_tick_under_the_pointer_in_place() {
    let mut r = Rig::new(&[(60, 0, 480)]);
    let before = r.view();
    let p = r.at(1920, 60);
    let tick = before.tick_at(p.x);
    r.frame(vec![Event::PointerMoved(p)], Modifiers::NONE);
    r.frame(vec![Event::Zoom(2.0)], Modifiers::NONE);
    r.frames(1, Modifiers::NONE);
    let after = r.view();
    assert!((after.px_per_tick / before.px_per_tick - 2.0).abs() < 0.01, "{before:?} {after:?}");
    assert!((after.tick_at(p.x) - tick).abs() < 2.0);
    // A note drawn after zooming lands where the pointer is.
    r.app.run("edit.tool", json!({"tool": "pencil"})).unwrap();
    let at = r.at(2400, 64);
    r.click(at, Modifiers::NONE);
    assert!(r.notes().contains(&(2400, 64, 240, 100)), "{:?}", r.notes());
}
