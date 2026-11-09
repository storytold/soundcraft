//! Split tracks into mono without replacing the right/surround channels with channel zero.

use serde_json::json;
use soundcraft_audio_io::{AudioBuffer, FileFormat};
use soundcraft_engine::{Engine, io};
use soundcraft_model::{ChannelFormat, Clip, Session, SourceAudio, SourceId, TrackId, TrackKind};
use soundcraft_time::Range;
use std::sync::Arc;

fn fixture(format: ChannelFormat, channels: usize) -> (Engine, TrackId, SourceId) {
    let mut s = Session::default();
    let track = s.add_track(TrackKind::Audio, format, Some("Original"));
    let channels = (0..channels).map(|ch| (0..256).map(|i| (ch + 1) as f32 * 0.1 + i as f32 * 0.0001).collect()).collect();
    let source = io::add_source(&mut s, "Original.wav", AudioBuffer { sample_rate: 48_000, channels }, None, FileFormat::Wav);
    s.sources[0].time_reference = 123_456;
    let mut clip = Clip::audio(s.new_clip_id(), "Edited", source, 7, 13, 47);
    clip.gain_db = -6.0;
    clip.gain_env = vec![(0, -3.0), (20, 0.0), (46, -9.0)];
    clip.fade_in.len = 5;
    clip.fade_out.len = 9;
    clip.stretch = 1.25;
    let mut second = clip.clone();
    second.id = s.new_clip_id();
    second.start = 100;
    second.content = soundcraft_model::ClipContent::Audio { source, offset: 90 };
    s.track_mut(track).unwrap().playlist_mut().unwrap().clips = vec![clip, second];
    (Engine::new(s), track, source)
}

fn split(e: &mut Engine, track: TrackId) -> Vec<TrackId> {
    let result = e.execute("track.split_into_mono", &json!({"track": track.0})).unwrap();
    result["tracks"].as_array().unwrap().iter().map(|id| TrackId(id.as_u64().unwrap())).collect()
}

#[test]
fn split_into_mono_preserves_every_channel_and_clip_edit() {
    for (format, channels) in [(ChannelFormat::Stereo, 2), (ChannelFormat::Surround51, 6)] {
        let (mut e, track, source) = fixture(format, channels);
        let original = e.session().track(track).unwrap().clone();
        let audio = Arc::clone(e.session().pool.get(source).unwrap());
        let expected = soundcraft_mix::render_clips(e.session(), track, Range::new(0, 192));
        let tracks = split(&mut e, track);
        assert_eq!(tracks.len(), channels);
        assert_eq!(e.session().track(track).unwrap(), &original);
        assert!(Arc::ptr_eq(e.session().pool.get(source).unwrap(), &audio));
        // Two clips referencing the same source share one extracted source per channel.
        assert_eq!(e.session().sources.len(), 1 + channels);
        for (ch, &id) in tracks.iter().enumerate() {
            let mono = e.session().track(id).unwrap();
            assert_eq!(mono.format, ChannelFormat::Mono);
            let actual = soundcraft_mix::render_clips(e.session(), id, Range::new(0, 192));
            assert_eq!(actual[0], expected[ch], "{format:?} channel {ch}");
            let mono_source = mono.clips()[0].source().unwrap();
            assert_ne!(mono_source, source);
            assert!(mono.clips().iter().all(|clip| clip.source() == Some(mono_source)));
            let data = e.session().pool.get(mono_source).unwrap();
            assert_eq!(data.buffer.channels, vec![audio.buffer.channels[ch].clone()]);
            assert_eq!(data.peaks.len(), 1);
            let metadata = e.session().source(mono_source).unwrap();
            assert_eq!((metadata.channels, metadata.time_reference), (1, 123_456));
            assert!(metadata.unsaved);
            for (clip, original) in mono.clips().iter().zip(original.clips()) {
                let mut expected_clip = original.clone();
                expected_clip.id = clip.id;
                expected_clip.name = clip.name.clone();
                expected_clip.content = soundcraft_model::ClipContent::Audio { source: mono_source, offset: original.source_offset() };
                assert_eq!(clip, &expected_clip);
            }
        }
    }
}

#[test]
fn split_stereo_matches_the_original_mix_in_playback_and_bounce_blocks() {
    let (mut e, track, _) = fixture(ChannelFormat::Stereo, 2);
    let original = e.session().clone();
    let tracks = split(&mut e, track);
    assert_eq!(e.session().track(tracks[0]).unwrap().name, "Original.L");
    assert_eq!(e.session().track(tracks[1]).unwrap().name, "Original.R");
    e.session_mut().track_mut(track).unwrap().inactive = true;
    for block in [37, 512, 1024] {
        let expected = soundcraft_mix::render_range(&original, Range::new(0, 192), block);
        let actual = soundcraft_mix::render_range(e.session(), Range::new(0, 192), block);
        assert_eq!(actual, expected, "block {block}");
    }
}

#[test]
fn split_stereo_preserves_distinct_left_and_right_levels() {
    let (mut e, track, source) = fixture(ChannelFormat::Stereo, 2);
    let s = e.session_mut();
    s.pool.insert(source, Arc::new(SourceAudio::new(AudioBuffer { sample_rate: 48_000, channels: vec![vec![0.2; 256], vec![0.7; 256]] })));
    let clip = Clip::audio(s.new_clip_id(), "Distinct channels", source, 0, 0, 256);
    s.track_mut(track).unwrap().playlist_mut().unwrap().clips = vec![clip];
    split(&mut e, track);
    e.session_mut().track_mut(track).unwrap().inactive = true;
    let audio = soundcraft_mix::render_range(e.session(), Range::new(0, 128), 64);
    eprintln!("Split into Mono output: L={}, R={}", audio[0][64], audio[1][64]);
    assert_eq!(audio[0][64], 0.2);
    assert_eq!(audio[1][64], 0.7);
}

#[test]
fn split_sources_survive_save_and_reopen_with_distinct_audio_files() {
    let (mut e, track, _) = fixture(ChannelFormat::Stereo, 2);
    let tracks = split(&mut e, track);
    let (bytes, saved) = io::session_bundle(e.session()).unwrap();
    let paths: std::collections::BTreeSet<_> = saved.sources.iter().map(|source| &source.path).collect();
    assert_eq!(paths.len(), 3);
    let mut reopened = Engine::default();
    assert!(io::open_session_bytes(&mut reopened, "split.scraftz", &bytes).unwrap().is_empty());
    for id in tracks {
        let expected = soundcraft_mix::render_clips(e.session(), id, Range::new(0, 192));
        let actual = soundcraft_mix::render_clips(reopened.session(), id, Range::new(0, 192));
        assert_eq!(actual, expected);
    }
}

#[test]
fn split_tracks_and_sources_undo_and_redo_together() {
    let (mut e, track, _) = fixture(ChannelFormat::Stereo, 2);
    let before = e.session().clone();
    split(&mut e, track);
    let after = e.session().clone();
    assert_eq!(e.undo_history().len(), 1);
    assert!(e.undo());
    assert_eq!(e.session(), &before);
    assert!(e.redo());
    assert_eq!(e.session(), &after);
}

#[test]
fn split_reuses_a_mono_source_on_a_stereo_track() {
    let (mut e, track, source) = fixture(ChannelFormat::Stereo, 1);
    let expected = soundcraft_mix::render_clips(e.session(), track, Range::new(0, 192));
    for id in split(&mut e, track) {
        assert!(e.session().track(id).unwrap().clips().iter().all(|clip| clip.source() == Some(source)));
        assert_eq!(soundcraft_mix::render_clips(e.session(), id, Range::new(0, 192))[0], expected[0]);
    }
    assert_eq!(e.session().sources.len(), 1);
}

#[test]
fn missing_or_empty_media_refuses_the_split_without_partial_changes() {
    for empty in [false, true] {
        let (mut e, track, _) = fixture(ChannelFormat::Stereo, 2);
        let missing = SourceId(9999);
        if empty {
            e.session_mut().pool.insert(missing, Arc::new(SourceAudio::new(AudioBuffer { sample_rate: 48_000, channels: vec![] })));
        }
        let clip = Clip::audio(e.session_mut().new_clip_id(), "Unavailable", missing, 0, 160, 16);
        e.session_mut().track_mut(track).unwrap().playlist_mut().unwrap().clips.push(clip);
        let before = e.session().clone();
        let error = e.execute("track.split_into_mono", &json!({"track": track.0})).unwrap_err();
        assert!(matches!(error, soundcraft_engine::EngineError::BadParams(..)), "{error}");
        assert_eq!(e.session(), &before);
        assert!(e.undo_history().is_empty());
    }
}
