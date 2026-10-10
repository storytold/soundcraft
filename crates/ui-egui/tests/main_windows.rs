use egui::{Event, Modifiers, Pos2, RawInput, Rect, ViewportEvent, ViewportId, pos2, vec2};
use serde_json::json;
use soundcraft_ui_egui::{
    MainWindow, Services, SoundApp, UiState,
    main_windows::{self, Arrangement},
};

fn app() -> SoundApp {
    SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default())
}

fn frame(ctx: &egui::Context, app: &mut SoundApp, events: Vec<Event>) -> egui::FullOutput {
    frame_at_size(ctx, app, events, vec2(1600.0, 1000.0))
}

fn frame_at_size(ctx: &egui::Context, app: &mut SoundApp, events: Vec<Event>, size: egui::Vec2) -> egui::FullOutput {
    let raw = RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, size)), events, ..Default::default() };
    let mut output = ctx.run_ui(raw, |ui| {
        app.logic(ui.ctx());
        app.ui(ui);
    });
    output.textures_delta.clear();
    output
}

fn ready(ctx: &egui::Context, app: &mut SoundApp) {
    for _ in 0..6 {
        let _ = frame(ctx, app, vec![]);
    }
}

#[test]
fn opening_and_focusing_main_windows_preserves_the_session_and_playback() {
    let mut app = app();
    let ctx = egui::Context::default();
    ready(&ctx, &mut app);
    let session = app.engine.session().clone();
    app.run("transport.play", json!({})).unwrap();
    let _ = frame(&ctx, &mut app, vec![]);
    for command in ["window.mix", "window.edit", "window.toggle_mix_edit", "window.toggle_mix_edit"] {
        app.run(command, json!({})).unwrap();
        assert!(app.ui.both_windows);
        assert!(app.main_windows.is_open(&app, MainWindow::Edit));
        assert!(app.main_windows.is_open(&app, MainWindow::Mix));
        assert!(app.is_playing());
        assert_eq!(*app.engine.session(), session);
    }
    assert_eq!(app.ui.window, MainWindow::Edit);
}

#[test]
fn embedded_views_render_both_and_share_edits_and_undo() {
    let mut app = app();
    let ctx = egui::Context::default();
    ready(&ctx, &mut app);
    app.run("window.mix", json!({})).unwrap();
    for _ in 0..6 {
        let _ = frame(&ctx, &mut app, vec![]);
    }
    app.run("window.arrange_tile_v", json!({})).unwrap();
    let _ = frame(&ctx, &mut app, vec![]);
    let state = app.inspect(&ctx);
    assert_eq!(state["main_windows"]["edit"]["open"], true);
    assert_eq!(state["main_windows"]["mix"]["open"], true);
    let edit = state["main_windows"]["edit"]["rect"].as_array().unwrap();
    let mix = state["main_windows"]["mix"]["rect"].as_array().unwrap();
    assert!(edit[2].as_f64().unwrap() <= mix[0].as_f64().unwrap() + 1.0);
    for _ in 0..6 {
        let _ = frame(&ctx, &mut app, vec![]);
        let state = app.inspect(&ctx);
        let edit = state["main_windows"]["edit"]["rect"].as_array().unwrap();
        let mix = state["main_windows"]["mix"]["rect"].as_array().unwrap();
        assert!(edit[2].as_f64().unwrap() <= mix[0].as_f64().unwrap() + 1.0, "Tile must survive idle frames: {state}");
    }
    assert!(app.edit_layout.timeline[2] - app.edit_layout.timeline[0] > 100.0);
    let before = app.engine.session().track_by_name("Kick").unwrap().mixer.volume_db;
    app.run("window.mix", json!({})).unwrap();
    app.run("mix.volume", json!({"track": "Kick", "db": -18})).unwrap();
    let _ = frame(&ctx, &mut app, vec![]);
    app.run("window.edit", json!({})).unwrap();
    assert_eq!(app.engine.session().track_by_name("Kick").unwrap().mixer.volume_db, -18.0);
    app.run("edit.undo", json!({})).unwrap();
    assert_eq!(app.engine.session().track_by_name("Kick").unwrap().mixer.volume_db, before);
    assert!(app.ui.both_windows);
}

#[test]
fn shortcuts_switch_focus_and_close_only_one_view() {
    let mut app = app();
    let ctx = egui::Context::default();
    ready(&ctx, &mut app);
    let scale = ctx.zoom_factor();
    let chord = |key| Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::COMMAND };
    let _ = frame(&ctx, &mut app, vec![chord(egui::Key::Equals)]);
    assert_eq!(app.ui.window, MainWindow::Mix);
    assert!(app.ui.both_windows);
    assert_eq!(ctx.zoom_factor(), scale);
    let _ = frame(
        &ctx,
        &mut app,
        vec![Event::Key { key: egui::Key::Equals, physical_key: None, pressed: false, repeat: false, modifiers: Modifiers::COMMAND }],
    );
    let _ = frame(&ctx, &mut app, vec![chord(egui::Key::Equals)]);
    assert_eq!(app.ui.window, MainWindow::Edit);
    assert!(app.ui.both_windows);
    assert_eq!(ctx.zoom_factor(), scale);
    let session = app.engine.session().clone();
    let _ = frame(&ctx, &mut app, vec![chord(egui::Key::W)]);
    assert!(!app.ui.both_windows);
    assert_eq!(app.ui.window, MainWindow::Mix);
    assert_eq!(*app.engine.session(), session);
    app.run("window.edit", json!({})).unwrap();
    assert!(app.ui.both_windows);
    assert_eq!(app.ui.window, MainWindow::Edit);
}

#[test]
fn closing_either_view_keeps_the_other_and_can_reopen_it() {
    for window in [MainWindow::Edit, MainWindow::Mix] {
        let mut app = app();
        let ctx = egui::Context::default();
        ready(&ctx, &mut app);
        app.run("window.mix", json!({})).unwrap();
        let _ = frame(&ctx, &mut app, vec![]);
        let session = app.engine.session().clone();
        main_windows::close(&mut app, window, &ctx);
        let _ = frame(&ctx, &mut app, vec![]);
        assert!(!app.ui.both_windows);
        assert_eq!(app.ui.window, window.other());
        assert!(!app.quit_requested);
        assert_eq!(*app.engine.session(), session);
        main_windows::activate(&mut app, window);
        let _ = frame(&ctx, &mut app, vec![]);
        assert!(app.ui.both_windows);
        assert_eq!(app.ui.window, window);
    }
}

#[test]
fn root_close_keeps_the_other_window_but_quit_is_not_canceled() {
    for quit in [false, true] {
        let mut app = app();
        app.run("window.mix", json!({})).unwrap();
        app.quit_requested = quit;
        let ctx = egui::Context::default();
        let mut raw = RawInput::default();
        raw.viewports.get_mut(&ViewportId::ROOT).unwrap().events.push(ViewportEvent::Close);
        let mut output = ctx.run_ui(raw, |ui| app.logic(ui.ctx()));
        output.textures_delta.clear();
        let canceled = output.viewport_output[&ViewportId::ROOT].commands.contains(&egui::ViewportCommand::CancelClose);
        assert_eq!(canceled, !quit);
        assert_eq!(app.ui.both_windows, quit);
        if !quit {
            assert_eq!(app.ui.window, MainWindow::Mix);
        }
    }
}

#[test]
fn arrangements_have_distinct_geometry_inside_the_workspace() {
    for area in [Rect::from_min_size(Pos2::ZERO, vec2(1600.0, 1000.0)), Rect::from_min_size(pos2(-1600.0, 60.0), vec2(1000.0, 1600.0))] {
        for mode in [Arrangement::Tile, Arrangement::Horizontal, Arrangement::Vertical, Arrangement::Cascade] {
            let (first, second) = main_windows::arranged_rects(area, mode);
            assert!(area.contains_rect(first));
            assert!(area.contains_rect(second));
            assert!(first.is_positive() && second.is_positive());
            if mode == Arrangement::Cascade {
                assert!(first.intersects(second));
                assert!(second.min.x > first.min.x && second.min.y > first.min.y);
            } else {
                assert!(!first.shrink(0.1).intersects(second.shrink(0.1)));
                assert_eq!(first.union(second), area);
            }
        }
        let (h, _) = main_windows::arranged_rects(area, Arrangement::Horizontal);
        let (v, _) = main_windows::arranged_rects(area, Arrangement::Vertical);
        assert_eq!(h.width(), area.width());
        assert_eq!(v.height(), area.height());
        assert_ne!(h, v);
    }
}

#[test]
fn old_preferences_stay_single_window_and_dual_window_preferences_round_trip() {
    let old: UiState = serde_json::from_value(json!({"window": "Mix"})).unwrap();
    assert!(!old.both_windows);
    let mut app = app();
    app.ui = old;
    app.run("window.edit", json!({})).unwrap();
    assert_eq!(app.main_windows.primary(&app), MainWindow::Mix);
    assert!(app.ui.both_windows);
    let saved = serde_json::to_value(&app.ui).unwrap();
    let restored: UiState = serde_json::from_value(saved).unwrap();
    assert!(restored.both_windows);
    assert_eq!(restored.window, MainWindow::Edit);
}

#[test]
fn native_tile_moves_two_viewports_and_repeated_tile_keeps_the_workspace() {
    let ctx = egui::Context::default();
    ctx.set_embed_viewports(false);
    // Exercise the real native-viewport branch without needing a display server.
    egui::Context::set_immediate_viewport_renderer(|ctx, mut viewport| {
        let mut raw = RawInput {
            viewport_id: viewport.ids.this,
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(800.0, 972.0))),
            focused: false,
            ..Default::default()
        };
        raw.viewports.insert(
            viewport.ids.this,
            egui::ViewportInfo {
                parent: Some(ViewportId::ROOT),
                outer_rect: Some(Rect::from_min_size(pos2(-800.0, 60.0), vec2(800.0, 1000.0))),
                inner_rect: Some(Rect::from_min_size(pos2(-800.0, 88.0), vec2(800.0, 972.0))),
                ..Default::default()
            },
        );
        let mut output = ctx.run_ui(raw, |ui| (viewport.viewport_ui_cb)(ui));
        output.textures_delta.clear();
    });
    let mut app = app();
    let native_frame = |app: &mut SoundApp| {
        let mut raw = RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1600.0, 972.0))), ..Default::default() };
        let root = raw.viewports.get_mut(&ViewportId::ROOT).unwrap();
        root.outer_rect = Some(Rect::from_min_size(pos2(-1600.0, 60.0), vec2(1600.0, 1000.0)));
        root.inner_rect = Some(Rect::from_min_size(pos2(-1600.0, 88.0), vec2(1600.0, 972.0)));
        let mut output = ctx.run_ui(raw, |ui| {
            app.logic(ui.ctx());
            app.ui(ui);
        });
        output.textures_delta.clear();
        output
    };
    for _ in 0..6 {
        let _ = native_frame(&mut app);
    }
    for _ in 0..2 {
        app.run("window.arrange_tile_v", json!({})).unwrap();
        let output = native_frame(&mut app);
        for (id, x) in [(ViewportId::ROOT, -1600.0), (main_windows::secondary_id(), -800.0)] {
            let commands = &output.viewport_output[&id].commands;
            assert!(commands.contains(&egui::ViewportCommand::OuterPosition(pos2(x, 60.0))), "{commands:?}");
            assert!(commands.contains(&egui::ViewportCommand::InnerSize(vec2(800.0, 972.0))), "{commands:?}");
        }
        assert!(app.ui.both_windows);
    }
}

#[test]
fn native_window_commands_work_in_logic_only_frames_while_minimized() {
    let ctx = egui::Context::default();
    ctx.set_embed_viewports(false);
    let mut app = app();
    let logic = |app: &mut SoundApp| {
        let mut raw = RawInput::default();
        raw.viewports.get_mut(&ViewportId::ROOT).unwrap().minimized = Some(true);
        let mut output = ctx.run_ui(raw, |ui| app.logic(ui.ctx()));
        output.textures_delta.clear();
        output
    };
    app.run("window.mix", json!({})).unwrap();
    let output = logic(&mut app);
    let commands = &output.viewport_output[&ViewportId::ROOT].commands;
    assert!(commands.contains(&egui::ViewportCommand::Minimized(false)));
    assert!(commands.contains(&egui::ViewportCommand::Focus));
    assert!(app.ui.both_windows);
    app.run("window.close", json!({})).unwrap();
    let _ = logic(&mut app);
    assert!(!app.ui.both_windows);
    assert_eq!(app.ui.window, MainWindow::Edit);
    assert!(!app.quit_requested);
}

#[test]
fn narrow_tiles_keep_valid_timeline_geometry_and_sidebar_preferences() {
    for midi in [false, true] {
        let mut app = app();
        let ctx = egui::Context::default();
        ready(&ctx, &mut app);
        app.ui.show_midi_editor = midi;
        app.run("window.arrange_tile_v", json!({})).unwrap();
        for size in [vec2(800.0, 700.0), vec2(800.0, 450.0), vec2(1600.0, 1000.0)] {
            app.run("window.arrange_tile_v", json!({})).unwrap();
            for _ in 0..6 {
                let _ = frame_at_size(&ctx, &mut app, vec![], size);
                let r = app.edit_layout.timeline;
                assert!(r.iter().all(|v| v.is_finite()));
                assert!(r[2] >= r[0] && r[3] >= r[1], "invalid timeline: {r:?}");
                assert!(app.ui.show_tracks_list && app.ui.show_clip_list);
            }
            if size.y >= 700.0 {
                let r = app.edit_layout.timeline;
                assert!(r[2] - r[0] >= 64.0 && r[3] - r[1] > 0.0, "timeline must remain usable: {r:?}");
            }
        }
    }
}
