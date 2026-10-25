use egui::os::OperatingSystem;
use egui::{Event, Key, Modifiers, PointerButton, Pos2, RawInput, Rect, pos2, vec2};
use serde_json::json;
use soundcraft_model::TrackHeight;
use soundcraft_ui_egui::{Services, SoundApp, shortcuts};

fn frame(ctx: &egui::Context, app: &mut SoundApp, events: Vec<Event>) {
    let raw = RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1600.0, 1000.0))), events, ..Default::default() };
    ctx.run_ui(raw, |ui| {
        app.logic(ui.ctx());
        app.ui(ui);
    })
    .textures_delta
    .clear();
}

fn press(ctx: &egui::Context, app: &mut SoundApp, key: Key, modifiers: Modifiers) {
    let events = [true, false].map(|pressed| Event::Key { key, physical_key: None, pressed, repeat: false, modifiers }).to_vec();
    ctx.run_ui(RawInput { events, ..Default::default() }, |ui| shortcuts::handle(app, ui.ctx())).textures_delta.clear();
}

fn button(pos: Pos2, pressed: bool) -> Event {
    Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers: Default::default() }
}

fn rig() -> (egui::Context, SoundApp) {
    let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
    let ctx = egui::Context::default();
    frame(&ctx, &mut app, vec![]);
    frame(&ctx, &mut app, vec![]);
    (ctx, app)
}

fn id(app: &SoundApp, name: &str) -> u64 {
    app.engine.session().track_by_name(name).unwrap().id.0
}

fn height(app: &SoundApp, name: &str) -> TrackHeight {
    app.engine.session().track_by_name(name).unwrap().height
}

fn row(app: &SoundApp, name: &str) -> [f32; 4] {
    let id = id(app, name);
    app.edit_layout.rows.iter().find(|(t, _)| *t == id).unwrap().1
}

fn select(app: &mut SoundApp, names: &[&str]) {
    let ids: Vec<u64> = names.iter().map(|n| id(app, n)).collect();
    app.engine.execute("edit.select", &json!({"tracks": ids})).unwrap();
}

/// Drags `name`'s bottom edge by `dy` with `m` held, holding still for a few frames before letting go.
fn drag_edge(ctx: &egui::Context, app: &mut SoundApp, name: &str, dy: f32, m: Modifiers) {
    let [x, _, _, bottom] = row(app, name);
    let at = pos2(x + 40.0, bottom - 1.0);
    frame(ctx, app, vec![Event::ModifiersChanged(m), Event::PointerMoved(at)]);
    frame(ctx, app, vec![button(at, true)]);
    frame(ctx, app, vec![Event::PointerMoved(at + vec2(0.0, dy))]);
    let held: Vec<TrackHeight> = app.engine.session().tracks.iter().map(|t| t.height).collect();
    for _ in 0..3 {
        frame(ctx, app, vec![]);
        assert!(app.engine.session().tracks.iter().map(|t| t.height).eq(held.iter().copied()), "the heights hold while the pointer does");
    }
    frame(ctx, app, vec![button(at + vec2(0.0, dy), false), Event::ModifiersChanged(Modifiers::NONE)]);
    frame(ctx, app, vec![]);
}

#[test]
fn dragging_a_track_edge_resizes_it_live_in_one_undo_step() {
    let (ctx, mut app) = rig();
    assert_eq!(height(&app, "Kick"), TrackHeight::Medium);
    let [x, _, _, bottom] = row(&app, "Kick");
    let at = |dy: f32| pos2(x + 40.0, bottom + dy);

    frame(&ctx, &mut app, vec![Event::PointerMoved(at(-1.0))]);
    frame(&ctx, &mut app, vec![button(at(-1.0), true)]);
    for (dy, want) in [(20.0, TrackHeight::Medium), (44.0, TrackHeight::Large), (132.0, TrackHeight::Jumbo), (40.0, TrackHeight::Large)] {
        frame(&ctx, &mut app, vec![Event::PointerMoved(at(dy))]);
        frame(&ctx, &mut app, vec![]);
        let [_, y0, _, y1] = row(&app, "Kick");
        assert_eq!((height(&app, "Kick"), y1 - y0), (want, want.points()), "{dy}");
    }
    frame(&ctx, &mut app, vec![button(at(40.0), false)]);
    frame(&ctx, &mut app, vec![]);

    app.engine.execute("edit.undo", &json!({})).unwrap();
    assert_eq!(height(&app, "Kick"), TrackHeight::Medium);
}

#[test]
fn cmd_drags_to_any_height_alt_resizes_every_track_and_alt_shift_the_selected_ones() {
    let (ctx, mut app) = rig();
    drag_edge(&ctx, &mut app, "Kick", 20.0, Modifiers::COMMAND);
    assert_eq!((height(&app, "Kick"), height(&app, "Snare")), (TrackHeight::Custom(108), TrackHeight::Medium));

    drag_edge(&ctx, &mut app, "Snare", 44.0, Modifiers::ALT);
    assert!(
        app.edit_layout.rows.iter().all(|(t, _)| app.engine.session().track(soundcraft_model::TrackId(*t)).unwrap().height == TrackHeight::Large)
    );

    select(&mut app, &["Hats", "Bass"]);
    drag_edge(&ctx, &mut app, "Kick", -86.0, Modifiers { alt: true, shift: true, ..Modifiers::NONE });
    let small = [TrackHeight::Small, TrackHeight::Large, TrackHeight::Small, TrackHeight::Small];
    assert_eq!(["Kick", "Snare", "Hats", "Bass"].map(|n| height(&app, n)), small);

    app.engine.execute("edit.undo", &json!({})).unwrap();
    assert_eq!(["Kick", "Snare", "Hats", "Bass"].map(|n| height(&app, n)), [TrackHeight::Large; 4]);
}

#[test]
fn ctrl_arrows_step_the_selected_tracks_and_ctrl_alt_arrows_every_track() {
    let pc_ctrl = Modifiers { ctrl: true, command: true, ..Modifiers::NONE };
    for (os, ctrl) in [(OperatingSystem::Mac, Modifiers::CTRL), (OperatingSystem::Windows, pc_ctrl), (OperatingSystem::Nix, pc_ctrl)] {
        let (ctx, mut app) = rig();
        ctx.set_os(os);
        select(&mut app, &["Kick"]);
        press(&ctx, &mut app, Key::ArrowUp, ctrl);
        assert_eq!((height(&app, "Kick"), height(&app, "Snare")), (TrackHeight::Large, TrackHeight::Medium), "{os:?}");
        press(&ctx, &mut app, Key::ArrowDown, ctrl);
        press(&ctx, &mut app, Key::ArrowDown, ctrl);
        assert_eq!(height(&app, "Kick"), TrackHeight::Small, "{os:?}");
        press(&ctx, &mut app, Key::ArrowUp, Modifiers { alt: true, ..ctrl });
        assert_eq!((height(&app, "Kick"), height(&app, "Snare")), (TrackHeight::Medium, TrackHeight::Large), "{os:?}");
        assert_eq!(app.engine.session().edit.selected_tracks.len(), 1, "{os:?}");
    }
}

#[test]
fn control_shift_p_and_semicolon_extend_the_edit_on_macos() {
    let (ctx, mut app) = rig();
    ctx.set_os(OperatingSystem::Mac);
    select(&mut app, &["Snare"]);
    let ctrl_shift = Modifiers { ctrl: true, shift: true, ..Modifiers::NONE };
    press(&ctx, &mut app, Key::P, ctrl_shift);
    press(&ctx, &mut app, Key::Colon, ctrl_shift);
    let selected: Vec<u64> = app.engine.session().edit.selected_tracks.iter().map(|t| t.0).collect();
    assert_eq!(selected, ["Kick", "Snare", "Hats"].map(|n| id(&app, n)));
}
