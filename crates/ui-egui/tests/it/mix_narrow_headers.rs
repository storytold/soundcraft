use egui::{FontDefinitions, FontFamily, Shape, vec2};
use serde_json::json;
use soundcraft_ui_egui::mix_window::strip_width;
use soundcraft_ui_egui::{Services, SoundApp};

const FULL: [&str; 7] = ["INSERTS A-E", "INSERTS F-J", "SENDS A-E", "SENDS F-J", "INSTRUMENT", "comments", "no group"];
const SHORT: [&str; 6] = ["I A-E", "I F-J", "S A-E", "S F-J", "INST", "nogrp"];

/// egui's built-in fonts only, as on a system with no fonts installed (the FreeBSD CI VM): the
/// "bold" family is then the regular built-in face, narrower than any system bold.
fn builtin_fonts() -> FontDefinitions {
    let mut fonts = FontDefinitions::default();
    let base = fonts.families.get(&FontFamily::Proportional).cloned().unwrap_or_default();
    fonts.families.insert(FontFamily::Name("bold".into()), base);
    fonts
}

fn painted_labels(narrow: bool) -> Vec<(String, f32, bool)> {
    painted_labels_with(narrow, None)
}

fn painted_labels_with(narrow: bool, fonts: Option<FontDefinitions>) -> Vec<(String, f32, bool)> {
    let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
    app.run("window.mix", json!({})).unwrap();
    app.run("view.mix_section", json!({"section": "all"})).unwrap();
    app.run("view.mix_section", json!({"section": "instruments"})).unwrap();
    app.ui.narrow_mix = narrow;
    let mut harness = egui_kittest::Harness::builder().with_size(vec2(1600.0, 1000.0)).build_ui_state(
        |ui, app: &mut SoundApp| {
            app.logic(ui.ctx());
            if let Some(f) = &fonts {
                ui.ctx().set_fonts(f.clone());
            }
            app.ui(ui);
        },
        app,
    );
    for _ in 0..4 {
        harness.step();
    }
    harness
        .output()
        .shapes
        .iter()
        .filter_map(|s| match &s.shape {
            Shape::Text(t) if FULL.contains(&t.galley.job.text.as_str()) || SHORT.contains(&t.galley.job.text.as_str()) => {
                Some((t.galley.job.text.clone(), t.visual_bounding_rect().width(), t.galley.elided))
            }
            _ => None,
        })
        .collect()
}

fn assert_narrow_labels(labels: &[(String, f32, bool)]) {
    for name in SHORT {
        assert!(labels.iter().any(|(text, ..)| text == name), "{name} missing in {labels:?}");
    }
    for (text, width, elided) in labels {
        assert!(SHORT.contains(&text.as_str()), "{text} painted in a narrow strip, where it doesn't fit");
        assert!(*width <= strip_width(true) - 16.0 && !elided, "{text} is {width} pt wide, elided: {elided}");
    }
}

#[test]
fn narrow_strips_use_short_section_names_that_fit() {
    assert_narrow_labels(&painted_labels(true));
}

/// With a narrower font SENDS A-E fits a narrow strip, but narrow strips still use the short names
/// (this failed on FreeBSD, whose CI VM has no fonts installed).
#[test]
fn narrow_strips_use_short_section_names_with_the_builtin_fonts() {
    assert_narrow_labels(&painted_labels_with(true, Some(builtin_fonts())));
}

#[test]
fn normal_strips_keep_the_full_section_names() {
    let labels = painted_labels(false);
    for name in FULL {
        assert!(labels.iter().any(|(text, ..)| text == name), "{name} missing in {labels:?}");
    }
    assert!(labels.iter().all(|(text, ..)| FULL.contains(&text.as_str())), "{labels:?}");
}
