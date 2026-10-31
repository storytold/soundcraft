//! #990: track and group names in the side lists are clipped to their row, so a long name cannot
//! paint across the scrollbar gutter and over the panel's edge.

use egui::{Shape, vec2};
use egui_kittest::{Harness, kittest::Queryable};
use serde_json::json;
use soundcraft_ui_egui::{Services, SoundApp};

fn harness() -> Harness<'static, SoundApp> {
    let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
    let first = app.engine.session().tracks.first().map(|t| t.id.0).unwrap_or_default();
    app.run("track.rename", json!({"track": first, "name": LONG_TRACK})).unwrap();
    app.run("track.group", json!({"name": LONG_GROUP, "tracks": [first]})).unwrap();
    app.engine.session_mut().edit.selected_tracks.clear();
    let mut h = Harness::builder().with_size(vec2(1600.0, 1000.0)).build_ui_state(
        |ui, app: &mut SoundApp| {
            let ctx = ui.ctx().clone();
            app.logic(&ctx);
            app.ui(ui);
        },
        app,
    );
    h.run_steps(3);
    h
}

const LONG_TRACK: &str = "24H2026_27_JANVIER_SESSION_AUDIO_BED_A";
const LONG_GROUP: &str = "Rhythm section drums bass and percussion group";

/// The name's paint must not be clipped any further right than its row: a wider clip lets the text
/// run over the lists' scrollbar gutter and across the panel's edge. Only the paint inside the row
/// counts — the name is drawn in its own clipped column in the edit window's headers, too.
fn name_is_clipped_to_its_row(h: &Harness<'_, SoundApp>, name: &str) {
    let row = h.get_by_label(name).rect();
    let painted = h.output().shapes.iter().filter_map(|s| match &s.shape {
        Shape::Text(t) if t.galley.text().contains(name) && s.clip_rect.intersects(row) => Some(s.clip_rect),
        _ => None,
    });
    let mut count = 0;
    for clip in painted {
        count += 1;
        assert!(clip.right() <= row.right() + 0.5, "the name's paint is clipped at x {}, past its row's right edge {}", clip.right(), row.right());
    }
    assert!(count > 0, "no painted text found for {name:?}");
}

#[test]
fn a_long_track_name_stays_inside_its_row() {
    let h = harness();
    name_is_clipped_to_its_row(&h, LONG_TRACK);
}

#[test]
fn a_long_group_name_stays_inside_its_row() {
    let h = harness();
    name_is_clipped_to_its_row(&h, LONG_GROUP);
}
