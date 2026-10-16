use serde_json::{Value, json};
use soundcraft_audio_io::{AudioBuffer, FileFormat};
use soundcraft_engine::{Engine, autosave, io};
use soundcraft_midi::{CtrlEvent, CtrlKind, Note, Sequence};
use soundcraft_model::{ChannelFormat, Clip, ClipContent, ClipDefinitionId, ClipId, Session, SourceId, TrackId, TrackKind, VideoSource};
use soundcraft_time::Range;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

struct Directory(PathBuf);
impl Directory {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!("soundcraft-clip-lifecycle-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }
    fn path(&self, relative: &str) -> PathBuf {
        self.0.join(relative)
    }
    fn text(&self, relative: &str) -> String {
        self.path(relative).to_string_lossy().into_owned()
    }
}
impl Drop for Directory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[derive(Debug, PartialEq)]
struct State {
    session: Session,
    revision: u64,
    dirty: bool,
    undo: Vec<String>,
    redo: Option<String>,
    path: Option<String>,
    journal: Vec<(String, Value)>,
}
impl State {
    fn of(engine: &Engine) -> Self {
        Self {
            session: engine.session().clone(),
            revision: engine.revision,
            dirty: engine.is_dirty(),
            undo: engine.undo_history(),
            redo: engine.redo_label().map(str::to_owned),
            path: engine.path.clone(),
            journal: engine.journal.clone(),
        }
    }
}

fn sample(channel: usize, frame: usize) -> f32 {
    if channel == 0 { (frame as f32 - 23.0) / 128.0 } else { (13.0 - frame as f32) / 64.0 }
}
fn buffer() -> AudioBuffer {
    AudioBuffer { sample_rate: 48_000, channels: (0..2).map(|channel| (0..96).map(|frame| sample(channel, frame)).collect()).collect() }
}
fn audio() -> (Engine, TrackId, ClipId, SourceId) {
    let mut session = Session::default();
    let track = session.add_track(TrackKind::Audio, ChannelFormat::Stereo, Some("Synthetic"));
    session.track_mut(track).unwrap().mixer.pan = vec![-1.0, 1.0];
    let source = io::add_source(&mut session, "Synthetic", buffer(), None, FileFormat::Wav);
    let clip = session.new_clip_id();
    session.track_mut(track).unwrap().playlist_mut().unwrap().clips.push(Clip::audio(clip, "Whole", source, 5, 11, 32));
    (Engine::new(session), track, clip, source)
}
fn capture(engine: &mut Engine, clip: ClipId) -> ClipDefinitionId {
    let result = engine.execute("clip.capture", &json!({"clip": clip.0})).unwrap();
    ClipDefinitionId(result["definitions"][0]["id"].as_u64().unwrap())
}
fn remove_instances(engine: &mut Engine) {
    let clips: Vec<_> = engine.session().all_clips().map(|clip| clip.id.0).collect();
    engine.execute("clip.clear", &json!({"clips": clips})).unwrap();
    assert_eq!(engine.session().all_clips().count(), 0);
}
fn place(engine: &mut Engine, definition: ClipDefinitionId, track: TrackId, at: i64) -> ClipId {
    ClipId(
        engine.execute("clip.place_definition", &json!({"definition": definition.0, "track": track.0, "at": at})).unwrap()["clip"].as_u64().unwrap(),
    )
}
fn pcm(engine: &Engine, track: TrackId, at: i64) {
    let rendered = soundcraft_mix::render_clips(engine.session(), track, Range { start: at, end: at + 32 });
    assert_eq!(rendered.len(), 2);
    for (channel, data) in rendered.iter().enumerate() {
        assert_eq!(data.len(), 32);
        for (frame, actual) in data.iter().enumerate() {
            let expected = sample(channel, frame + 5);
            assert!((*actual - expected).abs() < 1e-7, "channel {channel}, frame {frame}: {actual} != {expected}");
        }
    }
}

#[test]
fn definition_only_audio_survives_native_save_and_open_with_original_pcm() {
    let dir = Directory::new();
    let (mut engine, track, clip, _) = audio();
    let original = engine.session().find_clip(clip).unwrap().1.clone();
    let definition = capture(&mut engine, clip);
    assert_eq!(engine.session().find_clip(clip).unwrap().1, &original);
    let retained = engine.session().clip_definition(definition).unwrap().clone();
    remove_instances(&mut engine);
    assert_eq!(io::save_session(&mut engine, &dir.text("Saved.scraft")).unwrap(), 1);
    let document: Value = serde_json::from_slice(&std::fs::read(dir.path("Saved.scraft")).unwrap()).unwrap();
    assert_eq!(document["format_version"], 2);
    let mut opened = Engine::default();
    assert!(io::open_session(&mut opened, &dir.text("Saved.scraft")).unwrap().is_empty());
    assert_eq!(opened.session().all_clips().count(), 0);
    assert_eq!(opened.session().clip_definition(definition), Some(&retained));
    place(&mut opened, definition, track, 103);
    pcm(&opened, track, 103);
}

#[test]
fn save_as_and_save_copy_keep_definition_ids_and_copy_preserves_open_state() {
    let dir = Directory::new();
    let (mut engine, track, clip, _) = audio();
    let definition = capture(&mut engine, clip);
    remove_instances(&mut engine);
    engine.execute("track.new", &json!({"name": "Undoable"})).unwrap();
    assert!(engine.undo());
    engine.execute("session.save_as", &json!({"path": dir.text("A/Song.scraft")})).unwrap();
    assert!(engine.redo_label().is_some());
    let params = json!({"path": dir.text("B/Copy.scraft")});
    let mut expected = State::of(&engine);
    expected.journal.push(("session.save_copy".into(), params.clone()));
    engine.execute("session.save_copy", &params).unwrap();
    assert_eq!(State::of(&engine), expected);
    let mut copied = Engine::default();
    assert!(io::open_session(&mut copied, &dir.text("B/Copy.scraft")).unwrap().is_empty());
    assert_eq!(copied.session().clip_definitions[0].id, definition);
    place(&mut copied, definition, track, 203);
    pcm(&copied, track, 203);
}

#[test]
fn bundle_round_trip_keeps_definition_only_audio_and_leaves_document_unchanged() {
    let (mut engine, track, clip, _) = audio();
    let definition = capture(&mut engine, clip);
    remove_instances(&mut engine);
    let before = State::of(&engine);
    let (bytes, _) = io::session_bundle(engine.session()).unwrap();
    assert_eq!(State::of(&engine), before);
    let mut opened = Engine::default();
    assert!(io::open_session_bytes(&mut opened, "Retained.scraftz", &bytes).unwrap().is_empty());
    assert_eq!(opened.session().all_clips().count(), 0);
    assert_eq!(opened.session().clip_definition(definition), engine.session().clip_definition(definition));
    place(&mut opened, definition, track, 303);
    pcm(&opened, track, 303);
}

#[test]
fn actual_autosaver_recovers_definition_only_audio_without_rewriting_its_copy() {
    let dir = Directory::new();
    let (mut engine, track, clip, _) = audio();
    let definition = capture(&mut engine, clip);
    remove_instances(&mut engine);
    let before = State::of(&engine);
    let root = dir.path("Recovery");
    let writer = autosave::Autosaver::new(root.clone());
    writer.write(engine.session_arc(), None, None);
    writer.flush().unwrap();
    let folder = std::fs::read_dir(&root).unwrap().next().unwrap().unwrap().path();
    let files: Vec<_> = std::fs::read_dir(folder.join("Audio Files")).unwrap().map(|entry| entry.unwrap().path()).collect();
    assert_eq!(files.len(), 1);
    let bytes = std::fs::read(&files[0]).unwrap();
    writer.write(engine.session_arc(), None, None);
    writer.flush().unwrap();
    assert_eq!(std::fs::read_dir(folder.join("Audio Files")).unwrap().count(), 1);
    assert_eq!(std::fs::read(&files[0]).unwrap(), bytes);
    drop(writer);
    assert_eq!(State::of(&engine), before);
    let leftovers = autosave::leftovers(&root);
    assert_eq!(leftovers.len(), 1);
    let mut recovered = Engine::default();
    assert!(autosave::recover(&mut recovered, &leftovers[0]).unwrap().is_empty());
    assert_eq!(recovered.session().all_clips().count(), 0);
    place(&mut recovered, definition, track, 403);
    pcm(&recovered, track, 403);
}

#[test]
fn midi_saved_performance_and_movie_reference_survive_without_instances() {
    let dir = Directory::new();
    std::fs::write(dir.path("Synthetic.mov"), b"synthetic referenced movie").unwrap();
    let mut session = Session::default();
    let midi_track = session.add_track(TrackKind::Midi, ChannelFormat::Mono, None);
    let video_track = session.add_track(TrackKind::Video, ChannelFormat::Mono, None);
    let sequence = Sequence {
        notes: vec![Note { pitch: 60, velocity: 100, release_velocity: 31, channel: 2, start: 17, length: 120 }],
        ctrls: vec![
            CtrlEvent { tick: 0, channel: 2, kind: CtrlKind::Cc(64), value: 127 },
            CtrlEvent { tick: 0, channel: 2, kind: CtrlKind::Cc(64), value: 0 },
        ],
    };
    let mut performance = sequence.clone();
    performance.notes[0].pitch = 62;
    let midi_clip = session.new_clip_id();
    session.track_mut(midi_track).unwrap().playlist_mut().unwrap().clips.push(Clip::midi(midi_clip, "Notes", 211, 12_000, sequence.clone()));
    session.midi_originals.insert(midi_clip.0, performance.clone());
    let movie_source = SourceId(session.alloc());
    session.videos.push(VideoSource {
        id: movie_source,
        name: "Picture".into(),
        path: dir.text("Synthetic.mov"),
        width: 320,
        height: 240,
        frame_rate: 24.0,
        duration: 1.0,
        codec: "synthetic".into(),
    });
    let movie_clip = session.new_clip_id();
    session.track_mut(video_track).unwrap().playlist_mut().unwrap().clips.push(Clip::video(movie_clip, "Picture", movie_source, 31, 211, 421));
    let mut engine = Engine::new(session);
    let midi_definition = capture(&mut engine, midi_clip);
    let movie_definition = capture(&mut engine, movie_clip);
    remove_instances(&mut engine);
    io::save_session(&mut engine, &dir.text("Mixed.scraft")).unwrap();
    let mut opened = Engine::default();
    assert!(io::open_session(&mut opened, &dir.text("Mixed.scraft")).unwrap().is_empty());
    assert_eq!(opened.session().all_clips().count(), 0);
    let fresh_midi = place(&mut opened, midi_definition, midi_track, 64_000);
    assert_eq!(opened.session().find_clip(fresh_midi).unwrap().1.content, ClipContent::Midi { sequence });
    assert_eq!(opened.session().midi_originals.get(&fresh_midi.0), Some(&performance));
    let fresh_movie = place(&mut opened, movie_definition, video_track, 70_000);
    assert_eq!(opened.session().find_clip(fresh_movie).unwrap().1.content, ClipContent::Video { source: movie_source, offset: 31 });
    assert_eq!(opened.session().video(movie_source).unwrap().path, dir.text("Synthetic.mov"));
}

#[test]
fn definitions_guard_primary_and_elastic_sources_and_undo_restores_ownership() {
    let dir = Directory::new();
    let (mut engine, _, clip, source) = audio();
    let original_source = io::add_source(engine.session_mut(), "Unwarped", buffer(), None, FileFormat::Wav);
    for (key, value) in [("src", original_source.0 as f64), ("off", 2.0), ("len", 40.0)] {
        engine.session_mut().edit.values.insert(format!("elastic.{}.{key}", clip.0), value);
    }
    let definition = capture(&mut engine, clip);
    remove_instances(&mut engine);
    io::save_session(&mut engine, &dir.text("Owned.scraft")).unwrap();
    let files: Vec<_> = engine
        .session()
        .sources
        .iter()
        .map(|source| {
            let path = dir.path(&source.path);
            let bytes = std::fs::read(&path).unwrap();
            (path, bytes)
        })
        .collect();
    for retained_source in [source, original_source] {
        for with_clips in [false, true] {
            let before = State::of(&engine);
            let error = engine.execute("clip.clear", &json!({"sources": [retained_source.0], "with_clips": with_clips})).unwrap_err();
            assert!(error.to_string().contains("retained by reusable definition"), "{error}");
            assert_eq!(State::of(&engine), before);
        }
    }
    let held = engine.session_arc();
    engine.execute("clip.clear_definitions", &json!({"definitions": [definition.0]})).unwrap();
    engine.execute("clip.clear", &json!({"sources": [source.0, original_source.0]})).unwrap();
    assert!(engine.session().sources.is_empty());
    assert!(held.pool.contains(source) && held.pool.contains(original_source));
    for (path, bytes) in files {
        assert_eq!(std::fs::read(path).unwrap(), bytes);
    }
    assert!(engine.undo());
    assert!(engine.session().pool.contains(source) && engine.session().pool.contains(original_source));
    assert!(engine.undo());
    assert!(engine.session().clip_definition(definition).is_some());
}

#[test]
fn clearing_definitions_preserves_placed_clips_media_and_undo_redo() {
    let (mut engine, track, clip, source) = audio();
    let definition = capture(&mut engine, clip);
    remove_instances(&mut engine);
    let placed = place(&mut engine, definition, track, 503);
    let retained = engine.session().find_clip(placed).unwrap().1.clone();
    engine.execute("clip.clear_definitions", &json!({"definitions": [definition.0]})).unwrap();
    assert!(engine.session().clip_definitions.is_empty());
    assert_eq!(engine.session().find_clip(placed).unwrap().1, &retained);
    assert!(engine.session().source(source).is_some() && engine.session().pool.contains(source));
    pcm(&engine, track, 503);
    assert!(engine.undo());
    assert!(engine.session().clip_definition(definition).is_some());
    assert!(engine.redo());
    assert!(engine.session().clip_definitions.is_empty());
    assert_eq!(engine.session().find_clip(placed).unwrap().1, &retained);
}

#[test]
fn malformed_or_future_definition_documents_preserve_pending_redo_on_open() {
    let dir = Directory::new();
    let (mut engine, _, clip, _) = audio();
    capture(&mut engine, clip);
    engine.execute("track.new", &json!({"name": "Pending redo"})).unwrap();
    assert!(engine.undo());
    let before = State::of(&engine);
    let valid: Value = serde_json::from_str(&engine.session().to_json().unwrap()).unwrap();
    let mut bad_geometry = valid.clone();
    bad_geometry["clip_definitions"][0]["template"]["length"] = json!(0);
    let mut bad_source = valid.clone();
    bad_source["clip_definitions"][0]["template"]["content"]["Audio"]["source"] = json!(999_999);
    let mut future = valid;
    future["format_version"] = json!(3);
    for bytes in
        [serde_json::to_vec(&bad_geometry).unwrap(), serde_json::to_vec(&bad_source).unwrap(), serde_json::to_vec(&future).unwrap(), b"{".to_vec()]
    {
        std::fs::write(dir.path("Bad.scraft"), &bytes).unwrap();
        assert!(engine.execute("session.open", &json!({"path": dir.text("Bad.scraft")})).is_err());
        assert_eq!(State::of(&engine), before);
        assert!(io::open_session_bytes(&mut engine, "Bad.scraft", &bytes).is_err());
        assert_eq!(State::of(&engine), before);
    }
}

#[test]
fn invalid_definitions_refuse_native_copy_bundle_and_autosave_before_side_effects() {
    let dir = Directory::new();
    let (mut engine, _, clip, _) = audio();
    capture(&mut engine, clip);
    remove_instances(&mut engine);
    io::save_session(&mut engine, &dir.text("Existing.scraft")).unwrap();
    let existing = std::fs::read(dir.path("Existing.scraft")).unwrap();
    io::add_source(engine.session_mut(), "Must not write", buffer(), None, FileFormat::Wav);
    engine.session_mut().clip_definitions[0].template.length = 0;
    let before = State::of(&engine);
    assert!(io::save_session(&mut engine, &dir.text("Existing.scraft")).is_err());
    assert_eq!(std::fs::read(dir.path("Existing.scraft")).unwrap(), existing);
    assert_eq!(std::fs::read_dir(dir.path("Audio Files")).unwrap().count(), 1);
    assert!(io::save_session(&mut engine, &dir.text("Refused/New.scraft")).is_err());
    assert!(io::save_session_copy(&engine, &dir.text("Refused/Copy.scraft")).is_err());
    assert!(io::save_copy(&engine, &dir.text("Refused/Recovery.scraft")).is_err());
    assert!(!dir.path("Refused").exists());
    assert!(io::session_bundle(engine.session()).is_err());
    let recovery_root = dir.path("Refused recovery");
    let writer = autosave::Autosaver::new(recovery_root.clone());
    writer.write(engine.session_arc(), None, None);
    assert!(writer.flush().is_err());
    drop(writer);
    assert!(!recovery_root.exists());
    assert_eq!(State::of(&engine), before);
}

#[test]
fn missing_audio_is_reported_without_discarding_its_definition() {
    let dir = Directory::new();
    let (mut engine, track, clip, source) = audio();
    let definition = capture(&mut engine, clip);
    remove_instances(&mut engine);
    io::save_session(&mut engine, &dir.text("Offline.scraft")).unwrap();
    std::fs::remove_file(dir.path(&engine.session().source(source).unwrap().path)).unwrap();
    let mut opened = Engine::default();
    assert_eq!(io::open_session(&mut opened, &dir.text("Offline.scraft")).unwrap().len(), 1);
    assert_eq!(opened.session().clip_definition(definition), engine.session().clip_definition(definition));
    assert!(opened.session().source(source).is_some());
    assert!(!opened.session().pool.contains(source));
    let before = State::of(&opened);
    assert!(opened.execute("clip.place_definition", &json!({"definition": definition.0, "track": track.0, "at": 603})).is_err());
    assert_eq!(State::of(&opened), before);
    io::save_copy(&opened, &dir.text("Recovery/Offline.scraft")).unwrap();
    assert_eq!(State::of(&opened), before);
}

#[test]
fn session_templates_clear_definitions_and_media_without_changing_open_session() {
    let dir = Directory::new();
    let (mut engine, _, clip, _) = audio();
    capture(&mut engine, clip);
    let params = json!({"path": dir.text("Empty.scraft")});
    let mut expected = State::of(&engine);
    expected.journal.push(("session.save_template".into(), params.clone()));
    engine.execute("session.save_template", &params).unwrap();
    assert_eq!(State::of(&engine), expected);
    let template = Session::from_json(&std::fs::read_to_string(dir.path("Empty.scraft")).unwrap()).unwrap();
    assert_eq!(template.all_clips().count(), 0);
    assert!(template.clip_definitions.is_empty() && template.sources.is_empty() && template.videos.is_empty());
    assert!(!dir.path("Audio Files").exists());
}

#[test]
fn right_trimmed_gain_envelope_survives_capture_save_open_and_placement() {
    let dir = Directory::new();
    let (mut engine, track, clip, _) = audio();
    let envelope = vec![(0, -12.0), (32, 0.0)];
    engine.session_mut().find_clip_mut(clip).unwrap().gain_env = envelope.clone();
    engine.execute("edit.trim_end_to_insertion", &json!({"tracks": [track.0], "at": 27})).unwrap();
    let trimmed = engine.session().find_clip(clip).unwrap().1.clone();
    assert_eq!((trimmed.start, trimmed.length), (11, 16));
    assert_eq!(trimmed.gain_env, envelope);
    let before = soundcraft_mix::render_clips(engine.session(), track, Range { start: 11, end: 27 });
    assert_eq!(before.len(), 2);
    for (channel, data) in before.iter().enumerate() {
        assert_eq!(data.len(), 16);
        let first = sample(channel, 5) * 10.0f32.powf(-12.0 / 20.0);
        // The point after the trimmed end still defines the audible interpolation.
        let last = sample(channel, 20) * 10.0f32.powf((-12.0 + 12.0 * 15.0 / 32.0) / 20.0);
        assert!((data[0] - first).abs() < 1e-7);
        assert!((data[15] - last).abs() < 1e-7);
    }
    let definition = capture(&mut engine, clip);
    assert_eq!(engine.session().find_clip(clip).unwrap().1, &trimmed);
    assert_eq!(engine.session().clip_definition(definition).unwrap().template.gain_env, envelope);
    remove_instances(&mut engine);
    io::save_session(&mut engine, &dir.text("Trimmed.scraft")).unwrap();
    let mut opened = Engine::default();
    assert!(io::open_session(&mut opened, &dir.text("Trimmed.scraft")).unwrap().is_empty());
    assert_eq!(opened.session().clip_definition(definition).unwrap().template.gain_env, envelope);
    let placed = place(&mut opened, definition, track, 103);
    let instance = opened.session().find_clip(placed).unwrap().1;
    assert_eq!(instance.length, 16);
    assert_eq!(instance.gain_env, envelope);
    assert_eq!(soundcraft_mix::render_clips(opened.session(), track, Range { start: 103, end: 119 }), before);
}
