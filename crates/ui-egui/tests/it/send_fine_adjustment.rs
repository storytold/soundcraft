//! Fine adjustment of a send: Ctrl (Command on a Mac) makes a drag on a send's level, on the
//! Level and Pan sliders of its options menu, and on the pan knobs move the value a fraction as far.

use egui::accesskit::Role;
use egui::{Event, Modifiers, PointerButton, Pos2, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use serde_json::json;
use soundcraft_model::SendSlot;
use soundcraft_ui_egui::{MainWindow, Services, SoundApp};

fn harness() -> Harness<'static, SoundApp> {
    let mut engine = soundcraft_engine::Engine::default();
    engine.execute("track.new", &json!({"name": "Vox"})).unwrap();
    let vox = engine.session().tracks[0].id;
    engine.execute("mix.new_bus", &json!({"name": "Verb"})).unwrap();
    engine.execute("mix.send", &json!({"track": vox.0, "slot": 0, "bus": "Verb", "level_db": -10.0})).unwrap();
    let mut app = SoundApp::new(engine, None, Services::default());
    app.ui.window = MainWindow::Mix;
    let mut h = Harness::builder().with_size(vec2(1400.0, 900.0)).with_step_dt(1.0 / 60.0).build_ui_state(
        |ui, app: &mut SoundApp| {
            app.logic(&ui.ctx().clone());
            app.ui(ui);
        },
        app,
    );
    h.run_steps(4);
    h
}

fn send(h: &Harness<SoundApp>) -> SendSlot {
    h.state().engine.session().tracks[0].mixer.sends[0].clone().unwrap()
}

/// Puts the send back at -10 dB and centered, so that every drag starts from the same place.
fn reset(h: &mut Harness<SoundApp>) {
    let vox = h.state().engine.session().tracks[0].id;
    h.state_mut().engine.execute("mix.send_level", &json!({"track": vox.0, "slot": 0, "db": -10.0, "pan": 0.0})).unwrap();
    h.state_mut().engine.execute("mix.pan", &json!({"track": vox.0, "pan": 0.0})).unwrap();
    h.run_steps(2);
}

fn hold(h: &mut Harness<SoundApp>, modifiers: Modifiers) {
    h.event(Event::ModifiersChanged(modifiers));
    h.run_steps(1);
}

fn press(h: &mut Harness<SoundApp>, pos: Pos2, pressed: bool, modifiers: Modifiers) {
    h.event(Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers });
    h.run_steps(1);
}

fn move_to(h: &mut Harness<SoundApp>, pos: Pos2) {
    h.event(Event::PointerMoved(pos));
    h.run_steps(1);
}

/// Presses `from` and drags by `by` in eight steps with `modifiers` held; returns how far `read`
/// moved from where the press left it.
fn drag(h: &mut Harness<SoundApp>, from: Pos2, by: egui::Vec2, modifiers: Modifiers, read: fn(&Harness<SoundApp>) -> f32) -> f32 {
    reset(h);
    hold(h, modifiers);
    move_to(h, from);
    press(h, from, true, modifiers);
    let start = read(h);
    for i in 1..=8 {
        move_to(h, from + by * (i as f32 / 8.0));
    }
    press(h, from + by, false, modifiers);
    hold(h, Modifiers::NONE);
    read(h) - start
}

fn level(h: &Harness<SoundApp>) -> f32 {
    send(h).level_db
}

fn pan(h: &Harness<SoundApp>) -> f32 {
    send(h).pan
}

/// A vertical drag on a send's level moves it as little with Ctrl or Command held as with Shift.
#[test]
fn ctrl_dragging_a_sends_level_is_as_fine_as_shift_dragging_it() {
    let mut h = harness();
    let at = h.get_by_label("Send A level for Vox").rect().center();
    let by = vec2(0.0, -40.0);
    let plain = drag(&mut h, at, by, Modifiers::NONE, level);
    let shift = drag(&mut h, at, by, Modifiers::SHIFT, level);
    assert!(plain > 0.0, "a plain drag did not raise the send");
    assert!(shift > 0.0 && shift < plain * 0.2, "Shift-drag moved {shift} dB, a plain drag {plain} dB");
    for (name, modifiers) in [("Ctrl", Modifiers::CTRL), ("Command", Modifiers::COMMAND)] {
        let fine = drag(&mut h, at, by, modifiers, level);
        assert!((fine - shift).abs() < 1e-4, "{name}-drag moved {fine} dB, a Shift-drag {shift} dB");
    }
}

/// Opens the send's options menu with a right-click on its level.
fn options_menu(h: &mut Harness<SoundApp>) {
    let at = h.get_by_label("Send A level for Vox").rect().center();
    h.hover_at(at);
    for pressed in [true, false] {
        h.event(Event::PointerButton { pos: at, button: PointerButton::Secondary, pressed, modifiers: Modifiers::NONE });
    }
    h.run_steps(3);
    assert!(h.query_by_role_and_label(Role::CheckBox, "Pre-fader").is_some(), "the send's options menu is open");
}

/// The Level and Pan sliders of a send's options menu move a tenth as far while Ctrl or Command is held.
#[test]
fn the_send_options_sliders_move_a_tenth_as_far_with_ctrl_held() {
    let mut h = harness();
    options_menu(&mut h);
    let read: [(&str, fn(&Harness<SoundApp>) -> f32); 2] = [("Level", level), ("Pan", pan)];
    for (label, read) in read {
        let rail = h.get_by_role_and_label(Role::Slider, label).rect().center();
        let plain = drag(&mut h, rail, vec2(30.0, 0.0), Modifiers::NONE, read);
        let fine = drag(&mut h, rail, vec2(30.0, 0.0), Modifiers::COMMAND, read);
        assert!(plain > 0.0, "{label}: a plain drag did not move the value");
        let ratio = fine / plain;
        assert!((0.08..=0.12).contains(&ratio), "{label}: Ctrl-drag moved {fine}, a plain drag {plain}");
        assert!(h.query_by_role_and_label(Role::CheckBox, "Pre-fader").is_some(), "{label}: the drags closed the menu");
    }
}

/// A Ctrl or Command press on the Level or Pan rail of a send's options menu leaves the value
/// where it is, where a plain press sets the value under the pointer.
#[test]
fn a_ctrl_press_on_a_send_options_rail_leaves_the_value_where_it_is() {
    let mut h = harness();
    options_menu(&mut h);
    let read: [(&str, fn(&Harness<SoundApp>) -> f32); 2] = [("Level", level), ("Pan", pan)];
    for (label, read) in read {
        let at = h.get_by_role_and_label(Role::Slider, label).rect().left_center() + vec2(4.0, 0.0);
        for (name, modifiers) in [("plain", Modifiers::NONE), ("Ctrl", Modifiers::CTRL), ("Command", Modifiers::COMMAND)] {
            reset(&mut h);
            let start = read(&h);
            hold(&mut h, modifiers);
            move_to(&mut h, at);
            press(&mut h, at, true, modifiers);
            let moved = read(&h) - start;
            // Move on before the release, so that the press ends as a drag that leaves the menu open.
            for i in 1..=4 {
                move_to(&mut h, at + vec2(2.0 * i as f32, 0.0));
            }
            press(&mut h, at + vec2(8.0, 0.0), false, modifiers);
            hold(&mut h, Modifiers::NONE);
            assert!(h.query_by_role_and_label(Role::CheckBox, "Pre-fader").is_some(), "{label}: the {name} drag closed the menu");
            if modifiers.is_none() {
                assert!(moved.abs() > 0.5, "{label}: a plain press near the rail's left end moved the value by {moved}");
            } else {
                assert!(moved.abs() < 1e-4, "{label}: a {name} press moved the value by {moved}");
            }
        }
    }
}

fn track_pan(h: &Harness<SoundApp>) -> f32 {
    h.state().engine.session().tracks[0].mixer.pan[0]
}

/// The Send window's pan knob, and a track's pan knob, which is the same widget, turn a tenth as
/// far while Ctrl or Command is held.
#[test]
fn pan_knobs_turn_a_tenth_as_far_with_ctrl_held() {
    let mut h = harness();
    h.get_by_label("Send A level for Vox").click();
    h.run_steps(3);
    let knobs: [(&str, fn(&Harness<SoundApp>) -> f32); 2] =
        [("Send pan (drag; double-click centres)", pan), ("Pan (drag; double-click centres)", track_pan)];
    for (label, read) in knobs {
        let at = h.get_by_role_and_label(Role::Slider, label).rect().center();
        let plain = drag(&mut h, at, vec2(0.0, -30.0), Modifiers::NONE, read);
        for (name, modifiers) in [("Ctrl", Modifiers::CTRL), ("Command", Modifiers::COMMAND)] {
            let fine = drag(&mut h, at, vec2(0.0, -30.0), modifiers, read);
            assert!(plain > 0.0, "{label}: a plain drag did not turn the knob");
            let ratio = fine / plain;
            assert!((0.08..=0.12).contains(&ratio), "{label}: {name}-drag turned it {fine}, a plain drag {plain}");
        }
    }
    assert!(h.state().ui.send_window.is_some(), "the drags closed the Send window");
}

/// The Level value box of a send's options menu still shows its value in dB.
#[test]
fn the_send_options_level_box_shows_db() {
    let mut h = harness();
    options_menu(&mut h);
    let shown = h.get_by_role_and_label(Role::SpinButton, "Level").value().unwrap_or_default();
    assert!(shown.starts_with("-10") && shown.ends_with(" dB"), "the Level box shows {shown:?}");
}
