use egui::{Event, Id, PointerButton, Pos2, RawInput, Rect, pos2, vec2};
use serde_json::json;
use soundcraft_ui_egui::{Services, SoundApp, midi_editor};

fn frame(ctx: &egui::Context, app: &mut SoundApp, events: Vec<Event>) {
    let raw = RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1600.0, 1000.0))), events, ..Default::default() };
    ctx.run_ui(raw, |ui| {
        app.logic(ui.ctx());
        app.ui(ui);
    })
    .textures_delta
    .clear();
}

/// The MIDI editor's last laid-out body rectangle, stored by the dock.
fn panel(ctx: &egui::Context) -> Rect {
    ctx.data(|data| data.get_temp::<Rect>(Id::unique("soundcraft-midi-body"))).unwrap()
}

fn button(pos: Pos2, pressed: bool) -> Event {
    Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers: Default::default() }
}

#[test]
fn a_selected_video_clip_is_not_a_midi_target() {
    use soundcraft_model::{ChannelFormat, Clip, ClipContent, SourceId, Track, TrackKind};
    let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
    app.run("edit.select", json!({"tracks": ["Keys"]})).unwrap();
    let midi = midi_editor::target_clip(&app).unwrap();
    let s = app.engine.session_mut();
    let track = s.add_track(TrackKind::Video, ChannelFormat::Mono, None);
    let video = s.new_clip_id();
    let source = SourceId(s.alloc());
    s.track_mut(track).and_then(Track::playlist_mut).unwrap().clips.push(Clip::video(video, "picture", source, 0, 0, 48_000));

    app.run("edit.select", json!({"clips": [video.0]})).unwrap();
    assert_eq!(midi_editor::target_clip(&app), None, "a video clip alone leaves the MIDI editors without a target");
    app.run("edit.select", json!({"clips": [video.0, midi.0]})).unwrap();
    assert_eq!(midi_editor::target_clip(&app), Some(midi), "the MIDI clip after a video clip is the target");
    let (_, clip) = app.engine.session().find_clip(midi).unwrap();
    assert!(matches!(clip.content, ClipContent::Midi { .. }));
}

#[test]
fn only_pencil_inserts_notes_in_the_midi_roll() {
    use soundcraft_model::{ClipContent, Tool};

    let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
    app.ui.show_midi_editor = true;
    let created = app.run("midi.new_clip", json!({"track": "Keys", "start": 0, "length": 96000})).unwrap();
    let clip = created["clip"].as_u64().unwrap();
    app.run("edit.select", json!({"clips": [clip]})).unwrap();
    let ctx = egui::Context::default();
    soundcraft_ui_egui::framework::install_headless(&ctx);
    for _ in 0..6 {
        frame(&ctx, &mut app, vec![]);
    }
    let body = panel(&ctx);
    let point = pos2(body.left() + 200.0, body.top() + 60.0);
    for tool in Tool::ALL.into_iter().filter(|tool| *tool != Tool::Pencil).chain([Tool::Pencil]) {
        app.run("edit.tool", json!({"tool": tool.id()})).unwrap();
        let before = app.engine.session().clone();
        frame(&ctx, &mut app, vec![Event::PointerMoved(point)]);
        frame(&ctx, &mut app, vec![button(point, true)]);
        frame(&ctx, &mut app, vec![button(point, false)]);
        let (_, edited) = app.engine.session().find_clip(soundcraft_model::ClipId(clip)).unwrap();
        let ClipContent::Midi { sequence } = &edited.content else { panic!("MIDI clip required") };
        if tool == Tool::Pencil {
            assert_eq!(sequence.notes.len(), 1);
            app.run("edit.undo", json!({})).unwrap();
        }
        assert!(*app.engine.session() == before, "{tool:?} click must not change the session");
    }
}
