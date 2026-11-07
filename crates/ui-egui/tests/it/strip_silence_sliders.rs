use egui::accesskit::Role;
use egui::{Event, Modifiers, PointerButton, Pos2, vec2};
use egui_kittest::{Harness, kittest::Queryable};
use serde_json::json;
use soundcraft_ui_egui::dialogs::Dialog;
use soundcraft_ui_egui::{Services, SoundApp, menus};

const SLIDERS: [&str; 4] = ["Threshold dB", "Min strip duration ms", "Clip start pad ms", "Clip end pad ms"];

/// Selects the whole Kick track and opens Edit > Strip Silence from the menu.
fn strip_silence_dialog() -> Harness<'static, SoundApp> {
    let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
    let track = app.engine.session().track_by_name("Kick").unwrap().id;
    let end = app.engine.session().content_end();
    app.run("edit.select", json!({"tracks": [track.0], "start": 0, "end": end})).unwrap();
    menus::invoke_menu(&mut app, "edit.strip_silence", "Edit > Strip Silence");
    let mut h = Harness::builder().with_size(vec2(1600.0, 1000.0)).build_ui_state(
        |ui, app: &mut SoundApp| {
            let ctx = ui.ctx().clone();
            app.logic(&ctx);
            app.ui(ui);
        },
        app,
    );
    h.run_steps(3);
    h
}

/// The value of the slider labeled `label`, in the order the window shows them.
fn value(h: &Harness<'_, SoundApp>, label: &str) -> f64 {
    let Some(Dialog::StripSilence { threshold, min_ms, pre_ms, post_ms }) = h.state().dialogs.open else { panic!("Strip Silence is not open") };
    let i = SLIDERS.iter().position(|s| *s == label).unwrap();
    [threshold, min_ms, pre_ms, post_ms][i]
}

fn hold(h: &mut Harness<'_, SoundApp>, modifiers: Modifiers) {
    h.event(Event::ModifiersChanged(modifiers));
    h.run_steps(1);
}

fn press(h: &mut Harness<'_, SoundApp>, pos: Pos2, pressed: bool, modifiers: Modifiers) {
    h.event(Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers });
    h.run_steps(1);
}

fn move_to(h: &mut Harness<'_, SoundApp>, pos: Pos2) {
    h.event(Event::PointerMoved(pos));
    h.run_steps(1);
}

/// Presses the middle of a slider's rail (`role` `Slider`) or value box (`SpinButton`) and drags
/// `dx` points right in six steps, with `modifiers` held; returns how far the value moved from
/// where the press put it.
fn drag_on(h: &mut Harness<'_, SoundApp>, role: Role, label: &str, dx: f32, modifiers: Modifiers) -> f64 {
    hold(h, modifiers);
    let from = h.get_by_role_and_label(role, label).rect().center();
    move_to(h, from);
    press(h, from, true, modifiers);
    let start = value(h, label);
    for i in 1..=6 {
        move_to(h, from + vec2(dx * i as f32 / 6.0, 0.0));
    }
    let moved = value(h, label) - start;
    press(h, from + vec2(dx, 0.0), false, modifiers);
    assert_eq!(value(h, label) - start, moved, "{label}: releasing the button moved the value");
    hold(h, Modifiers::NONE);
    moved
}

fn drag(h: &mut Harness<'_, SoundApp>, label: &str, dx: f32, modifiers: Modifiers) -> f64 {
    drag_on(h, Role::Slider, label, dx, modifiers)
}

#[test]
fn strip_silence_sliders_move_a_tenth_as_far_with_ctrl_held() {
    let mut h = strip_silence_dialog();
    for label in SLIDERS {
        let plain = drag(&mut h, label, 30.0, Modifiers::NONE);
        let fine = drag(&mut h, label, 30.0, Modifiers::COMMAND);
        assert!(plain > 0.0, "{label}: a plain drag did not move the value");
        let ratio = fine / plain;
        assert!((0.08..=0.12).contains(&ratio), "{label}: Ctrl-drag moved {fine}, a plain drag {plain}");
    }
}

#[test]
fn ctrl_dragging_the_value_box_moves_a_tenth_as_far_too() {
    let mut h = strip_silence_dialog();
    for label in ["Threshold dB", "Clip start pad ms"] {
        let plain = drag_on(&mut h, Role::SpinButton, label, 20.0, Modifiers::NONE);
        let fine = drag_on(&mut h, Role::SpinButton, label, 20.0, Modifiers::COMMAND);
        assert!(plain > 0.0, "{label}: a plain drag on the value box did not move the value");
        let ratio = fine / plain;
        assert!((0.07..=0.13).contains(&ratio), "{label}: Ctrl-drag on the value box moved {fine}, a plain drag {plain}");
    }
}

#[test]
fn the_value_box_shows_the_tenths_a_ctrl_drag_reaches() {
    let mut h = strip_silence_dialog();
    drag(&mut h, "Threshold dB", 7.0, Modifiers::COMMAND);
    let v = value(&h, "Threshold dB");
    let shown = h.get_by_role_and_label(Role::SpinButton, "Threshold dB").value().unwrap_or_default();
    assert_eq!(shown, format!("{v:.1}"));
}

#[test]
fn pressing_and_releasing_ctrl_during_a_drag_keeps_the_value() {
    let mut h = strip_silence_dialog();
    let label = "Threshold dB";
    let plain = drag(&mut h, label, 30.0, Modifiers::NONE);
    let from = h.get_by_role_and_label(Role::Slider, label).rect().center();
    move_to(&mut h, from);
    press(&mut h, from, true, Modifiers::NONE);
    move_to(&mut h, from + vec2(10.0, 0.0));
    let before = value(&h, label);
    hold(&mut h, Modifiers::COMMAND);
    assert_eq!(value(&h, label), before, "pressing Ctrl moved the value");
    for i in 1..=6 {
        move_to(&mut h, from + vec2(10.0 + 5.0 * i as f32, 0.0));
    }
    let fine = value(&h, label) - before;
    assert!((0.08..=0.12).contains(&(fine / plain)), "Ctrl-drag moved {fine}, a plain drag {plain}");
    let held = value(&h, label);
    hold(&mut h, Modifiers::NONE);
    assert_eq!(value(&h, label), held, "releasing Ctrl moved the value back to the pointer");
    press(&mut h, from + vec2(40.0, 0.0), false, Modifiers::NONE);
    assert_eq!(value(&h, label), held, "releasing the button moved the value back to the pointer");
}

#[test]
fn pressing_ctrl_during_a_drag_on_the_value_box_keeps_the_value() {
    let mut h = strip_silence_dialog();
    let label = "Threshold dB";
    let plain = drag_on(&mut h, Role::SpinButton, label, 30.0, Modifiers::NONE);
    let held = drag_on(&mut h, Role::SpinButton, label, 30.0, Modifiers::COMMAND);
    assert!(held < plain / 2.0, "a Ctrl-drag on the value box moved {held}, a plain drag {plain}");
    let from = h.get_by_role_and_label(Role::SpinButton, label).rect().center();
    move_to(&mut h, from);
    press(&mut h, from, true, Modifiers::NONE);
    move_to(&mut h, from + vec2(10.0, 0.0));
    let before = value(&h, label);
    hold(&mut h, Modifiers::COMMAND);
    assert_eq!(value(&h, label), before, "pressing Ctrl moved the value");
    for i in 1..=6 {
        move_to(&mut h, from + vec2(10.0 + 5.0 * i as f32, 0.0));
    }
    let fine = value(&h, label) - before;
    // The rest of the drag moves as far as a drag with Ctrl held from the start.
    assert!((0.8..=1.2).contains(&(fine / held)), "after Ctrl, the value box moved {fine}; a Ctrl-drag moves {held}");
    press(&mut h, from + vec2(40.0, 0.0), true, Modifiers::COMMAND);
    press(&mut h, from + vec2(40.0, 0.0), false, Modifiers::COMMAND);
    hold(&mut h, Modifiers::NONE);
}
