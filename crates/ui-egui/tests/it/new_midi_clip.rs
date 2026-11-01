//! Empty MIDI clips from the Pencil, in whole bars, and from the empty MIDI Editor (#282).

use egui::{Event, Id, PointerButton, Pos2, RawInput, Rect, pos2, vec2};
use serde_json::json;
use soundcraft_model::ClipContent;
use soundcraft_time::BarBeat;
use soundcraft_ui_egui::{Services, SoundApp, edit_window::x_of, midi_editor};

struct Rig {
    ctx: egui::Context,
    app: SoundApp,
}

impl Rig {
    fn new(kind: &str) -> Rig {
        let mut engine = soundcraft_engine::Engine::default();
        engine.execute("track.new", &json!({"kind": kind})).unwrap();
        engine.execute("edit.tool", &json!({"tool": "pencil"})).unwrap();
        let mut r = Rig { ctx: egui::Context::default(), app: SoundApp::new(engine, None, Services::default()) };
        r.frames(4);
        r
    }

    fn frame(&mut self, events: Vec<Event>) {
        let raw = RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1600.0, 1000.0))), events, ..Default::default() };
        self.ctx
            .run_ui(raw, |ui| {
                self.app.logic(ui.ctx());
                self.app.ui(ui);
            })
            .textures_delta
            .clear();
    }

    fn frames(&mut self, n: usize) {
        for _ in 0..n {
            self.frame(vec![]);
        }
    }

    fn bar(&self, bar: i64) -> i64 {
        let s = self.app.engine.session();
        s.tempo.samples_at_bar_beat(BarBeat { bar, beat: 1, tick: 0 }, s.sample_rate)
    }

    fn at(&self, at: i64) -> Pos2 {
        let [x0, y0, x1, y1] = self.app.edit_layout.timeline;
        let x = x_of(self.app.engine.session(), Rect::from_min_max(pos2(x0, y0), pos2(x1, y1)), at);
        pos2(x, self.app.edit_layout.rows[0].1[1] + 10.0)
    }

    fn press(&mut self, at: Pos2, pressed: bool) {
        self.frame(vec![
            Event::PointerMoved(at),
            Event::PointerButton { pos: at, button: PointerButton::Primary, pressed, modifiers: Default::default() },
        ]);
    }

    fn drag(&mut self, from: i64, to: i64) {
        let (a, b) = (self.at(from), self.at(to));
        self.press(a, true);
        for k in 1..=6 {
            self.frame(vec![Event::PointerMoved(a.lerp(b, k as f32 / 6.0))]);
        }
        self.press(b, false);
        self.frames(2);
    }

    fn click(&mut self, at: i64) {
        self.click_at(self.at(at));
    }

    fn click_at(&mut self, p: Pos2) {
        self.press(p, true);
        self.press(p, false);
        self.frames(2);
    }

    fn clips(&self) -> Vec<(i64, i64)> {
        self.app.engine.session().tracks[0].clips().iter().map(|c| (c.start, c.end())).collect()
    }
}

#[test]
fn a_pencil_click_on_an_empty_instrument_track_makes_a_one_bar_clip() {
    let mut r = Rig::new("instrument");
    let mid = (r.bar(3) + r.bar(4)) / 2;
    r.click(mid);
    assert_eq!(r.clips(), vec![(r.bar(3), r.bar(4))]);
    let s = r.app.engine.session();
    let clip = &s.tracks[0].clips()[0];
    assert!(matches!(&clip.content, ClipContent::Midi { sequence } if sequence.notes.is_empty()));
    assert_eq!(s.edit.selected_clips, vec![clip.id]);
}

#[test]
fn a_pencil_drag_makes_a_clip_over_the_bars_it_crosses() {
    let mut r = Rig::new("midi");
    let (from, to) = ((r.bar(2) + r.bar(3)) / 2, (r.bar(4) + r.bar(5)) / 2);
    r.drag(from, to);
    assert_eq!(r.clips(), vec![(r.bar(2), r.bar(5))]);
    r.app.run("edit.undo", json!({})).unwrap();
    r.drag(to, from);
    assert_eq!(r.clips(), vec![(r.bar(2), r.bar(5))]);
}

#[test]
fn a_pencil_drag_stops_at_the_next_clip_and_clicks_on_clips_add_nothing() {
    let mut r = Rig::new("instrument");
    r.click(r.bar(5));
    r.drag(r.bar(2), (r.bar(6) + r.bar(7)) / 2);
    assert_eq!(r.clips(), vec![(r.bar(2), r.bar(5)), (r.bar(5), r.bar(6))]);
    r.click((r.bar(2) + r.bar(3)) / 2);
    assert_eq!(r.clips().len(), 2);
}

#[test]
fn the_pencil_makes_no_clips_on_audio_tracks() {
    let mut r = Rig::new("audio");
    r.click(r.bar(3));
    r.drag(r.bar(2), r.bar(4));
    assert!(r.clips().is_empty());
}

/// Clicking the empty MIDI editor adds a clip spanning the selected bars at the insertion point and selects it.
#[test]
fn a_click_in_the_empty_midi_editor_adds_a_clip_at_the_insertion_point() {
    let mut r = Rig::new("instrument");
    r.app.ui.show_midi_editor = true;
    let track = r.app.engine.session().tracks[0].id.0;
    r.app.run("edit.select", json!({"tracks": [track], "start": (r.bar(3) + r.bar(4)) / 2})).unwrap();
    r.frames(4);
    assert_eq!(midi_editor::target_clip(&r.app), None);
    let body = r.ctx.data(|d| d.get_temp::<Rect>(Id::unique("soundcraft-midi-body"))).unwrap();
    r.click_at(body.center());
    assert_eq!(r.clips(), vec![(r.bar(3), r.bar(4))]);
    assert_eq!(midi_editor::target_clip(&r.app), Some(r.app.engine.session().tracks[0].clips()[0].id));
}

#[test]
fn a_pencil_click_on_a_clip_selects_it_for_the_midi_editor() {
    let mut r = Rig::new("instrument");
    r.app.ui.show_midi_editor = true;
    r.drag((r.bar(2) + r.bar(3)) / 2, (r.bar(3) + r.bar(4)) / 2);
    r.drag((r.bar(6) + r.bar(7)) / 2, (r.bar(7) + r.bar(8)) / 2);
    let ids: Vec<_> = r.app.engine.session().tracks[0].clips().iter().map(|c| c.id).collect();
    assert_eq!(r.clips(), vec![(r.bar(2), r.bar(4)), (r.bar(6), r.bar(8))]);
    assert_eq!(midi_editor::target_clip(&r.app), Some(ids[1]));
    r.click((r.bar(2) + r.bar(3)) / 2);
    assert_eq!(r.clips().len(), 2);
    assert_eq!(r.app.engine.session().edit.selected_clips, vec![ids[0]]);
    assert_eq!(midi_editor::target_clip(&r.app), Some(ids[0]));
}
