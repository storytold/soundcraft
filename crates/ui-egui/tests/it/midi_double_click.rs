//! Double-clicking a MIDI clip with the Grabber opens it in the MIDI Editor, as in Pro Tools.

use egui::{Event, Modifiers, PointerButton, Pos2, RawInput, Rect, pos2, vec2};
use serde_json::json;
use soundcraft_engine::model::ClipId;
use soundcraft_ui_egui::{Services, SoundApp, edit_window, midi_editor};

struct Rig {
    ctx: egui::Context,
    app: SoundApp,
}

impl Rig {
    fn new(tool: &str) -> Rig {
        let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
        app.engine.execute("edit.tool", &json!({"tool": tool})).unwrap();
        let mut r = Rig { ctx: egui::Context::default(), app };
        for _ in 0..4 {
            r.frame(vec![]);
        }
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

    /// The first clip on `track` and its rectangle on screen.
    fn clip(&self, track: &str) -> (ClipId, Rect) {
        let s = self.app.engine.session();
        let t = s.track_by_name(track).unwrap();
        let c = &t.clips()[0];
        let tl = edit_window::rect(self.app.edit_layout.timeline);
        let row = self.app.edit_layout.rows.iter().find(|(id, _)| *id == t.id.0).unwrap().1;
        let x0 = edit_window::x_of(s, tl, c.start).max(tl.min.x);
        let x1 = edit_window::x_of(s, tl, c.end()).min(tl.max.x);
        (c.id, Rect::from_min_max(pos2(x0, row[1]), pos2(x1, row[1] + t.height.points())))
    }

    fn double_click(&mut self, at: Pos2) {
        // A second of frames first: egui counts a click within 0.6 s of the one before last as a triple-click.
        for _ in 0..60 {
            self.frame(vec![]);
        }
        for pressed in [true, false, true, false] {
            self.frame(vec![
                Event::PointerMoved(at),
                Event::PointerButton { pos: at, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE },
            ]);
        }
        self.frame(vec![]);
    }

    fn editing(&self) -> Option<ClipId> {
        self.app.ui.show_midi_editor.then(|| midi_editor::target_clip(&self.app)).flatten()
    }
}

#[test]
fn double_clicking_a_midi_clip_with_the_grabber_opens_the_midi_editor() {
    let mut r = Rig::new("grabber");
    let (keys, at) = r.clip("Keys");
    assert_eq!(r.editing(), None);
    r.double_click(at.center());
    assert_eq!(r.editing(), Some(keys));
}

#[test]
fn the_smart_tool_opens_it_from_the_lower_half_and_selects_from_the_upper_half() {
    let mut r = Rig::new("smart");
    let (keys, at) = r.clip("Keys");
    r.double_click(pos2(at.center().x, at.top() + at.height() * 0.25));
    assert!(!r.app.ui.show_midi_editor);
    assert_eq!(r.app.engine.session().edit.selected_clips, vec![keys]);
    r.double_click(pos2(at.center().x, at.top() + at.height() * 0.75));
    assert_eq!(r.editing(), Some(keys));
}

#[test]
fn double_clicking_an_audio_clip_leaves_the_midi_editor_closed() {
    let mut r = Rig::new("grabber");
    let (kick, at) = r.clip("Kick");
    r.double_click(at.center());
    assert!(!r.app.ui.show_midi_editor);
    assert_eq!(r.app.engine.session().edit.selected_clips, vec![kick]);
}
