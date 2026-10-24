//! The Edit toolbar fits a default 13-inch MacBook window (#704), and in a narrower window its scroll
//! bar sits under the controls instead of over them.

use egui::{Rect, Shape, pos2, vec2};
use egui_kittest::Harness;
use soundcraft_ui_egui::{Services, SoundApp, theme::Tokens, toolbar};

/// What the toolbar painted: its panel, the group backgrounds, anything wide and thin (a scroll bar) and everything
/// else it holds (the groups and what is not in one, such as the output meter).
struct Painted {
    panel: Rect,
    groups: Vec<Rect>,
    bars: Vec<Rect>,
    content: Vec<Rect>,
    right: f32,
}

fn paint(width: f32) -> Painted {
    let app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
    let mut harness = Harness::builder().with_size(vec2(width, 956.0)).build_ui_state(
        |ui, app: &mut SoundApp| {
            app.logic(ui.ctx());
            app.ui(ui);
        },
        app,
    );
    for _ in 0..8 {
        harness.step();
    }
    // A floating scroll bar only appears under the pointer; this one is over the toolbar's lower row.
    harness.hover_at(pos2(width / 2.0, 100.0));
    for _ in 0..8 {
        harness.step();
    }
    let t = Tokens::current();
    let output = harness.output();
    let panel = output
        .shapes
        .iter()
        .find_map(|s| match &s.shape {
            // The panel is painted twice, one pixel apart in height.
            Shape::Rect(r) if r.fill == t.toolbar_bg && (toolbar::HEIGHT - 2.0..=toolbar::HEIGHT).contains(&r.rect.height()) => Some(r.rect),
            _ => None,
        })
        .expect("the toolbar panel is painted");
    let mut painted = Painted { panel, groups: vec![], bars: vec![], content: vec![], right: 0.0 };
    for s in &output.shapes {
        let r = s.shape.visual_bounding_rect();
        // Skip shapes outside the toolbar and the panel itself, but keep a full-width scroll bar track.
        let full_width = r.width() >= panel.width() - 1.0;
        if r.min.y < panel.min.y - 1.0 || r.max.y > panel.min.y + toolbar::HEIGHT + 1.0 || (full_width && r.height() >= toolbar::HEIGHT - 2.0) {
            continue;
        }
        // Full-width shapes (borders, a scroll bar track) span the window: they say nothing about the content's reach.
        if !full_width {
            painted.right = painted.right.max(r.max.x);
        }
        let is_bar = r.height() < 16.0 && r.width() > panel.width() / 2.0;
        if !full_width && !is_bar {
            painted.content.push(r);
        }
        if let Shape::Rect(rs) = &s.shape {
            if rs.fill == t.toolbar_group || rs.fill == t.counter_bg {
                painted.groups.push(r);
            } else if is_bar {
                painted.bars.push(r);
            }
        }
    }
    painted
}

#[test]
fn every_section_fits_a_1470_point_window_without_a_scroll_bar() {
    let p = paint(1470.0);
    assert!(p.groups.len() >= 7, "toolbar groups painted: {}", p.groups.len());
    assert!(p.bars.is_empty(), "scroll bar at 1470 pt: {:?}", p.bars);
    assert!(p.right <= p.panel.max.x, "toolbar content reaches x = {} in a {} pt window", p.right, p.panel.max.x);
}

#[test]
fn a_scroll_bar_in_a_narrow_window_does_not_cover_the_controls() {
    let p = paint(1280.0);
    assert!(!p.bars.is_empty(), "the toolbar no longer overflows at 1280 pt: this test needs a narrower window");
    for bar in &p.bars {
        for c in &p.content {
            assert!(!c.shrink(0.5).intersects(*bar), "scroll bar {bar:?} covers the content {c:?}");
        }
    }
}
