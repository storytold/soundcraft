//! Window › Tab View: the Tab panel docks like the other panels, in Edit and in Mix, and draws
//! the target MIDI track while stopped and while playing.

use egui::{Pos2, RawInput, Rect, vec2};
use serde_json::json;
use soundcraft_ui_egui::docking::Panel;
use soundcraft_ui_egui::{MainWindow, Services, SoundApp};

fn frame(ctx: &egui::Context, app: &mut SoundApp) {
    let raw = RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1600.0, 1000.0))), ..Default::default() };
    ctx.run_ui(raw, |ui| {
        app.logic(ui.ctx());
        app.ui(ui);
    })
    .textures_delta
    .clear();
}

#[test]
fn tab_panel_opens_closes_and_follows_playback() {
    let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
    let ctx = egui::Context::default();
    frame(&ctx, &mut app);
    assert!(!app.ui.docking.layout.contains(&Panel::GuitarTab), "hidden by default");

    app.run("window.guitar_tab", json!({})).unwrap();
    for _ in 0..3 {
        frame(&ctx, &mut app);
    }
    assert!(app.ui.show_guitar_tab && app.ui.docking.layout.contains(&Panel::GuitarTab));

    let track = soundcraft_engine::guitar_tab::target_track(app.engine.session()).unwrap();
    app.run("track.tuning", json!({"track": track.0, "tuning": "drop_d"})).unwrap();
    app.run("transport.play", json!({})).unwrap();
    for _ in 0..3 {
        frame(&ctx, &mut app);
    }
    app.run("transport.stop", json!({})).unwrap();

    app.run("ui.dock", json!({"operation": "close", "panel": "guitar_tab"})).unwrap();
    frame(&ctx, &mut app);
    assert!(!app.ui.show_guitar_tab && !app.ui.docking.layout.contains(&Panel::GuitarTab));

    // The Mix window has its own arrangement and takes the panel too.
    app.run("window.mix", json!({})).unwrap();
    app.run("ui.dock", json!({"operation": "open", "panel": "guitar_tab"})).unwrap();
    for _ in 0..3 {
        frame(&ctx, &mut app);
    }
    assert_eq!(app.ui.window, MainWindow::Mix);
    assert!(app.ui.mix_docking.layout.contains(&Panel::GuitarTab));
}

#[test]
fn tab_track_view_draws_at_every_zoom() {
    let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
    let ctx = egui::Context::default();
    let track = soundcraft_engine::guitar_tab::target_track(app.engine.session()).unwrap();
    app.run("track.view", json!({"tracks": [track.0], "view": "guitar_tab"})).unwrap();
    for height in ["mini", "medium", "jumbo"] {
        app.run("track.height", json!({"tracks": [track.0], "height": height})).unwrap();
        for spp in [20.0, 400.0, 20_000.0] {
            app.run("view.zoom_set", json!({"samples_per_px": spp})).unwrap();
            frame(&ctx, &mut app);
        }
    }
    assert_eq!(app.engine.session().track(track).map(|t| t.view.clone()).as_deref(), Some("guitar_tab"));
}

/// One harness frame, with scripted input fed through the raw-input hook.
fn step(h: &mut egui_kittest::Harness<'_, SoundApp>) {
    let mut raw = std::mem::take(h.input_mut());
    let ctx = h.ctx.clone();
    h.state_mut().raw_input_hook(&ctx, &mut raw);
    *h.input_mut() = raw;
    h.step();
}

#[test]
fn clicking_the_tab_moves_the_insertion_point_to_that_sixteenth() {
    use egui::{Event, Modifiers, PointerButton, pos2};
    let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
    // A floating panel at a known place: the header (24), the window's title bar, then the first
    // system's 16-point bar number strip and strings, all inside one 88-point-tall click target.
    app.run("ui.dock", json!({"operation": "open", "panel": "guitar_tab"})).unwrap();
    app.run("ui.dock", json!({"operation": "float", "panel": "guitar_tab", "rect": [100.0, 100.0, 900.0, 500.0]})).unwrap();
    // The tuning picker has no accessible name (it shows its value, not a label).
    let mut h = egui_kittest::Harness::builder().with_size(vec2(1600.0, 1000.0)).with_accessibility_check(false).build_ui_state(
        |ui, app: &mut SoundApp| {
            let ctx = ui.ctx().clone();
            app.logic(&ctx);
            app.ui(ui);
        },
        app,
    );
    for _ in 0..6 {
        step(&mut h);
    }
    // Middle of sixteenth 3 of bar 1: string labels (24), measure padding (8), 3.5 cells of 16.
    let pos = pos2(100.0 + 24.0 + 8.0 + 3.5 * 16.0, 100.0 + 94.0);
    h.state_mut().synthetic.extend([
        Event::PointerMoved(pos),
        Event::PointerButton { pos, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE },
        Event::PointerButton { pos, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE },
    ]);
    for _ in 0..6 {
        step(&mut h);
    }
    let s = h.state().engine.session();
    let at = s.tempo.tick_to_samples(3 * soundcraft_engine::score::UNIT, s.sample_rate);
    assert_eq!((s.edit.selection.start, s.edit.selection.end), (at, at));
}
