//! Consolidate makes one clip per track that covers exactly the edit selection (#164).

use serde_json::json;
use soundcraft_audio_io::{AudioBuffer, FileFormat};
use soundcraft_engine::{Engine, EngineError};
use soundcraft_model::{Clip, TrackId};
use soundcraft_time::Range;

fn stereo_tracks(n: u64) -> Engine {
    let mut e = Engine::default();
    e.execute("track.new", &json!({"count": n, "format": "stereo"})).unwrap();
    e.session_mut().edit.selected_tracks = e.session().tracks.iter().map(|t| t.id).collect();
    e
}

fn sr(e: &Engine) -> i64 {
    e.session().sample_rate.samples(1.0)
}

/// Put a constant-level clip of `secs` seconds at the start of track `i`.
fn add_clip(e: &mut Engine, i: usize, secs: i64) -> TrackId {
    let frames = usize::try_from(secs * sr(e)).unwrap();
    let t = e.session().tracks[i].id;
    let buf = AudioBuffer { sample_rate: 48_000, channels: vec![vec![0.25; frames]; 2] };
    let src = soundcraft_engine::io::add_source(e.session_mut(), "tone", buf, None, FileFormat::Wav);
    let id = e.session_mut().new_clip_id();
    soundcraft_engine::edit::place_clip(e.session_mut(), t, Clip::audio(id, "tone", src, 0, 0, frames as i64));
    t
}

fn clips(e: &Engine, i: usize) -> Vec<(i64, i64)> {
    e.session().tracks[i].clips().iter().map(|c| (c.start, c.end())).collect()
}

/// Frames in the source behind the only clip on track `i`, and its first sample.
fn source_of_only_clip(e: &Engine, i: usize) -> (usize, f32) {
    let s = e.session();
    let c = &s.tracks[i].clips()[0];
    let audio = s.pool.get(c.source().unwrap()).unwrap();
    (audio.buffer.frames(), audio.buffer.channels[0][0])
}

fn assert_each_track_has(e: &Engine, start: i64, end: i64) {
    for (i, t) in e.session().tracks.iter().enumerate() {
        assert_eq!(clips(e, i), vec![(start, end)], "track {}", t.name);
        assert_eq!(source_of_only_clip(e, i).0 as i64, end - start, "track {}", t.name);
    }
}

#[test]
fn consolidating_silence_from_zero_covers_the_whole_selection() {
    let mut e = stereo_tracks(5);
    let sr = sr(&e);
    e.session_mut().edit.selection = Range::new(0, 5 * sr);
    e.execute("edit.consolidate", &json!({})).unwrap();
    assert_each_track_has(&e, 0, 5 * sr);
}

#[test]
fn consolidating_silence_later_in_the_timeline_covers_the_whole_selection() {
    let mut e = stereo_tracks(5);
    let sr = sr(&e);
    e.session_mut().edit.selection = Range::new(2 * sr, 7 * sr);
    e.execute("edit.consolidate", &json!({})).unwrap();
    assert_each_track_has(&e, 2 * sr, 7 * sr);
}

#[test]
fn consolidating_past_the_end_of_a_clip_keeps_the_trailing_silence() {
    let mut e = stereo_tracks(1);
    let sr = sr(&e);
    add_clip(&mut e, 0, 1);
    e.session_mut().edit.selection = Range::new(0, 5 * sr);
    e.execute("edit.consolidate", &json!({})).unwrap();
    assert_each_track_has(&e, 0, 5 * sr);
    assert_eq!(source_of_only_clip(&e, 0).1, 0.25);
}

#[test]
fn multi_track_consolidate_is_one_undo_step_and_fills_empty_tracks() {
    let mut e = stereo_tracks(3);
    let sr = sr(&e);
    add_clip(&mut e, 1, 2);
    let before = e.session().clone();
    let undo_depth = e.undo_history().len();
    e.session_mut().edit.selection = Range::new(sr, 4 * sr);
    e.execute("edit.consolidate", &json!({})).unwrap();
    // The clip on track 2 is split at the selection start; the head stays put.
    assert_eq!(clips(&e, 0), vec![(sr, 4 * sr)]);
    assert_eq!(clips(&e, 1), vec![(0, sr), (sr, 4 * sr)]);
    assert_eq!(clips(&e, 2), vec![(sr, 4 * sr)]);
    assert_eq!(e.undo_history().len(), undo_depth + 1);
    assert!(e.undo());
    for (a, b) in e.session().tracks.iter().zip(before.tracks.iter()) {
        assert_eq!(a.clips(), b.clips(), "track {}", a.name);
    }
}

#[test]
fn a_selection_too_long_to_render_is_refused_not_shortened() {
    let mut e = stereo_tracks(2);
    add_clip(&mut e, 0, 1);
    let before: Vec<_> = (0..2).map(|i| clips(&e, i)).collect();
    // A stereo track renders at most MAX_RENDER_SAMPLES / 2 frames (about 93 minutes at 48 kHz).
    let max = i64::try_from(soundcraft_mix::MAX_RENDER_SAMPLES / 2).unwrap();
    let err = e.execute("edit.consolidate", &json!({"start": 0, "end": max + 1})).unwrap_err();
    assert!(matches!(err, EngineError::BadParams(..)), "{err}");
    assert!(err.to_string().contains("too long"), "{err}");
    let after: Vec<_> = (0..2).map(|i| clips(&e, i)).collect();
    assert_eq!(after, before);
}

#[test]
fn prepare_dpe_still_consolidates_each_track_to_its_own_material() {
    let mut e = stereo_tracks(2);
    let sr = sr(&e);
    let a = add_clip(&mut e, 0, 1);
    let b = add_clip(&mut e, 1, 3);
    e.execute("options.prepare_dpe_tracks", &json!({"tracks": [a.0, b.0]})).unwrap();
    // Unchanged from before #164: each track runs to its last clip plus a second, capped at the session end.
    assert_eq!(clips(&e, 0), vec![(0, 2 * sr)]);
    assert_eq!(clips(&e, 1), vec![(0, 3 * sr)]);
    assert_eq!(source_of_only_clip(&e, 0), (2 * sr as usize, 0.25));
}
