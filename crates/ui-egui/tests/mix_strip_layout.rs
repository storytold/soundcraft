use egui::{Color32, FullOutput, Shape, epaint::TextShape, vec2};
use egui_kittest::Harness;
use serde_json::json;
use soundcraft_ui_egui::{
    Services, SoundApp,
    theme::{ThemeMode, Tokens},
};

const SCALE: [&str; 10] = ["12", "6", "0", "5", "10", "15", "20", "30", "40", "60"];

// The palette is process-wide, so tests in this file take turns.
static PALETTE: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn mix(mode: ThemeMode) -> Harness<'static, SoundApp> {
    let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
    app.engine.session_mut().edit.selected_tracks.clear();
    // Fader caps at the bottom, clear of the scale.
    for track in &mut app.engine.session_mut().tracks {
        track.mixer.volume_db = -144.0;
    }
    app.run("window.mix", json!({})).unwrap();
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
    harness
}

fn texts<'a>(output: &'a FullOutput, keep: impl Fn(&TextShape) -> bool + 'a) -> impl Iterator<Item = &'a TextShape> + 'a {
    output.shapes.iter().filter_map(move |shape| match &shape.shape {
        Shape::Text(text) if keep(text) => Some(text),
        _ => None,
    })
}

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
fn fader_scale_numbers_are_readable_and_clear_of_their_ticks() {
    let _palette = PALETTE.lock().unwrap_or_else(|e| e.into_inner());
    for (mode, palette) in [(ThemeMode::Dark, Tokens::DARK), (ThemeMode::Light, Tokens::LIGHT)] {
        let harness = mix(mode);
        let output = harness.output();
        let numbers: Vec<_> =
            texts(output, |t| t.galley.job.sections[0].format.font_id.size == 8.0 && SCALE.contains(&t.galley.job.text.as_str())).collect();
        assert!(numbers.len() >= SCALE.len() * 2, "{mode:?}: found {} scale numbers", numbers.len());
        for number in numbers {
            let color = number.override_text_color.unwrap_or(number.galley.job.sections[0].format.color);
            let ratio = contrast(color, palette.strip_bg);
            assert!(ratio >= 4.5, "{mode:?}: scale number {:?} contrast {ratio:.2}:1", number.galley.job.text);
            let glyphs = number.visual_bounding_rect();
            for shape in &output.shapes {
                if let Shape::LineSegment { points: [a, b], .. } = &shape.shape {
                    let crosses = a.y == b.y && glyphs.y_range().contains(a.y) && a.x.min(b.x) < glyphs.max.x && a.x.max(b.x) > glyphs.min.x;
                    assert!(!crosses, "{mode:?}: a tick runs through {:?}", number.galley.job.text);
                }
            }
        }
    }
}

#[test]
fn stereo_strips_line_up_with_mono_strips() {
    let _palette = PALETTE.lock().unwrap_or_else(|e| e.into_inner());
    let harness = mix(ThemeMode::Dark);
    let solo_keys: Vec<f32> = texts(harness.output(), |t| t.galley.job.text == "S").map(|t| t.visual_bounding_rect().center().y).collect();
    assert!(solo_keys.len() >= 6, "found {} solo keys", solo_keys.len());
    let (top, bottom) = solo_keys.iter().fold((f32::MAX, f32::MIN), |(lo, hi), &y| (lo.min(y), hi.max(y)));
    assert!(bottom - top < 0.5, "solo keys span {top}..{bottom}");
}

#[test]
fn the_meter_starts_level_with_the_top_of_the_faders_travel() {
    let _palette = PALETTE.lock().unwrap_or_else(|e| e.into_inner());
    let harness = mix(ThemeMode::Dark);
    let numbers: Vec<_> = texts(harness.output(), |t| t.galley.job.sections[0].format.font_id.size == 8.0)
        .map(|t| (t.galley.job.text.as_str(), t.galley.rect.translate(t.pos.to_vec2())))
        .collect();
    let fader_tops: Vec<_> = numbers.iter().filter(|(text, _)| *text == "12").map(|(_, r)| *r).collect();
    assert!(fader_tops.len() >= 6, "found {} fader scales", fader_tops.len());
    for top in fader_tops {
        let meter_top = numbers.iter().find(|(text, r)| *text == "0" && r.min.x > top.max.x + 16.0 && r.min.x < top.max.x + 60.0);
        assert!(meter_top.is_some_and(|(_, r)| (r.center().y - top.center().y).abs() < 0.5), "meter scale beside {top:?}: {meter_top:?}");
    }
}

#[test]
fn narrow_strips_show_ticks_without_numbers() {
    let _palette = PALETTE.lock().unwrap_or_else(|e| e.into_inner());
    let mut harness = mix(ThemeMode::Dark);
    harness.state_mut().ui.narrow_mix = true;
    for _ in 0..3 {
        harness.step();
    }
    let numbers = texts(harness.output(), |t| t.galley.job.sections[0].format.font_id.size == 8.0 && SCALE.contains(&t.galley.job.text.as_str()));
    assert_eq!(numbers.count(), 0);
}
