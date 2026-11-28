use serde_json::{Value, json};
use soundcraft_audio_io::{AudioBuffer, FileFormat};
use soundcraft_engine::{Engine, io};
use soundcraft_midi::{CtrlEvent, CtrlKind, Note, Sequence};
use soundcraft_model::{AutoParam, AutomationLane, ChannelFormat, Clip, ClipContent, ClipId, EditMode, Playlist, Session, TrackId, TrackKind};
use soundcraft_time::{Range, SampleRate, TimeFormat};
use std::sync::Arc;

fn fixture(shuffle: bool, follow: bool) -> (Engine, TrackId, TrackId, TrackId) {
    let mut s = Session::default();
    let audio = s.add_track(TrackKind::Audio, ChannelFormat::Stereo, Some("Audio"));
    let midi = s.add_track(TrackKind::Midi, ChannelFormat::Mono, Some("MIDI"));
    let untouched = s.add_track(TrackKind::Audio, ChannelFormat::Mono, Some("Other"));
    let source = io::add_source(
        &mut s,
        "synthetic",
        AudioBuffer { sample_rate: 48_000, channels: vec![vec![0.5; 30_000], vec![-0.25; 30_000]] },
        None,
        FileFormat::Wav,
    );
    for (start, length) in [(1200, 400), (2400, 300), (2500, 1000), (3600, 800), (12_000, 500)] {
        let id = s.new_clip_id();
        s.track_mut(audio).unwrap().playlist_mut().unwrap().clips.push(Clip::audio(id, "audio", source, 100, start, length));
    }
    // No overlapping source pieces in the ordinary fixture.
    s.track_mut(audio).unwrap().playlist_mut().unwrap().clips.remove(1);
    let note = Note { pitch: 60, velocity: 90, release_velocity: 64, channel: 0, start: 10, length: 30 };
    let sequence = Sequence { notes: vec![note], ctrls: vec![CtrlEvent { tick: 0, channel: 0, kind: CtrlKind::Cc(7), value: 100 }] };
    for start in [1000, 12_000] {
        let id = s.new_clip_id();
        s.track_mut(midi).unwrap().playlist_mut().unwrap().clips.push(Clip::midi(id, "MIDI", start, 2000, sequence.clone()));
        s.midi_originals.insert(id.0, sequence.clone());
    }
    let id = s.new_clip_id();
    s.track_mut(untouched).unwrap().playlist_mut().unwrap().clips.push(Clip::audio(id, "other", source, 0, 3500, 2000));
    let mut alternative = Playlist::new("Alternative");
    let id = s.new_clip_id();
    alternative.clips.push(Clip::audio(id, "alternate", source, 0, 3500, 2000));
    s.track_mut(audio).unwrap().playlists.push(alternative);
    for id in [audio, midi] {
        let t = s.track_mut(id).unwrap();
        for param in
            [AutoParam::Volume, AutoParam::Pan(0), AutoParam::Mute, AutoParam::SendLevel(0), AutoParam::Plugin { slot: 0, param: "gain".into() }]
        {
            let mut lane = AutomationLane::new(param.clone());
            if matches!(param, AutoParam::Mute) {
                for (at, value) in [(0, 0.0), (1800, 1.0), (2500, 0.0), (7000, 1.0), (14_000, 0.0)] {
                    lane.set_point(at, value);
                }
            } else {
                let scale = if matches!(param, AutoParam::Pan(_)) { 0.05 } else { 1.0 };
                for (at, value) in [(500, -2.0), (3500, -8.0), (6000, -12.0), (14_000, -6.0)] {
                    let value = if matches!(param, AutoParam::Plugin { .. }) { (value + 12.0) / 10.0 } else { value * scale };
                    lane.set_point(at, value);
                }
            }
            t.automation.push(lane);
        }
    }
    s.edit.selection = Range::new(1000, 3000);
    s.edit.timeline_selection = s.edit.selection;
    s.edit.selected_tracks = vec![audio, midi];
    s.edit.edit_mode = if shuffle { EditMode::Shuffle } else { EditMode::Slip };
    s.edit.automation_follows_edit = follow;
    (Engine::new(s), audio, midi, untouched)
}

fn assert_refused(mut e: Engine, command: &str, params: Value) {
    let before = e.session_arc();
    let revision = e.revision;
    let dirty = e.is_dirty();
    let undo = e.undo_history();
    let redo = e.can_redo();
    let journal = e.journal.clone();
    let clipboard = e.clipboard.clone();
    assert!(e.execute(command, &params).is_err(), "{command}: {params}");
    assert!(Arc::ptr_eq(&before, &e.session_arc()));
    assert_eq!(e.revision, revision);
    assert_eq!(e.is_dirty(), dirty);
    assert_eq!(e.undo_history(), undo);
    assert_eq!(e.can_redo(), redo);
    assert_eq!(e.journal, journal);
    assert_eq!(e.clipboard, clipboard);
}

#[test]
fn repeat_captures_original_curves_and_ripples_once_with_follow_enabled() {
    for shuffle in [false, true] {
        let (mut e, audio, midi, other) = fixture(shuffle, true);
        let original = e.session().clone();
        let result = e.execute("edit.repeat", &json!({"count": 3})).unwrap();
        assert_eq!(result["repeats"], 3);
        assert_eq!(result["destination"], json!({"start": 3000, "end": 9000}));
        for id in [audio, midi] {
            let before = original.track(id).unwrap();
            let after = e.session().track(id).unwrap();
            for (src, dst) in before.automation.iter().zip(&after.automation) {
                assert_eq!(src.enabled, dst.enabled);
                for k in 0..3 {
                    for offset in [0, 1, 123, 799, 800, 1499, 1500, 1998, 1999] {
                        let expected = src.value_at(1000 + offset, 0.0);
                        let actual = dst.value_at(3000 + k * 2000 + offset, 0.0);
                        assert!(
                            (actual - expected).abs() < 0.00002,
                            "{:?} shuffle={shuffle} k={k} offset={offset}: {actual} != {expected}",
                            src.param
                        );
                    }
                }
                for at in [0, 450, 999, 1000, 1999, 2999] {
                    assert!((src.value_at(at, 0.0) - dst.value_at(at, 0.0)).abs() < 0.00002);
                }
                for at in [9000, 10_000, 12_000, 14_000] {
                    let source_at = if shuffle { at - 6000 } else { at };
                    assert!((src.value_at(source_at, 0.0) - dst.value_at(at, 0.0)).abs() < 0.00002);
                }
            }
            assert!(after.clips().iter().any(|c| c.start == if shuffle { 18_000 } else { 12_000 }));
        }
        assert_eq!(e.session().track(other), original.track(other));
        assert_eq!(e.session().track(audio).unwrap().playlists[1], original.track(audio).unwrap().playlists[1]);
        assert_eq!(e.session().edit.selection, original.edit.selection);
        assert_eq!(e.undo_history(), ["Repeat..."]);
        let edited = e.session().to_json().unwrap();
        assert!(e.undo());
        assert_eq!(e.session().to_json().unwrap(), original.to_json().unwrap());
        assert!(e.redo());
        assert_eq!(e.session().to_json().unwrap(), edited);
    }
}

#[test]
fn repeated_volume_curve_reaches_the_mixer_and_matches_calculated_pcm() {
    for shuffle in [false, true] {
        let mut s = Session::default();
        let track = s.add_track(TrackKind::Audio, ChannelFormat::Stereo, Some("Rendered volume"));
        let source = io::add_source(
            &mut s,
            "DC",
            AudioBuffer { sample_rate: 48_000, channels: vec![vec![0.5; 1024], vec![-0.25; 1024]] },
            None,
            FileFormat::Wav,
        );
        for start in [0, 4096] {
            let id = s.new_clip_id();
            s.track_mut(track).unwrap().playlist_mut().unwrap().clips.push(Clip::audio(id, "DC", source, 0, start, 1024));
        }
        let lane = s.track_mut(track).unwrap().lane_mut(&AutoParam::Volume);
        for (at, value) in [(0, -6.0), (512, -12.0), (1023, -3.0), (1024, -20.0), (4096, -9.0), (5119, -6.0)] {
            lane.set_point(at, value);
        }
        s.edit.selection = Range::new(0, 1024);
        s.edit.selected_tracks = vec![track];
        s.edit.edit_mode = if shuffle { EditMode::Shuffle } else { EditMode::Slip };
        s.edit.automation_follows_edit = true;
        let original = s.clone();
        let mut e = Engine::new(s);
        e.execute("edit.repeat", &json!({"count": 3})).unwrap();
        let original_pcm = soundcraft_mix::render_range(&original, Range::new(0, 1024), 16);
        assert_eq!(original_pcm.len(), 2);
        assert!(original_pcm.iter().all(|channel| channel.len() == 1024));
        // Independently calculated from the declared dB ramp, DC amplitudes and documented
        // mixer gain interpolation between each block's first/last sample, with hard L/R pan.
        let db = |at: usize| if at <= 512 { -6.0 - 6.0 * at as f64 / 512.0 } else { -12.0 + 9.0 * (at - 512) as f64 / 511.0 };
        let mut expected = vec![vec![0.0f32; 1024]; 2];
        for (channel, amplitude) in expected.iter_mut().zip([0.5, -0.25]) {
            for (at, value) in channel.iter_mut().enumerate() {
                let block_start = at / 16 * 16;
                let gain0 = 10f64.powf(db(block_start) / 20.0);
                let gain1 = 10f64.powf(db(block_start + 15) / 20.0);
                let gain = gain0 + (gain1 - gain0) * ((at - block_start) as f64 / 15.0);
                *value = (amplitude * gain) as f32;
            }
        }
        for (channel, calculated) in original_pcm.iter().zip(&expected) {
            for (actual, expected) in channel.iter().zip(calculated) {
                assert!((actual - expected).abs() < 0.000002, "baseline mixer PCM {actual} != calculated {expected}");
            }
        }
        for k in 0..=3 {
            let rendered = soundcraft_mix::render_range(e.session(), Range::new(k * 1024, (k + 1) * 1024), 16);
            assert_eq!(rendered.len(), 2);
            assert!(rendered.iter().all(|channel| channel.len() == 1024));
            for (channel, calculated) in rendered.iter().zip(&expected) {
                for (actual, expected) in channel.iter().zip(calculated) {
                    assert!((actual - expected).abs() < 0.000002, "shuffle={shuffle} iteration={k}: PCM {actual} != calculated {expected}");
                }
            }
        }
        let after = if shuffle { 7168 } else { 4096 };
        let outside_before = soundcraft_mix::render_range(&original, Range::new(4096, 5120), 16);
        let outside_after = soundcraft_mix::render_range(e.session(), Range::new(after, after + 1024), 16);
        assert_eq!(outside_before.len(), 2);
        assert!(outside_before.iter().all(|channel| channel.len() == 1024));
        assert_eq!(outside_after.len(), 2);
        assert!(outside_after.iter().all(|channel| channel.len() == 1024));
        assert_eq!(outside_after, outside_before);
    }
}

#[test]
fn follow_off_preserves_every_lane_in_slip_and_shuffle() {
    for shuffle in [false, true] {
        let (mut e, audio, midi, _) = fixture(shuffle, false);
        let lanes: Vec<_> = [audio, midi].iter().map(|id| e.session().track(*id).unwrap().automation.clone()).collect();
        e.execute("edit.repeat", &json!({"count": 3.0})).unwrap();
        for (id, expected) in [audio, midi].iter().zip(lanes) {
            assert_eq!(e.session().track(*id).unwrap().automation, expected);
        }
    }
}

#[test]
fn slip_copies_silence_and_shuffle_preserves_crossing_destination_suffix() {
    for shuffle in [false, true] {
        let (mut e, audio, _, _) = fixture(shuffle, false);
        let original = e.session().clone();
        e.execute("edit.repeat", &json!({"count": 3})).unwrap();
        let source = soundcraft_mix::render_clips(&original, audio, Range::new(1000, 3000));
        for k in 0..3 {
            let actual = soundcraft_mix::render_clips(e.session(), audio, Range::new(3000 + k * 2000, 5000 + k * 2000));
            assert_eq!(source, actual, "copied silence/audio must match for iteration {k}");
        }
        if shuffle {
            let suffix = e.session().track(audio).unwrap().clips().iter().find(|c| c.start == 9000).unwrap();
            assert_eq!(suffix.length, 500);
            assert_eq!(suffix.source_offset(), 600);
        } else {
            assert!(soundcraft_mix::render_clips(e.session(), audio, Range::new(3600, 4000)).iter().flatten().all(|v| *v == 0.0));
        }
        assert_eq!(
            soundcraft_mix::render_clips(&original, audio, Range::new(0, 3000)),
            soundcraft_mix::render_clips(e.session(), audio, Range::new(0, 3000))
        );
    }
}

#[test]
fn duplicate_repeat_one_and_extend_share_material_and_selection_policy() {
    for shuffle in [false, true] {
        for follow in [false, true] {
            let (mut duplicate, audio, midi, _) = fixture(shuffle, follow);
            let (mut repeat, _, _, _) = fixture(shuffle, follow);
            let (mut extend, _, _, _) = fixture(shuffle, follow);
            duplicate.execute("edit.duplicate", &json!({})).unwrap();
            repeat.execute("edit.repeat", &json!({"count": 1})).unwrap();
            extend.execute("edit.duplicate_extend", &json!({})).unwrap();
            for id in [audio, midi] {
                assert_eq!(duplicate.session().track(id), repeat.session().track(id));
                assert_eq!(duplicate.session().track(id), extend.session().track(id));
            }
            assert_eq!(duplicate.session().edit.selection, Range::new(3000, 5000));
            assert_eq!(repeat.session().edit.selection, Range::new(1000, 3000));
            assert_eq!(extend.session().edit.selection, Range::new(1000, 5000));
            assert_eq!(extend.session().edit.timeline_selection, extend.session().edit.selection);
            assert!(duplicate.session().edit.selected_clips.iter().all(|id| duplicate.session().find_clip(*id).is_some()));
            assert_eq!(duplicate.journal.len(), 1);
        }
    }
}

#[test]
fn copied_effect_identity_is_exact_and_survives_native_save_reopen() {
    let mut s = Session::default();
    let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, Some("Gain"));
    let src = io::add_source(&mut s, "dc", AudioBuffer { sample_rate: 48_000, channels: vec![vec![0.5; 1000]] }, None, FileFormat::Wav);
    s.track_mut(t).unwrap().playlist_mut().unwrap().clips =
        vec![Clip::audio(ClipId(12), "whole", src, 0, 0, 1000), Clip::audio(ClipId(123), "other", src, 0, 5000, 1000)];
    s.next_id = 200;
    s.edit.values.insert("clip_fx.12.gain".into(), -6.0);
    s.edit.values.insert("clip_fx.12.custom.{id}".into(), 4.0);
    s.edit.values.insert("clip_fx.123.gain".into(), -20.0);
    s.edit.flags.insert("clip_fx.bypass.123".into());
    s.edit.selected_tracks = vec![t];
    s.edit.selected_clips = vec![ClipId(12)];
    s.edit.selection = Range::new(0, 1000);
    let mut e = Engine::new(s);
    e.execute("edit.duplicate", &json!({})).unwrap();
    let new = e.session().edit.selected_clips[0];
    assert_eq!(new, ClipId(200));
    assert_eq!(e.session().edit.values.get("clip_fx.200.gain"), Some(&-6.0));
    assert_eq!(e.session().edit.values.get("clip_fx.200.custom.{id}"), Some(&4.0));
    assert!(!e.session().edit.flag("clip_fx.bypass.200"));
    assert_eq!(e.session().edit.values.get("clip_fx.123.gain"), Some(&-20.0));
    let out = soundcraft_mix::render_clips(e.session(), t, Range::new(1000, 2000));
    let expected = 0.5 * 10f32.powf(-6.0 / 20.0);
    assert!((out[0][100] - expected).abs() < 0.00001);
    let directory = std::env::temp_dir().join(format!(
        "soundcraft-repeat-{}-{}",
        std::process::id(),
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
    ));
    std::fs::create_dir(&directory).unwrap();
    let path = directory.join("Repeat.scraft");
    e.execute("session.save", &json!({"path": path})).unwrap();
    let mut reopened = Engine::default();
    reopened.execute("session.open", &json!({"path": path})).unwrap();
    assert_eq!(reopened.session().edit.values, e.session().edit.values);
    assert_eq!(reopened.session().edit.flags, e.session().edit.flags);
    assert_eq!(reopened.session().track(t).unwrap().clips(), e.session().track(t).unwrap().clips());
    std::fs::remove_dir_all(&directory).unwrap();
}

#[test]
fn whole_midi_and_saved_performance_are_copied_without_reusing_ids() {
    let (mut e, _, midi, _) = fixture(false, true);
    let source = e.session().track(midi).unwrap().clips()[0].clone();
    let original = e.session().midi_originals.get(&source.id.0).unwrap().clone();
    e.execute("edit.repeat", &json!({"count": 3})).unwrap();
    for start in [3000, 5000, 7000] {
        let copy = e.session().track(midi).unwrap().clips().iter().find(|c| c.start == start).unwrap();
        assert_ne!(copy.id, source.id);
        assert_eq!(copy.content, source.content);
        assert_eq!(e.session().midi_originals.get(&copy.id.0), Some(&original));
    }
    let decoded = Session::from_json(&e.session().to_json().unwrap()).unwrap();
    assert_eq!(decoded.midi_originals, e.session().midi_originals);
}

#[test]
fn strict_counts_targets_and_unsupported_material_refuse_before_mutation() {
    for value in [json!(0), json!(-1), json!(1000), json!(2.5), json!("2"), json!(true), Value::Null] {
        let (e, _, _, _) = fixture(false, true);
        assert_refused(e, "edit.repeat", json!({"count": value}));
    }
    for what in ["fade", "envelope", "stretch", "effects", "locked", "group", "MIDI", "automation_view", "bad_sequence"] {
        let (mut e, audio, midi, _) = fixture(false, true);
        let s = e.session_mut();
        let c = &mut s.track_mut(audio).unwrap().playlist_mut().unwrap().clips[0];
        match what {
            "fade" => c.fade_in.len = 20,
            "envelope" => c.gain_env.push((10, -6.0)),
            "stretch" => c.stretch = 2.0,
            "effects" => {
                let id = c.id;
                s.edit.values.insert(format!("clip_fx.{}.gain", id.0), -6.0);
            }
            "locked" => c.edit_locked = true,
            "group" => c.group = Some(1),
            "MIDI" => {
                s.edit.selection = Range::new(1200, 2800);
            }
            "automation_view" => s.track_mut(audio).unwrap().view = "volume".into(),
            "bad_sequence" => {
                if let ClipContent::Midi { sequence } = &mut s.track_mut(midi).unwrap().playlist_mut().unwrap().clips[0].content {
                    sequence.notes[0].length = 1000;
                }
            }
            _ => unreachable!(),
        }
        // Make audio processing partial, without making the MIDI selection partial.
        if matches!(what, "fade" | "envelope" | "stretch" | "effects") {
            s.track_mut(audio).unwrap().playlist_mut().unwrap().clips[0].start = 900;
        }
        assert_refused(e, "edit.repeat", json!({"count": 1}));
    }
}

#[test]
fn hostile_positions_id_exhaustion_and_plan_products_are_atomic() {
    for what in [
        "selection",
        "clip",
        "offset",
        "id",
        "duplicate_id",
        "budget",
        "point",
        "pan_index",
        "curve_overflow",
        "sample_rate",
        "selected_id",
        "metadata_id",
    ] {
        let (mut e, audio, _, _) = fixture(true, true);
        let s = e.session_mut();
        match what {
            "selection" => s.edit.selection = Range { start: i64::MIN, end: i64::MAX },
            "clip" => s.track_mut(audio).unwrap().playlist_mut().unwrap().clips[0].start = i64::MAX - 1,
            "offset" => {
                if let ClipContent::Audio { offset, .. } = &mut s.track_mut(audio).unwrap().playlist_mut().unwrap().clips[0].content {
                    *offset = i64::MAX;
                }
            }
            "id" => s.next_id = u64::MAX - 1,
            "duplicate_id" => {
                let c = s.track(audio).unwrap().clips()[0].clone();
                s.track_mut(audio).unwrap().playlists[1].clips.push(c);
            }
            "budget" => {
                s.track_mut(audio).unwrap().automation[0].points =
                    (0..2000).map(|at| soundcraft_model::AutomationPoint { at, value: -1.0 }).collect();
            }
            "point" => s.track_mut(audio).unwrap().automation[0].points[0].value = f32::NAN,
            "pan_index" => s.track_mut(audio).unwrap().automation[0].param = AutoParam::Pan(u8::MAX),
            "curve_overflow" => {
                let lane = &mut s.track_mut(audio).unwrap().automation[0];
                lane.points[0].value = f32::MIN;
                lane.points[1].value = f32::MAX;
            }
            "sample_rate" => s.sample_rate = serde_json::from_value(json!(0)).unwrap(),
            "selected_id" => s.edit.selected_clips.push(ClipId(u64::MAX)),
            "metadata_id" => {
                s.edit.values.insert(format!("clip_fx.{}.gain", s.next_id), -2.0);
            }
            _ => unreachable!(),
        }
        assert_refused(e, "edit.repeat", json!({"count": 999}));
    }
}

#[test]
fn musical_grid_is_absolute_or_refused_without_sample_drift() {
    let (mut e, audio, _, _) = fixture(false, true);
    e.session_mut().edit.main_counter = TimeFormat::BarsBeats;
    // Integer samples per model tick: every destination is independently tick-derived.
    let result = e.execute("edit.repeat", &json!({"tracks": [audio], "count": 7})).unwrap();
    assert_eq!(result["musical"], true);
    assert_eq!(result["destination"]["end"], 17_000);
    for tempo_change in [false, true] {
        let mut s = Session::new("Fractional", SampleRate::HZ_44100);
        s.tempo.set_tempo(0, 123.0).unwrap();
        let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, None);
        let start = s.tempo.tick_to_samples(0, s.sample_rate);
        let end = s.tempo.tick_to_samples(3840, s.sample_rate);
        s.edit.selected_tracks = vec![t];
        s.edit.selection = Range::new(start, end);
        s.edit.main_counter = TimeFormat::BarsBeats;
        if tempo_change {
            s.tempo.set_tempo(3840, 140.0).unwrap();
        }
        let absolute_last = s.tempo.tick_to_samples(3840 * 100, s.sample_rate);
        assert_ne!(absolute_last, end * 100);
        assert_refused(Engine::new(s), "edit.repeat", json!({"count": 99}));
    }
}

#[test]
fn one_sample_selection_and_empty_lanes_keep_clear_boundary_semantics() {
    let (mut e, audio, _, _) = fixture(false, true);
    let empty = AutomationLane::new(AutoParam::Pan(1));
    e.session_mut().track_mut(audio).unwrap().automation.push(empty.clone());
    let before = e.session().track(audio).unwrap().automation[0].value_at(1300, 0.0);
    e.execute("edit.repeat", &json!({"tracks": [audio, audio], "start": 1300, "end": 1301, "count": 3})).unwrap();
    let t = e.session().track(audio).unwrap();
    for at in 1301..1304 {
        assert_eq!(t.automation[0].value_at(at, 0.0), before);
    }
    assert_eq!(t.automation.last().unwrap(), &empty);
    assert_eq!(t.clips().iter().filter(|c| c.start >= 1301 && c.start < 1304).count(), 3);
    assert_eq!(e.undo_history().len(), 1);
}

#[test]
fn looped_sources_and_destinations_refuse_without_consuming_redo() {
    for shuffle in [false, true] {
        for command in ["edit.duplicate", "edit.repeat", "edit.duplicate_extend"] {
            for index in [0, 1, 2, 3] {
                // In Slip, the last clip is outside the destination and is retained intact.
                if !shuffle && index == 3 {
                    continue;
                }
                let (mut e, audio, _, _) = fixture(shuffle, false);
                let s = e.session_mut();
                s.track_mut(audio).unwrap().playlist_mut().unwrap().clips[index].loop_id = Some(42);
                s.next_id = 100;
                e.execute("mix.volume", &json!({"track": audio, "db": -2.0})).unwrap();
                assert!(e.undo());
                assert!(e.can_redo());
                assert_refused(e, command, json!({"count": 1}));
            }
        }
        let (mut e, audio, _, other) = fixture(shuffle, false);
        let s = e.session_mut();
        s.next_id = 100;
        s.track_mut(other).unwrap().playlist_mut().unwrap().clips[0].loop_id = Some(42);
        s.track_mut(audio).unwrap().playlists[1].clips[0].loop_id = Some(43);
        if !shuffle {
            s.track_mut(audio).unwrap().playlist_mut().unwrap().clips[3].loop_id = Some(44);
        }
        let other_before = s.track(other).unwrap().clone();
        let alternate = s.track(audio).unwrap().playlists[1].clone();
        let last = s.track(audio).unwrap().clips()[3].clone();
        e.execute("edit.repeat", &json!({"count": 1})).unwrap();
        assert_eq!(e.session().track(other).unwrap(), &other_before);
        assert_eq!(e.session().track(audio).unwrap().playlists[1], alternate);
        if !shuffle {
            assert_eq!(e.session().find_clip(last.id).unwrap().1, &last);
        }
    }
}

#[test]
fn untouched_loop_identity_cannot_collide_with_new_clip_ids() {
    for command in ["edit.duplicate", "edit.repeat", "edit.duplicate_extend"] {
        let (mut e, _, _, other) = fixture(false, false);
        let s = e.session_mut();
        let next = s.next_id;
        s.track_mut(other).unwrap().playlist_mut().unwrap().clips[0].loop_id = Some(next);
        assert_refused(e, command, json!({"count": 1}));
    }
}

#[test]
fn whole_curved_fades_copy_exactly_and_undo_as_one_command() {
    for shuffle in [false, true] {
        for command in ["edit.duplicate", "edit.repeat", "edit.duplicate_extend"] {
            let mut s = Session::default();
            let track = s.add_track(TrackKind::Audio, ChannelFormat::Stereo, None);
            let source = io::add_source(
                &mut s,
                "curved synthetic",
                AudioBuffer { sample_rate: 48_000, channels: vec![vec![0.5; 128], vec![-0.25; 128]] },
                None,
                FileFormat::Wav,
            );
            let id = s.new_clip_id();
            let mut clip = Clip::audio(id, "curved", source, 0, 0, 128);
            clip.fade_in = soundcraft_model::Fade { len: 31, shape: soundcraft_model::FadeShape::SCurve, curvature: 0.5 };
            clip.fade_out = soundcraft_model::Fade { len: 17, shape: soundcraft_model::FadeShape::EqualPower, curvature: -0.4 };
            s.track_mut(track).unwrap().playlist_mut().unwrap().clips.push(clip.clone());
            s.edit.selected_tracks = vec![track];
            s.edit.selection = Range::new(0, 128);
            s.edit.edit_mode = if shuffle { EditMode::Shuffle } else { EditMode::Slip };
            let before = s.clone();
            let rendered = soundcraft_mix::render_clips(&s, track, Range::new(0, 128));
            let mut e = Engine::new(s);
            e.execute(command, &json!({"count": 1})).unwrap();
            let copied = &e.session().track(track).unwrap().clips()[1];
            let mut expected = clip;
            expected.id = copied.id;
            expected.start = 128;
            assert_eq!(copied, &expected);
            assert_eq!(soundcraft_mix::render_clips(e.session(), track, Range::new(128, 256)), rendered);
            assert_eq!(e.undo_history().len(), 1);
            let after = e.session().clone();
            assert!(e.undo());
            assert_eq!(e.session(), &before);
            assert!(e.redo());
            assert_eq!(e.session(), &after);
        }
    }
}

#[test]
fn malformed_fade_curvature_refuses_before_mutation() {
    for command in ["edit.duplicate", "edit.repeat", "edit.duplicate_extend"] {
        for fade_in in [false, true] {
            for curvature in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, -1.01, 1.01] {
                let (mut e, audio, _, _) = fixture(false, false);
                let clip = &mut e.session_mut().track_mut(audio).unwrap().playlist_mut().unwrap().clips[0];
                if fade_in {
                    clip.fade_in.curvature = curvature;
                } else {
                    clip.fade_out.curvature = curvature;
                }
                e.execute("mix.volume", &json!({"track": audio, "db": -2.0})).unwrap();
                assert!(e.undo());
                assert!(e.can_redo());
                assert_refused(e, command, json!({"count": 1}));
            }
        }
    }
}

#[test]
fn repeated_headroom_and_effect_metadata_preserve_pcm_without_file_delivery() {
    let mut s = Session::default();
    let track = s.add_track(TrackKind::Audio, ChannelFormat::Stereo, None);
    let buffer = AudioBuffer { sample_rate: 48_000, channels: vec![[1.25, -2.5, -0.0, 0.25].repeat(16), [-2.5, 1.25, -0.0, -0.25].repeat(16)] };
    let pcm_bits: Vec<Vec<_>> = buffer.channels.iter().map(|ch| ch.iter().map(|x| x.to_bits()).collect()).collect();
    let source = io::add_source(&mut s, "Headroom", buffer, None, FileFormat::Wav);
    let data = Arc::clone(s.pool.get(source).unwrap());
    let left = s.new_clip_id();
    let right = s.new_clip_id();
    s.track_mut(track).unwrap().playlist_mut().unwrap().clips =
        vec![Clip::audio(left, "Left", source, 0, 0, 32), Clip::audio(right, "Right", source, 32, 32, 32)];
    for id in [left, right] {
        s.edit.values.insert(format!("clip_fx.{}.gain", id.0), -6.0);
    }
    s.edit.flags.insert(format!("clip_fx.bypass.{}", right.0));
    s.edit.selected_tracks = vec![track];
    s.edit.selected_clips = vec![left, right];
    s.edit.selection = Range::new(0, 64);
    let before = s.clone();
    let rendered = soundcraft_mix::render_clips(&s, track, Range::new(0, 64));
    assert!(rendered.iter().flatten().any(|x| x.abs() > 1.0));
    let mut e = Engine::new(s);
    e.execute("edit.repeat", &json!({"count": 1})).unwrap();
    let copied = &e.session().track(track).unwrap().clips()[2..];
    assert_eq!(copied.len(), 2);
    for clip in copied {
        assert_ne!(clip.id, left);
        assert_ne!(clip.id, right);
        assert_eq!(clip.source(), Some(source));
        assert_eq!(e.session().edit.values.get(&format!("clip_fx.{}.gain", clip.id.0)), Some(&-6.0));
        assert_eq!(e.session().edit.flag(&format!("clip_fx.bypass.{}", clip.id.0)), clip.start == 96);
    }
    assert_eq!(soundcraft_mix::render_clips(e.session(), track, Range::new(64, 128)), rendered);
    assert!(Arc::ptr_eq(e.session().pool.get(source).unwrap(), &data));
    assert_eq!(data.buffer.channels.iter().map(|ch| ch.iter().map(|x| x.to_bits()).collect::<Vec<_>>()).collect::<Vec<_>>(), pcm_bits);
    assert_eq!(e.undo_history().len(), 1);
    let after = e.session().clone();
    assert!(e.undo());
    assert_eq!(e.session(), &before);
    assert!(e.redo());
    assert_eq!(e.session(), &after);
    assert!(Arc::ptr_eq(e.session().pool.get(source).unwrap(), &data));
}

fn barrier_fixture(copies: i64, exact_gap: bool, follow: bool) -> (Engine, TrackId, ClipId, ClipId, Option<ClipId>) {
    let mut s = Session::default();
    let track = s.add_track(TrackKind::Audio, ChannelFormat::Mono, None);
    let source = io::add_source(&mut s, "barrier", AudioBuffer { sample_rate: 48_000, channels: vec![vec![0.25; 1024]] }, None, FileFormat::Wav);
    let lock_at = if exact_gap { 100 + copies * 100 } else { 500 };
    let first = s.new_clip_id();
    let moving = (!exact_gap).then(|| s.new_clip_id());
    let locked = s.new_clip_id();
    let tail = s.new_clip_id();
    let mut clips = vec![Clip::audio(first, "source", source, 0, 0, 100)];
    if let Some(id) = moving {
        clips.push(Clip::audio(id, "moving", source, 100, 100, 100));
    }
    let mut barrier = Clip::audio(locked, "locked", source, 400, lock_at, 100);
    barrier.time_locked = true;
    clips.extend([barrier, Clip::audio(tail, "tail", source, 800, 800, 100)]);
    let target = s.track_mut(track).unwrap();
    target.playlist_mut().unwrap().clips = clips;
    for param in [AutoParam::Volume, AutoParam::Mute] {
        let mut lane = AutomationLane::new(param.clone());
        for (at, volume, mute) in [(0, -6.0, 0.0), (40, -12.0, 1.0), (99, -3.0, 0.0), (lock_at, -2.0, 0.0), (900, -9.0, 1.0)] {
            lane.set_point(at, if matches!(param, AutoParam::Mute) { mute } else { volume });
        }
        if !exact_gap {
            lane.set_point(150, if matches!(param, AutoParam::Mute) { 1.0 } else { -18.0 });
        }
        target.automation.push(lane);
    }
    s.edit.selected_tracks = vec![track];
    s.edit.selection = Range::new(0, 100);
    s.edit.edit_mode = EditMode::Shuffle;
    s.edit.automation_follows_edit = follow;
    (Engine::new(s), track, locked, tail, moving)
}

#[test]
fn shuffle_fits_before_a_locked_barrier_and_preserves_its_automation_tail() {
    for command in ["edit.duplicate", "edit.repeat", "edit.duplicate_extend"] {
        for exact_gap in [false, true] {
            for follow in [false, true] {
                let copies = if command == "edit.repeat" { if exact_gap { 3 } else { 2 } } else { 1 };
                let (mut e, track, locked, tail, moving) = barrier_fixture(copies, exact_gap, follow);
                let before = e.session().clone();
                let barrier = before.find_clip(locked).unwrap().1;
                let tail_clip = before.find_clip(tail).unwrap().1;
                e.execute(command, &json!({"count": copies})).unwrap();
                assert_eq!(e.session().find_clip(locked).unwrap().1, barrier);
                assert_eq!(e.session().find_clip(tail).unwrap().1, tail_clip);
                if let Some(id) = moving {
                    assert_eq!(e.session().find_clip(id).unwrap().1.start, 100 + copies * 100);
                }
                let old_lanes = &before.track(track).unwrap().automation;
                let new_lanes = &e.session().track(track).unwrap().automation;
                if follow {
                    for (old, new) in old_lanes.iter().zip(new_lanes) {
                        for point in old.points.iter().filter(|p| p.at >= barrier.start) {
                            assert!(new.points.contains(point));
                        }
                        for at in [barrier.start, barrier.start + 1, 700, 800, 900] {
                            assert!((new.value_at(at, 0.0) - old.value_at(at, 0.0)).abs() < 0.00002);
                        }
                        for k in 0..copies {
                            for offset in [0, 1, 39, 40, 98, 99] {
                                assert!((new.value_at(100 + k * 100 + offset, 0.0) - old.value_at(offset, 0.0)).abs() < 0.00002);
                            }
                        }
                        if moving.is_some() {
                            assert_eq!(new.value_at(150 + copies * 100, 0.0), old.value_at(150, 0.0));
                        }
                    }
                } else {
                    assert_eq!(new_lanes, old_lanes);
                }
                assert_eq!(e.undo_history().len(), 1);
                let after = e.session().clone();
                assert!(e.undo());
                assert_eq!(e.session(), &before);
                assert!(e.redo());
                assert_eq!(e.session(), &after);
            }
        }
    }
}

#[test]
fn shuffle_tight_gaps_crossfades_and_automation_barrier_collisions_are_atomic() {
    for command in ["edit.duplicate", "edit.repeat", "edit.duplicate_extend"] {
        for problem in ["gap", "crossfade", "automation"] {
            let (mut e, track, locked, _, _) = barrier_fixture(1, false, true);
            let s = e.session_mut();
            let expected = match problem {
                "gap" => {
                    s.find_clip_mut(locked).unwrap().start = 250;
                    "locked clip"
                }
                "crossfade" => {
                    let first = s.track(track).unwrap().clips()[0].id;
                    let clip = s.find_clip_mut(first).unwrap();
                    clip.length = 120;
                    clip.fade_out = soundcraft_model::Fade { len: 20, shape: soundcraft_model::FadeShape::Linear, curvature: 0.0 };
                    "crossfade"
                }
                "automation" => {
                    s.track_mut(track).unwrap().automation[0].set_point(400, -7.0);
                    "automation would cross a locked clip boundary"
                }
                _ => unreachable!(),
            };
            e.execute("mix.volume", &json!({"track": track, "db": -2.0})).unwrap();
            assert!(e.undo());
            assert!(e.can_redo());
            let params = json!({"count": 1});
            let mut diagnostic = Engine::new(e.session().clone());
            let error = diagnostic.execute(command, &params).unwrap_err().to_string();
            assert!(error.contains(expected), "{command} {problem}: {error}");
            assert_refused(e, command, params);
        }
    }
}

#[test]
fn planned_ids_reserve_nonclip_inventory_without_mutating_rejected_plans() {
    for command in ["edit.duplicate", "edit.repeat", "edit.duplicate_extend"] {
        let (mut e, _, _, _) = fixture(false, false);
        let mut extra = e.session().sources[0].clone();
        extra.id = soundcraft_model::SourceId(50_000);
        e.session_mut().sources.push(extra);
        let before = e.session().clone();
        let old_ids: Vec<_> = before.all_clips().map(|c| c.id).collect();
        e.execute(command, &json!({"count": 1})).unwrap();
        let new_ids: Vec<_> = e.session().all_clips().filter(|c| !old_ids.contains(&c.id)).map(|c| c.id.0).collect();
        assert!(!new_ids.is_empty());
        assert!(new_ids.iter().all(|id| *id > 50_000));
        assert!(new_ids.iter().all(|id| *id < e.session().next_id));
        assert_eq!(e.session().sources, before.sources);
        assert!(e.undo());
        assert_eq!(e.session(), &before);
        for high in [50_000, u64::MAX - 1, u64::MAX] {
            let (mut e, audio, _, _) = fixture(false, false);
            let mut extra = e.session().sources[0].clone();
            extra.id = soundcraft_model::SourceId(high);
            e.session_mut().sources.push(extra);
            if high == 50_000 {
                e.session_mut().track_mut(audio).unwrap().playlist_mut().unwrap().clips[0].gain_db = f32::NAN;
            }
            e.execute("mix.volume", &json!({"track": audio, "db": -2.0})).unwrap();
            assert!(e.undo());
            assert!(e.can_redo());
            assert_refused(e, command, json!({"count": 1}));
        }
    }
}
