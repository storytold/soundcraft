use egui::{Event, Key, Modifiers, PointerButton, Pos2, RawInput, Rect, pos2, vec2};
use egui_kittest::kittest::Queryable;
use serde_json::json;
use soundcraft_engine::Engine;
use soundcraft_engine::edit::{RELATIVE_GRID, SHUFFLE_LOCK, SNAP_TO_GRID};
use soundcraft_model::{Clip, ClipId, EditMode, Source, SourceId, TrackId};
use soundcraft_ui_egui::{Services, SoundApp, edit_window};

const SEC: i64 = 48_000;

fn frame(ctx: &egui::Context, app: &mut SoundApp, events: Vec<Event>) {
    let raw = RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1600.0, 1000.0))), events, ..Default::default() };
    ctx.run_ui(raw, |ui| {
        app.logic(ui.ctx());
        app.ui(ui);
    })
    .textures_delta
    .clear();
}

fn button(pos: Pos2, pressed: bool) -> Event {
    Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE }
}

fn key(key: Key) -> Vec<Event> {
    chord(key, Modifiers::NONE)
}

fn chord(key: Key, modifiers: Modifiers) -> Vec<Event> {
    [true, false].map(|pressed| key_event(key, pressed, modifiers)).to_vec()
}

fn key_event(key: Key, pressed: bool, modifiers: Modifiers) -> Event {
    Event::Key { key, physical_key: None, pressed, repeat: false, modifiers }
}

fn app(mode: &str, tool: &str, spans: &[(i64, i64)]) -> (SoundApp, egui::Context, TrackId, Vec<ClipId>) {
    let (app, ctx, tracks, ids) = tracks(mode, tool, &[spans]);
    (app, ctx, tracks[0], ids)
}

fn tracks(mode: &str, tool: &str, lanes: &[&[(i64, i64)]]) -> (SoundApp, egui::Context, Vec<TrackId>, Vec<ClipId>) {
    let mut e = Engine::default();
    let made = e.execute("track.new", &json!({"count": lanes.len()})).unwrap()["tracks"].clone();
    let tracks: Vec<TrackId> = (0..lanes.len()).map(|i| TrackId(made[i].as_u64().unwrap())).collect();
    let s = e.session_mut();
    s.sources.push(Source {
        id: SourceId(7),
        name: "src".into(),
        path: "src.wav".into(),
        channels: 1,
        frames: 20 * SEC as u64,
        sample_rate: 48_000,
        format: soundcraft_audio_io::FileFormat::Wav,
        time_reference: 0,
        unsaved: false,
    });
    let mut ids = Vec::new();
    for (t, spans) in tracks.iter().zip(lanes) {
        for &(start, len) in *spans {
            let id = s.new_clip_id();
            s.track_mut(*t).and_then(|t| t.playlist_mut()).unwrap().clips.push(Clip::audio(id, "c", SourceId(7), start, start, len));
            ids.push(id);
        }
    }
    e.execute("edit.mode", &json!({"mode": mode})).unwrap();
    e.execute("edit.tool", &json!({"tool": tool})).unwrap();
    e.execute("view.zoom_set", &json!({"samples_per_px": 400.0})).unwrap();
    let mut app = SoundApp::new(e, None, Services::default());
    let ctx = egui::Context::default();
    frame(&ctx, &mut app, vec![]);
    frame(&ctx, &mut app, vec![]);
    (app, ctx, tracks, ids)
}

fn point(app: &SoundApp, t: TrackId, at: i64) -> Pos2 {
    let s = app.engine.session();
    let [x0, y0, x1, y1] = app.edit_layout.timeline;
    let row = app.edit_layout.rows.iter().find(|(id, _)| *id == t.0).unwrap().1;
    let x = edit_window::x_of(s, Rect::from_min_max(pos2(x0, y0), pos2(x1, y1)), at);
    pos2(x, row[1] + s.track(t).unwrap().height.points() * 0.75)
}

fn drag(ctx: &egui::Context, app: &mut SoundApp, t: TrackId, from: i64, to: i64) {
    let (from, to) = (point(app, t, from), point(app, t, to));
    drag_between(ctx, app, from, to);
}

fn drag_between(ctx: &egui::Context, app: &mut SoundApp, from: Pos2, to: Pos2) {
    frame(ctx, app, vec![Event::PointerMoved(from)]);
    frame(ctx, app, vec![button(from, true)]);
    let started = from + vec2(7.0_f32.copysign(to.x - from.x), 0.0);
    for k in 0..=5 {
        frame(ctx, app, vec![Event::PointerMoved(started + (to - started) * (k as f32 / 5.0))]);
    }
    frame(ctx, app, vec![button(to, false)]);
    frame(ctx, app, vec![]);
}

fn click(ctx: &egui::Context, app: &mut SoundApp, at: Pos2) {
    frame(ctx, app, vec![Event::PointerMoved(at)]);
    frame(ctx, app, vec![button(at, true)]);
    frame(ctx, app, vec![button(at, false)]);
}

fn starts(app: &SoundApp, ids: &[ClipId]) -> Vec<i64> {
    ids.iter().map(|id| app.engine.session().find_clip(*id).unwrap().1.start).collect()
}

#[test]
fn shuffle_drags_swap_clips() {
    let (mut app, ctx, t, ids) = app("shuffle", "grabber", &[(0, SEC), (SEC, SEC)]);
    drag(&ctx, &mut app, t, SEC * 3 / 2, SEC / 2 - 2000);
    assert_eq!(starts(&app, &ids), vec![SEC, 0]);
}

#[test]
fn grid_drags_snap_the_dragged_clip_not_the_one_under_the_pointer() {
    let (mut app, ctx, t, ids) = app("grid", "grabber", &[(1000, SEC), (3 * SEC + 777, SEC)]);
    let s = app.engine.session();
    let step = s.edit.grid.step_samples(0, s.sample_rate, &s.tempo, s.frame_rate);
    drag(&ctx, &mut app, t, SEC / 2, 3 * SEC + SEC / 2);
    assert_eq!(starts(&app, &ids[..1])[0] % step, 0);
}

#[test]
fn spot_clicks_open_the_spot_dialog() {
    let (mut app, ctx, t, ids) = app("spot", "grabber", &[(SEC, SEC)]);
    let at = point(&app, t, SEC * 3 / 2);
    click(&ctx, &mut app, at);
    frame(&ctx, &mut app, vec![]);
    assert_eq!(app.dialogs.open_name(), Some("spot"));
    frame(&ctx, &mut app, vec![Event::Text("4.5".into())]);
    frame(&ctx, &mut app, key(Key::Enter));
    assert!(app.dialogs.open.is_none());
    assert_eq!(starts(&app, &ids), vec![SEC * 9 / 2]);
}

#[test]
fn spot_drags_ask_where_before_moving() {
    let (mut app, ctx, t, ids) = app("spot", "grabber", &[(SEC, SEC)]);
    drag(&ctx, &mut app, t, SEC * 3 / 2, SEC * 5 / 2);
    assert_eq!(starts(&app, &ids), vec![SEC]);
    assert_eq!(app.dialogs.open_name(), Some("spot"));
    frame(&ctx, &mut app, key(Key::Enter));
    assert!((starts(&app, &ids)[0] - 2 * SEC).abs() < 4000);
}

#[test]
fn slip_trims_stop_at_the_next_clip() {
    let (mut app, ctx, t, ids) = app("slip", "trim", &[(0, SEC), (2 * SEC, SEC)]);
    drag(&ctx, &mut app, t, SEC * 3 / 4, 4 * SEC);
    let s = app.engine.session();
    assert_eq!(s.find_clip(ids[0]).unwrap().1.end(), 2 * SEC);
    assert_eq!(starts(&app, &ids), vec![0, 2 * SEC]);
}

#[test]
fn f4_toggles_absolute_and_relative_grid_and_comes_back_to_the_last_one() {
    let (mut app, ctx, _, _) = app("slip", "grabber", &[]);
    let mode = |app: &SoundApp| app.engine.session().edit.edit_mode;
    let mut seen = Vec::new();
    for k in [Key::F4, Key::F4, Key::F4, Key::F4, Key::F2, Key::F4, Key::F1, Key::Backtick, Key::Backtick, Key::Backtick] {
        frame(&ctx, &mut app, key(k));
        seen.push(mode(&app));
    }
    use EditMode::*;
    assert_eq!(seen, [Grid, GridRelative, Grid, GridRelative, Slip, GridRelative, Shuffle, Slip, Spot, GridRelative]);
    app.engine.execute("options.keyboard_lock", &json!({})).unwrap();
    frame(&ctx, &mut app, key(Key::F4));
    frame(&ctx, &mut app, chord(Key::Num4, Modifiers::ALT));
    assert_eq!(mode(&app), GridRelative);
}

#[test]
fn snap_to_grid_snaps_the_edit_cursor_while_slip_moves_stay_free() {
    let (mut app, ctx, t, ids) = app("slip", "grabber", &[(1000, SEC)]);
    app.engine.execute("edit.snap_to_grid", &json!({})).unwrap();
    let s = app.engine.session();
    let step = s.edit.grid.step_samples(0, s.sample_rate, &s.tempo, s.frame_rate);
    drag(&ctx, &mut app, t, SEC / 2, SEC / 2 + 10_000);
    let moved = starts(&app, &ids)[0];
    assert!(moved > 1000 && moved % step != 0, "{moved}");
    app.engine.execute("edit.tool", &json!({"tool": "selector"})).unwrap();
    let at = point(&app, t, 3 * SEC + 1234);
    click(&ctx, &mut app, at);
    assert_eq!(app.engine.session().edit.selection.start % step, 0);
}

#[test]
fn mode_keys_add_snap_to_grid_and_cycle_past_a_locked_shuffle() {
    let (mut app, ctx, _, _) = app("slip", "grabber", &[]);
    let state = |app: &SoundApp| (app.engine.session().edit.edit_mode, app.engine.session().edit.flag(SNAP_TO_GRID));
    frame(&ctx, &mut app, chord(Key::F4, Modifiers::SHIFT));
    assert_eq!(state(&app), (EditMode::Slip, true));
    let f4 = |pressed| key_event(Key::F4, pressed, Modifiers::NONE);
    frame(&ctx, &mut app, vec![f4(true)]);
    assert_eq!(state(&app), (EditMode::Grid, false));
    frame(&ctx, &mut app, key(Key::F1));
    frame(&ctx, &mut app, vec![f4(false)]);
    assert_eq!(state(&app), (EditMode::Shuffle, true));
    frame(&ctx, &mut app, chord(Key::Num3, Modifiers::ALT));
    assert_eq!(state(&app), (EditMode::Spot, false));
    app.engine.execute("edit.shuffle_lock", &json!({})).unwrap();
    let mut seen = Vec::new();
    for _ in 0..4 {
        frame(&ctx, &mut app, key(Key::Backtick));
        seen.push(state(&app).0);
    }
    assert_eq!(seen, [EditMode::Grid, EditMode::Slip, EditMode::Spot, EditMode::Grid]);
    frame(&ctx, &mut app, key(Key::F1));
    assert_eq!(state(&app).0, EditMode::Grid);
}

#[test]
fn toolbar_clicks_add_snap_to_grid_lock_shuffle_and_switch_grid_modes_despite_the_keyboard_lock() {
    let mut e = Engine::default();
    e.execute("edit.mode", &json!({"mode": "slip"})).unwrap();
    let app = SoundApp::new(e, None, Services::default());
    let mut h = egui_kittest::Harness::builder().with_size(vec2(1600.0, 1000.0)).build_ui_state(
        |ui, app: &mut SoundApp| {
            let ctx = ui.ctx().clone();
            app.logic(&ctx);
            app.ui(ui);
        },
        app,
    );
    h.run_steps(3);
    let click = |h: &mut egui_kittest::Harness<'_, SoundApp>, label: &str, modifiers: Modifiers| {
        h.get_by_label(label).click_modifiers(modifiers);
        h.step();
    };
    let state = |h: &egui_kittest::Harness<'_, SoundApp>| {
        let ed = &h.state().engine.session().edit;
        (ed.edit_mode, ed.flag(SNAP_TO_GRID), ed.flag(SHUFFLE_LOCK))
    };
    click(&mut h, "GRID", Modifiers::SHIFT);
    assert_eq!(state(&h), (EditMode::Slip, true, false));
    click(&mut h, "SPOT", Modifiers::NONE);
    assert_eq!(state(&h), (EditMode::Spot, false, false));
    click(&mut h, "SHUFFLE", Modifiers::COMMAND);
    click(&mut h, "SHUFFLE", Modifiers::NONE);
    assert_eq!(state(&h), (EditMode::Spot, false, true));
    assert!(h.state().ui.status.contains("locked"), "{}", h.state().ui.status);
    click(&mut h, "SLIP", Modifiers::SHIFT);
    assert_eq!(state(&h), (EditMode::Slip, true, true));
    h.state_mut().engine.execute("options.keyboard_lock", &json!({})).unwrap();
    for (label, want) in [("GRID", EditMode::Grid), ("GRID", EditMode::GridRelative), ("SLIP", EditMode::Slip), ("GRID", EditMode::GridRelative)] {
        click(&mut h, label, Modifiers::NONE);
        assert_eq!(state(&h).0, want, "{label}");
    }
}

#[test]
fn dragging_clips_on_two_tracks_moves_both_down_a_track() {
    let (mut app, ctx, t, ids) = tracks("slip", "grabber", &[&[(0, SEC)], &[(2 * SEC, SEC)], &[]]);
    app.engine.execute("edit.select", &json!({"clips": [ids[0].0, ids[1].0]})).unwrap();
    let (from, to) = (point(&app, t[0], SEC / 2), point(&app, t[1], SEC / 2));
    drag_between(&ctx, &mut app, from, to);
    let s = app.engine.session();
    assert_eq!(s.find_clip(ids[0]).map(|(t, _)| t), Some(t[1]));
    assert_eq!(s.find_clip(ids[1]).map(|(t, _)| t), Some(t[2]));
}

#[test]
fn shuffle_drags_of_clips_on_two_tracks_land_on_the_grabbed_tracks_boundaries() {
    let lanes: [&[(i64, i64)]; 2] = [&[(4 * SEC, SEC), (5 * SEC, 3 * SEC), (17 * SEC / 2, SEC / 2)], &[(0, SEC), (SEC, 6 * SEC)]];
    let (mut app, ctx, t, ids) = tracks("shuffle", "grabber", &lanes);
    app.engine.execute("edit.select", &json!({"clips": [ids[0].0, ids[3].0]})).unwrap();
    let before = starts(&app, &ids);
    drag(&ctx, &mut app, t[0], 9 * SEC / 2, 9 * SEC / 2 + 3 * SEC / 10);
    assert_eq!(starts(&app, &ids), before);
}

#[test]
fn shuffle_drags_leave_locked_clips_in_the_selection_out_of_it() {
    let (mut app, ctx, t, ids) = app("shuffle", "grabber", &[(0, SEC), (3 * SEC, SEC)]);
    app.engine.session_mut().find_clip_mut(ids[0]).unwrap().time_locked = true;
    app.engine.execute("edit.select", &json!({"clips": [ids[0].0, ids[1].0]})).unwrap();
    drag(&ctx, &mut app, t, 7 * SEC / 2, 2 * SEC);
    assert_eq!(starts(&app, &ids), vec![0, SEC]);
}

#[test]
fn f4_pressed_before_f1_leaves_the_kind_of_grid_alone() {
    let (mut app, ctx, _, _) = app("grid", "grabber", &[]);
    let f4 = |pressed| key_event(Key::F4, pressed, Modifiers::NONE);
    frame(&ctx, &mut app, vec![f4(true)]);
    frame(&ctx, &mut app, key(Key::F1));
    frame(&ctx, &mut app, vec![f4(false)]);
    let ed = &app.engine.session().edit;
    assert_eq!((ed.edit_mode, ed.flag(SNAP_TO_GRID), ed.flag(RELATIVE_GRID)), (EditMode::Shuffle, true, false));
}
