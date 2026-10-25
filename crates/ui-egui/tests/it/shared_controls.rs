//! Interaction and optional offscreen evidence for the application's painted controls.

use egui::{Event, Key, Modifiers, PointerButton, Response, pos2, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use soundcraft_ui_egui::{Services, SoundApp, theme, widgets};

#[derive(Default)]
struct ButtonState {
    record: bool,
    selected: bool,
    disabled: bool,
    clicks: usize,
    response: Option<Response>,
}

impl ButtonState {
    fn label(&self) -> &'static str {
        if self.record { "Record enable" } else { "Loop playback" }
    }
}

fn button_frame(ctx: &egui::Context, state: &mut ButtonState, events: Vec<Event>) -> egui::FullOutput {
    let mut output = ctx.run_ui(egui::RawInput { events, ..Default::default() }, |ui| {
        let response = ui
            .add_enabled_ui(!state.disabled, |ui| {
                if state.record {
                    widgets::rec_toggle(ui, vec2(38.0, 28.0), state.selected, state.label())
                } else {
                    widgets::icon_button(ui, vec2(38.0, 28.0), "loop", state.selected, state.label())
                }
            })
            .inner;
        state.clicks += usize::from(response.clicked());
        state.response = Some(response);
    });
    output.textures_delta.clear();
    output
}

fn key(key: Key, pressed: bool) -> Event {
    Event::Key { key, physical_key: None, pressed, repeat: false, modifiers: Modifiers::NONE }
}

fn pointer(pos: egui::Pos2, pressed: bool) -> Event {
    Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE }
}

#[test]
fn tool_and_record_buttons_preserve_distinct_selected_colors_and_caller_state() {
    for record in [false, true] {
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        let mut state = ButtonState { record, selected: true, ..Default::default() };
        button_frame(&ctx, &mut state, vec![]);
        let center = state.response.as_ref().unwrap().rect.center();
        let out = button_frame(&ctx, &mut state, vec![Event::PointerMoved(center), pointer(center, true)]);
        let expected = if record { theme::Tokens::current().rec } else { theme::Tokens::current().accent };
        assert!(out.shapes.iter().any(|s| matches!(&s.shape, egui::Shape::Rect(r) if r.fill == expected)));
        if record {
            assert!(out.shapes.iter().any(|s| matches!(&s.shape, egui::Shape::Circle(c) if c.fill == egui::Color32::WHITE)));
        }
        let update = out.platform_output.accesskit_update.unwrap();
        let node = &update.nodes.iter().find(|(_, n)| n.label() == Some(state.label())).unwrap().1;
        assert_eq!(node.role(), egui::accesskit::Role::Button);
        assert_eq!(node.toggled(), Some(egui::accesskit::Toggled::True));
        button_frame(&ctx, &mut state, vec![pointer(center, false)]);
        assert_eq!(state.clicks, 1);
        assert!(state.selected, "selection and record arming remain caller-owned");
    }
}

#[test]
fn tool_and_record_buttons_handle_keyboard_disabled_state_and_cancelled_clicks() {
    for record in [false, true] {
        for key_code in [Key::Enter, Key::Space] {
            let ctx = egui::Context::default();
            ctx.enable_accesskit();
            let mut state = ButtonState { record, ..Default::default() };
            button_frame(&ctx, &mut state, vec![]);
            button_frame(&ctx, &mut state, vec![]);
            let response = state.response.clone().unwrap();
            ctx.memory_mut(|m| m.request_focus(response.id));
            let out = button_frame(&ctx, &mut state, vec![]);
            assert!(out.shapes.iter().any(|s| matches!(&s.shape, egui::Shape::Rect(r)
                if r.rect == response.rect.shrink(2.0) && r.stroke.color == theme::Tokens::current().accent)));
            button_frame(&ctx, &mut state, vec![key(key_code, true)]);
            button_frame(&ctx, &mut state, vec![key(key_code, false)]);
            assert_eq!(state.clicks, 1);
            let center = response.rect.center();
            button_frame(&ctx, &mut state, vec![Event::PointerMoved(center), pointer(center, true)]);
            button_frame(&ctx, &mut state, vec![Event::PointerMoved(pos2(300.0, 150.0))]);
            button_frame(&ctx, &mut state, vec![pointer(pos2(300.0, 150.0), false)]);
            assert_eq!(state.clicks, 1);
            state.disabled = true;
            button_frame(&ctx, &mut state, vec![Event::PointerMoved(center), pointer(center, true), key(key_code, true)]);
            let out = button_frame(&ctx, &mut state, vec![pointer(center, false), key(key_code, false)]);
            assert_eq!(state.clicks, 1);
            assert!(!state.response.as_ref().unwrap().enabled());
            assert!(out.platform_output.accesskit_update.unwrap().nodes.iter().any(|(_, n)| n.label() == Some(state.label()) && n.is_disabled()));
        }
    }
}

#[test]
fn toolbar_keyboard_activation_changes_tools_without_transport_or_marker_actions() {
    let app = SoundApp::new(soundcraft_engine::Engine::default(), None, Services::default());
    let mut h = Harness::builder().with_size(vec2(1600.0, 1000.0)).build_ui_state(
        |ui, app: &mut SoundApp| {
            app.logic(&ui.ctx().clone());
            app.ui(ui);
        },
        app,
    );
    h.run_steps(5);
    h.get_by_label("Trim (F6)").click();
    h.run_steps(3);
    assert_eq!(h.state().engine.session().edit.tool, soundcraft_model::Tool::Trim);
    let markers = h.state().engine.session().markers.len();
    for (label, tool, key) in
        [("Pencil (F10)", soundcraft_model::Tool::Pencil, Key::Enter), ("Grabber (F8)", soundcraft_model::Tool::Grabber, Key::Space)]
    {
        h.get_by_label(label).focus();
        h.run_steps(2);
        assert!(h.get_by_label(label).is_focused());
        h.key_press(key);
        h.run_steps(3);
        assert_eq!(h.state().engine.session().edit.tool, tool);
        assert!(!h.state().engine.transport.playing);
        assert!(!h.state().engine.transport.recording);
        assert_eq!(h.state().engine.session().markers.len(), markers);
    }
}

#[test]
fn track_record_button_arms_the_track_without_starting_transport() {
    let mut app = SoundApp::new(soundcraft_engine::Engine::default(), None, Services::default());
    app.run("track.new", serde_json::json!({"name": "Voice"})).unwrap();
    let mut h = Harness::builder().with_size(vec2(1600.0, 1000.0)).build_ui_state(
        |ui, app: &mut SoundApp| {
            app.logic(&ui.ctx().clone());
            app.ui(ui);
        },
        app,
    );
    h.run_steps(5);
    h.get_by_label("Record enable").click();
    h.run_steps(3);
    assert!(h.state().engine.session().tracks[0].mixer.record_arm);
    for (key, armed) in [(Key::Enter, false), (Key::Space, true)] {
        h.get_by_label("Record enable").focus();
        h.run_steps(2);
        h.key_press(key);
        h.run_steps(3);
        assert_eq!(h.state().engine.session().tracks[0].mixer.record_arm, armed);
        assert!(!h.state().engine.transport.playing);
        assert!(!h.state().engine.transport.recording);
        assert!(h.state().engine.session().markers.is_empty());
    }
}

#[test]
fn shared_controls_visuals() {
    let Some(directory) = std::env::var_os("CRAFT_UI_SUITE_SHOTS").map(std::path::PathBuf::from) else { return };
    std::fs::create_dir_all(&directory).unwrap();
    for scale in [1.0, 2.0] {
        let app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
        let mut h = Harness::builder().with_size(vec2(1600.0, 1000.0)).with_pixels_per_point(scale).wgpu().build_ui_state(
            |ui, app: &mut SoundApp| {
                app.logic(&ui.ctx().clone());
                app.ui(ui);
            },
            app,
        );
        h.input_mut().max_texture_side = Some(8192);
        for (name, size) in [("normal", vec2(1600.0, 1000.0)), ("narrow", vec2(1000.0, 700.0))] {
            h.set_size(size);
            h.run_steps(6);
            h.render().unwrap().save(directory.join(format!("soundcraft-{name}-dark-{scale}x.png"))).unwrap();
        }
        let mut controls = Harness::builder().with_size(vec2(340.0, 150.0)).with_pixels_per_point(scale).wgpu().build_ui_state(
            |ui, rects: &mut Vec<egui::Rect>| {
                theme::apply(ui.ctx(), false);
                rects.clear();
                for enabled in [true, false] {
                    ui.add_enabled_ui(enabled, |ui| {
                        ui.horizontal(|ui| {
                            for selected in [false, true] {
                                rects.push(widgets::icon_button(ui, vec2(38.0, 28.0), "loop", selected, "Loop Playback").rect);
                                rects.push(widgets::icon_button(ui, vec2(36.0, 26.0), "trim", selected, "Trim").rect);
                                rects.push(widgets::rec_toggle(ui, vec2(30.0, 18.0), selected, "Record enable").rect);
                            }
                        });
                    });
                }
            },
            Vec::new(),
        );
        controls.run_steps(4);
        controls.render().unwrap().save(directory.join(format!("soundcraft-controls-dark-{scale}x.png"))).unwrap();
        for (name, index, pressed) in [("hover", 0, false), ("selected-hover", 3, false), ("disabled-hover", 6, false), ("pressed", 0, true)] {
            let center = controls.state()[index].center();
            controls.event(Event::PointerMoved(center));
            if pressed {
                controls.event(pointer(center, true));
            }
            controls.run_steps(2);
            controls.render().unwrap().save(directory.join(format!("soundcraft-controls-{name}-dark-{scale}x.png"))).unwrap();
        }
        controls.event(pointer(controls.state()[0].center(), false));
        controls.event(Event::PointerGone);
        controls.run_steps(2);
        controls.key_press(Key::Tab);
        controls.run_steps(2);
        controls.render().unwrap().save(directory.join(format!("soundcraft-controls-focus-dark-{scale}x.png"))).unwrap();
    }
}
