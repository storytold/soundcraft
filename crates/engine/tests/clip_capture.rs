//! Public command regressions with owned synthetic media.

use serde_json::{Value, json};
use soundcraft_audio_io::{AudioBuffer, FileFormat};
use soundcraft_engine::Engine;
use soundcraft_midi::{Note, Sequence};
use soundcraft_model::{
    ChannelFormat, Clip, ClipDefinitionId, ClipId, EditMode, Fade, FadeShape, Playlist, Session, Source, SourceAudio, SourceId, TrackId, TrackKind,
    VideoSource,
};
use soundcraft_time::{MAX_POSITION, Range};
use std::sync::Arc;

fn fixture() -> (Engine, [ClipId; 3]) {
    let mut session = Session::default();
    let source = SourceId(session.alloc());
    session.sources.push(Source {
        id: source,
        name: "Tone".into(),
        path: "Tone.wav".into(),
        channels: 1,
        frames: 256,
        sample_rate: 48_000,
        format: FileFormat::Wav,
        time_reference: 0,
        unsaved: false,
    });
    session.pool.insert(
        source,
        Arc::new(SourceAudio::new(AudioBuffer { sample_rate: 48_000, channels: vec![(0..256).map(|i| i as f32 / 256.0 - 0.5).collect()] })),
    );
    let audio = session.add_track(TrackKind::Audio, ChannelFormat::Mono, Some("Audio snapshot"));
    let a = session.new_clip_id();
    session.track_mut(audio).unwrap().playlist_mut().unwrap().clips.push(Clip::audio(a, "Tone clip", source, 5, 0, 64));
    let midi = session.add_track(TrackKind::Midi, ChannelFormat::Mono, Some("MIDI snapshot"));
    let m = session.new_clip_id();
    let sequence = Sequence { notes: vec![Note { pitch: 60, velocity: 90, release_velocity: 50, channel: 2, start: 0, length: 2 }], ctrls: vec![] };
    session.track_mut(midi).unwrap().playlist_mut().unwrap().clips.push(Clip::midi(m, "Notes", 100, 64, sequence));
    let video = SourceId(session.alloc());
    session.videos.push(VideoSource {
        id: video,
        name: "Picture".into(),
        path: "offline.mov".into(),
        width: 1920,
        height: 1080,
        frame_rate: 24.0,
        duration: 1.0,
        codec: "fixture".into(),
    });
    let track = session.add_track(TrackKind::Video, ChannelFormat::Mono, Some("Video snapshot"));
    let v = session.new_clip_id();
    session.track_mut(track).unwrap().playlist_mut().unwrap().clips.push(Clip::video(v, "Picture clip", video, 10, 200, 64));
    (Engine::new(session), [a, m, v])
}

fn capture(engine: &mut Engine, clip: ClipId) -> ClipDefinitionId {
    ClipDefinitionId(engine.execute("clip.capture", &json!({"clip":clip})).unwrap()["definitions"][0]["id"].as_u64().unwrap())
}

fn place(engine: &mut Engine, definition: ClipDefinitionId, at: i64) -> (TrackId, ClipId) {
    let value = engine.execute("clip.place_definition", &json!({"definition":definition,"at":at})).unwrap();
    (TrackId(value["track"].as_u64().unwrap()), ClipId(value["clip"].as_u64().unwrap()))
}

fn rejected(engine: &mut Engine, command: &str, params: Value) {
    let before = engine.session().clone();
    let history = engine.undo_history();
    let journal = engine.journal.clone();
    let dirty = engine.is_dirty();
    let redo = engine.can_redo();
    let revision = engine.revision;
    assert!(engine.execute(command, &params).is_err(), "{command}: {params}");
    assert_eq!(engine.session(), &before);
    assert_eq!(engine.undo_history(), history);
    assert_eq!(engine.journal, journal);
    assert_eq!(engine.is_dirty(), dirty);
    assert_eq!(engine.can_redo(), redo);
    assert_eq!(engine.revision, revision);
}

#[test]
fn audio_midi_video_capture_leaves_timeline_unchanged_and_query_is_read_only() {
    let (mut engine, ids) = fixture();
    let tracks = engine.session().tracks.clone();
    let value = engine.execute("clip.capture", &json!({"clips":ids})).unwrap();
    assert_eq!(value["count"], 3);
    assert_eq!(engine.session().tracks, tracks);
    let before = engine.session_arc();
    let revision = engine.revision;
    let journal = engine.journal.clone();
    let history = engine.undo_history();
    let list = engine.execute("clip.definitions", &json!({})).unwrap();
    assert_eq!(list["definitions"].as_array().unwrap().len(), 3);
    assert!(Arc::ptr_eq(&before, &engine.session_arc()));
    assert_eq!(engine.revision, revision);
    assert_eq!(engine.journal, journal);
    assert_eq!(engine.undo_history(), history);
    assert!(engine.undo());
    assert!(engine.session().clip_definitions.is_empty());
    assert!(engine.redo());
    assert_eq!(engine.session().clip_definitions.len(), 3);
}

#[test]
fn fresh_instances_remain_independent_after_originals_are_removed() {
    let (mut engine, ids) = fixture();
    let value = engine.execute("clip.capture", &json!({"clips":ids,"name":"Saved"})).unwrap();
    let definitions: Vec<_> = value["definitions"].as_array().unwrap().iter().map(|v| ClipDefinitionId(v["id"].as_u64().unwrap())).collect();
    engine.execute("clip.clear", &json!({"clips":ids})).unwrap();
    assert_eq!(engine.session().all_clips().count(), 0);
    for (definition, kind) in definitions.iter().zip([TrackKind::Audio, TrackKind::Midi, TrackKind::Video]) {
        let (track, first) = place(&mut engine, *definition, 300);
        let (_, second) = place(&mut engine, *definition, 500);
        assert_ne!(first, second);
        assert_eq!(engine.session().track(track).unwrap().kind, kind);
        engine.session_mut().find_clip_mut(first).unwrap().gain_db = -9.0;
        assert_eq!(engine.session().find_clip(second).unwrap().1.gain_db, 0.0);
        assert_eq!(engine.session().clip_definition(*definition).unwrap().template.gain_db, 0.0);
    }
    let clips = engine.session().tracks.clone();
    engine.execute("clip.clear_definitions", &json!({"definitions":definitions})).unwrap();
    assert_eq!(engine.session().tracks, clips);
    assert!(engine.session().clip_definitions.is_empty());
    assert!(engine.undo());
    assert_eq!(engine.session().clip_definitions.len(), 3);
}

#[test]
fn captured_audio_preserves_pcm_gain_fades_curvature_stretch_and_effects() {
    let (mut engine, ids) = fixture();
    let (original_track, _) = engine.session().find_clip(ids[0]).unwrap();
    let clip = engine.session_mut().find_clip_mut(ids[0]).unwrap();
    clip.gain_db = -2.0;
    clip.gain_env = vec![(0, -3.0), (32, 1.0), (63, 0.0)];
    clip.stretch = 1.5;
    clip.fade_in = Fade { len: 16, shape: FadeShape::SCurve, curvature: 0.4 };
    clip.fade_out = Fade { len: 24, shape: FadeShape::EqualPower, curvature: -0.3 };
    engine.execute("clip.effects_set", &json!({"clip":ids[0],"params":{"gain":-4.0,"eq.mid_gain":1.0,"custom.setting":8.0}})).unwrap();
    let expected = soundcraft_mix::render_clips(engine.session(), original_track, Range { start: 0, end: 64 });
    let definition = capture(&mut engine, ids[0]);
    assert_eq!(engine.session().clip_definition(definition).unwrap().metadata.effects["custom.setting"], 8.0);
    let (track, _) = place(&mut engine, definition, 128);
    let actual = soundcraft_mix::render_clips(engine.session(), track, Range { start: 128, end: 192 });
    assert!(expected[0].iter().any(|x| x.abs() > 0.01));
    assert_eq!(actual, expected);
}

#[test]
fn effects_bypass_and_elastic_original_are_rebound_to_fresh_ids() {
    let (mut engine, ids) = fixture();
    let original = engine.session().find_clip(ids[0]).unwrap().1.source().unwrap();
    for (suffix, value) in [("src", original.0 as f64), ("off", 8.0), ("len", 80.0), ("ratio", 1.25)] {
        engine.session_mut().edit.values.insert(format!("elastic.{}.{suffix}", ids[0].0), value);
    }
    for (family, value) in [("clip.bpm", 100.0), ("clip.len0", 64.0), ("clip.pitch", 2.0)] {
        engine.session_mut().edit.values.insert(format!("{family}.{}", ids[0].0), value);
    }
    engine.execute("clip.effects_set", &json!({"clip":ids[0],"params":{"gain":-12.0}})).unwrap();
    engine.execute("clip.effects_bypass", &json!({"clip":ids[0],"value":true})).unwrap();
    engine.session_mut().edit.values.insert("clip_fx.999.gain".into(), -30.0);
    let definition = capture(&mut engine, ids[0]);
    let (_, clip) = place(&mut engine, definition, 300);
    assert!(clip.0 > 999);
    assert!(engine.session().edit.flag(&format!("clip_fx.bypass.{}", clip.0)));
    assert_eq!(engine.session().edit.values[&format!("clip_fx.{}.gain", clip.0)], -12.0);
    assert_eq!(engine.session().edit.values[&format!("clip.bpm.{}", clip.0)], 100.0);
    assert_eq!(engine.session().edit.values[&format!("clip.len0.{}", clip.0)], 64.0);
    assert_eq!(engine.session().edit.values[&format!("clip.pitch.{}", clip.0)], 2.0);
    engine.execute("clip.remove_warp", &json!({"clip":clip})).unwrap();
    let restored = engine.session().find_clip(clip).unwrap().1;
    assert_eq!((restored.source(), restored.source_offset(), restored.length), (Some(original), 8, 80));
    assert_eq!(engine.session().clip_definition(definition).unwrap().template.length, 64);
}

#[test]
fn saved_midi_performance_and_fixed_sample_window_survive_placement() {
    let (mut engine, ids) = fixture();
    let original = Sequence { notes: vec![Note { pitch: 65, velocity: 80, release_velocity: 64, channel: 0, start: 1, length: 1 }], ctrls: vec![] };
    engine.session_mut().midi_originals.insert(ids[1].0, original.clone());
    let definition = capture(&mut engine, ids[1]);
    engine.session_mut().tempo.set_tempo(0, 60.0).unwrap();
    let instrument = engine.session_mut().add_track(TrackKind::Instrument, ChannelFormat::Stereo, Some("Target instrument"));
    let result = engine.execute("clip.place_definition", &json!({"definition":definition,"track":instrument,"at":500})).unwrap();
    let clip = ClipId(result["clip"].as_u64().unwrap());
    assert_eq!(engine.session().find_clip(clip).unwrap().1.length, 64);
    assert_eq!(engine.session().midi_originals[&clip.0], original);
    assert_eq!(engine.session().find_clip(clip).unwrap().1.content, engine.session().clip_definition(definition).unwrap().template.content);
}

#[test]
fn fresh_instances_clear_group_loop_and_locks_but_keep_visible_settings() {
    let (mut engine, ids) = fixture();
    let clip = engine.session_mut().find_clip_mut(ids[0]).unwrap();
    clip.edit_locked = true;
    clip.time_locked = true;
    clip.group = Some(100);
    clip.loop_id = Some(101);
    clip.muted = true;
    clip.rating = 5;
    clip.sync_point = 20;
    clip.color = Some([9, 8, 7]);
    let definition = capture(&mut engine, ids[0]);
    assert!(definition.0 > 101);
    let (_, id) = place(&mut engine, definition, 300);
    let placed = engine.session().find_clip(id).unwrap().1;
    assert!(!placed.edit_locked && !placed.time_locked);
    assert_eq!((placed.group, placed.loop_id), (None, None));
    assert_eq!((placed.muted, placed.rating, placed.sync_point, placed.color), (true, 5, 20, Some([9, 8, 7])));
}

#[test]
fn selection_capture_refuses_partial_ranges_while_explicit_targets_are_whole() {
    let (mut engine, ids) = fixture();
    let track = engine.session().find_clip(ids[0]).unwrap().0;
    engine.session_mut().edit.selected_tracks = vec![track];
    engine.session_mut().edit.selection = Range { start: 10, end: 50 };
    rejected(&mut engine, "clip.capture", json!({}));
    let definition = capture(&mut engine, ids[0]);
    assert_eq!(engine.session().clip_definition(definition).unwrap().template.length, 64);
    engine.session_mut().edit.selection = Range { start: 0, end: 64 };
    assert_eq!(engine.execute("clip.capture", &json!({})).unwrap()["count"], 1);
    engine.session_mut().edit.selected_clips = vec![ids[0]];
    engine.session_mut().edit.selection = Range { start: 1, end: 64 };
    rejected(&mut engine, "clip.capture", json!({}));
}

#[test]
fn invalid_and_mixed_inactive_targets_leave_no_partial_definitions() {
    let (mut engine, ids) = fixture();
    let track = engine.session().find_clip(ids[0]).unwrap().0;
    let inactive = engine.session_mut().new_clip_id();
    let source = engine.session().find_clip(ids[0]).unwrap().1.source().unwrap();
    engine
        .session_mut()
        .track_mut(track)
        .unwrap()
        .playlists
        .push(Playlist { name: "Alternate".into(), clips: vec![Clip::audio(inactive, "Inactive", source, 0, 0, 64)] });
    for params in [
        json!({"clips":[ids[0],inactive]}),
        json!({"clips":[ids[0],ids[0]]}),
        json!({"clips":[ids[0],99999]}),
        json!({"clips":[]}),
        json!({"clip":"bad"}),
        json!({"clip":ids[0],"name":"x".repeat(257)}),
    ] {
        rejected(&mut engine, "clip.capture", params);
    }
    engine.session_mut().find_clip_mut(ids[1]).unwrap().length = 0;
    rejected(&mut engine, "clip.capture", json!({"clips":[ids[0],ids[1]]}));
    assert!(engine.session().clip_definitions.is_empty());
}

#[test]
fn placement_refuses_overlaps_wrong_kinds_bad_positions_and_missing_audio() {
    let (mut engine, ids) = fixture();
    let audio = engine.session().find_clip(ids[0]).unwrap().0;
    let midi = engine.session().find_clip(ids[1]).unwrap().0;
    let definition = capture(&mut engine, ids[0]);
    for mode in [EditMode::Slip, EditMode::Shuffle, EditMode::Spot, EditMode::Grid] {
        engine.session_mut().edit.edit_mode = mode;
        rejected(&mut engine, "clip.place_definition", json!({"definition":definition,"track":audio,"at":20}));
    }
    engine.session_mut().find_clip_mut(ids[0]).unwrap().edit_locked = true;
    rejected(&mut engine, "clip.place_definition", json!({"definition":definition,"track":audio,"at":0}));
    for params in [
        json!({"definition":definition,"track":midi,"at":500}),
        json!({"definition":definition,"at":-1}),
        json!({"definition":definition,"at":MAX_POSITION}),
        json!({"definition":99999,"at":500}),
    ] {
        rejected(&mut engine, "clip.place_definition", params);
    }
    let source = engine.session().find_clip(ids[0]).unwrap().1.source().unwrap();
    engine.session_mut().pool.remove(source);
    rejected(&mut engine, "clip.place_definition", json!({"definition":definition,"at":500}));
    assert!(engine.session().to_json().is_ok());
}

#[test]
fn malformed_decoded_geometry_and_exhausted_ids_are_rejected_atomically() {
    let (mut engine, ids) = fixture();
    let source = engine.session().find_clip(ids[0]).unwrap().1.source().unwrap();
    engine.session_mut().pool.insert(source, Arc::new(SourceAudio::new(AudioBuffer { sample_rate: 48_000, channels: vec![vec![0.0; 255]] })));
    rejected(&mut engine, "clip.capture", json!({"clip":ids[0]}));
    let (mut engine, ids) = fixture();
    let source = engine.session().find_clip(ids[0]).unwrap().1.source().unwrap();
    engine.session_mut().sources.iter_mut().find(|s| s.id == source).unwrap().channels = 17;
    engine.session_mut().pool.insert(source, Arc::new(SourceAudio::new(AudioBuffer { sample_rate: 48_000, channels: vec![vec![0.1; 256]; 17] })));
    let definition = capture(&mut engine, ids[0]);
    rejected(&mut engine, "clip.place_definition", json!({"definition":definition,"at":500}));
    let (mut engine, ids) = fixture();
    engine.session_mut().next_id = u64::MAX;
    rejected(&mut engine, "clip.capture", json!({"clip":ids[0]}));
}

#[test]
fn failed_requests_preserve_a_nonempty_redo_stack_and_revision() {
    let (mut engine, ids) = fixture();
    let definition = capture(&mut engine, ids[0]);
    let (_, placed) = place(&mut engine, definition, 300);
    assert!(engine.undo());
    assert!(engine.can_redo());
    assert!(engine.session().find_clip(placed).is_none());
    rejected(&mut engine, "clip.capture", json!({"clips":[ids[0],ids[0]]}));
    rejected(&mut engine, "clip.place_definition", json!({"definition":definition,"at":-1}));
    rejected(&mut engine, "clip.clear_definitions", json!({"definitions":[99999]}));
    assert!(engine.redo());
    assert!(engine.session().find_clip(placed).is_some());
}

#[test]
fn oversized_capture_content_is_refused_without_a_partial_definition() {
    let (mut engine, ids) = fixture();
    engine.session_mut().find_clip_mut(ids[0]).unwrap().gain_env = vec![(0, 0.0); soundcraft_model::ClipDefinition::MAX_POINTS + 1];
    rejected(&mut engine, "clip.capture", json!({"clip":ids[0]}));
    assert!(engine.session().clip_definitions.is_empty());
}

#[test]
fn clearing_unknown_or_duplicate_definitions_keeps_existing_instances() {
    let (mut engine, ids) = fixture();
    let definition = capture(&mut engine, ids[0]);
    let (_, clip) = place(&mut engine, definition, 500);
    for params in [json!({"definitions":[definition,definition]}), json!({"definitions":[definition,99999]}), json!({"definitions":[]})] {
        rejected(&mut engine, "clip.clear_definitions", params);
    }
    engine.execute("clip.clear_definitions", &json!({"definitions":[definition]})).unwrap();
    assert!(engine.session().find_clip(clip).is_some());
    assert!(engine.session().sources.len() == 1 && engine.session().pool.len() == 1);
}
