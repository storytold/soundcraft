use egui::accesskit::Role;
use egui::{Event, Key, Modifiers, PointerButton, vec2};
use egui_kittest::{Harness, kittest::Queryable};
use serde_json::json;
use soundcraft_audio_io::{AudioBuffer, FileFormat};
use soundcraft_engine::{Engine, io};
use soundcraft_midi::{Note, Sequence};
use soundcraft_model::{ChannelFormat, Clip, ClipContent, ClipDefinitionId, Session, SourceId, TrackId, TrackKind, VideoSource};
use soundcraft_ui_egui::{Services, SoundApp, panels, shortcuts};

const AUDIO_LABEL: &str = "Saved Audio (Audio)";
const MIDI_LABEL: &str = "Saved MIDI (MIDI)";
const VIDEO_LABEL: &str = "Saved Video (Video)";

struct Fixture {
    engine: Engine,
    audio_track: TrackId,
    audio_source: SourceId,
    definitions: [ClipDefinitionId; 3],
}

fn fixture() -> Fixture {
    let mut session = Session::default();
    let audio_track = session.add_track(TrackKind::Audio, ChannelFormat::Mono, Some("Audio destination"));
    let midi_track = session.add_track(TrackKind::Midi, ChannelFormat::Mono, Some("MIDI destination"));
    let video_track = session.add_track(TrackKind::Video, ChannelFormat::Mono, Some("Video destination"));
    let audio_source =
        io::add_source(&mut session, "Synthetic source", AudioBuffer { sample_rate: 48_000, channels: vec![vec![0.25; 128]] }, None, FileFormat::Wav);
    let audio_clip = session.new_clip_id();
    session.track_mut(audio_track).unwrap().playlist_mut().unwrap().clips.push(Clip::audio(audio_clip, "Saved Audio", audio_source, 3, 0, 64));
    let midi_clip = session.new_clip_id();
    let sequence =
        Sequence { notes: vec![Note { pitch: 60, velocity: 100, release_velocity: 64, channel: 0, start: 0, length: 120 }], ctrls: Vec::new() };
    session.track_mut(midi_track).unwrap().playlist_mut().unwrap().clips.push(Clip::midi(midi_clip, "Saved MIDI", 0, 64, sequence));
    let video_source = SourceId(session.alloc());
    session.videos.push(VideoSource {
        id: video_source,
        name: "Synthetic picture".into(),
        path: "synthetic.mov".into(),
        width: 320,
        height: 240,
        frame_rate: 24.0,
        duration: 1.0,
        codec: "synthetic".into(),
    });
    let video_clip = session.new_clip_id();
    session.track_mut(video_track).unwrap().playlist_mut().unwrap().clips.push(Clip::video(video_clip, "Saved Video", video_source, 7, 0, 64));
    session.edit.selection.start = 256;
    session.edit.selection.end = 256;
    session.edit.selected_tracks = vec![audio_track];
    let mut engine = Engine::new(session);
    let result = engine.execute("clip.capture", &json!({"clips": [audio_clip.0, midi_clip.0, video_clip.0]})).unwrap();
    let definitions = std::array::from_fn(|index| ClipDefinitionId(result["definitions"][index]["id"].as_u64().unwrap()));
    engine.execute("clip.clear", &json!({"clips": [audio_clip.0, midi_clip.0, video_clip.0]})).unwrap();
    Fixture { engine, audio_track, audio_source, definitions }
}

fn harness(engine: Engine) -> Harness<'static, SoundApp> {
    let app = SoundApp::new(engine, None, Services::default());
    let mut harness = Harness::builder().with_size(vec2(460.0, 360.0)).with_pixels_per_point(2.0).wgpu().build_ui_state(
        |ui, app: &mut SoundApp| {
            let context = ui.ctx().clone();
            app.logic(&context);
            // Production font installation takes effect on the next frame.
            if context.cumulative_frame_nr() == 0 {
                return;
            }
            shortcuts::handle(app, &context);
            panels::clip_list(app, ui);
        },
        app,
    );
    harness.input_mut().max_texture_side = Some(8192);
    harness.run_steps(5);
    harness
}

fn action(harness: &mut Harness<'_, SoundApp>, row: &str, action: &str) {
    harness.get_by_label(row).click_button(PointerButton::Secondary);
    harness.run_steps(3);
    harness.get_by_label(action).click();
    harness.run_steps(3);
}

fn undo(harness: &mut Harness<'_, SoundApp>) {
    for pressed in [true, false] {
        harness.event(Event::Key { key: Key::Z, physical_key: None, pressed, repeat: false, modifiers: Modifiers::COMMAND });
    }
    harness.run_steps(3);
}

fn proof(harness: &mut Harness<'_, SoundApp>, name: &str) {
    if let Some(directory) = std::env::var_os("SOUNDCRAFT_REUSABLE_CLIPS_PROOF_DIR").map(std::path::PathBuf::from) {
        std::fs::create_dir_all(&directory).unwrap();
        harness.render().unwrap().save(directory.join(name)).unwrap();
    }
}

#[test]
fn reusable_audio_midi_video_rows_share_the_existing_clip_filter() {
    let fixture = fixture();
    let mut harness = harness(fixture.engine);
    for label in [AUDIO_LABEL, MIDI_LABEL, VIDEO_LABEL] {
        assert!(harness.query_by_label(label).is_some(), "{label}");
    }
    proof(&mut harness, "reusable-clip-list.png");
    let before = harness.state().engine.session().clone();
    let revision = harness.state().engine.revision;
    let history = harness.state().engine.undo_history();
    harness.get_by_role(Role::TextInput).click();
    harness.run_steps(2);
    harness.event(Event::Text("mIdI".into()));
    harness.run_steps(2);
    assert!(harness.query_by_label(MIDI_LABEL).is_some());
    assert!(harness.query_by_label(AUDIO_LABEL).is_none());
    assert!(harness.query_by_label(VIDEO_LABEL).is_none());
    assert_eq!(*harness.state().engine.session(), before);
    assert_eq!(harness.state().engine.revision, revision);
    assert_eq!(harness.state().engine.undo_history(), history);
}

#[test]
fn context_menu_places_each_definition_on_a_new_track_and_keyboard_undo_restores_it() {
    let fixture = fixture();
    let definitions = fixture.definitions;
    let mut harness = harness(fixture.engine);
    let initial_tracks = harness.state().engine.session().tracks.len();
    for (index, (label, expected_kind)) in
        [(AUDIO_LABEL, TrackKind::Audio), (MIDI_LABEL, TrackKind::Midi), (VIDEO_LABEL, TrackKind::Video)].into_iter().enumerate()
    {
        action(&mut harness, label, "Place on New Track");
        let session = harness.state().engine.session();
        assert_eq!(session.tracks.len(), initial_tracks + 1);
        let (track, clip) = session.tracks.iter().find_map(|track| track.clips().first().map(|clip| (track, clip))).unwrap();
        assert_eq!(track.kind, expected_kind);
        assert_eq!((clip.start, clip.length, clip.name.as_str()), (256, 64, label.split(" (").next().unwrap()));
        assert_ne!(clip.id.0, definitions[index].0);
        assert_eq!(session.clip_definitions.len(), 3);
        if index == 0 {
            proof(&mut harness, "reusable-clip-placed.png");
        }
        undo(&mut harness);
        assert_eq!(harness.state().engine.session().tracks.len(), initial_tracks);
        assert_eq!(harness.state().engine.session().all_clips().count(), 0);
        assert!(harness.query_by_label(label).is_some());
    }
}

#[test]
fn selected_track_placement_and_definition_removal_use_real_menu_actions_and_undo() {
    let fixture = fixture();
    let (track, source, definition) = (fixture.audio_track, fixture.audio_source, fixture.definitions[0]);
    let mut harness = harness(fixture.engine);
    action(&mut harness, AUDIO_LABEL, "Place on Selected Track");
    let placed = harness.state().engine.session().track(track).unwrap().clips()[0].clone();
    assert_eq!(placed.content, ClipContent::Audio { source, offset: 3 });
    assert_eq!(placed.start, 256);
    assert_eq!(harness.state().engine.session().tracks.len(), 3);
    action(&mut harness, AUDIO_LABEL, "Remove Definition");
    assert!(harness.query_by_label(AUDIO_LABEL).is_none());
    let session = harness.state().engine.session();
    assert!(session.clip_definition(definition).is_none());
    assert_eq!(session.find_clip(placed.id).unwrap().1, &placed);
    assert!(session.source(source).is_some() && session.pool.contains(source));
    assert_eq!(session.clip_definitions.len(), 2);
    undo(&mut harness);
    assert!(harness.query_by_label(AUDIO_LABEL).is_some());
    assert!(harness.state().engine.session().clip_definition(definition).is_some());
    assert_eq!(harness.state().engine.session().find_clip(placed.id).unwrap().1, &placed);
    undo(&mut harness);
    assert_eq!(harness.state().engine.session().all_clips().count(), 0);
}
