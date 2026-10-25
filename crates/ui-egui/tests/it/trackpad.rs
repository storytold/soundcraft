use egui::{Pos2, RawInput, Rect, pos2, vec2};
use serde_json::{Value, json};
use soundcraft_time::to_samples;
use soundcraft_ui_egui::{ControlRequest, Services, SoundApp};
use std::sync::mpsc::{Sender, channel};

struct Rig {
    ctx: egui::Context,
    app: SoundApp,
    tx: Sender<ControlRequest>,
    width: f32,
}

impl Rig {
    /// A demo app with a control channel, a chosen window width and deterministic non-kinetic scrolling for exact
    /// offset assertions.
    fn new(width: f32) -> Rig {
        let (tx, rx) = channel();
        let app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default()).with_control(rx);
        let mut r = Rig { ctx: egui::Context::default(), app, tx, width };
        // egui 0.37 scroll areas treat point wheel events as a trackpad: they rubber-band at an
        // edge and can coast after the gesture. These tests assert exact offsets, so the rig uses
        // deterministic scrolling (production keeps the kinetic feel).
        r.ctx.all_styles_mut(|s| {
            s.spacing.scroll.kinetic.decay_time = 0.0;
            s.spacing.scroll.kinetic.rubber_band = false;
        });
        r.frames(6);
        r
    }

    /// Run `n` frames through the raw-input hook and the app's logic and UI.
    fn frames(&mut self, n: usize) {
        for _ in 0..n {
            let mut raw = RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(self.width, 900.0))), ..Default::default() };
            self.app.raw_input_hook(&self.ctx, &mut raw);
            self.ctx
                .run_ui(raw, |ui| {
                    self.app.logic(ui.ctx());
                    self.app.ui(ui);
                })
                .textures_delta
                .clear();
        }
    }

    fn send(&mut self, method: &str, params: Value) {
        let (req, reply) = ControlRequest::new(method, params);
        self.tx.send(req).unwrap();
        self.frames(40);
        assert_eq!(reply.try_recv().unwrap()["ok"], true, "{method}");
    }

    fn view(&self) -> (f64, i64) {
        let z = self.app.engine.session().edit.zoom;
        (z.samples_per_px, z.scroll)
    }

    fn timeline(&self) -> Rect {
        let [x0, y0, x1, y1] = self.app.edit_layout.timeline;
        Rect::from_min_max(pos2(x0, y0), pos2(x1, y1))
    }
}

#[test]
fn swipes_and_wheels_scroll_with_the_fingers_on_one_axis_even_slowly() {
    let mut r = Rig::new(1400.0);
    let (tl, (spp, scroll)) = (r.timeline(), r.view());
    let (x, y) = (tl.center().x, tl.center().y);
    assert!(r.app.edit_layout.content_h - tl.height() > 60.0);
    r.send("ui.scroll", json!({"x": x, "y": y, "dx": -240.0, "dy": -60.0}));
    assert_eq!(r.view(), (spp, scroll + to_samples(240.0 * spp)));
    assert_eq!(r.app.edit_layout.scroll_y, 0.0);
    r.send("ui.scroll", json!({"x": x, "y": y, "dx": -30.0, "dy": -60.0}));
    assert_eq!(r.view(), (spp, scroll + to_samples(240.0 * spp)));
    assert_eq!(r.app.edit_layout.scroll_y, 60.0);
    r.send("ui.scroll", json!({"x": x, "y": y, "dy": 1.0, "unit": "line", "steps": 1}));
    assert!((r.app.edit_layout.scroll_y - 20.0).abs() < 1.0, "{}", r.app.edit_layout.scroll_y);
    r.send("ui.scroll", json!({"x": x, "y": tl.min.y - 20.0, "dx": 100_000.0}));
    assert_eq!(r.view(), (spp, 0));
    r.app.engine.execute("view.zoom_set", &json!({"samples_per_px": 0.25})).unwrap();
    let scroll_y = r.app.edit_layout.scroll_y;
    r.send("ui.scroll", json!({"x": x, "y": y, "dx": -12.0, "dy": -6.0, "steps": 24}));
    assert_eq!(r.view().1, 3);
    assert_eq!(r.app.edit_layout.scroll_y, scroll_y);
}

#[test]
fn pinch_and_cmd_scroll_zoom_around_the_pointer_and_shift_scroll_pans() {
    let mut r = Rig::new(1400.0);
    let tl = r.timeline();
    let (x, y) = (tl.min.x + 300.0, tl.center().y);
    let under = |r: &Rig| r.view().1 as f64 + 300.0 * r.view().0;
    let (spp, at) = (r.view().0, under(&r));
    r.send("ui.zoom", json!({"x": x, "y": y, "factor": 4.0}));
    assert!((r.view().0 * 4.0 / spp - 1.0).abs() < 1e-4, "{spp} -> {}", r.view().0);
    assert!((under(&r) - at).abs() < 8.0);
    r.send("ui.scroll", json!({"x": x, "y": y, "dy": 100.0, "cmd": true}));
    assert!(r.view().0 < spp / 4.0);
    assert!((under(&r) - at).abs() < 8.0);
    let ((spp, scroll), scroll_y) = (r.view(), r.app.edit_layout.scroll_y);
    r.send("ui.scroll", json!({"x": x, "y": y, "dy": -100.0, "shift": true}));
    assert_eq!(r.view(), (spp, scroll + to_samples(100.0 * spp)));
    assert_eq!(r.app.edit_layout.scroll_y, scroll_y);
}
