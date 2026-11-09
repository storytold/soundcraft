use egui_kittest::Harness;
use soundcraft_ui_egui::{Services, SoundApp};

/// A plugin window taller than a short screen is clamped so its bottom remains visible.
#[test]
fn a_long_plugin_window_stays_on_a_short_screen() {
    let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
    let kick = app.engine.session().track_by_name("Kick").unwrap().id;
    app.ui.plugin_windows = vec![(kick, 0)];
    let mut h = Harness::builder().with_size(egui::vec2(1200.0, 500.0)).build_ui_state(
        |ui, app: &mut SoundApp| {
            app.logic(&ui.ctx().clone());
            app.ui(ui);
        },
        app,
    );
    h.run_steps(4);
    let window = h.ctx.memory(|m| m.area_rect(egui::Id::unique(("plugin", kick.0, 0usize)))).unwrap();
    assert!(window.bottom() <= 500.0, "{window:?}");
}
