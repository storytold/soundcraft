//! Heal must preserve rendered material and refuse clip semantics it cannot combine.

use serde_json::{Value, json};
use soundcraft_audio_io::{AudioBuffer, FileFormat};
use soundcraft_engine::{Engine, edit, io};
use soundcraft_model::{ChannelFormat, Clip, ClipContent, ClipId, Fade, FadeShape, Playlist, Session, SourceId, TrackId, TrackKind};
use soundcraft_time::Range;
use std::sync::Arc;

fn fixture(format: ChannelFormat) -> (Session, TrackId, SourceId, Vec<ClipId>) {
    let mut s = Session::default();
    let track = s.add_track(TrackKind::Audio, format, None);
    let channels = (0..format.channels()).map(|ch| (0..384).map(|n| ((n * 17 + ch * 11) % 97) as f32 / 128.0 - 0.25).collect()).collect();
    let buffer = AudioBuffer { sample_rate: s.sample_rate.hz(), channels };
    let source = io::add_source(&mut s, "Synthetic original", buffer, None, FileFormat::Wav);
    let id = s.new_clip_id();
    let mut clip = Clip::audio(id, "Original", source, 7, 20, 301);
    clip.gain_db = -3.25;
    clip.rating = 4;
    clip.color = Some([15, 80, 130]);
    clip.sync_point = 9;
    s.track_mut(track).unwrap().playlist_mut().unwrap().clips.push(clip);
    assert_eq!(edit::separate_at(&mut s, track, 121), 1);
    assert_eq!(edit::separate_at(&mut s, track, 227), 1);
    let ids = s.track(track).unwrap().clips().iter().map(|c| c.id).collect();
    s.edit.selected_tracks = vec![track];
    s.edit.selection = Range::new(20, 321);
    (s, track, source, ids)
}

fn render(e: &Engine, track: TrackId) -> Vec<Vec<f32>> {
    soundcraft_mix::render_clips(e.session(), track, Range::new(0, 384))
}

fn assert_noop(e: &mut Engine, track: TrackId, params: Value, reason: Option<&str>) {
    let before = e.session_arc();
    let stored = e.session().to_json().unwrap();
    let audio = render(e, track);
    let pool: Vec<_> = e.session().sources.iter().filter_map(|src| e.session().pool.get(src.id).map(|data| (src.id, Arc::clone(data)))).collect();
    let revision = e.revision;
    let dirty = e.is_dirty();
    let history = e.undo_history();
    let redo = (e.can_redo(), e.redo_label().map(str::to_owned));
    let result = e.execute("edit.heal", &params).unwrap();
    assert_eq!(result["healed"], 0, "{result}");
    if let Some(reason) = reason {
        assert!(result["skipped"].as_array().unwrap().iter().any(|pair| pair["reason"] == reason), "{result}");
    } else {
        assert_eq!(result["skipped"], json!([]));
    }
    assert!(Arc::ptr_eq(&before, &e.session_arc()));
    assert_eq!(e.revision, revision);
    assert_eq!(e.is_dirty(), dirty);
    assert_eq!(e.undo_history(), history);
    assert_eq!((e.can_redo(), e.redo_label().map(str::to_owned)), redo);
    assert_eq!(e.session().to_json().unwrap(), stored);
    assert_eq!(render(e, track), audio);
    for (source, data) in pool {
        assert!(Arc::ptr_eq(e.session().pool.get(source).unwrap(), &data));
    }
}

#[test]
fn heal_unaligned_splits_preserves_mono_stereo_samples_and_one_undo() {
    for format in [ChannelFormat::Mono, ChannelFormat::Stereo] {
        let (mut s, track, source, ids) = fixture(format);
        s.edit.selected_clips = vec![ids[2], ids[1], ids[0], ids[2]];
        let alternate = Playlist { name: "Alternate".into(), clips: vec![Clip::audio(s.new_clip_id(), "Alternate", source, 0, 0, 8)] };
        s.track_mut(track).unwrap().playlists.push(alternate.clone());
        let other = s.add_track(TrackKind::Audio, format, None);
        let other_clip = Clip::audio(s.new_clip_id(), "Other", source, 0, 4, 5);
        s.track_mut(other).unwrap().playlist_mut().unwrap().clips.push(other_clip.clone());
        s.edit.selected_clips.push(other_clip.id);
        let before = s.clone();
        let mut e = Engine::new(s);
        let audio = render(&e, track);
        assert!(audio.iter().flatten().any(|x| *x != 0.0));
        assert_eq!(e.execute("edit.heal", &json!({"tracks": [track, track]})).unwrap(), json!({"healed": 2, "skipped": []}));
        let merged = e.session().track(track).unwrap().clips();
        assert_eq!(merged.len(), 1);
        let original_left = &before.track(track).unwrap().clips()[0];
        let mut expected = original_left.clone();
        expected.length = 301;
        assert_eq!(merged[0], expected);
        assert_eq!(e.session().edit.selected_clips, vec![ids[0], other_clip.id]);
        assert_eq!(e.session().track(track).unwrap().playlists[1], alternate);
        assert_eq!(e.session().track(other).unwrap().clips(), &[other_clip]);
        assert_eq!(render(&e, track), audio);
        assert_eq!(e.undo_history(), vec!["Heal Separation"]);
        let after = e.session().clone();
        assert!(e.undo());
        assert_eq!(e.session(), &before);
        assert_eq!(render(&e, track), audio);
        assert!(e.redo());
        assert_eq!(e.session(), &after);
        assert_eq!(render(&e, track), audio);
        let reopened = Session::from_json(&e.session().to_json().unwrap()).unwrap();
        assert_eq!(reopened.track(track).unwrap().clips(), after.track(track).unwrap().clips());
        assert_eq!(reopened.edit.selected_clips, after.edit.selected_clips);
    }
}

#[test]
fn heal_remaps_a_right_only_selection_and_preserves_muted_material() {
    let (mut s, track, _, ids) = fixture(ChannelFormat::Mono);
    for clip in &mut s.track_mut(track).unwrap().playlist_mut().unwrap().clips {
        clip.muted = true;
    }
    s.edit.selected_clips = vec![ids[1]];
    let mut e = Engine::new(s);
    let audio = render(&e, track);
    assert!(audio.iter().flatten().all(|x| *x == 0.0));
    assert_eq!(e.execute("edit.heal", &json!({})).unwrap()["healed"], 2);
    assert_eq!(e.session().edit.selected_clips, vec![ids[0]]);
    assert!(e.session().find_clip(ids[1]).is_none());
    assert!(e.session().find_clip(ids[2]).is_none());
    assert_eq!(render(&e, track), audio);
}

#[test]
fn heal_preserves_upstream_cut_gap_stretch_and_envelope_joins_with_one_undo() {
    let (mut s, track, source, ids) = fixture(ChannelFormat::Mono);
    let mut left = Clip::audio(ids[0], "Left", source, 20, 10, 100);
    let mut right = Clip::audio(ids[1], "Right", source, 95, 160, 100);
    left.stretch = 2.0;
    right.stretch = 2.0;
    left.gain_env = vec![(0, 0.0), (99, -6.0)];
    right.gain_env = vec![(0, -12.0), (99, -3.0)];
    s.track_mut(track).unwrap().playlist_mut().unwrap().clips = vec![left.clone(), right.clone()];
    s.edit.selected_clips = vec![right.id];
    let before = s.clone();
    let data = Arc::clone(s.pool.get(source).unwrap());
    let mut e = Engine::new(s);
    assert_eq!(e.execute("edit.heal", &json!({"start":100,"end":170})).unwrap(), json!({"healed":1,"skipped":[]}));
    let joined = &e.session().track(track).unwrap().clips()[0];
    assert_eq!((joined.id, joined.start, joined.length, joined.source_offset(), joined.stretch), (left.id, 10, 250, 20, 2.0));
    for n in 0..100 {
        assert!((joined.env_db_at(n) - left.env_db_at(n)).abs() < 1e-4);
        assert!((joined.env_db_at(n + 150) - right.env_db_at(n)).abs() < 1e-4);
    }
    assert_eq!(e.session().edit.selected_clips, vec![left.id]);
    assert!(Arc::ptr_eq(e.session().pool.get(source).unwrap(), &data));
    assert_eq!(e.undo_history(), vec!["Heal Separation"]);
    let after = e.session().clone();
    assert!(e.undo());
    assert_eq!(e.session(), &before);
    assert!(e.redo());
    assert_eq!(e.session(), &after);
}

#[test]
fn heal_refuses_processing_and_locks_without_consuming_redo() {
    let changes: &[(fn(&mut Clip), &str)] = &[
        (|c| c.muted = true, "mute_mismatch"),
        (|c| c.gain_db += 0.0000005, "gain_mismatch"),
        (|c| c.gain_db = f32::NAN, "gain_mismatch"),
        (|c| c.gain_env = vec![(c.length, -1.0), (0, 0.0)], "unsupported_envelope"),
        (|c| c.gain_env = vec![(0, f32::NAN)], "unsupported_envelope"),
        (|c| c.stretch = 1.000000001, "unsupported_stretch"),
        (|c| c.stretch = f64::NAN, "unsupported_stretch"),
        (|c| c.fade_in = Fade { len: 1, shape: FadeShape::Linear, curvature: 0.0 }, "unsupported_fade"),
        (|c| c.fade_out = Fade { len: -1, shape: FadeShape::Linear, curvature: 0.0 }, "unsupported_fade"),
        (|c| c.edit_locked = true, "locked"),
        (|c| c.time_locked = true, "locked"),
        (|c| c.group = Some(4), "grouped"),
    ];
    for &(change, reason) in changes {
        let (mut s, track, _, ids) = fixture(ChannelFormat::Mono);
        change(s.find_clip_mut(ids[1]).unwrap());
        let mut e = Engine::new(s);
        e.execute("mix.volume", &json!({"track": track, "db": -2.0})).unwrap();
        assert!(e.undo());
        assert!(e.can_redo());
        assert_noop(&mut e, track, json!({}), Some(reason));
    }
}

#[test]
fn heal_refuses_unaligned_inner_and_outer_fades_without_changing_render() {
    for (clip_index, fade_in) in [(0, true), (0, false), (1, true), (1, false), (2, false)] {
        let (mut s, track, _, ids) = fixture(ChannelFormat::Stereo);
        let c = s.find_clip_mut(ids[clip_index]).unwrap();
        let fade = Fade { len: 43, shape: FadeShape::EqualPower, curvature: 0.0 };
        if fade_in {
            c.fade_in = fade;
        } else {
            c.fade_out = fade;
        }
        let mut e = Engine::new(s);
        // Pick the seam involving that fade; other eligible seams are outside this range.
        let seam = if clip_index == 2 { 227 } else { 121 };
        assert_noop(&mut e, track, json!({"start": seam, "end": seam}), Some("unsupported_fade"));
    }
}

#[test]
fn heal_refuses_every_known_per_id_reference_on_either_side() {
    for left in [true, false] {
        for kind in ["fx", "bypass", "elastic", "pitch", "bpm", "len0", "midi"] {
            let (mut s, track, _, ids) = fixture(ChannelFormat::Mono);
            let id = ids[usize::from(!left)];
            match kind {
                "fx" => {
                    s.edit.values.insert(format!("clip_fx.{}.gain", id.0), -2.0);
                }
                "bypass" => {
                    s.edit.set_flag(&format!("clip_fx.bypass.{}", id.0), true);
                }
                "elastic" => {
                    s.edit.values.insert(format!("elastic.{}.src", id.0), 999.0);
                }
                "midi" => {
                    s.midi_originals.insert(id.0, soundcraft_midi::Sequence::default());
                }
                key => {
                    s.edit.values.insert(format!("clip.{key}.{}", id.0), 120.0);
                }
            }
            let mut e = Engine::new(s);
            assert_noop(&mut e, track, json!({"start": 121, "end": 121}), Some("clip_metadata"));
        }
    }
}

#[test]
fn heal_metadata_keys_match_complete_ids_and_mixed_selection_keeps_refused_clip() {
    let (mut s, track, _, ids) = fixture(ChannelFormat::Mono);
    // This unrelated ID starts with the same digits as a real ID.
    s.edit.values.insert(format!("clip_fx.{}0.gain", ids[0].0), -2.0);
    s.edit.values.insert(format!("elastic.{}0.src", ids[0].0), 8.0);
    s.edit.values.insert(format!("clip.bpm.{}0", ids[0].0), 120.0);
    s.edit.set_flag(&format!("clip_fx.bypass.{}0", ids[0].0), true);
    s.find_clip_mut(ids[2]).unwrap().muted = true;
    s.edit.selected_clips = ids.clone();
    let mut e = Engine::new(s);
    let before = render(&e, track);
    let refused = e.session().find_clip(ids[2]).unwrap().1.clone();
    let values = e.session().edit.values.clone();
    let flags = e.session().edit.flags.clone();
    let result = e.execute("edit.heal", &json!({})).unwrap();
    assert_eq!(result, json!({"healed": 1, "skipped": [{"track": track, "left": ids[0], "right": ids[2], "reason": "mute_mismatch"}]}));
    assert_eq!(e.session().find_clip(ids[2]).unwrap().1, &refused);
    assert_eq!(e.session().edit.selected_clips, vec![ids[0], ids[2]]);
    assert_eq!(e.session().edit.values, values);
    assert_eq!(e.session().edit.flags, flags);
    assert_eq!(render(&e, track), before);
}

#[test]
fn heal_refuses_noncontinuous_sources_malformed_clips_and_duplicate_ids() {
    let changes: &[(fn(&mut Clip), &str)] = &[
        (|c| c.start += 1, "not_contiguous"),
        (|c| c.start -= 1, "not_contiguous"),
        (|c| c.content = ClipContent::Audio { source: SourceId(999), offset: c.source_offset() }, "different_source"),
        (|c| c.content = ClipContent::Audio { source: c.source().unwrap(), offset: c.source_offset() + 1 }, "not_contiguous"),
        (|c| c.content = ClipContent::Audio { source: c.source().unwrap(), offset: -1 }, "invalid_clip"),
        (|c| c.length = 0, "invalid_clip"),
        (|c| c.length = -1, "invalid_clip"),
        (|c| c.start = -1, "invalid_clip"),
        (|c| c.content = ClipContent::Midi { sequence: soundcraft_midi::Sequence::default() }, "unsupported_content"),
        (|c| c.content = ClipContent::Video { source: SourceId(999), offset: 0 }, "unsupported_content"),
    ];
    for &(change, reason) in changes {
        let (mut s, track, _, ids) = fixture(ChannelFormat::Mono);
        // Refuse the first pair only; the second pair is outside the range.
        change(s.find_clip_mut(ids[1]).unwrap());
        let at = s.find_clip(ids[1]).unwrap().1.start;
        let mut e = Engine::new(s);
        assert_noop(&mut e, track, json!({"start": at, "end": at}), Some(reason));
    }
    let (mut s, track, _, ids) = fixture(ChannelFormat::Mono);
    let duplicate = s.find_clip(ids[1]).unwrap().1.clone();
    s.track_mut(track).unwrap().playlists.push(Playlist { name: "Ambiguous duplicate".into(), clips: vec![duplicate] });
    let mut e = Engine::new(s);
    assert_noop(&mut e, track, json!({}), Some("duplicate_id"));
}

#[test]
fn heal_checked_arithmetic_and_extreme_saved_point_selections_are_total() {
    for (left_start, left_len, right_start, right_len, left_offset, right_offset) in [
        (i64::MAX - 1, 2, i64::MAX, 1, 0, 2),
        (0, 1, 1, i64::MAX, 0, 1),
        (0, 1, 1, 1, i64::MAX, i64::MAX),
        (0, 1, 1, 1, i64::MAX - 1, i64::MAX),
        (0, i64::MAX - 1, i64::MAX - 1, 2, 0, i64::MAX - 1),
    ] {
        let (mut s, track, source, ids) = fixture(ChannelFormat::Mono);
        s.track_mut(track).unwrap().playlist_mut().unwrap().clips = vec![
            Clip::audio(ids[0], "a", source, left_offset, left_start, left_len),
            Clip::audio(ids[1], "b", source, right_offset, right_start, right_len),
        ];
        s.edit.selection = Range::point(right_start);
        let mut e = Engine::new(s);
        assert_noop(&mut e, track, json!({}), Some("arithmetic_overflow"));
    }
    for at in [i64::MIN, i64::MAX] {
        let (mut s, track, _, _) = fixture(ChannelFormat::Mono);
        s.edit.selection = Range::point(at);
        let mut e = Engine::new(s);
        assert_noop(&mut e, track, json!({}), None);
    }
    for envelope_overflow in [false, true] {
        let (mut s, track, _, ids) = fixture(ChannelFormat::Mono);
        if envelope_overflow {
            s.find_clip_mut(ids[1]).unwrap().gain_env = vec![(i64::MAX, 0.0)];
        } else {
            for clip in &mut s.track_mut(track).unwrap().playlist_mut().unwrap().clips {
                clip.stretch = f64::MIN_POSITIVE;
            }
        }
        let before = s.to_json().unwrap();
        let plan = edit::plan_heal(&s, &[track], Range::new(120, 122));
        assert_eq!(plan.report.healed, 0);
        assert!(plan.report.skipped.iter().any(|skip| skip.reason == "arithmetic_overflow"));
        assert_eq!(s.to_json().unwrap(), before);
    }
}

#[test]
fn heal_one_sample_and_range_endpoint_behavior_is_explicit() {
    for (range, count) in [
        (Range::point(101), 1),
        (Range::point(100), 1),
        (Range::point(102), 1),
        (Range::point(103), 0),
        (Range::new(101, 102), 1),
        (Range::new(99, 101), 1),
    ] {
        let (mut s, track, source, ids) = fixture(ChannelFormat::Mono);
        s.track_mut(track).unwrap().playlist_mut().unwrap().clips =
            vec![Clip::audio(ids[0], "a", source, 1, 100, 1), Clip::audio(ids[1], "b", source, 2, 101, 1)];
        s.edit.selection = range;
        let mut e = Engine::new(s);
        let before = render(&e, track);
        let result = e.execute("edit.heal", &json!({})).unwrap();
        assert_eq!(result["healed"], count);
        assert_eq!(render(&e, track), before);
    }
}

#[test]
fn heal_empty_and_unsorted_noops_do_not_rewrite_the_playlist() {
    let (mut s, track, _, _) = fixture(ChannelFormat::Mono);
    s.edit.selection = Range::point(350);
    s.track_mut(track).unwrap().playlist_mut().unwrap().clips.reverse();
    let mut e = Engine::new(s);
    assert_noop(&mut e, track, json!({}), None);
    let before = e.session().to_json().unwrap();
    assert_eq!(edit::heal(e.session_mut(), track, Range::new(350, 360)), 0);
    assert_eq!(e.session().to_json().unwrap(), before);
    let mut s = Session::default();
    let track = s.add_track(TrackKind::Audio, ChannelFormat::Mono, None);
    let mut e = Engine::new(s);
    assert_noop(&mut e, track, json!({"track": track}), None);
}

#[test]
fn heal_preserves_headroom_pcm_and_render_across_undo_redo() {
    let (mut s, track, source, ids) = fixture(ChannelFormat::Stereo);
    let buffer = AudioBuffer { sample_rate: 48_000, channels: vec![[1.25, -2.5, -0.0, 0.25].repeat(96), [-2.5, 1.25, -0.0, -0.25].repeat(96)] };
    let pcm_bits: Vec<Vec<_>> = buffer.channels.iter().map(|ch| ch.iter().map(|x| x.to_bits()).collect()).collect();
    let data = Arc::new(soundcraft_model::SourceAudio::new(buffer));
    s.pool.insert(source, Arc::clone(&data));
    s.edit.selected_clips = ids.clone();
    let split = s.clone();
    let mut e = Engine::new(s);
    let before_render = render(&e, track);
    assert!(before_render.iter().flatten().any(|sample| sample.abs() > 1.0));
    assert_eq!(e.execute("edit.heal", &json!({})).unwrap(), json!({"healed": 2, "skipped": []}));
    assert_eq!(e.session().edit.selected_clips, vec![ids[0]]);
    assert_eq!(render(&e, track), before_render);
    assert_eq!(e.undo_history(), vec!["Heal Separation"]);
    let healed = e.session_arc();
    for session in [&split, healed.as_ref()] {
        let saved = session.pool.get(source).unwrap();
        assert!(Arc::ptr_eq(saved, &data));
        assert_eq!(saved.buffer.channels.iter().map(|ch| ch.iter().map(|x| x.to_bits()).collect::<Vec<_>>()).collect::<Vec<_>>(), pcm_bits);
    }
    assert!(e.undo());
    assert_eq!(e.session(), &split);
    assert_eq!(render(&e, track), before_render);
    assert!(Arc::ptr_eq(e.session().pool.get(source).unwrap(), &data));
    assert!(e.redo());
    assert_eq!(e.session(), healed.as_ref());
    assert_eq!(render(&e, track), before_render);
    assert!(Arc::ptr_eq(e.session().pool.get(source).unwrap(), &data));
}

#[test]
fn heal_refuses_loop_identities_without_consuming_redo() {
    for sides in [[true, false], [false, true], [true, true]] {
        let (mut s, track, _, ids) = fixture(ChannelFormat::Mono);
        for (id, looped) in ids.iter().zip(sides) {
            if looped {
                s.find_clip_mut(*id).unwrap().loop_id = Some(42);
            }
        }
        let mut e = Engine::new(s);
        e.execute("mix.volume", &json!({"track": track, "db": -2.0})).unwrap();
        assert!(e.undo());
        assert!(e.can_redo());
        assert_noop(&mut e, track, json!({"start": 121, "end": 121}), Some("looped"));
    }
}
