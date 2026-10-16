//! Exercise the real divider input, persistence and document isolation.
use egui::{Event, PointerButton, Pos2, RawInput, Rect, pos2, vec2};
use serde_json::json;
use soundcraft_ui_egui::{Services, SoundApp, UiState, midi_editor::DEFAULT_VELOCITY_HEIGHT};

fn frame(ctx: &egui::Context, app: &mut SoundApp, events: Vec<Event>) {
    render(ctx, app, events).textures_delta.clear();
}

fn render(ctx: &egui::Context, app: &mut SoundApp, events: Vec<Event>) -> egui::FullOutput {
    ctx.run_ui(RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1600.0, 1000.0))), events, ..Default::default() }, |ui| {
        app.logic(ui.ctx());
        app.ui(ui);
    })
}

fn setup() -> (egui::Context, SoundApp) {
    let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
    app.ui.show_midi_editor = true;
    app.run("edit.select", json!({"tracks": ["Keys"]})).unwrap();
    app.run("ui.midi_scrolling", json!({"mode": "center"})).unwrap();
    app.engine.mark_clean();
    let ctx = egui::Context::default();
    for _ in 0..6 {
        frame(&ctx, &mut app, vec![]);
    }
    (ctx, app)
}

fn button(pos: Pos2, pressed: bool) -> Event {
    Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers: Default::default() }
}

fn separator(app: &SoundApp) -> Pos2 {
    let [x0, y0, x1, y1] = app.midi.velocity_divider;
    pos2((x0 + x1) * 0.5, (y0 + y1) * 0.5)
}

fn height(app: &SoundApp) -> f32 {
    app.midi.velocity[3] - app.midi.velocity[1]
}

fn start_drag(ctx: &egui::Context, app: &mut SoundApp) -> Pos2 {
    let start = separator(app);
    frame(ctx, app, vec![Event::PointerMoved(start)]);
    frame(ctx, app, vec![button(start, true)]);
    start
}

#[test]
fn divider_resizes_both_ways_and_keeps_notes_cues_and_preferences() {
    let (ctx, mut app) = setup();
    app.midi.selected = vec![0];
    let session = app.engine.session().clone();
    let undo = app.engine.undo_history();
    let follow = app.midi.follow.clone();
    let viewport = (app.midi.zoom, app.midi.scroll_ticks);
    let bottom = app.midi.velocity[3];
    for delta in [-80.0, 48.0] {
        let old_height = height(&app);
        let old_bottom = app.midi.roll[3];
        let start = start_drag(&ctx, &mut app);
        let end = start + vec2(0.0, delta);
        for step in 1..=6 {
            frame(&ctx, &mut app, vec![Event::PointerMoved(start.lerp(end, step as f32 / 6.0))]);
        }
        frame(&ctx, &mut app, vec![button(end, false)]);
        for _ in 0..4 {
            frame(&ctx, &mut app, vec![]);
        }
        assert!((height(&app) - (old_height - delta)).abs() < 0.1);
        assert!((app.midi.roll[3] - (old_bottom + delta)).abs() < 0.1);
        assert_eq!(app.midi.velocity[3], bottom);
        assert_eq!(app.midi.selected, vec![0]);
        assert_eq!(app.engine.session(), &session);
        assert_eq!(app.engine.undo_history(), undo);
        assert!(!app.engine.is_dirty());
        assert_eq!(app.midi.follow.owns_transport, follow.owns_transport);
        assert_eq!((app.midi.zoom, app.midi.scroll_ticks), viewport);
    }
    let preferred = app.ui.midi_velocity_height;
    let before = app.midi.velocity;
    app.run("window.midi_editor", json!({})).unwrap();
    frame(&ctx, &mut app, vec![]);
    app.run("window.midi_editor", json!({})).unwrap();
    let saved = serde_json::to_string(&app.ui).unwrap();
    app.ui = serde_json::from_str(&saved).unwrap();
    for _ in 0..4 {
        frame(&ctx, &mut app, vec![]);
    }
    assert_eq!(app.ui.midi_velocity_height, preferred);
    assert_eq!(app.midi.velocity, before);
    app.run("edit.select", json!({"tracks": ["Lead"]})).unwrap();
    frame(&ctx, &mut app, vec![]);
    assert_eq!(app.ui.midi_velocity_height, preferred);
    assert!((height(&app) - preferred).abs() < 0.1);
}

#[test]
fn divider_limits_and_reversing_at_a_limit_respond_immediately() {
    let (ctx, mut app) = setup();
    let start = start_drag(&ctx, &mut app);
    let above = start - vec2(0.0, 500.0);
    frame(&ctx, &mut app, vec![Event::PointerMoved(above)]);
    assert!((app.midi.roll[3] - app.midi.roll[1] - 72.0).abs() < 0.1);
    let maximum = height(&app);
    let reverse = above + vec2(0.0, 12.0);
    frame(&ctx, &mut app, vec![Event::PointerMoved(reverse)]);
    assert!((height(&app) - maximum + 12.0).abs() < 0.1);
    frame(&ctx, &mut app, vec![Event::PointerMoved(start + vec2(0.0, 500.0))]);
    assert!((height(&app) - 24.0).abs() < 0.1);
    frame(&ctx, &mut app, vec![button(start + vec2(0.0, 500.0), false)]);
    app.run("ui.midi_velocity_height", json!({"height": 4096})).unwrap();
    frame(&ctx, &mut app, vec![]);
    assert!(height(&app) < 4096.0);
    assert_eq!(app.ui.midi_velocity_height, 4096.0, "a small dock must not overwrite the preferred height");
    assert!(app.midi.roll[3] - app.midi.roll[1] >= 72.0);
}

#[test]
fn old_preferences_and_command_validation_are_safe() {
    let old: UiState = serde_json::from_str("{}").unwrap();
    assert_eq!(old.midi_velocity_height, DEFAULT_VELOCITY_HEIGHT);
    let (ctx, mut app) = setup();
    for params in [json!({"height": null}), json!({"height": "large"}), json!({"height": true})] {
        assert!(app.run("ui.midi_velocity_height", params).is_err());
        assert_eq!(app.ui.midi_velocity_height, DEFAULT_VELOCITY_HEIGHT);
    }
    for (requested, expected) in [(-1e300, 24.0), (1e300, 4096.0), (96.0, 96.0)] {
        let result = app.run("ui.midi_velocity_height", json!({"height": requested})).unwrap();
        assert_eq!(result["height"], json!(expected));
        assert_eq!(app.run("ui.midi_velocity_height", json!({})).unwrap(), result);
    }
    app.ui.midi_velocity_height = f32::NAN;
    frame(&ctx, &mut app, vec![]);
    assert!((height(&app) - DEFAULT_VELOCITY_HEIGHT).abs() < 0.1);
    assert!(app.midi.roll.iter().all(|v| v.is_finite()));
}

#[test]
fn hiding_editor_or_setting_height_cancels_a_held_divider() {
    for hide in [true, false] {
        let (ctx, mut app) = setup();
        let start = start_drag(&ctx, &mut app);
        let moved = start - vec2(0.0, 40.0);
        frame(&ctx, &mut app, vec![Event::PointerMoved(moved)]);
        assert!((height(&app) - 104.0).abs() < 0.1);
        if hide {
            app.run("window.midi_editor", json!({})).unwrap();
            frame(&ctx, &mut app, vec![]);
            app.run("window.midi_editor", json!({})).unwrap();
        } else {
            app.run("ui.midi_velocity_height", json!({"height": 80})).unwrap();
        }
        let expected = app.ui.midi_velocity_height;
        frame(&ctx, &mut app, vec![Event::PointerMoved(start - vec2(0.0, 80.0))]);
        assert_eq!(app.ui.midi_velocity_height, expected);
        frame(&ctx, &mut app, vec![button(start - vec2(0.0, 80.0), false)]);
    }
}

#[test]
fn focused_divider_supports_keyboard_resizing_without_moving_notes() {
    let (ctx, mut app) = setup();
    let session = app.engine.session().clone();
    let start = start_drag(&ctx, &mut app);
    frame(&ctx, &mut app, vec![button(start, false)]);
    for (key, expected) in [(egui::Key::ArrowUp, 72.0), (egui::Key::ArrowDown, 64.0)] {
        frame(&ctx, &mut app, vec![Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers: Default::default() }]);
        assert!((height(&app) - expected).abs() < 0.1, "{key:?}: height {}, focus {:?}", height(&app), ctx.memory(|m| m.focused()));
        frame(&ctx, &mut app, vec![Event::Key { key, physical_key: None, pressed: false, repeat: false, modifiers: Default::default() }]);
        assert_eq!(app.engine.session(), &session);
    }
}

#[test]
fn double_click_restores_default_height_without_editing_the_document() {
    let (ctx, mut app) = setup();
    app.run("ui.midi_velocity_height", json!({"height": 120})).unwrap();
    frame(&ctx, &mut app, vec![]);
    let session = app.engine.session().clone();
    let start = separator(&app);
    frame(&ctx, &mut app, vec![Event::PointerMoved(start)]);
    for _ in 0..2 {
        frame(&ctx, &mut app, vec![button(start, true)]);
        frame(&ctx, &mut app, vec![button(start, false)]);
    }
    assert_eq!(app.ui.midi_velocity_height, DEFAULT_VELOCITY_HEIGHT);
    assert!((height(&app) - DEFAULT_VELOCITY_HEIGHT).abs() < 0.1);
    assert_eq!(app.engine.session(), &session);
}

#[test]
fn velocity_editing_uses_the_resized_lane() {
    for requested in [24.0, 160.0] {
        let (ctx, mut app) = setup();
        app.run("ui.midi_velocity_height", json!({"height": requested})).unwrap();
        frame(&ctx, &mut app, vec![]);
        // Refresh hit-test geometry before pressing into the resized lane.
        frame(&ctx, &mut app, vec![]);
        let cid = app.midi.clip.unwrap();
        let note = match &app.engine.session().find_clip(cid).unwrap().1.content {
            soundcraft_model::ClipContent::Midi { sequence } => sequence.notes[0],
            _ => panic!("expected MIDI"),
        };
        let x = app.midi.roll[0]
            + ((note.start as f64 - app.midi.scroll_ticks) / app.midi.len_ticks * f64::from(app.midi.roll[2] - app.midi.roll[0]) * app.midi.zoom)
                as f32;
        let pos = pos2(x + 1.0, app.midi.velocity[3] - 2.0 - (height(&app) - 4.0) * 64.0 / 127.0);
        frame(&ctx, &mut app, vec![Event::PointerMoved(pos)]);
        frame(&ctx, &mut app, vec![button(pos, true)]);
        frame(&ctx, &mut app, vec![button(pos, false)]);
        match &app.engine.session().find_clip(cid).unwrap().1.content {
            soundcraft_model::ClipContent::Midi { sequence } => assert_eq!(sequence.notes[0].velocity, 64),
            _ => panic!("expected MIDI"),
        }
    }
}

fn overlapping_stems() -> (egui::Context, SoundApp) {
    let (ctx, mut app) = setup();
    let cid = app.midi.clip.unwrap();
    let count = notes(&app).len();
    app.run("midi.note_delete", json!({"clip":cid.0, "indices": (0..count).collect::<Vec<_>>()})).unwrap();
    for (pitch, start, velocity) in [(48, 0, 92), (52, 0, 120), (55, 480, 75)] {
        app.run("midi.note_add", json!({"clip":cid.0, "pitch":pitch, "start_ticks":start, "velocity":velocity})).unwrap();
    }
    app.engine.mark_clean();
    for _ in 0..3 {
        frame(&ctx, &mut app, vec![]);
    }
    (ctx, app)
}

fn notes(app: &SoundApp) -> Vec<soundcraft_midi::Note> {
    match &app.engine.session().find_clip(app.midi.clip.unwrap()).unwrap().1.content {
        soundcraft_model::ClipContent::Midi { sequence } => sequence.notes.clone(),
        _ => panic!("expected MIDI"),
    }
}

fn stem_at(app: &SoundApp, index: usize, velocity: u8) -> Pos2 {
    let note = notes(app)[index];
    let x = app.midi.roll[0]
        + ((note.start as f64 - app.midi.scroll_ticks) / app.midi.len_ticks * f64::from(app.midi.roll[2] - app.midi.roll[0]) * app.midi.zoom) as f32;
    pos2(x + 1.0, app.midi.velocity[3] - 2.0 - (height(app) - 4.0) * f32::from(velocity) / 127.0)
}

fn double_click(ctx: &egui::Context, app: &mut SoundApp, pos: Pos2) {
    frame(ctx, app, vec![Event::PointerMoved(pos)]);
    for _ in 0..2 {
        frame(ctx, app, vec![button(pos, true)]);
        frame(ctx, app, vec![button(pos, false)]);
    }
}

#[test]
fn double_click_resets_the_overlapping_stem_under_the_pointer_and_undo_restores_it() {
    for requested in [24.0, 160.0] {
        let (ctx, mut app) = overlapping_stems();
        if requested == 160.0 {
            app.run("ui.dock", json!({"operation":"float", "panel":"midi", "rect":[120,160,900,560]})).unwrap();
        }
        app.run("ui.midi_velocity_height", json!({"height":requested})).unwrap();
        for _ in 0..4 {
            frame(&ctx, &mut app, vec![]);
        }
        let before = app.engine.session().clone();
        let undo = app.engine.undo_history().len();
        let pos = stem_at(&app, 1, 120);
        double_click(&ctx, &mut app, pos);
        let mut expected = before.clone();
        match &mut expected.find_clip_mut(app.midi.clip.unwrap()).unwrap().content {
            soundcraft_model::ClipContent::Midi { sequence } => sequence.notes[1].velocity = 100,
            _ => panic!("expected MIDI"),
        }
        assert_eq!(app.engine.session(), &expected);
        assert_eq!(app.engine.undo_history().len(), undo + 1);
        assert_eq!(app.ui.midi_velocity_height, requested);
        assert!(app.engine.undo());
        assert_eq!(app.engine.session(), &before);
        assert!(app.engine.redo());
        assert_eq!(app.engine.session(), &expected);
    }
}

#[test]
fn dragging_an_overlapping_stem_keeps_its_note_when_crossing_another_handle() {
    let (ctx, mut app) = overlapping_stems();
    let start = stem_at(&app, 1, 120);
    frame(&ctx, &mut app, vec![Event::PointerMoved(start)]);
    frame(&ctx, &mut app, vec![button(start, true)]);
    for velocity in [100, 92, 80, 60] {
        let pos = stem_at(&app, 1, velocity);
        frame(&ctx, &mut app, vec![Event::PointerMoved(pos)]);
    }
    let end = stem_at(&app, 1, 60);
    frame(&ctx, &mut app, vec![button(end, false)]);
    assert_eq!(notes(&app).iter().map(|n| n.velocity).collect::<Vec<_>>(), vec![92, 60, 75]);
}

#[test]
fn double_click_in_empty_velocity_space_keeps_notes_and_undo_unchanged() {
    let (ctx, mut app) = overlapping_stems();
    let before = app.engine.session().clone();
    let undo = app.engine.undo_history().len();
    let pos = pos2(app.midi.velocity[2] - 20.0, app.midi.velocity[1] + 10.0);
    double_click(&ctx, &mut app, pos);
    assert_eq!(app.engine.session(), &before);
    assert_eq!(app.engine.undo_history().len(), undo);
}

#[test]
fn dragging_duplicate_notes_follows_the_edited_note_after_sorting_and_undoes_once() {
    let (ctx, mut app) = overlapping_stems();
    app.run("midi.note_edit", json!({"clip":app.midi.clip.unwrap().0,"index":1,"pitch":48})).unwrap();
    frame(&ctx, &mut app, vec![]);
    let before = app.engine.session().clone();
    let undo = app.engine.undo_history().len();
    let start = stem_at(&app, 1, 120);
    frame(&ctx, &mut app, vec![Event::PointerMoved(start)]);
    frame(&ctx, &mut app, vec![button(start, true)]);
    for velocity in [100, 92, 80, 60] {
        let pos = stem_at(&app, 1, velocity);
        frame(&ctx, &mut app, vec![Event::PointerMoved(pos)]);
    }
    let end = stem_at(&app, 0, 60);
    frame(&ctx, &mut app, vec![button(end, false)]);
    assert_eq!(notes(&app).iter().map(|n| n.velocity).collect::<Vec<_>>(), vec![60, 92, 75]);
    assert_eq!(app.engine.undo_history().len(), undo + 1);
    assert!(app.engine.undo());
    assert_eq!(app.engine.session(), &before);
}

#[test]
fn grabbing_a_tail_beside_another_stem_edits_its_own_note_and_supports_reset_and_undo() {
    let (ctx, mut app) = overlapping_stems();
    let cid = app.midi.clip.unwrap();
    app.run("midi.note_edit", json!({"clip":cid.0,"index":0,"length_ticks":3840})).unwrap();
    app.run("midi.note_edit", json!({"clip":cid.0,"index":2,"start_ticks":1920})).unwrap();
    for _ in 0..3 {
        frame(&ctx, &mut app, vec![]);
    }
    let before = app.engine.session().clone();
    let undo = app.engine.undo_history().len();
    let mut start = stem_at(&app, 0, 92);
    start.x = stem_at(&app, 2, 75).x;
    start.y += 3.0; // Grab inside the expanded target, just below the visible tail.
    assert!(start.x - stem_at(&app, 0, 92).x > 8.0, "the tail must work beyond the old stem hit area");
    frame(&ctx, &mut app, vec![Event::PointerMoved(start)]);
    frame(&ctx, &mut app, vec![button(start, true)]);
    for velocity in [80, 75, 60] {
        let mut pos = stem_at(&app, 0, velocity);
        pos.x = start.x;
        pos.y += 3.0;
        frame(&ctx, &mut app, vec![Event::PointerMoved(pos)]);
    }
    let end = pos2(start.x, stem_at(&app, 0, 60).y);
    frame(&ctx, &mut app, vec![button(end, false)]);
    assert_eq!(notes(&app).iter().map(|n| n.velocity).collect::<Vec<_>>(), vec![60, 120, 75]);
    assert_eq!(app.engine.undo_history().len(), undo + 1);
    assert!(app.engine.undo());
    assert_eq!(app.engine.session(), &before);
    double_click(&ctx, &mut app, start);
    assert_eq!(notes(&app).iter().map(|n| n.velocity).collect::<Vec<_>>(), vec![100, 120, 75]);
    assert!(app.engine.undo());
    assert_eq!(app.engine.session(), &before);
}

#[test]
fn rendered_tails_show_note_length_follow_zoom_and_pan_and_clip_to_the_lane() {
    let (ctx, mut app) = overlapping_stems();
    app.run("midi.note_edit", json!({"clip":app.midi.clip.unwrap().0,"index":0,"length_ticks":3840})).unwrap();
    app.run("ui.midi_scrolling", json!({"mode":"none"})).unwrap();
    app.run("ui.midi_zoom_at", json!({"factor":16,"anchor_px":0})).unwrap();
    let pixels_per_tick = f64::from(app.midi.roll[2] - app.midi.roll[0]) * app.midi.zoom / app.midi.len_ticks;
    app.run("ui.midi_scroll", json!({"by_px":960.0*pixels_per_tick})).unwrap();
    app.midi.selected = vec![0];
    let before = app.engine.session().clone();
    let undo = app.engine.undo_history().len();
    let mut output = render(&ctx, &mut app, vec![]);
    output.textures_delta.clear();
    let m = &app.midi;
    let end = m.roll[0] + ((3840.0 - m.scroll_ticks) * f64::from(m.roll[2] - m.roll[0]) * m.zoom / m.len_ticks) as f32;
    let tail = output
        .shapes
        .iter()
        .find(|shape| {
            matches!(&shape.shape,
                egui::Shape::LineSegment { points: [a,b], stroke }
                if a.y == b.y && a.y >= m.velocity[1] && a.y <= m.velocity[3] && stroke.width == 2.0 && stroke.color == egui::Color32::WHITE
            )
        })
        .expect("the offscreen note's visible tail must still be drawn");
    if let egui::Shape::LineSegment { points: [a, b], .. } = &tail.shape {
        assert_eq!(a.x, m.velocity[0]);
        assert!((b.x - end).abs() < 0.01, "the tail ends at the note's timeline end");
        assert!(tail.clip_rect.min.x >= m.velocity[0]);
        assert!(tail.clip_rect.min.y >= m.velocity[1]);
        assert!(tail.clip_rect.max.x <= m.velocity[2]);
        assert!(tail.clip_rect.max.y <= m.velocity[3]);
    }
    assert_eq!(app.engine.session(), &before);
    assert_eq!(app.engine.undo_history().len(), undo);
}
