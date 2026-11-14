//! Edit window track-header meters: one bar per channel, and a header column that widens to fit
//! the widest meter shown, as Pro Tools draws them.

use egui::vec2;
use egui_kittest::Harness;
use serde_json::json;
use soundcraft_ui_egui::edit_window::{HEADER_W, meter_channels, meter_width, track_header_width};
use soundcraft_ui_egui::{Services, SoundApp};

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

/// Where the timeline starts: the left edge of the window plus the header column.
fn timeline_x(h: &Harness<'_, SoundApp>) -> f32 {
    h.state().edit_layout.timeline[0]
}

#[test]
fn meter_widths_follow_the_channel_count() {
    // Mono and stereo keep the meter the headers always had.
    assert_eq!(meter_width(1), 8.0);
    assert_eq!(meter_width(2), 8.0);
    // Wider formats add a 2.5 pt bar and a 1 pt gap per channel.
    assert_eq!(meter_width(6), 22.0);
    assert_eq!(meter_width(8), 29.0);
    for n in 2..16 {
        assert!(meter_width(n + 1) > meter_width(n), "{n} → {}", n + 1);
    }
    // Hostile counts are clamped, not trusted.
    assert_eq!(meter_width(0), meter_width(1));
    assert_eq!(meter_width(usize::MAX), meter_width(16));
}

#[test]
fn a_surround_track_widens_every_header_and_hiding_it_narrows_them_again() {
    let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
    // The demo session is mono and stereo: the headers keep their usual width.
    assert_eq!(track_header_width(app.engine.session()), HEADER_W);
    app.run("track.new", json!({"count": 1, "format": "5.1", "name": "Surround"})).unwrap();
    let s = app.engine.session();
    let surround = s.track_by_name("Surround").unwrap();
    assert_eq!(meter_channels(s, surround), 6);
    assert_eq!(track_header_width(s), HEADER_W + meter_width(6) - meter_width(2));

    let mut h = harness(app);
    let wide = timeline_x(&h);
    h.state_mut().run("track.hide", json!({"tracks": ["Surround"], "hidden": true})).unwrap();
    h.run_steps(4);
    let narrow = timeline_x(&h);
    assert_eq!(wide - narrow, meter_width(6) - meter_width(2), "the timeline moves by the extra meter width");
    assert_eq!(track_header_width(h.state().engine.session()), HEADER_W);
}

#[test]
fn the_master_meter_has_one_bar_per_main_output_channel() {
    let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
    app.run("track.new", json!({"count": 1, "kind": "master", "name": "Main"})).unwrap();
    let s = app.engine.session();
    let master = s.track_by_name("Main").unwrap();
    assert_eq!(meter_channels(s, master), s.main_format().channels());
}
