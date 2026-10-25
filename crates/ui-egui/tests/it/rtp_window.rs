use egui::accesskit::Role;
use egui::{Event, Modifiers, PointerButton, Pos2, vec2};
use egui_kittest::{Harness, kittest::Queryable};
use serde_json::json;
use soundcraft_ui_egui::{Services, SoundApp, menus};

fn press(h: &mut Harness<'_, SoundApp>, pos: Pos2, pressed: bool) {
    h.event(Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE });
    h.run_steps(2);
}

/// #444: the MIDI Real-Time Properties window submitted all four properties from its form on any
/// change, so editing Velocity reset the target's Transpose, Duration and Delay to the form's
/// (stale) defaults — and the form never showed the target's own values.
#[test]
fn editing_rtp_velocity_leaves_the_other_properties_alone() {
    let mut app = SoundApp::new(soundcraft_engine::Engine::default(), None, Services::default());
    app.engine.execute("track.new", &json!({"kind": "instrument", "name": "Synth"})).unwrap();
    let t = app.engine.session().tracks.last().unwrap().id;
    app.engine.execute("event.midi_rtp", &json!({"track": t.0, "transpose": 7, "duration": 50.0})).unwrap();
    app.run("edit.select", json!({"tracks": [t.0], "exact": true})).unwrap();
    menus::invoke_menu(&mut app, "window.rtp", "Event > MIDI Real-Time Properties");

    let mut h = Harness::builder().with_size(vec2(1600.0, 1000.0)).build_ui_state(
        |ui, app: &mut SoundApp| {
            let ctx = ui.ctx().clone();
            app.logic(&ctx);
            app.ui(ui);
        },
        app,
    );
    h.run_steps(3);
    assert!(h.state().ui.show_rtp, "the window did not open");
    let rtp = |h: &Harness<'_, SoundApp>, key: &str, d: f64| h.state().engine.session().edit.value(&format!("rtp.{}.{key}", t.0), d);

    // Drag the Velocity slider right: the velocity changes, the rest must stay as the track had it.
    let rect = h.get_by_role_and_label(Role::Slider, "Velocity +").rect();
    let from = rect.center();
    let to = from + vec2(30.0, 0.0);
    h.event(Event::PointerMoved(from));
    h.run_steps(1);
    press(&mut h, from, true);
    h.event(Event::PointerMoved(to));
    h.run_steps(2);
    press(&mut h, to, false);
    assert!(rtp(&h, "velocity", 0.0) > 0.0, "the velocity drag did not reach the engine");
    assert_eq!(rtp(&h, "transpose", 0.0), 7.0, "editing Velocity reset Transpose");
    assert_eq!(rtp(&h, "duration", 100.0), 50.0, "editing Velocity reset Duration");
    assert_eq!(rtp(&h, "delay", 0.0), 0.0, "editing Velocity reset Delay");

    // The form shows the target track's values, not its defaults.
    assert_eq!(h.state().ops.rtp_transpose, 7, "the form did not show the track's Transpose");
    assert_eq!(h.state().ops.rtp_duration, 50.0, "the form did not show the track's Duration");
}
