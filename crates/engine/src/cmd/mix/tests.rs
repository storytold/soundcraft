use super::*;
use soundcraft_audio_io::AudioBuffer;
use soundcraft_model::{ChannelFormat, Clip, SourceAudio, SourceId, TrackKind};
use soundcraft_time::Range;
use std::sync::Arc;

fn rms_db(before: &[f32], after: &[f32]) -> f32 {
    let energy = |xs: &[f32]| xs.iter().map(|x| x * x).sum::<f32>() / xs.len().max(1) as f32;
    10.0 * (energy(after).max(1e-20) / energy(before).max(1e-20)).log10()
}

fn tone_session(points: &[(i64, f32)]) -> (Engine, TrackId) {
    let mut s = Session::default();
    let n = 8192;
    let tone: Vec<f32> = (0..n).map(|i| (i as f32 * 0.07).sin() * 0.25).collect();
    s.pool.insert(SourceId(910), Arc::new(SourceAudio::new(AudioBuffer { sample_rate: 48_000, channels: vec![tone] })));
    let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, Some("Tone"));
    let id = s.new_clip_id();
    s.track_mut(t).unwrap().playlist_mut().unwrap().clips.push(Clip::audio(id, "tone", SourceId(910), 0, 0, n));
    s.track_mut(t).unwrap().mixer.volume_db = -3.0;
    for &(at, value) in points {
        s.track_mut(t).unwrap().lane_mut(&AutoParam::Volume).set_point(at, value);
    }
    s.edit.selected_tracks = vec![t];
    s.edit.selection = Range::new(2048, 6144);
    (Engine::new(s), t)
}

fn rendered(s: &Session) -> Vec<f32> {
    soundcraft_mix::render_range(s, Range::new(0, 8192), 1024).into_iter().next().unwrap_or_default()
}

fn convert(e: &mut Engine, t: TrackId, start: i64, end: i64) {
    e.session_mut().edit.selection = Range::new(start, end);
    e.execute("automation.volume_to_clip_gain", &json!({"track": t.0, "start": start, "end": end})).unwrap();
}

#[test]
fn partial_volume_to_clip_gain_keeps_each_region() {
    let points = [(0, -6.0), (2048, -6.0), (4096, -6.0), (6144, -6.0), (8192, -6.0)];
    let (mut e, t) = tone_session(&points);
    let before = rendered(e.session());
    convert(&mut e, t, 2048, 6144);
    let after = rendered(e.session());
    for (start, end) in [(0, 2048), (2048, 6144), (6144, 8192)] {
        let delta = rms_db(&before[start..end], &after[start..end]);
        assert!(delta.abs() < 0.02, "constant {start}..{end} changed by {delta} dB");
    }
    let tr = e.session().track(t).unwrap();
    assert!((tr.mixer.volume_db + 3.0).abs() < 1e-5);
    let lane = tr.lane(&AutoParam::Volume).unwrap();
    assert!((lane.value_at(100, 0.0) + 6.0).abs() < 1e-3);
    assert!((lane.value_at(3000, 0.0) + 3.0).abs() < 1e-3);
    assert!((lane.value_at(7000, 0.0) + 6.0).abs() < 1e-3);

    let (mut e, t) = tone_session(&[]);
    let before = rendered(e.session());
    convert(&mut e, t, 2048, 6144);
    let after = rendered(e.session());
    let diff = before.iter().zip(&after).map(|(a, b)| (a - b).abs()).fold(0.0f32, f32::max);
    assert!(diff < 1e-5, "no lane changed a sample by {diff}");

    let (mut e, t) = tone_session(&points);
    let before = rendered(e.session());
    convert(&mut e, t, 0, 8193);
    let after = rendered(e.session());
    let diff = before.iter().zip(&after).map(|(a, b)| (a - b).abs()).fold(0.0f32, f32::max);
    assert!(diff < 1e-4, "full selection changed a sample by {diff}");
    assert!(e.session().track(t).unwrap().lane(&AutoParam::Volume).is_none_or(|l| l.points.is_empty()));

    for points in
        [[(0, -12.0), (2048, -9.0), (4096, -6.0), (6144, -3.0), (8192, 0.0)], [(0, -12.0), (2048, -6.0), (4096, -12.0), (6144, -6.0), (8192, -12.0)]]
    {
        let (mut e, t) = tone_session(&points);
        let before = rendered(e.session());
        convert(&mut e, t, 2048, 6144);
        let after = rendered(e.session());
        for (start, end) in [(0, 2048), (6144, 8192)] {
            let delta = rms_db(&before[start..end], &after[start..end]);
            assert!(delta.abs() < 0.02, "outside {start}..{end} changed by {delta} dB for {points:?}");
        }
        let delta = rms_db(&before[2048..6144], &after[2048..6144]);
        assert!(delta.abs() < 0.15, "inside changed by {delta} dB for {points:?}");
    }
}

#[test]
fn lane_points_all_inside_the_selection_keep_the_outside_level() {
    let (mut e, t) = tone_session(&[(2048, -9.0), (6143, -9.0)]);
    let before = rendered(e.session());
    convert(&mut e, t, 2048, 6144);
    let after = rendered(e.session());
    for (start, end) in [(0, 2000), (2048, 6144), (6200, 8192)] {
        let delta = rms_db(&before[start..end], &after[start..end]);
        assert!(delta.abs() < 0.05, "{start}..{end} changed by {delta} dB");
    }
}

#[test]
fn set_point_from_drops_the_points_in_between() {
    let (mut e, t) = tone_session(&[(100, -20.0), (150, -20.0), (200, -20.0), (250, -20.0), (300, -20.0)]);
    e.execute("automation.set_point", &json!({"track": t.0, "from": 100, "at": 250, "value": 0.0})).unwrap();
    // A loop wrap: `from` lies after `at`, so nothing goes.
    e.execute("automation.set_point", &json!({"track": t.0, "from": 400, "at": 50, "value": -6.0})).unwrap();
    let lane = e.session().track(t).unwrap().lane(&AutoParam::Volume).unwrap();
    let points: Vec<(i64, f32)> = lane.points.iter().map(|p| (p.at, p.value)).collect();
    assert_eq!(points, [(50, -6.0), (100, -20.0), (250, 0.0), (300, -20.0)]);
}

#[test]
fn clip_gain_points_inside_the_selection_are_kept() {
    let (mut e, t) = tone_session(&[(0, -6.0), (8192, -6.0)]);
    let clip = e.session().track(t).unwrap().clips()[0].id;
    e.session_mut().find_clip_mut(clip).unwrap().gain_env = vec![(3000, 0.0), (4000, -20.0), (5000, 0.0)];
    let before = rendered(e.session());
    convert(&mut e, t, 2048, 6144);
    let after = rendered(e.session());
    for (start, end) in [(2048, 3000), (3500, 4500), (5000, 6144)] {
        let delta = rms_db(&before[start..end], &after[start..end]);
        assert!(delta.abs() < 0.2, "{start}..{end} changed by {delta} dB");
    }
}

#[test]
fn clip_gain_to_volume_keeps_the_existing_lane() {
    let clip_gain_to_volume = |points: &[(i64, f32)], env: Vec<(i64, f32)>| {
        let (mut e, t) = tone_session(points);
        let clip = e.session().track(t).unwrap().clips()[0].id;
        let c = e.session_mut().find_clip_mut(clip).unwrap();
        c.gain_db = 6.0;
        c.gain_env = env;
        let before = rendered(e.session());
        e.execute("automation.clip_gain_to_volume", &json!({"track": t.0, "start": 0, "end": 8192})).unwrap();
        let after = rendered(e.session());
        assert!(e.session().find_clip(clip).unwrap().1.gain_db == 0.0);
        (before, after)
    };
    let (before, after) = clip_gain_to_volume(&[(0, -6.0), (8192, -6.0)], Vec::new());
    let delta = rms_db(&before, &after);
    assert!(delta.abs() < 0.02, "level changed by {delta} dB");

    // A moving lane keeps its shape, and its points inside the clip carry the gain.
    let (before, after) = clip_gain_to_volume(&[(0, -12.0), (4096, -6.0), (8192, -12.0)], Vec::new());
    for (start, end) in [(0, 2048), (2048, 4096), (4096, 6144), (6144, 8192)] {
        let delta = rms_db(&before[start..end], &after[start..end]);
        assert!(delta.abs() < 0.1, "{start}..{end} changed by {delta} dB");
    }

    // The clip's own gain envelope rides on the lane value as well.
    let (before, after) = clip_gain_to_volume(&[(0, -6.0), (8192, -6.0)], vec![(2000, 0.0), (6000, -10.0)]);
    for (start, end) in [(3900, 4100), (5000, 5200)] {
        let delta = rms_db(&before[start..end], &after[start..end]);
        assert!(delta.abs() < 0.2, "{start}..{end} changed by {delta} dB");
    }
}

#[test]
fn clip_gain_to_volume_holds_the_envelope_ends_out_to_the_clip_edges() {
    let (mut e, t) = tone_session(&[(0, -6.0), (8192, -6.0)]);
    let clip = e.session().track(t).unwrap().clips()[0].id;
    let c = e.session_mut().find_clip_mut(clip).unwrap();
    c.gain_db = 6.0;
    c.gain_env = vec![(2000, 0.0), (6000, -10.0)];
    let before = rendered(e.session());
    e.execute("automation.clip_gain_to_volume", &json!({"track": t.0, "start": 0, "end": 8192})).unwrap();
    let after = rendered(e.session());
    for (start, end) in [(0, 1000), (1000, 2000), (6000, 7000), (7000, 8192)] {
        let delta = rms_db(&before[start..end], &after[start..end]);
        assert!(delta.abs() < 0.2, "{start}..{end} changed by {delta} dB");
    }
}

#[test]
fn clip_gain_to_volume_leaves_audio_outside_the_clip_alone() {
    // Three clips in a row; only the middle one is converted, and the lane extends past it on both sides.
    let (mut e, t) = tone_session(&[(0, -6.0), (16384, -6.0)]);
    let (middle, last) = (e.session_mut().new_clip_id(), e.session_mut().new_clip_id());
    let pl = e.session_mut().track_mut(t).unwrap().playlist_mut().unwrap();
    if let Some(c) = pl.clips.first_mut() {
        c.length = 2048;
    }
    pl.clips.push(Clip::audio(middle, "mid", SourceId(910), 2048, 2048, 4096));
    pl.clips.push(Clip::audio(last, "end", SourceId(910), 6144, 6144, 2048));
    e.session_mut().find_clip_mut(middle).unwrap().gain_db = 6.0;
    let before = rendered(e.session());
    e.execute("automation.clip_gain_to_volume", &json!({"track": t.0, "start": 2048, "end": 6144})).unwrap();
    let after = rendered(e.session());
    assert!(e.session().find_clip(middle).unwrap().1.gain_db == 0.0);
    for (start, end) in [(0, 2040), (2056, 6136), (6152, 8192)] {
        let delta = rms_db(&before[start..end], &after[start..end]);
        assert!(delta.abs() < 0.1, "{start}..{end} changed by {delta} dB");
    }
}

#[test]
fn insert_params_rejects_unknown_and_non_numeric_keys() {
    let mut e = Engine::default();
    e.execute("track.new", &json!({})).unwrap();
    let t = e.session().tracks[0].id.0;
    e.execute("mix.insert", &json!({"track": t, "slot": 0, "plugin": "gain"})).unwrap();
    fn gain(e: &Engine) -> f32 {
        e.session().tracks[0].mixer.inserts[0].as_ref().unwrap().params["gain"]
    }

    // A typo names no parameter: the call fails and says which key.
    let err = e.execute("mix.insert_params", &json!({"track": t, "slot": 0, "params": {"gain_db": -14.0}})).unwrap_err();
    assert!(err.to_string().contains("gain_db"), "{err}");

    // Mixed keys fail as a whole and change nothing.
    let err = e.execute("mix.insert_params", &json!({"track": t, "slot": 0, "params": {"gain": -14.0, "width": 1.0}})).unwrap_err();
    assert!(err.to_string().contains("width"), "{err}");
    assert_eq!(gain(&e), 0.0);

    // Non-numeric and non-finite values are rejected too.
    assert!(e.execute("mix.insert_params", &json!({"track": t, "slot": 0, "params": {"gain": "loud"}})).is_err());
    assert!(e.execute("mix.insert_params", &json!({"track": t, "slot": 0, "params": {"gain": null}})).is_err());
    assert_eq!(gain(&e), 0.0);

    // Correct keys still apply, clamped to the parameter range.
    let r = e.execute("mix.insert_params", &json!({"track": t, "slot": 0, "params": {"gain": -14.0}})).unwrap();
    assert_eq!(r["set"], 1);
    assert_eq!(gain(&e), -14.0);
    e.execute("mix.insert_params", &json!({"track": t, "slot": 0, "params": {"gain": 999.0}})).unwrap();
    assert_eq!(gain(&e), 24.0);

    // Empty params with a preset name still recall (the rename path).
    e.execute("mix.insert_params", &json!({"track": t, "slot": 0, "params": {}, "preset": "Saved"})).unwrap();
    assert_eq!(e.session().tracks[0].mixer.inserts[0].as_ref().unwrap().preset, "Saved");
}

#[test]
fn copy_to_send_rejects_an_invalid_send_slot() {
    let mut e = crate::demo::demo_engine();
    let history = e.undo_history().len();
    let lanes = |e: &Engine| e.session().tracks.iter().filter(|t| t.lane(&AutoParam::SendLevel(0)).is_some_and(|l| !l.points.is_empty())).count();
    let before = lanes(&e);
    for bad_params in [json!({"track": "Kick", "send": 99}), json!({"track": "Kick", "send": "zz"})] {
        let r = e.execute("automation.copy_to_send", &bad_params);
        assert!(r.is_err(), "{bad_params} accepted: {r:?}");
        assert_eq!(e.undo_history().len(), history, "{bad_params} pushed an undo step");
        assert_eq!(lanes(&e), before, "{bad_params} edited Send A");
    }
    // No `send` (the menu item's call) still means Send A.
    e.execute("automation.copy_to_send", &json!({"track": "Kick"})).unwrap();
    assert_eq!(lanes(&e), before + 1);
    // A valid slot still copies.
    e.execute("automation.copy_to_send", &json!({"track": "Kick", "send": "a"})).unwrap();
    assert_eq!(lanes(&e), before + 1);
}

#[test]
fn retargeting_a_send_keeps_its_settings() {
    let mut e = Engine::default();
    e.execute("track.new", &json!({"count": 1, "name": "Gtr", "format": "mono"})).unwrap();
    e.execute("mix.new_bus", &json!({"name": "Verb"})).unwrap();
    e.execute("mix.send", &json!({"track": "Gtr", "slot": 0, "bus": "Verb", "level_db": -9.0, "pre_fader": true})).unwrap();
    e.execute("mix.send_level", &json!({"track": "Gtr", "slot": 0, "pan": 0.5, "mute": true})).unwrap();
    e.execute("mix.new_bus", &json!({"name": "Delay"})).unwrap();
    e.execute("mix.send", &json!({"track": "Gtr", "slot": 0, "bus": "Delay"})).unwrap();
    let sl = e.session().tracks[0].mixer.sends[0].clone().unwrap();
    let delay = e.session().busses.iter().find(|b| b.name == "Delay").unwrap().id;
    assert_eq!(sl.target, soundcraft_model::Route::Bus(delay));
    assert_eq!((sl.level_db, sl.pan, sl.mute, sl.pre_fader), (-9.0, 0.5, true, true));
}

fn grouped_demo() -> Engine {
    let mut e = crate::demo::demo_engine();
    e.execute("mix.volume", &json!({"tracks": [3], "db": 0.0})).unwrap();
    e.execute("mix.volume", &json!({"tracks": [6], "db": 0.0})).unwrap();
    e.execute("track.group", &json!({"tracks": [3, 6]})).unwrap();
    e
}

fn volumes(e: &Engine) -> Vec<f32> {
    [3, 6].iter().map(|id| e.session().track(TrackId(*id)).unwrap().mixer.volume_db).collect()
}

#[test]
fn volume_delta_on_a_whole_group_applies_once() {
    let mut e = grouped_demo();
    e.execute("mix.volume", &json!({"tracks": [3, 6], "delta_db": -3.0})).unwrap();
    assert_eq!(volumes(&e), [-3.0, -3.0]);

    let mut e = grouped_demo();
    e.execute("mix.volume", &json!({"tracks": [3], "delta_db": -3.0})).unwrap();
    assert_eq!(volumes(&e), [-3.0, -3.0]);
}

#[test]
fn volume_absolute_on_a_whole_group_keeps_offsets() {
    let mut e = grouped_demo();
    e.session_mut().track_mut(TrackId(6)).unwrap().mixer.volume_db = -2.0;
    e.execute("mix.volume", &json!({"tracks": [3, 6], "db": -10.0})).unwrap();
    assert_eq!(volumes(&e), [-10.0, -12.0]);
}

#[test]
fn clear_with_unknown_param_keeps_lanes() {
    let (mut e, t) = tone_session(&[(0, -6.0), (100, -3.0)]);
    e.session_mut().track_mut(t).unwrap().lane_mut(&AutoParam::Pan(0)).set_point(0, 0.5);
    e.session_mut().edit.selection = Range::new(0, 0);
    let count = |e: &Engine| e.session().track(t).unwrap().automation.iter().map(|l| l.points.len()).sum::<usize>();
    let r = e.execute("automation.clear", &json!({"track": t.0, "param": "volum"}));
    assert!(r.is_err(), "unknown param succeeded: {r:?}");
    assert_eq!(count(&e), 3, "unknown param changed the lanes");
    e.execute("automation.clear", &json!({"track": t.0, "param": "volume"})).unwrap();
    assert_eq!(count(&e), 1, "named param cleared the wrong lanes");
    e.execute("automation.clear", &json!({"track": t.0})).unwrap();
    assert_eq!(count(&e), 0, "absent param must still clear every lane");
}

#[test]
fn send_rejects_unknown_bus() {
    let mut e = Engine::default();
    e.execute("track.new", &json!({"count": 1, "name": "Gtr", "format": "mono"})).unwrap();
    e.execute("mix.new_bus", &json!({"name": "Verb"})).unwrap();
    let err = e.execute("mix.send", &json!({"track": "Gtr", "bus": "Nowhere"})).unwrap_err();
    assert!(err.to_string().contains("no bus named \"Nowhere\""), "{err}");
    assert_eq!(e.session().busses.len(), 1);
    assert!(e.session().tracks[0].mixer.sends.iter().all(Option::is_none));
    e.execute("mix.send", &json!({"track": "Gtr", "bus": "verb"})).unwrap();
    assert_eq!(e.session().busses.len(), 1);
}

#[test]
fn insert_and_send_reject_invalid_slot() {
    let mut e = Engine::default();
    e.execute("track.new", &json!({"count": 1, "name": "Gtr", "format": "mono"})).unwrap();
    e.execute("mix.new_bus", &json!({"name": "Verb"})).unwrap();
    for slot in [json!(12), json!("k"), json!("x1")] {
        let r = e.execute("mix.insert", &json!({"track": "Gtr", "slot": slot, "plugin": "compressor"}));
        assert!(r.is_err(), "insert slot {slot} accepted: {r:?}");
    }
    for slot in [json!(12), json!("k")] {
        let r = e.execute("mix.send", &json!({"track": "Gtr", "slot": slot, "bus": "Verb"}));
        assert!(r.is_err(), "send slot {slot} accepted: {r:?}");
    }
    let mixer = &e.session().tracks[0].mixer;
    assert!(mixer.inserts.iter().all(Option::is_none));
    assert!(mixer.sends.iter().all(Option::is_none));

    // Leaving `slot` out still picks the first free slot.
    let r = e.execute("mix.insert", &json!({"track": "Gtr", "plugin": "compressor"})).unwrap();
    assert_eq!(r["slot"], 0);
    let r = e.execute("mix.send", &json!({"track": "Gtr", "bus": "Verb"})).unwrap();
    assert_eq!(r["slot"], 0);
}
