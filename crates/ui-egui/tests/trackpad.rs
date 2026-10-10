use egui::{Event, Modifiers, MouseWheelUnit, Pos2, TouchPhase, Vec2, pos2, vec2};
use egui_kittest::Harness;
use serde_json::json;
use soundcraft_ui_egui::{Services, SoundApp};

fn harness() -> Harness<'static, SoundApp> {
    let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
    app.run("view.zoom_set", json!({"samples_per_px": 1000})).unwrap();
    app.run("view.scroll", json!({"to": 200_000})).unwrap();
    let mut h = Harness::builder().with_size(vec2(1600.0, 600.0)).build_ui_state(
        |ui, app: &mut SoundApp| {
            let ctx = ui.ctx().clone();
            app.logic(&ctx);
            app.ui(ui);
        },
        app,
    );
    for _ in 0..6 {
        h.step();
    }
    h
}

fn point(h: &Harness<'_, SoundApp>, ruler: bool) -> Pos2 {
    let r = h.state().edit_layout.timeline;
    pos2(r[0] + 200.0, if ruler { r[1] - 8.0 } else { r[1] + 20.0 })
}

fn wheel(h: &mut Harness<'_, SoundApp>, p: Pos2, phase: TouchPhase, delta: Vec2, modifiers: Modifiers) {
    h.input_mut().events.extend([
        Event::PointerMoved(p),
        Event::ModifiersChanged(modifiers),
        Event::MouseWheel { unit: MouseWheelUnit::Point, delta, phase, modifiers },
    ]);
    h.step();
}

#[test]
fn pinch_on_the_ruler_preserves_the_sample_under_the_pointer() {
    let mut h = harness();
    let p = point(&h, true);
    let anchor = 200_000.0 + 200.0 * 1000.0;
    for factor in [1.25, 0.8] {
        h.input_mut().events.extend([Event::PointerMoved(p), Event::Zoom(factor)]);
        h.step();
        let z = &h.state().engine.session().edit.zoom;
        assert!((z.scroll as f64 + 200.0 * z.samples_per_px - anchor).abs() <= 0.5);
    }
    assert!((h.state().engine.session().edit.zoom.samples_per_px - 1000.0).abs() < 0.001);
    assert_eq!(h.state().edit_layout.scroll_y, 0.0);
}

#[test]
fn two_finger_scroll_accepts_diagonals_and_small_momentum_deltas() {
    let mut h = harness();
    let p = point(&h, false);
    wheel(&mut h, p, TouchPhase::Start, Vec2::ZERO, Modifiers::NONE);
    for delta in [vec2(-20.0, -15.0), vec2(-10.0, -8.0), vec2(-2.0, -1.0), vec2(-0.25, -0.25)] {
        wheel(&mut h, p, TouchPhase::Move, delta, Modifiers::NONE);
    }
    wheel(&mut h, p, TouchPhase::End, Vec2::ZERO, Modifiers::NONE);
    assert_eq!(h.state().engine.session().edit.zoom.scroll, 232_250);
    assert!((h.state().edit_layout.scroll_y - 24.25).abs() < 0.01);
    // The platform can deliver momentum movement after the finger-contact phase ended.
    wheel(&mut h, p, TouchPhase::Move, vec2(-0.25, -0.25), Modifiers::NONE);
    assert_eq!(h.state().engine.session().edit.zoom.scroll, 232_500);
    assert!((h.state().edit_layout.scroll_y - 24.5).abs() < 0.01);
    wheel(&mut h, p, TouchPhase::End, Vec2::ZERO, Modifiers::NONE);
    let scroll = h.state().engine.session().edit.zoom.scroll;
    for _ in 0..8 {
        h.step();
    }
    assert_eq!(h.state().engine.session().edit.zoom.scroll, scroll);
}

#[test]
fn shift_scroll_pans_horizontally_without_moving_tracks() {
    let mut h = harness();
    let p = point(&h, true);
    let shift = Modifiers { shift: true, ..Modifiers::NONE };
    wheel(&mut h, p, TouchPhase::Start, Vec2::ZERO, shift);
    wheel(&mut h, p, TouchPhase::Move, vec2(0.0, -12.0), shift);
    wheel(&mut h, p, TouchPhase::End, Vec2::ZERO, shift);
    assert_eq!(h.state().engine.session().edit.zoom.scroll, 212_000);
    assert_eq!(h.state().edit_layout.scroll_y, 0.0);
}

#[test]
fn command_scroll_zooms_continuously_instead_of_scrolling_tracks() {
    let mut h = harness();
    let p = point(&h, false);
    let cmd = Modifiers { command: true, mac_cmd: true, ..Modifiers::NONE };
    wheel(&mut h, p, TouchPhase::Start, Vec2::ZERO, cmd);
    wheel(&mut h, p, TouchPhase::Move, vec2(0.0, 12.0), cmd);
    let z = &h.state().engine.session().edit.zoom;
    assert!(z.samples_per_px < 1000.0 && z.samples_per_px > 500.0);
    assert!((z.scroll as f64 + 200.0 * z.samples_per_px - 400_000.0).abs() <= 0.5);
    assert_eq!(h.state().edit_layout.scroll_y, 0.0);
}

#[test]
fn gestures_over_other_panels_do_not_move_the_edit_timeline() {
    let mut h = harness();
    let p = pos2(20.0, 300.0);
    h.input_mut().events.extend([Event::PointerMoved(p), Event::Zoom(2.0)]);
    h.step();
    wheel(&mut h, p, TouchPhase::Start, Vec2::ZERO, Modifiers::NONE);
    wheel(&mut h, p, TouchPhase::Move, vec2(-20.0, 0.0), Modifiers::NONE);
    assert_eq!(h.state().engine.session().edit.zoom.scroll, 200_000);
    assert_eq!(h.state().engine.session().edit.zoom.samples_per_px, 1000.0);
}

#[test]
fn manual_panning_during_playback_holds_the_view_off_the_playhead() {
    let mut h = harness();
    h.state_mut().run("transport.play", json!({"from": 240_000})).unwrap();
    h.step();
    let p = point(&h, true);
    wheel(&mut h, p, TouchPhase::Start, Vec2::ZERO, Modifiers::NONE);
    wheel(&mut h, p, TouchPhase::Move, vec2(-1000.0, 0.0), Modifiers::NONE);
    wheel(&mut h, p, TouchPhase::End, Vec2::ZERO, Modifiers::NONE);
    let scroll = h.state().engine.session().edit.zoom.scroll;
    assert!(scroll > h.state().position());
    for _ in 0..10 {
        h.step();
    }
    assert!(h.state().is_playing());
    assert_eq!(h.state().engine.session().edit.zoom.scroll, scroll);
}
