//! Synthetic appearance checks for populated shared panels and custom controls.
use egui::{Color32, vec2};
use egui_kittest::Harness;
use serde_json::json;
use soundcraft_ui_egui::{Services, SoundApp, theme, widgets};

#[test]
fn shared_panels_and_controls_follow_manual_and_live_system_appearance() {
    // Keep palette changes in one owning test: upstream custom painters use process-global tokens.
    for scale in [1.0, 2.0] {
        let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
        app.run("edit.select", json!({"tracks":["Keys"]})).unwrap();
        for id in ["window.midi_editor", "window.memory_locations", "window.undo_history"] {
            app.run(id, json!({})).unwrap();
        }
        let session = app.engine.session().clone();
        let mut h = Harness::builder().with_size(vec2(1400.0, 1000.0)).with_pixels_per_point(scale).wgpu().build_ui_state(
            |ui, app: &mut SoundApp| {
                app.logic(&ui.ctx().clone());
                app.ui(ui);
            },
            app,
        );
        h.input_mut().max_texture_side = Some(8192);
        for (name, mode, system, light) in [
            ("dark", theme::ThemeMode::Dark, egui::Theme::Light, false),
            ("light", theme::ThemeMode::Light, egui::Theme::Dark, true),
            ("system-light", theme::ThemeMode::System, egui::Theme::Light, true),
            ("system-dark", theme::ThemeMode::System, egui::Theme::Dark, false),
        ] {
            h.state_mut().ui.theme = mode;
            h.input_mut().system_theme = Some(system);
            for (width_name, size) in [("normal", vec2(1400.0, 1000.0)), ("narrow", vec2(1000.0, 800.0))] {
                h.set_size(size);
                h.run_steps(5);
                assert_eq!(theme::is_light(), light);
                assert_eq!(h.state().ui.theme, mode);
                assert_eq!(*h.state().engine.session(), session);
                soundcraft_ui_egui::docking::validate(&h.state().ui.docking).unwrap();
                if let Some(directory) = std::env::var_os("CRAFT_UI_THEME_SHOTS").map(std::path::PathBuf::from) {
                    std::fs::create_dir_all(&directory).unwrap();
                    h.render().unwrap().save(directory.join(format!("sound-{name}-{width_name}-{scale}x.png"))).unwrap();
                }
            }
            let ctx = egui::Context::default();
            theme::apply(&ctx, light);
            let tokens = theme::Tokens::current();
            let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
                widgets::icon_button(ui, vec2(38.0, 28.0), "loop", true, "Loop playback");
                widgets::rec_toggle(ui, vec2(30.0, 18.0), true, "Record enable");
                ui.add_enabled_ui(false, |ui| widgets::icon_button(ui, vec2(38.0, 28.0), "loop", false, "Disabled loop"));
            });
            output.textures_delta.clear();
            for expected in [tokens.accent, tokens.rec] {
                assert!(output.shapes.iter().any(|shape| matches!(&shape.shape, egui::Shape::Rect(rect) if rect.fill == expected)));
            }
            assert!(output.shapes.iter().any(|shape| matches!(&shape.shape, egui::Shape::Circle(circle) if circle.fill == Color32::WHITE)));
            if let Some(directory) = std::env::var_os("CRAFT_UI_THEME_SHOTS").map(std::path::PathBuf::from) {
                let mut controls = Harness::builder().with_size(vec2(340.0, 150.0)).with_pixels_per_point(scale).wgpu().build_ui_state(
                    move |ui, _: &mut ()| {
                        theme::apply(ui.ctx(), light);
                        for enabled in [true, false] {
                            ui.add_enabled_ui(enabled, |ui| {
                                ui.horizontal(|ui| {
                                    for selected in [false, true] {
                                        widgets::icon_button(ui, vec2(38.0, 28.0), "loop", selected, "Loop playback");
                                        widgets::rec_toggle(ui, vec2(30.0, 18.0), selected, "Record enable");
                                    }
                                });
                            });
                        }
                    },
                    (),
                );
                controls.run_steps(4);
                controls.render().unwrap().save(directory.join(format!("sound-controls-{name}-{scale}x.png"))).unwrap();
                controls.key_press(egui::Key::Tab);
                controls.run_steps(2);
                controls.render().unwrap().save(directory.join(format!("sound-controls-focus-{name}-{scale}x.png"))).unwrap();
            }
        }
    }
}
