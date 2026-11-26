use egui::{Color32, Shape, vec2};
use egui_kittest::Harness;
use serde_json::json;
use soundcraft_ui_egui::{Services, SoundApp, theme::ThemeMode};

fn contrast(a: Color32, b: Color32) -> f64 {
    let luminance = |color: Color32| {
        let linear = |channel: u8| {
            let s = f64::from(channel) / 255.0;
            if s <= 0.04045 { s / 12.92 } else { ((s + 0.055) / 1.055).powf(2.4) }
        };
        0.2126 * linear(color.r()) + 0.7152 * linear(color.g()) + 0.0722 * linear(color.b())
    };
    let (a, b) = (luminance(a), luminance(b));
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}

#[test]
fn the_io_columns_vol_and_pan_readout_is_readable_in_both_themes() {
    for mode in [ThemeMode::Dark, ThemeMode::Light] {
        let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
        app.run("view.edit_io", json!({})).unwrap();
        app.run("track.height", json!({"tracks": ["Kick"], "height": "medium"})).unwrap();
        app.run("ui.theme", json!({"mode": mode.id()})).unwrap();
        let mut harness =
            Harness::builder().with_context_setup(soundcraft_ui_egui::framework::install_headless).with_size(vec2(1600.0, 1000.0)).build_ui_state(
                |ui, app: &mut SoundApp| {
                    app.logic(ui.ctx());
                    app.ui(ui);
                },
                app,
            );
        for _ in 0..4 {
            harness.step();
        }
        let output = harness.output();
        let labels: Vec<_> = output
            .shapes
            .iter()
            .filter_map(|shape| match &shape.shape {
                Shape::Text(text) if matches!(text.galley.job.text.as_str(), "vol" | "pan") => Some(text),
                _ => None,
            })
            .collect();
        assert!(!labels.is_empty(), "{mode:?}: no vol/pan readout painted");
        for label in labels {
            let center = label.visual_bounding_rect().center();
            let well = output
                .shapes
                .iter()
                .filter_map(|shape| match &shape.shape {
                    Shape::Rect(rect) if rect.rect.contains(center) && rect.fill.a() == 255 => Some(rect),
                    _ => None,
                })
                .min_by(|a, b| a.rect.area().total_cmp(&b.rect.area()))
                .unwrap_or_else(|| panic!("{mode:?}: no readout behind {:?}", label.galley.job.text));
            let ink = label.override_text_color.unwrap_or(label.galley.job.sections[0].format.color);
            let ratio = contrast(ink, well.fill);
            assert!(ratio >= 4.5, "{mode:?}: {:?} reads at {ratio:.2}:1", label.galley.job.text);
        }
    }
}
