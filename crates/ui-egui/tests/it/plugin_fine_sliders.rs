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

/// Presses the middle of `rail` and drags `dx` points right in six steps with `modifiers` held,
/// calling `read` after each step; returns how far `read` moved from where the press put it.
fn drag(h: &mut Harness<'_, SoundApp>, rail: Rect, dx: f32, modifiers: Modifiers, read: &dyn Fn(&Harness<'_, SoundApp>) -> f64) -> f64 {
    hold(h, modifiers);
    let from = rail.center();
    move_to(h, from);
    press(h, from, true, modifiers);
    let start = read(h);
    for i in 1..=6 {
        move_to(h, from + vec2(dx * i as f32 / 6.0, 0.0));
        read(h);
    }
    let moved = read(h) - start;
    press(h, from + vec2(dx, 0.0), false, modifiers);
    assert_eq!(read(h) - start, moved, "releasing the button moved the value");
    hold(h, Modifiers::NONE);
    moved
}

/// A plain 30-point drag and a Ctrl-drag of the same length on `rail`; returns how far each
/// moved `read`.
fn plain_and_fine(h: &mut Harness<'_, SoundApp>, rail: Rect, read: &dyn Fn(&Harness<'_, SoundApp>) -> f64) -> (f64, f64) {
    let plain = drag(h, rail, 30.0, Modifiers::NONE, read);
    let fine = drag(h, rail, 30.0, Modifiers::COMMAND, read);
    (plain, fine)
}

fn assert_tenth(what: &str, (plain, fine): (f64, f64), within: std::ops::RangeInclusive<f64>) {
    assert!(plain > 0.0, "{what}: a plain drag did not move the value");
    assert!(within.contains(&(fine / plain)), "{what}: Ctrl-drag moved {fine}, a plain drag {plain}");
}

/// The value box on the same row as `rail`, to the rail's right.
fn value_box(h: &Harness<'_, SoundApp>, rail: Rect) -> Rect {
    h.query_all_by_role(Role::SpinButton).map(|n| n.rect()).find(|r| r.left() > rail.right() && (r.center().y - rail.center().y).abs() < 4.0).unwrap()
}

/// The rails of the unlabeled sliders inside the window with id `window`, top to bottom.
fn window_sliders(h: &Harness<'_, SoundApp>, window: Id) -> Vec<Rect> {
    let area = h.ctx.memory(|m| m.area_rect(window)).unwrap();
    let mut rails: Vec<Rect> = h.query_all_by_role(Role::Slider).map(|n| n.rect()).filter(|r| area.contains_rect(*r)).collect();
    rails.sort_by(|a, b| a.top().total_cmp(&b.top()));
    rails
}

/// The plug-in's parameters that its window shows as sliders, in order.
fn slider_params(plugin: &str) -> Vec<&'static ParamInfo> {
    soundcraft_dsp::plugin_info(plugin).unwrap().params.iter().filter(|p| p.choices.is_empty()).collect()
}

fn insert_param(h: &Harness<'_, SoundApp>, track: TrackId, slot: usize, p: &ParamInfo) -> f64 {
    let tr = h.state().engine.session().track(track).unwrap();
    let ins = if slot == soundcraft_mix::INSTRUMENT_SLOT { tr.instrument.as_ref() } else { tr.mixer.inserts[slot].as_ref() };
    f64::from(ins.unwrap().params.get(p.id).copied().unwrap_or(p.default))
}

#[test]
fn plugin_window_sliders_move_a_tenth_as_far_with_ctrl_held() {
    let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
    let kick = app.engine.session().track_by_name("Kick").unwrap().id;
    app.ui.plugin_windows = vec![(kick, 0)];
    let mut h = harness(app);
    let rails = window_sliders(&h, Id::unique(("plugin", kick.0, 0usize)));
    let params = slider_params("eq_7band");
    assert_eq!((params[0].id, params[2].id), ("input_gain", "hpf_freq"));
    for i in [0, 2] {
        let p = params[i];
        // Along the rail, so that a log-taper parameter compares as a linear one does.
        let read = move |h: &Harness<'_, SoundApp>| f64::from(p.to_normalized(insert_param(h, kick, 0, p) as f32));
        let others = |h: &Harness<'_, SoundApp>| params.iter().filter(|o| o.id != p.id).map(|o| insert_param(h, kick, 0, o)).collect::<Vec<_>>();
        let before = others(&h);
        assert_tenth(p.name, plain_and_fine(&mut h, rails[i], &read), 0.08..=0.12);
        assert_eq!(others(&h), before, "dragging {} moved another parameter", p.name);
    }
}

#[test]
fn the_control_key_alone_also_gives_a_fine_drag_on_a_plugin_slider() {
    let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
    let kick = app.engine.session().track_by_name("Kick").unwrap().id;
    app.ui.plugin_windows = vec![(kick, 0)];
    let mut h = harness(app);
    let rails = window_sliders(&h, Id::unique(("plugin", kick.0, 0usize)));
    let p = slider_params("eq_7band")[0];
    let read = move |h: &Harness<'_, SoundApp>| insert_param(h, kick, 0, p);
    let plain = drag(&mut h, rails[0], 30.0, Modifiers::NONE, &read);
    let fine = drag(&mut h, rails[0], 30.0, Modifiers::CTRL, &read);
    assert_tenth(p.name, (plain, fine), 0.08..=0.12);
}

/// Presses `rail`'s middle, drags 10 points, presses Ctrl, drags 30 more, releases Ctrl and then
/// the button. Each change of the key and the release leave `read` where it was, and the 30
/// points move it less than `plain` moved for the same distance.
fn toggle_ctrl_during_a_drag(h: &mut Harness<'_, SoundApp>, what: &str, rail: Rect, plain: f64, read: &dyn Fn(&Harness<'_, SoundApp>) -> f64) {
    let from = rail.center();
    move_to(h, from);
    press(h, from, true, Modifiers::NONE);
    move_to(h, from + vec2(10.0, 0.0));
    let before = read(h);
    hold(h, Modifiers::COMMAND);
    assert_eq!(read(h), before, "{what}: pressing Ctrl moved the value");
    for i in 1..=6 {
        move_to(h, from + vec2(10.0 + 5.0 * i as f32, 0.0));
        read(h);
    }
    let held = read(h);
    assert!(held > before && held - before < plain / 2.0, "{what}: 30 points with Ctrl moved {}, a plain drag {plain}", held - before);
    hold(h, Modifiers::NONE);
    assert_eq!(read(h), held, "{what}: releasing Ctrl moved the value");
    press(h, from + vec2(40.0, 0.0), false, Modifiers::NONE);
    assert_eq!(read(h), held, "{what}: releasing the button moved the value");
}

#[test]
fn pressing_and_releasing_ctrl_during_a_drag_keeps_the_value_of_a_logarithmic_slider() {
    let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
    let kick = app.engine.session().track_by_name("Kick").unwrap().id;
    app.ui.plugin_windows = vec![(kick, 0)];
    let mut h = harness(app);
    let rails = window_sliders(&h, Id::unique(("plugin", kick.0, 0usize)));
    let p = slider_params("eq_7band")[2];
    assert_eq!(p.id, "hpf_freq");
    let read = move |h: &Harness<'_, SoundApp>| f64::from(p.to_normalized(insert_param(h, kick, 0, p) as f32));
    let plain = drag(&mut h, rails[2], 30.0, Modifiers::NONE, &read);
    toggle_ctrl_during_a_drag(&mut h, p.name, rails[2], plain, &read);
}

#[test]
fn pressing_and_releasing_ctrl_during_a_drag_keeps_a_whole_number_slider_whole() {
    let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
    let keys = app.engine.session().track_by_name("Keys").unwrap().id;
    app.run("mix.instrument", json!({"track": keys.0, "plugin": "sampler"})).unwrap();
    app.ui.plugin_windows = vec![(keys, soundcraft_mix::INSTRUMENT_SLOT)];
    let mut h = harness(app);
    let rails = window_sliders(&h, Id::unique(("plugin", keys.0, soundcraft_mix::INSTRUMENT_SLOT)));
    let root = slider_params("sampler")[0];
    assert_eq!(root.id, "root_note");
    let read = move |h: &Harness<'_, SoundApp>| {
        let v = insert_param(h, keys, soundcraft_mix::INSTRUMENT_SLOT, root);
        assert_eq!(v.fract(), 0.0, "Root Note is {v} during a drag");
        v
    };
    let plain = drag(&mut h, rails[0], 30.0, Modifiers::NONE, &read);
    toggle_ctrl_during_a_drag(&mut h, root.name, rails[0], plain, &read);
}

#[test]
fn a_small_ctrl_drag_at_the_low_end_of_a_log_slider_moves_the_value() {
    let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
    let kick = app.engine.session().track_by_name("Kick").unwrap().id;
    app.ui.plugin_windows = vec![(kick, 1)];
    let mut h = harness(app);
    let rails = window_sliders(&h, Id::unique(("plugin", kick.0, 1usize)));
    let attack = slider_params("compressor")[2];
    assert_eq!(attack.id, "attack");
    let from = rails[2].left_center() + vec2(1.0, 0.0);
    hold(&mut h, Modifiers::COMMAND);
    move_to(&mut h, from);
    press(&mut h, from, true, Modifiers::COMMAND);
    let start = insert_param(&h, kick, 1, attack);
    assert_eq!(start, f64::from(attack.min), "the press did not put Attack at its lowest value");
    move_to(&mut h, from + vec2(1.0, 0.0));
    let moved = insert_param(&h, kick, 1, attack);
    press(&mut h, from + vec2(1.0, 0.0), false, Modifiers::COMMAND);
    assert!(moved > start, "a one-point Ctrl-drag left Attack at {moved}");
    assert_eq!(insert_param(&h, kick, 1, attack), moved, "releasing the button moved the value");
}

#[test]
fn a_small_ctrl_drag_at_the_low_end_of_a_log_slider_with_a_value_box_moves_the_value() {
    let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
    let clip = app.engine.session().track_by_name("Kick").unwrap().clips().iter().find(|c| c.is_audio()).unwrap().id;
    app.run("edit.select", json!({"clips": [clip.0]})).unwrap();
    app.ui.show_clip_effects = true;
    let mut h = harness(app);
    let attack = slider_params("compressor")[2];
    assert_eq!(attack.id, "attack");
    let key = format!("clip_fx.{}.comp.attack", clip.0);
    let from = h.get_by_role_and_label(Role::Slider, attack.name).rect().left_center() + vec2(1.0, 0.0);
    hold(&mut h, Modifiers::COMMAND);
    move_to(&mut h, from);
    press(&mut h, from, true, Modifiers::COMMAND);
    let read = |h: &Harness<'_, SoundApp>| h.state().engine.session().edit.values.get(&key).copied().unwrap();
    let start = read(&h);
    assert!((start - f64::from(attack.min)).abs() < 1e-6, "the press put Attack at {start}");
    move_to(&mut h, from + vec2(1.0, 0.0));
    let moved = read(&h);
    press(&mut h, from + vec2(1.0, 0.0), false, Modifiers::COMMAND);
    assert!(moved > start, "a one-point Ctrl-drag left Attack at {moved}");
}

#[test]
fn whole_number_instrument_sliders_move_a_tenth_as_far_and_stay_whole() {
    let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
    let keys = app.engine.session().track_by_name("Keys").unwrap().id;
    app.run("mix.instrument", json!({"track": keys.0, "plugin": "sampler"})).unwrap();
    app.ui.plugin_windows = vec![(keys, soundcraft_mix::INSTRUMENT_SLOT)];
    let mut h = harness(app);
    let rails = window_sliders(&h, Id::unique(("plugin", keys.0, soundcraft_mix::INSTRUMENT_SLOT)));
    let root = slider_params("sampler")[0];
    assert_eq!(root.id, "root_note");
    // Each read during the drags checks that the instrument was sent a whole number.
    let read = move |h: &Harness<'_, SoundApp>| {
        let v = insert_param(h, keys, soundcraft_mix::INSTRUMENT_SLOT, root);
        assert_eq!(v.fract(), 0.0, "Root Note is {v} during a drag");
        v
    };
    assert_tenth(root.name, plain_and_fine(&mut h, rails[0], &read), 0.06..=0.14);
}

#[test]
fn clip_effects_sliders_move_a_tenth_as_far_with_ctrl_held() {
    let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
    let s = app.engine.session();
    let clip = s.track_by_name("Kick").unwrap().clips().iter().find(|c| c.is_audio()).unwrap().id;
    app.run("edit.select", json!({"clips": [clip.0]})).unwrap();
    app.ui.show_clip_effects = true;
    let mut h = harness(app);
    for (label, key, default) in [("Input", "eq.input_gain", 0.0), ("Threshold", "comp.threshold", -20.0), ("Clip effects gain (dB)", "gain", 0.0)] {
        if key == "gain" {
            let input = h.get_by_role_and_label(Role::Slider, "Input").rect();
            move_to(&mut h, input.center());
            h.event(Event::MouseWheel {
                unit: egui::MouseWheelUnit::Point,
                delta: vec2(0.0, -2000.0),
                modifiers: Modifiers::NONE,
                phase: egui::TouchPhase::Move,
                source: egui::MouseWheelSource::Unknown,
            });
            h.run_steps(30);
        }
        let key = format!("clip_fx.{}.{key}", clip.0);
        let read = move |h: &Harness<'_, SoundApp>| h.state().engine.session().edit.values.get(&key).copied().unwrap_or(default);
        let rail = h.get_by_role_and_label(Role::Slider, label).rect();
        assert_tenth(label, plain_and_fine(&mut h, rail, &read), 0.08..=0.12);
    }
}

#[test]
fn clip_effects_value_boxes_move_a_tenth_as_far_with_ctrl_held() {
    let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
    let s = app.engine.session();
    let clip = s.track_by_name("Kick").unwrap().clips().iter().find(|c| c.is_audio()).unwrap().id;
    app.run("edit.select", json!({"clips": [clip.0]})).unwrap();
    app.ui.show_clip_effects = true;
    let mut h = harness(app);
    for (label, key, default) in [("Input", "eq.input_gain", 0.0), ("Threshold", "comp.threshold", -20.0)] {
        let key = format!("clip_fx.{}.{key}", clip.0);
        let read = move |h: &Harness<'_, SoundApp>| h.state().engine.session().edit.values.get(&key).copied().unwrap_or(default);
        let rail = h.get_by_role_and_label(Role::Slider, label).rect();
        let boxed = value_box(&h, rail);
        assert_tenth(label, plain_and_fine(&mut h, boxed, &read), 0.07..=0.13);
    }
}

#[test]
fn clip_effects_log_taper_sliders_use_a_logarithmic_rail() {
    let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
    let s = app.engine.session();
    let clip = s.track_by_name("Kick").unwrap().clips().iter().find(|c| c.is_audio()).unwrap().id;
    app.run("edit.select", json!({"clips": [clip.0]})).unwrap();
    app.ui.show_clip_effects = true;
    let mut h = harness(app);
    let ratio = slider_params("compressor")[1];
    assert_eq!(ratio.id, "ratio");
    let rail = h.get_by_role_and_label(Role::Slider, ratio.name).rect();
    move_to(&mut h, rail.center());
    press(&mut h, rail.center(), true, Modifiers::NONE);
    press(&mut h, rail.center(), false, Modifiers::NONE);
    let key = format!("clip_fx.{}.comp.ratio", clip.0);
    let v = h.state().engine.session().edit.values.get(&key).copied().unwrap();
    // The middle of a 1 to 100 log rail is 10; a linear rail puts it near 50.
    assert!((7.0..=14.0).contains(&v), "a click in the middle of the Ratio rail set {v}");
}

#[test]
fn fallback_audiosuite_sliders_move_a_tenth_as_far_with_ctrl_held() {
    for (process, label, id) in
        [("normalize", "Peak dB", "target_db"), ("time_stretch", "Length ratio", "ratio"), ("pitch_shift", "Semitones", "semitones")]
    {
        let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
        app.ui.audiosuite = Some(process.into());
        let mut h = harness(app);
        let read = |h: &Harness<'_, SoundApp>| {
            let params: BTreeMap<String, f32> = h.ctx.data(|d| d.get_temp(Id::unique(("as_params", process.to_string())))).unwrap_or_default();
            f64::from(params[id])
        };
        let rail = h.get_by_role_and_label(Role::Slider, label).rect();
        assert_tenth(label, plain_and_fine(&mut h, rail, &read), 0.08..=0.12);
    }
}

#[test]
fn audiosuite_parameter_sliders_move_a_tenth_as_far_with_ctrl_held() {
    let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
    app.ui.audiosuite = Some("compressor".into());
    let mut h = harness(app);
    for (label, id) in [("Threshold", "threshold"), ("Knee", "knee")] {
        let read = move |h: &Harness<'_, SoundApp>| {
            let params: BTreeMap<String, f32> = h.ctx.data(|d| d.get_temp(Id::unique(("as_params", "compressor".to_string())))).unwrap_or_default();
            f64::from(params[id])
        };
        let rail = h.get_by_role_and_label(Role::Slider, label).rect();
        assert_tenth(label, plain_and_fine(&mut h, rail, &read), 0.08..=0.12);
    }
}

#[test]
fn audiosuite_value_boxes_move_a_tenth_as_far_with_ctrl_held() {
    let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
    app.ui.audiosuite = Some("compressor".into());
    let mut h = harness(app);
    for (label, id) in [("Threshold", "threshold"), ("Knee", "knee")] {
        let read = move |h: &Harness<'_, SoundApp>| {
            let params: BTreeMap<String, f32> = h.ctx.data(|d| d.get_temp(Id::unique(("as_params", "compressor".to_string())))).unwrap_or_default();
            f64::from(params[id])
        };
        let rail = h.get_by_role_and_label(Role::Slider, label).rect();
        let boxed = value_box(&h, rail);
        assert_tenth(label, plain_and_fine(&mut h, boxed, &read), 0.07..=0.13);
    }
}
