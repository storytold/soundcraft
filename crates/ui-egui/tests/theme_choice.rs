//! Exercise the actual Appearance selector, including both dark palettes and Reset.
use egui::vec2;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use soundcraft_ui_egui::{
    Services, SoundApp,
    theme::{ThemeMode, Tokens},
};

#[test]
fn appearance_selector_switches_between_both_dark_palettes_and_resets() {
    let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
    app.extra.show_ui_customization = true;
    let session = app.engine.session().clone();
    let mut h = Harness::builder().with_size(vec2(1600.0, 1000.0)).build_ui_state(
        |ui, app: &mut SoundApp| {
            app.logic(&ui.ctx().clone());
            app.ui(ui);
        },
        app,
    );
    h.run_steps(6);
    for (label, mode, panel) in [
        ("Dark (Avid-like)", ThemeMode::AvidDark, Tokens::AVID_DARK.panel_bg),
        ("Dark (ArtCraft)", ThemeMode::Dark, Tokens::DARK.panel_bg),
        ("Dark (Avid-like)", ThemeMode::AvidDark, Tokens::AVID_DARK.panel_bg),
    ] {
        let current = h.state().ui.theme.label();
        h.get_by_value(current).click();
        h.run_steps(2);
        h.get_by_label(label).click();
        h.run_steps(3);
        assert_eq!(h.state().ui.theme, mode);
        assert_eq!(h.ctx.global_style().visuals.panel_fill, panel);
        assert_eq!(serde_json::to_value(&h.state().ui).unwrap()["theme"], mode.id());
        assert_eq!(*h.state().engine.session(), session);
    }
    h.get_by_label("Reset").click();
    h.run_steps(3);
    assert_eq!(h.state().ui.theme, ThemeMode::Dark);
    assert_eq!(h.ctx.global_style().visuals.panel_fill, Tokens::DARK.panel_bg);
}
