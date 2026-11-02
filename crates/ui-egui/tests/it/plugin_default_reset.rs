use egui::accesskit::Role;
use egui::{Event, Id, Modifiers, PointerButton, Pos2, Rect, vec2};
use egui_kittest::{Harness, kittest::Queryable};
use serde_json::json;
use soundcraft_dsp::ParamInfo;
use soundcraft_model::TrackId;
use soundcraft_ui_egui::{Services, SoundApp};
use std::collections::BTreeMap;

fn harness(app: SoundApp) -> Harness<'static, SoundApp> {
    let mut h = Harness::builder().with_size(vec2(1600.0, 1000.0)).build_ui_state(
        |ui, app: &mut SoundApp| {
            let ctx = ui.ctx().clone();
            app.logic(&ctx);
            app.ui(ui);
        },
        app,
    );
    h.run_steps(4);
    h
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

/// Presses at `at` with Alt held, moves `dx` points right in six steps, and releases; `dx` of 0
/// is an Alt-click.
fn alt_press(h: &mut Harness<'_, SoundApp>, at: Pos2, dx: f32) {
    hold(h, Modifiers::ALT);
    move_to(h, at);
    press(h, at, true, Modifiers::ALT);
    for i in 1..=6 {
        if dx != 0.0 {
            move_to(h, at + vec2(dx * i as f32 / 6.0, 0.0));
        }
    }
    press(h, at + vec2(dx, 0.0), false, Modifiers::ALT);
    hold(h, Modifiers::NONE);
    h.run_steps(2);
}

/// A point near the left end of `rail`, where a plain click would set a value far from the
/// parameter's default.
fn near_left(rail: Rect) -> Pos2 {
    rail.left_center() + vec2(3.0, 0.0)
}

/// The rails of the unlabeled sliders inside the window with id `window`, top to bottom.
fn window_sliders(h: &Harness<'_, SoundApp>, window: Id) -> Vec<Rect> {
    let area = h.ctx.memory(|m| m.area_rect(window)).unwrap();
    let mut rails: Vec<Rect> = h.query_all_by_role(Role::Slider).map(|n| n.rect()).filter(|r| area.contains_rect(*r)).collect();
    rails.sort_by(|a, b| a.top().total_cmp(&b.top()));
    rails
}

fn param(plugin: &str, id: &str) -> &'static ParamInfo {
    soundcraft_dsp::plugin_info(plugin).unwrap().params.iter().find(|p| p.id == id).unwrap()
}

fn insert_param(h: &Harness<'_, SoundApp>, track: TrackId, slot: usize, p: &ParamInfo) -> f32 {
    let ins = h.state().engine.session().track(track).unwrap().mixer.inserts[slot].as_ref();
    ins.unwrap().params.get(p.id).copied().unwrap_or(p.default)
}

fn undo_steps(h: &Harness<'_, SoundApp>) -> usize {
    h.state().engine.undo_history().len()
}

/// Kick's EQ 7-Band window, with Input at 6 dB and HPF Slope on its last choice.
fn kick_eq() -> (Harness<'static, SoundApp>, TrackId) {
    let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
    let kick = app.engine.session().track_by_name("Kick").unwrap().id;
    let slope = param("eq_7band", "hpf_slope");
    app.run("mix.insert_param", json!({"track": kick.0, "slot": 0, "param": "input_gain", "value": 6.0})).unwrap();
    app.run("mix.insert_param", json!({"track": kick.0, "slot": 0, "param": "hpf_slope", "value": slope.max})).unwrap();
    app.engine.end_merge();
    app.ui.plugin_windows = vec![(kick, 0)];
    (harness(app), kick)
}

#[test]
fn alt_click_returns_a_plugin_window_slider_to_its_default_in_one_undo_step() {
    let (mut h, kick) = kick_eq();
    let input = param("eq_7band", "input_gain");
    assert_ne!(input.default, 6.0);
    let rail = window_sliders(&h, Id::unique(("plugin", kick.0, 0usize)))[0];
    let steps = undo_steps(&h);
    alt_press(&mut h, near_left(rail), 0.0);
    assert_eq!(insert_param(&h, kick, 0, input), input.default, "Alt-click did not return Input to its default");
    assert_eq!(undo_steps(&h), steps + 1, "Alt-click is not one undo step");
    h.state_mut().engine.execute("edit.undo", &json!({})).unwrap();
    assert_eq!(insert_param(&h, kick, 0, input), 6.0, "Undo did not bring Input back");
}

#[test]
fn alt_drag_leaves_a_plugin_window_slider_where_it_is() {
    let (mut h, kick) = kick_eq();
    let input = param("eq_7band", "input_gain");
    let rail = window_sliders(&h, Id::unique(("plugin", kick.0, 0usize)))[0];
    let steps = undo_steps(&h);
    alt_press(&mut h, near_left(rail), 40.0);
    assert_eq!(insert_param(&h, kick, 0, input), 6.0, "an Alt-drag moved Input");
    assert_eq!(undo_steps(&h), steps, "an Alt-drag added an undo step");
}

#[test]
fn alt_click_returns_a_plugin_window_choice_to_its_default_without_opening_the_list() {
    let (mut h, kick) = kick_eq();
    let slope = param("eq_7band", "hpf_slope");
    assert_ne!(slope.default, slope.max);
    let row = h.get_by_label("HPF Slope").rect();
    let combo = h.query_all_by_role(Role::ComboBox).map(|n| n.rect()).find(|r| (r.center().y - row.center().y).abs() < 4.0).unwrap();
    alt_press(&mut h, combo.center(), 0.0);
    assert_eq!(insert_param(&h, kick, 0, slope), slope.default, "Alt-click did not return HPF Slope to its default");
    assert!(!egui::Popup::is_any_open(&h.ctx), "Alt-click opened the list");
}

#[test]
fn alt_click_returns_clip_effects_sliders_and_value_boxes_to_their_defaults() {
    let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
    let s = app.engine.session();
    let clip = s.track_by_name("Kick").unwrap().clips().iter().find(|c| c.is_audio()).unwrap().id;
    app.run("edit.select", json!({"clips": [clip.0]})).unwrap();
    app.run("clip.effects_set", json!({"clips": [clip.0], "params": {"eq.input_gain": 6.0, "comp.threshold": -6.0}})).unwrap();
    app.engine.end_merge();
    app.ui.show_clip_effects = true;
    let mut h = harness(app);
    for (label, key, p, on_box) in
        [("Input", "eq.input_gain", param("eq_7band", "input_gain"), false), ("Threshold", "comp.threshold", param("compressor", "threshold"), true)]
    {
        let key = format!("clip_fx.{}.{key}", clip.0);
        let read = |h: &Harness<'_, SoundApp>| h.state().engine.session().edit.values.get(&key).copied();
        assert_ne!(read(&h), Some(f64::from(p.default)));
        let rail = h.get_by_role_and_label(Role::Slider, label).rect();
        let at = if on_box {
            h.query_all_by_role(Role::SpinButton)
                .map(|n| n.rect())
                .find(|r| r.left() > rail.right() && (r.center().y - rail.center().y).abs() < 4.0)
                .unwrap()
                .center()
        } else {
            near_left(rail)
        };
        // A plain click on the value box gives it the keyboard focus for typing.
        let focus = h.ctx.memory(|m| m.focused());
        let steps = undo_steps(&h);
        alt_press(&mut h, at, 0.0);
        assert_eq!(read(&h), Some(f64::from(p.default)), "Alt-click did not return {label} to its default");
        assert_eq!(undo_steps(&h), steps + 1, "Alt-click on {label} is not one undo step");
        assert_eq!(h.ctx.memory(|m| m.focused()), focus, "Alt-click opened {label}'s value box for typing");
    }
}

/// An Alt-click on a clip effects value box that is open for typing is an undo step of its own:
/// one undo brings back the typed value, not the value from before the typing.
#[test]
fn alt_click_after_typing_in_a_clip_effects_value_box_is_its_own_undo_step() {
    let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
    let s = app.engine.session();
    let clip = s.track_by_name("Kick").unwrap().clips().iter().find(|c| c.is_audio()).unwrap().id;
    app.run("edit.select", json!({"clips": [clip.0]})).unwrap();
    app.run("clip.effects_set", json!({"clips": [clip.0], "params": {"comp.threshold": -6.0}})).unwrap();
    app.engine.end_merge();
    app.ui.show_clip_effects = true;
    let mut h = harness(app);
    let p = param("compressor", "threshold");
    let key = format!("clip_fx.{}.comp.threshold", clip.0);
    let read = |h: &Harness<'_, SoundApp>| h.state().engine.session().edit.values.get(&key).copied();
    let rail = h.get_by_role_and_label(Role::Slider, "Threshold").rect();
    let at = h
        .query_all_by_role(Role::SpinButton)
        .map(|n| n.rect())
        .find(|r| r.left() > rail.right() && (r.center().y - rail.center().y).abs() < 4.0)
        .unwrap()
        .center();
    move_to(&mut h, at);
    press(&mut h, at, true, Modifiers::NONE);
    press(&mut h, at, false, Modifiers::NONE);
    h.run_steps(2);
    assert!(h.ctx.egui_wants_keyboard_input(), "a plain click did not open Threshold's value box for typing");
    let steps = undo_steps(&h);
    h.event(Event::Key { key: egui::Key::A, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::COMMAND });
    h.event(Event::Text("-12".into()));
    h.run_steps(2);
    assert_eq!(read(&h), Some(-12.0), "typing did not set Threshold");
    alt_press(&mut h, at, 0.0);
    assert_eq!(read(&h), Some(f64::from(p.default)), "Alt-click did not return Threshold to its default");
    assert_eq!(undo_steps(&h), steps + 2, "the typed value and the Alt-click are not two undo steps");
    h.state_mut().engine.execute("edit.undo", &json!({})).unwrap();
    assert_eq!(read(&h), Some(-12.0), "Undo did not bring back the typed value");
}

#[test]
fn alt_click_returns_audiosuite_parameter_sliders_to_their_defaults() {
    let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
    app.ui.audiosuite = Some("compressor".into());
    let mut h = harness(app);
    let read = |h: &Harness<'_, SoundApp>| -> BTreeMap<String, f32> {
        h.ctx.data(|d| d.get_temp(Id::unique(("as_params", "compressor".to_string())))).unwrap_or_default()
    };
    let threshold = param("compressor", "threshold");
    let rail = h.get_by_role_and_label(Role::Slider, "Threshold").rect();
    move_to(&mut h, near_left(rail));
    press(&mut h, near_left(rail), true, Modifiers::NONE);
    press(&mut h, near_left(rail), false, Modifiers::NONE);
    assert_ne!(read(&h)["threshold"], threshold.default, "a plain click did not move Threshold");
    alt_press(&mut h, near_left(rail), 0.0);
    assert_eq!(read(&h)["threshold"], threshold.default, "Alt-click did not return Threshold to its default");
}
