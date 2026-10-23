//! MIDI Editor controller lanes: the track's lanes, the Pencil's shapes, the Grabber and the
//! Trimmer on controller events, and Pencil shapes in the velocity lane.
use egui::{Event, Key, Modifiers, PointerButton, Pos2, RawInput, Rect, pos2, vec2};
use serde_json::{Value, json};
use soundcraft_midi::CtrlKind;
use soundcraft_ui_egui::midi_editor::RollView;
use soundcraft_ui_egui::midi_lanes::LaneView;
use soundcraft_ui_egui::{Services, SoundApp};

struct Rig {
    app: SoundApp,
    ctx: egui::Context,
    clip: Value,
}

impl Rig {
    /// A MIDI track with a 16-beat clip open in the MIDI Editor (Grid mode, 1/16 grid) and the
    /// given controller lanes.
    fn new(notes: &[(u8, i64, i64)], lanes: &[&str]) -> Self {
        let mut app = SoundApp::new(soundcraft_engine::Engine::default(), None, Services::default());
        app.run("track.new", json!({"name": "Keys", "kind": "midi"})).unwrap();
        let q = app.engine.session().sample_rate.samples(0.5);
        let clip = app.run("midi.new_clip", json!({"track": "Keys", "start": 0, "end": 16 * q})).unwrap()["clip"].clone();
        for &(pitch, start, length) in notes {
            app.run("midi.note_add", json!({"clip": clip, "pitch": pitch, "start_ticks": start, "length_ticks": length})).unwrap();
        }
        app.run("midi.lanes", json!({"track": "Keys", "lanes": lanes})).unwrap();
        app.run("edit.select", json!({"clips": [clip]})).unwrap();
        app.run("view.grid", json!({"value": "1/16"})).unwrap();
        app.run("edit.mode", json!({"mode": "grid"})).unwrap();
        app.ui.show_midi_editor = true;
        let mut r = Rig { app, ctx: egui::Context::default(), clip };
        r.frames(6);
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

    fn frames(&mut self, n: usize) {
        for _ in 0..n {
            self.frame(vec![], Modifiers::NONE);
        }
    }

    fn view(&self) -> RollView {
        self.app.midi.view.unwrap()
    }

    fn lane(&self, i: usize) -> LaneView {
        self.app.midi.lanes[i]
    }

    /// The point in lane `i` at a clip tick and value.
    fn at(&self, i: usize, tick: i64, value: f64) -> Pos2 {
        pos2(self.view().x_of(tick), self.lane(i).y_of(value))
    }

    fn drag(&mut self, from: Pos2, to: Pos2, modifiers: Modifiers) {
        let button = |pos, pressed| Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers };
        self.frame(vec![Event::PointerMoved(from)], modifiers);
        self.frame(vec![button(from, true)], modifiers);
        for step in 1..=8 {
            self.frame(vec![Event::PointerMoved(from.lerp(to, step as f32 / 8.0))], modifiers);
        }
        self.frame(vec![button(to, false)], modifiers);
        self.frames(2);
    }

    fn click(&mut self, at: Pos2, modifiers: Modifiers) {
        self.frame(vec![Event::PointerMoved(at)], modifiers);
        self.frame(vec![Event::PointerButton { pos: at, button: PointerButton::Primary, pressed: true, modifiers }], modifiers);
        self.frame(vec![Event::PointerButton { pos: at, button: PointerButton::Primary, pressed: false, modifiers }], modifiers);
        self.frames(2);
    }

    fn key(&mut self, key: Key) {
        self.frame(vec![Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE }], Modifiers::NONE);
        self.frames(1);
    }

    /// (tick, value) of the clip's events of `kind`.
    fn ctrls(&mut self, kind: &str) -> Vec<(i64, i64)> {
        let r = self.app.run("midi.ctrls", json!({"clip": self.clip, "kind": kind})).unwrap();
        r["ctrls"].as_array().unwrap().iter().map(|c| (c["tick"].as_i64().unwrap(), c["value"].as_i64().unwrap())).collect()
    }

    fn add(&mut self, kind: &str, tick: i64, value: i64) {
        self.app.run("midi.ctrl_add", json!({"clip": self.clip, "kind": kind, "tick": tick, "value": value})).unwrap();
    }

    fn velocities(&mut self) -> Vec<i64> {
        let n = self.app.run("midi.notes", json!({"clip": self.clip})).unwrap();
        n["notes"].as_array().unwrap().iter().map(|n| n["velocity"].as_i64().unwrap()).collect()
    }
}

#[test]
fn the_tracks_lanes_sit_under_the_velocity_lane() {
    let mut r = Rig::new(&[], &[]);
    let roll_alone = r.view().roll;
    assert!(r.app.midi.lanes.is_empty());
    r.app.run("midi.lanes", json!({"track": "Keys", "lanes": ["cc1", "pitchbend"]})).unwrap();
    r.frames(2);
    let (v, a, b) = (r.view(), r.lane(0), r.lane(1));
    assert_eq!((a.kind, b.kind), (CtrlKind::Cc(1), CtrlKind::PitchBend));
    assert!(v.roll.max.y < roll_alone.max.y, "the roll makes room");
    assert!(v.vel.max.y <= a.rect.min.y && a.rect.max.y <= b.rect.min.y);
    assert!(b.rect.max.y <= 1000.0);
}

#[test]
fn a_pencil_line_draws_a_ramp_from_the_press_to_the_release() {
    let mut r = Rig::new(&[], &["cc1"]);
    r.app.run("edit.tool", json!({"tool": "pencil"})).unwrap();
    r.app.run("edit.pencil_shape", json!({"shape": "line"})).unwrap();
    r.frames(1);
    r.drag(r.at(0, 0, 0.0), r.at(0, 1920, 127.0), Modifiers::NONE);
    let ev = r.ctrls("cc1");
    assert!(ev.len() > 10, "{ev:?}");
    assert_eq!(ev.first().map(|e| e.0), Some(0));
    assert_eq!(ev.last().map(|e| e.0), Some(1920));
    assert!(ev.first().unwrap().1 <= 2 && ev.last().unwrap().1 >= 125, "{ev:?}");
    assert!(ev.windows(2).all(|w| w[0].1 < w[1].1), "rising: {ev:?}");
    r.app.run("edit.undo", json!({})).unwrap();
    assert!(r.ctrls("cc1").is_empty(), "one undo step");
}

#[test]
fn a_free_hand_stroke_replaces_what_it_crosses() {
    let mut r = Rig::new(&[], &["cc11"]);
    r.add("cc11", 0, 10);
    r.add("cc11", 960, 10);
    r.add("cc11", 3000, 10);
    r.app.run("edit.tool", json!({"tool": "pencil"})).unwrap();
    r.frames(1);
    r.drag(r.at(0, 480, 100.0), r.at(0, 1440, 100.0), Modifiers::NONE);
    let ev = r.ctrls("cc11");
    assert_eq!(ev.first(), Some(&(0, 10)));
    assert_eq!(ev.last(), Some(&(3000, 10)));
    assert!(!ev.contains(&(960, 10)), "{ev:?}");
    assert!(ev.iter().any(|&(t, v)| (470..=500).contains(&t) && (98..=101).contains(&v)), "{ev:?}");
}

#[test]
fn the_grabber_adds_moves_and_alt_click_deletes() {
    let mut r = Rig::new(&[], &["cc7"]);
    r.app.run("edit.tool", json!({"tool": "grabber"})).unwrap();
    r.frames(1);
    // A click on empty lane adds an event on the grid.
    r.click(r.at(0, 250, 64.0), Modifiers::NONE);
    assert_eq!(r.ctrls("cc7"), vec![(240, 64)]);
    assert_eq!(r.app.midi.ctrl_selected, vec![0]);
    // Dragging it moves it by whole grid steps and changes its value.
    r.drag(r.at(0, 240, 64.0), r.at(0, 960, 100.0), Modifiers::NONE);
    assert_eq!(r.ctrls("cc7"), vec![(960, 100)]);
    r.click(r.at(0, 960, 100.0), Modifiers::ALT);
    assert!(r.ctrls("cc7").is_empty());
}

#[test]
fn a_band_selects_events_and_delete_removes_only_them() {
    let mut r = Rig::new(&[(60, 0, 480)], &["cc64"]);
    for t in [0, 480, 960, 1440] {
        r.add("cc64", t, 127);
    }
    r.app.run("edit.tool", json!({"tool": "grabber"})).unwrap();
    r.frames(1);
    let lane = r.lane(0).rect;
    r.drag(pos2(r.view().x_of(400), lane.min.y + 1.0), pos2(r.view().x_of(1000), lane.max.y - 1.0), Modifiers::NONE);
    assert_eq!(r.app.midi.ctrl_selected, vec![1, 2]);
    r.frame(vec![Event::PointerMoved(lane.center())], Modifiers::NONE);
    r.key(Key::Delete);
    assert_eq!(r.ctrls("cc64"), vec![(0, 127), (1440, 127)]);
    assert_eq!(r.velocities().len(), 1, "the note stays");
}

#[test]
fn the_trimmer_raises_every_event_in_the_lane() {
    let mut r = Rig::new(&[], &["cc1"]);
    for (t, v) in [(0, 10), (480, 40), (960, 120)] {
        r.add("cc1", t, v);
    }
    r.app.run("edit.tool", json!({"tool": "trim"})).unwrap();
    r.frames(1);
    r.drag(r.at(0, 700, 50.0), r.at(0, 700, 70.0), Modifiers::NONE);
    assert_eq!(r.ctrls("cc1"), vec![(0, 30), (480, 60), (960, 127)]);
}

#[test]
fn a_pencil_line_in_the_velocity_lane_ramps_the_notes() {
    let mut r = Rig::new(&[(60, 0, 240), (60, 480, 240), (60, 960, 240), (60, 1440, 240)], &[]);
    r.app.run("edit.tool", json!({"tool": "pencil"})).unwrap();
    r.app.run("edit.pencil_shape", json!({"shape": "line"})).unwrap();
    r.frames(1);
    let v = r.view();
    let y = |vel: f32| v.vel.max.y - 2.0 - (v.vel.height() - 4.0) * vel / 127.0;
    r.drag(pos2(v.x_of(0), y(20.0)), pos2(v.x_of(1440), y(120.0)), Modifiers::NONE);
    let vel = r.velocities();
    assert!(vel.windows(2).all(|w| w[0] < w[1]), "{vel:?}");
    assert!((18..=22).contains(&vel[0]) && (118..=122).contains(&vel[3]), "{vel:?}");
    r.app.run("edit.undo", json!({})).unwrap();
    assert_eq!(r.velocities(), vec![100; 4], "one undo step");
}
