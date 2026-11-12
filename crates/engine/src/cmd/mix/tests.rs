use super::*;
use craft_audio::AudioBuffer;
use soundcraft_dsp::Plugin;
use soundcraft_model::{ChannelFormat, Clip, INSTRUMENT_AUTO_SLOT, SourceAudio, SourceId, TrackKind};
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

#[test]
fn indexed_envelope_lookup_matches_the_walk() {
    let envs: [Vec<(i64, f32)>; 5] = [
        Vec::new(),
        vec![(100, -3.0)],
        vec![(0, 0.0), (100, -6.0), (100, 3.0), (250, -12.0), (900, 1.0)],
        // Out of order, as a hand-edited session file may hold.
        vec![(500, -1.0), (100, -6.0), (700, 4.0), (300, 2.0), (700, -9.0), (50, 0.5)],
        (0..200).map(|i| ((i * 37) % 1000, (i as f32 * 0.7).sin() * 12.0)).collect(),
    ];
    for env in &envs {
        let lookup = EnvLookup::new(env);
        for at in -20..1100 {
            assert_eq!(lookup.db(at).to_bits(), env_db(env, at).to_bits(), "{env:?} at {at}");
        }
    }
}

#[test]
fn clip_gain_to_volume_with_a_large_envelope_stays_fast() {
    let (mut e, t) = tone_session(&[]);
    let clip = e.session().track(t).unwrap().clips()[0].id;
    let n = 200_000i64;
    let c = e.session_mut().find_clip_mut(clip).unwrap();
    c.length = n * 5 + 10;
    c.gain_env = (0..n).map(|i| (i * 5, if i % 2 == 0 { -6.0 } else { 0.0 })).collect();
    let start = std::time::Instant::now();
    e.execute("automation.clip_gain_to_volume", &json!({"track": t.0, "start": 0, "end": n * 5 + 10})).unwrap();
    assert!(start.elapsed() < std::time::Duration::from_secs(10), "{:?}", start.elapsed());
    let lane = e.session().track(t).unwrap().lane(&AutoParam::Volume).unwrap();
    assert!(lane.points.len() as i64 >= n);
}

/// Live input toggles stay outside document history in both enabled and disabled states.
#[test]
fn live_input_states_are_transient_and_survive_document_history() {
    let mut e = Engine::default();
    e.execute("track.new", &json!({})).unwrap();
    let track = e.session().tracks[0].id;
    e.mark_clean();
    let history = e.undo_history();

    e.execute("mix.record_arm", &json!({"tracks": [track.0], "value": true})).unwrap();
    e.execute("track.set_record_to_input_only", &json!({})).unwrap();
    let mixer = &e.session().track(track).unwrap().mixer;
    assert!(mixer.record_arm && mixer.input_monitor);
    assert!(!e.is_dirty(), "live input state is not a document change");
    assert_eq!(e.undo_history(), history, "live input state has no Undo entry");

    // A later document edit captures a full Session snapshot. Undoing or redoing it must not
    // resurrect the live state that happened to be present when that snapshot was taken.
    e.execute("mix.volume", &json!({"tracks": [track.0], "db": -6.0})).unwrap();
    e.execute("mix.record_arm", &json!({"tracks": [track.0], "value": false})).unwrap();
    e.execute("mix.input_monitor", &json!({"tracks": [track.0], "value": false})).unwrap();
    assert!(e.is_dirty());

    assert!(e.undo());
    let mixer = &e.session().track(track).unwrap().mixer;
    assert!(!mixer.record_arm && !mixer.input_monitor);
    assert!(!e.is_dirty(), "undo returned to the saved document");

    assert!(e.redo());
    let mixer = &e.session().track(track).unwrap().mixer;
    assert!(!mixer.record_arm && !mixer.input_monitor);
    assert!(e.is_dirty());

    // Live states that are enabled after another document edit survive both directions too.
    e.execute("mix.volume", &json!({"tracks": [track.0], "db": -12.0})).unwrap();
    e.execute("mix.record_arm", &json!({"tracks": [track.0], "value": true})).unwrap();
    e.execute("mix.input_monitor", &json!({"tracks": [track.0], "value": true})).unwrap();

    assert!(e.undo());
    let mixer = &e.session().track(track).unwrap().mixer;
    assert!(mixer.record_arm && mixer.input_monitor);

    assert!(e.redo());
    let mixer = &e.session().track(track).unwrap().mixer;
    assert!(mixer.record_arm && mixer.input_monitor);
}

/// Returning to a saved point stays clean with independently enabled live flags on each track.
#[test]
fn live_input_states_keep_saved_history_points_clean() {
    let mut e = Engine::default();
    e.execute("track.new", &json!({"count": 2})).unwrap();
    let (a, b) = (e.session().tracks[0].id, e.session().tracks[1].id);
    e.mark_clean();
    e.execute("mix.volume", &json!({"tracks": [a.0], "db": -6.0})).unwrap();
    e.execute("mix.record_arm", &json!({"tracks": [a.0], "value": true})).unwrap();
    e.execute("mix.input_monitor", &json!({"tracks": [b.0], "value": true})).unwrap();
    assert!(e.undo());
    assert!(!e.is_dirty(), "live flags cannot dirty the saved point reached by Undo");
    for redo in [true, false, true] {
        assert!(if redo { e.redo() } else { e.undo() });
        let a = &e.session().track(a).unwrap().mixer;
        let b = &e.session().track(b).unwrap().mixer;
        assert!(a.record_arm && !a.input_monitor);
        assert!(!b.record_arm && b.input_monitor);
        assert_eq!(e.is_dirty(), redo);
    }
    e.mark_clean();
    e.execute("mix.record_arm", &json!({"tracks": [a.0], "value": false})).unwrap();
    e.execute("mix.input_monitor", &json!({"tracks": [b.0], "value": false})).unwrap();
    assert!(!e.is_dirty());
    assert!(e.undo());
    assert!(e.is_dirty());
    assert!(e.redo());
    assert!(!e.is_dirty(), "Redo to a saved armed snapshot stays clean after disarming");
}

/// Restoring a missing track never resurrects its historical arm or monitoring flags.
#[test]
fn live_input_states_of_restored_tracks_start_disabled() {
    let mut e = Engine::default();
    e.execute("track.new", &json!({"count": 2})).unwrap();
    let (a, b) = (e.session().tracks[0].id, e.session().tracks[1].id);
    e.execute("mix.record_arm", &json!({"tracks": [a.0], "value": true})).unwrap();
    e.execute("mix.input_monitor", &json!({"tracks": [b.0], "value": true})).unwrap();
    e.execute("track.delete", &json!({"tracks": [a.0]})).unwrap();
    assert!(e.undo());
    let mixer = &e.session().track(a).unwrap().mixer;
    assert!(!mixer.record_arm && !mixer.input_monitor);
    assert!(e.session().track(b).unwrap().mixer.input_monitor);
    assert!(e.redo());
    assert!(e.undo());
    assert!(!e.session().track(a).unwrap().mixer.record_arm);

    e.execute("track.new", &json!({})).unwrap();
    let c = e.session().tracks.last().unwrap().id;
    e.execute("mix.record_arm", &json!({"tracks": [c.0], "value": true})).unwrap();
    e.execute("mix.input_monitor", &json!({"tracks": [c.0], "value": true})).unwrap();
    assert!(e.undo());
    assert!(e.session().track(c).is_none());
    assert!(e.redo());
    let mixer = &e.session().track(c).unwrap().mixer;
    assert!(!mixer.record_arm && !mixer.input_monitor);
    assert!(e.session().track(b).unwrap().mixer.input_monitor);
}

fn instrument_params(e: &Engine, t: TrackId) -> std::collections::BTreeMap<String, f32> {
    e.session().track(t).and_then(|tr| tr.instrument.as_ref()).map(|i| i.params.clone()).unwrap_or_default()
}

#[test]
fn the_instrument_slot_takes_parameters_like_an_insert() {
    let mut e = Engine::default();
    e.execute("track.new", &json!({"kind": "instrument"})).unwrap();
    let t = e.session().tracks[0].id;
    let r = e.execute("mix.insert_param", &json!({"track": t.0, "slot": "Instrument", "param": "cutoff", "value": 5000})).unwrap();
    assert_eq!(r["value"], 5000.0);
    assert_eq!(instrument_params(&e, t).get("cutoff"), Some(&5000.0));
    let r = e.execute("mix.insert_params", &json!({"track": t.0, "slot": "instrument", "params": {"cutoff": 1.0e9, "level": -12}})).unwrap();
    assert_eq!(r["set"], 2);
    assert_eq!(instrument_params(&e, t).get("cutoff"), Some(&20_000.0), "clamped");
    assert_eq!(instrument_params(&e, t).get("level"), Some(&-12.0));
    e.execute("edit.undo", &json!({})).unwrap();
    assert_eq!(instrument_params(&e, t).get("cutoff"), Some(&5000.0));

    // Same checks as on an insert: unknown ids and bad values change nothing.
    for bad in [
        json!({"track": t.0, "slot": "instrument", "param": "nope", "value": 1}),
        json!({"track": t.0, "slot": "instrument", "param": "cutoff", "value": "open"}),
        json!({"track": t.0, "slot": "instrument", "param": -3, "value": 1}),
        json!({"track": t.0, "slot": "instrument", "param": 1.5, "value": 1}),
        json!({"track": t.0, "slot": "instruments", "param": "cutoff", "value": 1}),
    ] {
        assert!(e.execute("mix.insert_param", &bad).is_err(), "{bad}");
    }
    assert!(e.execute("mix.insert_params", &json!({"track": t.0, "slot": "instrument", "params": {"nope": 1}})).is_err());
    assert_eq!(instrument_params(&e, t).get("cutoff"), Some(&5000.0));

    // An audio track has no instrument; other insert commands keep refusing the name.
    e.execute("track.new", &json!({})).unwrap();
    let audio = e.session().tracks[1].id.0;
    let err = e.execute("mix.insert_param", &json!({"track": audio, "slot": "instrument", "param": "cutoff", "value": 1})).unwrap_err();
    assert!(err.to_string().contains("no instrument"), "{err}");
    assert!(e.execute("mix.insert_remove", &json!({"track": t.0, "slot": "instrument"})).is_err());
    assert!(e.execute("mix.insert_bypass", &json!({"track": t.0, "slot": "instrument"})).is_err());
}

#[test]
fn instrument_automation_lanes_take_the_stored_value() {
    let mut e = Engine::default();
    e.execute("track.new", &json!({"kind": "instrument"})).unwrap();
    let t = e.session().tracks[0].id;
    e.execute("mix.instrument_param", &json!({"track": t.0, "param": "cutoff", "value": 800})).unwrap();
    let param = AutoParam::parse("plugin:instrument:cutoff").unwrap();
    let tr = e.session().track(t).unwrap();
    assert_eq!(current_value(tr, &param), 800.0);
    e.execute("automation.set_point", &json!({"tracks": [t.0], "param": "plugin:instrument:cutoff", "at": 0, "value": 300})).unwrap();
    let lane = e.session().track(t).and_then(|tr| tr.lane(&param)).map(|l| l.value_at(0, 0.0));
    assert_eq!(lane, Some(300.0));
}

#[test]
fn toggles_and_choices_are_rounded_like_instrument_param() {
    let mut e = Engine::default();
    e.execute("track.new", &json!({})).unwrap();
    let t = e.session().tracks[0].id;
    e.execute("mix.insert", &json!({"track": t.0, "slot": 0, "plugin": "eq_7band", "params": {"hpf_on": 0.7}})).unwrap();
    let stored = |e: &Engine, k: &str| e.session().track(t).and_then(|tr| tr.mixer.inserts[0].as_ref()).and_then(|i| i.params.get(k).copied());
    assert_eq!(stored(&e, "hpf_on"), Some(1.0), "mix.insert");
    e.execute("mix.insert_param", &json!({"track": t.0, "slot": 0, "param": "hpf_on", "value": 0.3})).unwrap();
    assert_eq!(stored(&e, "hpf_on"), Some(0.0), "mix.insert_param");
    e.execute("mix.insert_params", &json!({"track": t.0, "slot": 0, "params": {"hpf_slope": 1.6}})).unwrap();
    assert_eq!(stored(&e, "hpf_slope"), Some(2.0), "mix.insert_params");
}

#[test]
fn a_lane_written_on_an_unstored_instrument_parameter_starts_from_its_default() {
    let mut e = Engine::default();
    e.execute("track.new", &json!({"kind": "instrument"})).unwrap();
    let t = e.session().tracks[0].id;
    if let Some(ins) = e.session_mut().track_mut(t).and_then(|tr| tr.instrument.as_mut()) {
        ins.params.clear();
    }
    let default = soundcraft_dsp::plugin_info("subtractive_synth").and_then(|i| i.param("cutoff")).map(|p| p.default).unwrap();
    e.execute("automation.write_range", &json!({"tracks": [t.0], "param": "plugin:instrument:cutoff", "start": 48_000, "end": 96_000, "value": 300}))
        .unwrap();
    let param = AutoParam::parse("plugin:instrument:cutoff").unwrap();
    let lane = e.session().track(t).and_then(|tr| tr.lane(&param)).cloned().unwrap();
    assert_eq!(lane.value_at(0, f32::NAN), default, "before the range: the default, not 0");
    assert_eq!(lane.value_at(72_000, f32::NAN), 300.0);
}

fn insert_param_value(e: &Engine, track: u64, id: &str) -> f32 {
    e.session()
        .track(TrackId(track))
        .and_then(|tr| tr.mixer.inserts.first())
        .and_then(|slot| slot.as_ref())
        .and_then(|ins| ins.params.get(id).copied())
        .unwrap_or(f32::NAN)
}

#[test]
fn text_sets_toggle_and_choice_parameters() {
    let mut e = Engine::default();
    e.execute("track.new", &json!({})).unwrap();
    let t = e.session().tracks[0].id.0;
    // mix.insert accepts both kinds of label in one call.
    e.execute("mix.insert", &json!({"track": t, "slot": 0, "plugin": "eq_7band", "params": {"hpf_on": "On", "hpf_slope": "24 dB/oct"}})).unwrap();
    assert_eq!(insert_param_value(&e, t, "hpf_on"), 1.0);
    assert_eq!(insert_param_value(&e, t, "hpf_slope"), 3.0);

    for (text, expected) in [("off", 0.0), ("true", 1.0), ("FALSE", 0.0), ("On", 1.0)] {
        let r = e.execute("mix.insert_param", &json!({"track": t, "slot": 0, "param": "hpf_on", "value": text})).unwrap();
        assert_eq!(r["value"], expected, "{text}");
        assert_eq!(insert_param_value(&e, t, "hpf_on"), expected, "{text}");
    }
    let r = e.execute("mix.insert_params", &json!({"track": t, "slot": 0, "params": {"hpf_slope": "18 db/oct"}})).unwrap();
    assert_eq!(r["set"], 1);
    assert_eq!(insert_param_value(&e, t, "hpf_slope"), 2.0, "choice labels are matched case-insensitively");
}

#[test]
fn instrument_slot_accepts_cutoff_as_text() {
    let mut e = Engine::default();
    e.execute("track.new", &json!({"kind": "instrument"})).unwrap();
    let t = e.session().tracks[0].id;
    let r = e.execute("mix.insert_param", &json!({"track": t.0, "slot": "instrument", "param": "cutoff", "value": "5000"})).unwrap();
    assert_eq!(r["value"], 5000.0);
    assert_eq!(instrument_params(&e, t).get("cutoff"), Some(&5000.0));
}

#[test]
fn bad_parameter_text_includes_examples() {
    let mut e = Engine::default();
    e.execute("track.new", &json!({})).unwrap();
    let t = e.session().tracks[0].id.0;
    e.execute("mix.insert", &json!({"track": t, "slot": 0, "plugin": "gain"})).unwrap();
    let gain = soundcraft_dsp::plugin_info("gain").unwrap().param("gain").unwrap();
    let examples = format!("{}, {}, {}", gain.format(gain.min), gain.format(gain.default), gain.format(gain.max));

    let err = e.execute("mix.insert_param", &json!({"track": t, "slot": 0, "param": "gain", "value": "loud"})).unwrap_err().to_string();
    assert!(err.contains("examples"), "{err}");
    assert!(err.contains("loud") && err.contains("Gain (gain)"), "{err}");
    assert!(err.contains(&examples), "{err}");
    assert_eq!(insert_param_value(&e, t, "gain"), 0.0);

    let huge = "x".repeat(257);
    let err = e.execute("mix.insert_param", &json!({"track": t, "slot": 0, "param": "gain", "value": huge})).unwrap_err().to_string();
    assert!(err.contains("examples") && err.contains(&examples), "{err}");
    assert_eq!(insert_param_value(&e, t, "gain"), 0.0);

    // A bad string on mix.insert stores nothing: the slot stays empty.
    let depth = e.revision;
    let err = e.execute("mix.insert", &json!({"track": t, "slot": 1, "plugin": "gain", "params": {"gain": "loud"}})).unwrap_err().to_string();
    assert!(err.contains("examples"), "{err}");
    assert!(e.session().tracks[0].mixer.inserts[1].is_none());
    assert_eq!(e.revision, depth);
}

#[test]
fn insert_params_applies_nothing_when_one_text_value_is_bad() {
    let mut e = Engine::default();
    e.execute("track.new", &json!({})).unwrap();
    let t = e.session().tracks[0].id.0;
    e.execute("mix.insert", &json!({"track": t, "slot": 0, "plugin": "trim"})).unwrap();
    e.execute("mix.insert_param", &json!({"track": t, "slot": 0, "param": "gain", "value": -6.0})).unwrap();
    let depth = e.revision;
    let preset = e.session().tracks[0].mixer.inserts[0].as_ref().unwrap().preset.clone();

    // "gain" parses; "invert" does not. Neither value, nor the preset name, is stored.
    let err = e
        .execute("mix.insert_params", &json!({"track": t, "slot": 0, "preset": "Broken", "params": {"gain": "-3", "invert": "maybe"}}))
        .unwrap_err()
        .to_string();
    assert!(err.contains("examples"), "{err}");
    assert!(err.contains("maybe"), "{err}");
    assert_eq!(insert_param_value(&e, t, "gain"), -6.0);
    assert_eq!(insert_param_value(&e, t, "invert"), 0.0);
    assert_eq!(e.session().tracks[0].mixer.inserts[0].as_ref().unwrap().preset, preset);
    assert_eq!(e.revision, depth);

    let r = e.execute("mix.insert_params", &json!({"track": t, "slot": 0, "params": {"gain": "-3", "invert": "On"}})).unwrap();
    assert_eq!(r["set"], 2);
    assert_eq!(insert_param_value(&e, t, "gain"), -3.0);
    assert_eq!(insert_param_value(&e, t, "invert"), 1.0);
}

/// Apple's Delay unit ships with macOS. A parameter is set from the text that unit itself
/// produces, and the stored number round-trips.
/// Ignored, not skipped: a green run must not claim AU text conversion was exercised.
#[cfg(target_os = "macos")]
#[test]
#[ignore = "no Apple Audio Unit converts parameter text (all 26 checked); run with --ignored against a unit that does"]
fn apple_au_parameter_text_round_trips() {
    let id = "au:aufx:dely:appl";
    assert!(super::super::clap::plugin_info(id).is_some(), "{id} ships with macOS");
    let mut e = Engine::default();
    e.execute("track.new", &json!({})).unwrap();
    let t = e.session().tracks[0].id.0;
    e.execute("mix.insert", &json!({"track": t, "slot": 0, "plugin": id})).unwrap();
    let rate = e.session().sample_rate.as_f64() as f32;
    let insert = e.session().tracks[0].mixer.inserts[0].clone().expect("inserted");
    let state = insert.state_bytes();
    let mut probe = soundcraft_mix::create_instance(id, state.as_deref(), rate, 512, 2).expect("Apple Delay instantiates");
    for (pid, value) in &insert.params {
        probe.plugin.set_param(pid, *value);
    }
    let info = probe.plugin.info();
    let mut fallback = None;
    let mut chosen = None;
    for param in info.params.iter().filter(|param| !matches!(param.unit, soundcraft_dsp::Unit::Toggle | soundcraft_dsp::Unit::Choice)) {
        let current = insert.params.get(param.id).copied().unwrap_or(param.default);
        let span = (param.max - param.min).abs();
        let mid = param.min + span / 2.0;
        for candidate in [param.max, param.min, mid, param.default] {
            if !candidate.is_finite() {
                continue;
            }
            let Some(text) = probe.plugin.value_text(param.id, candidate) else { continue };
            if text.is_empty() || text.chars().nth(256).is_some() {
                continue;
            }
            let Some(parsed) = probe.plugin.value_from_text(param.id, &text) else { continue };
            let tolerance = span.max(1.0) * 1e-4;
            if !parsed.is_finite() || (parsed - candidate).abs() > tolerance {
                continue;
            }
            let hit = (param.id, candidate, text, tolerance);
            if (candidate - current).abs() > tolerance {
                chosen = Some(hit);
                break;
            }
            fallback = Some(hit);
        }
        if chosen.is_some() {
            break;
        }
    }
    let Some((pid, candidate, text, tolerance)) = chosen.or(fallback) else {
        panic!("{id} exposes no parameter string conversion");
    };
    let result = e.execute("mix.insert_param", &json!({"track": t, "slot": 0, "param": pid, "value": text})).unwrap();
    let stored = insert_param_value(&e, t, pid);
    assert!((stored - candidate).abs() <= tolerance, "{candidate} -> {text:?} -> {stored}");
    let reported = result["value"].as_f64().unwrap_or(f64::NAN) as f32;
    assert!((reported - candidate).abs() <= tolerance, "{result}");
}

#[test]
fn parameter_text_matches_the_plugins_display() {
    // Equal after trim, Unicode case fold and collapsed whitespace.
    assert!(plugin_text_matches("29", "29"));
    assert!(plugin_text_matches("  29  ", "29"));
    assert!(plugin_text_matches("29 MS", "29 ms"));
    assert!(plugin_text_matches("29   ms", "29 ms"));
    // One trailing unit or label on the display, including a glued one.
    assert!(plugin_text_matches("29", "29 ms"));
    assert!(plugin_text_matches("50", "50%"));
    // A suffix may not continue the number, and two display tokens are not one unit.
    // "29" vs "29 ms extra" still agrees on the leading number, which is rule (b).
    assert!(!plugin_text_matches("29", "290"));
    assert!(!plugin_text_matches("1", "10 ms"));
    assert!(plugin_text_matches("29", "29 ms extra"));
    // The display's last digit sets the slack: no decimals → half of 1; one decimal → half of 0.1.
    assert!(plugin_text_matches("29.0", "29"));
    assert!(plugin_text_matches("30.4", "30"));
    assert!(!plugin_text_matches("30.6", "30"));
    assert!(plugin_text_matches("-6.04", "−6.0 dB"));
    assert!(!plugin_text_matches("-6.06", "−6.0 dB"));
    // 0.5% of the displayed 1000.00 is 5, which is wider than half of 0.01.
    assert!(plugin_text_matches("1003", "1000.00"));
    assert!(!plugin_text_matches("1006", "1000.00"));
    // ASCII hyphen and Unicode minus U+2212 are the same number.
    assert!(plugin_text_matches("-6", "−6.0 dB"));
    assert!(plugin_text_matches("−6", "-6.0 dB"));
    assert!(plugin_text_matches("−6.0", "−6.0 dB"));
    // Garbage has no reading that lands on the displayed number.
    assert!(!plugin_text_matches("twintig", "0"));
    assert!(!plugin_text_matches("twintig", "0.0 dB"));
    assert!(!plugin_text_matches("loud", "0.0 dB"));
}

#[test]
fn parameter_text_outside_the_displayed_range_is_flagged() {
    assert!(!text_outside_display_range("29", "0", "100"));
    assert!(!text_outside_display_range("  29  ", "0 ms", "100 ms"));
    assert!(!text_outside_display_range("30.4", "0", "30"), "half of a no-decimal step still counts as 30");
    assert!(text_outside_display_range("30.6", "0", "30"));
    assert!(!text_outside_display_range("100.4", "0", "100"));
    assert!(text_outside_display_range("100.6", "0", "100"));
    assert!(text_outside_display_range("500", "0", "100"));
    assert!(text_outside_display_range("-30", "-24.0 dB", "12.0 dB"));
    assert!(!text_outside_display_range("-6", "−24.0 dB", "12.0 dB"));
    assert!(!text_outside_display_range("−6", "-24.0 dB", "12.0 dB"));
    // Not a bare number, or an endpoint that is not numeric: no range error.
    assert!(!text_outside_display_range("twintig", "0", "100"));
    assert!(!text_outside_display_range("29 ms", "0", "100"));
    assert!(!text_outside_display_range("500", "-inf dB", "24.0 dB"));
    assert!(!text_outside_display_range("1", "Off", "On"));
    // Endpoints are an interval even when the plugin lists the larger one first.
    assert!(!text_outside_display_range("-3", "12.0 dB", "-24.0 dB"));
    assert!(text_outside_display_range("30", "12.0 dB", "-24.0 dB"));
}

/// A parser that answers 0 for every unknown string, the Ample Guitar failure mode.
struct GarbageParsesAsZero;

impl Plugin for GarbageParsesAsZero {
    fn info(&self) -> &'static soundcraft_dsp::PluginInfo {
        &soundcraft_dsp::plugins::utility::GAIN_INFO
    }
    fn prepare(&mut self, _sample_rate: f32, _max_block: usize, _channels: usize) {}
    fn reset(&mut self) {}
    fn set_param(&mut self, _id: &str, _value: f32) -> bool {
        true
    }
    fn param(&self, _id: &str) -> Option<f32> {
        None
    }
    fn process(&mut self, _io: &mut [Vec<f32>], _frames: usize) {}

    fn value_from_text(&mut self, id: &str, text: &str) -> Option<f32> {
        if id != "gain" {
            return None;
        }
        // Display numbers become a plain value; 29 is the scaled control from the report.
        // Everything else, including words, is reported as success with 0.
        if let Some(n) = entire_number(text) {
            if (n - 29.0).abs() <= 1e-9 {
                return Some(0.028);
            }
            let clamped = n.clamp(0.0, 100.0);
            return Some((clamped / 100.0) as f32);
        }
        Some(0.0)
    }

    fn value_text(&mut self, id: &str, value: f32) -> Option<String> {
        if id != "gain" {
            return None;
        }
        let param = self.info().params.iter().find(|param| param.id == id)?;
        if (value - 0.028).abs() <= 1e-5 {
            return Some("29 ms".to_string());
        }
        if (value - param.min).abs() <= 1e-3 {
            return Some("0".to_string());
        }
        if (value - param.max).abs() <= 1e-3 {
            return Some("100".to_string());
        }
        if value.abs() <= 1e-6 {
            return Some("0".to_string());
        }
        Some(format!("{:.0}", f64::from(value) * 100.0))
    }
}

/// A live instance that renders text but refuses every string.
struct RefusingLive;

impl crate::LivePlugins for RefusingLive {
    fn plugin_id(&mut self, _track: TrackId, _slot: usize) -> Option<String> {
        Some("clap:not.installed".to_string())
    }
    fn value_text(&mut self, _at: crate::LiveSlot<'_>, _id: &str, value: f32) -> Option<String> {
        Some(format!("{value:.1} live"))
    }
    fn value_from_text(&mut self, _at: crate::LiveSlot<'_>, _id: &str, _text: &str) -> Option<f32> {
        None
    }
    fn value(&mut self, _at: crate::LiveSlot<'_>, _id: &str) -> Option<f32> {
        None
    }
}

#[test]
fn text_the_live_instance_refuses_does_not_load_a_temporary_plugin() {
    let gain = soundcraft_dsp::plugin_info("gain").and_then(|info| info.param("gain")).unwrap();
    let mut live = RefusingLive;
    let insert = Insert::new("clap:not.installed");
    let mut resolver = ParameterValueResolver::new("mix.insert_param", &insert, 48_000.0, TrackId(0), 0, Some(&mut live));
    let err = resolver.resolve(gain, &json!("loud")).unwrap_err().to_string();
    assert!(err.contains("`loud` is not a value of"), "{err}");
    assert!(err.contains("live"), "examples come from the live instance: {err}");
    assert!(!resolver.tried_instance, "no temporary instance for refused text");
}

#[test]
fn a_plugin_that_parses_garbage_as_zero_is_rejected() {
    let gain = soundcraft_dsp::plugin_info("gain").and_then(|info| info.param("gain")).unwrap();
    let mut resolver = ParameterValueResolver::with_instance("mix.insert_param", &Insert::new("gain"), 48_000.0, Box::new(GarbageParsesAsZero));

    let zero = resolver.resolve(gain, &json!("0")).unwrap();
    assert_eq!(zero, 0.0, "a displayed zero is a real value");
    let twenty_nine = resolver.resolve(gain, &json!("29")).unwrap();
    assert!((twenty_nine - 0.028).abs() < 1e-6, "{twenty_nine}");
    let rounded = resolver.resolve(gain, &json!("30.4")).unwrap();
    assert!((rounded - 0.304).abs() < 1e-4, "30.4 displays as 30, {rounded}");

    let garbage = resolver.resolve(gain, &json!("twintig")).unwrap_err().to_string();
    assert!(garbage.contains("`twintig` is not a value of Gain (gain); examples:"), "{garbage}");
    assert!(garbage.contains("examples"), "{garbage}");
    assert!(!garbage.contains("out of range"), "{garbage}");

    let high = resolver.resolve(gain, &json!("500")).unwrap_err().to_string();
    assert!(high.contains("`500` is out of range for Gain (gain) (0 … 100)"), "{high}");
    let low = resolver.resolve(gain, &json!("-1")).unwrap_err().to_string();
    assert!(low.contains("out of range") && low.contains("(0 … 100)"), "{low}");
    let edge = resolver.resolve(gain, &json!("100.6")).unwrap_err().to_string();
    assert!(edge.contains("out of range"), "{edge}");
    let just_inside = resolver.resolve(gain, &json!("100.4")).unwrap();
    assert!((just_inside - 1.0).abs() < 1e-5, "{just_inside}");
}

/// A plugin whose display text is long and carries control characters. It parses nothing.
struct NoisyDisplay;

impl Plugin for NoisyDisplay {
    fn info(&self) -> &'static soundcraft_dsp::PluginInfo {
        &soundcraft_dsp::plugins::utility::GAIN_INFO
    }
    fn prepare(&mut self, _sample_rate: f32, _max_block: usize, _channels: usize) {}
    fn reset(&mut self) {}
    fn set_param(&mut self, _id: &str, _value: f32) -> bool {
        true
    }
    fn param(&self, _id: &str) -> Option<f32> {
        None
    }
    fn process(&mut self, _io: &mut [Vec<f32>], _frames: usize) {}
    fn value_text(&mut self, _id: &str, _value: f32) -> Option<String> {
        Some(format!("dB\nERROR forged line\u{1b}[2J{}", "x".repeat(500)))
    }
}

#[test]
fn plugin_display_text_in_an_error_is_bounded_and_has_no_control_characters() {
    let gain = soundcraft_dsp::plugin_info("gain").and_then(|info| info.param("gain")).unwrap();
    let mut resolver = ParameterValueResolver::with_instance("mix.insert_param", &Insert::new("gain"), 48_000.0, Box::new(NoisyDisplay));
    let err = resolver.resolve(gain, &json!("loud")).unwrap_err().to_string();
    assert!(err.contains("examples"), "{err}");
    assert!(!err.chars().any(char::is_control), "{err:?}");
    assert!(err.chars().count() < 3 * (MAX_PARAM_TEXT_CHARS + 4) + 200, "three capped examples: {}", err.chars().count());
}

#[test]
fn text_for_a_built_in_never_loads_a_temporary_instance() {
    let gain = soundcraft_dsp::plugin_info("gain").and_then(|info| info.param("gain")).unwrap();
    let mut resolver = ParameterValueResolver::new("mix.insert_param", &Insert::new("gain"), 48_000.0, TrackId(0), 0, None);

    assert_eq!(resolver.resolve(gain, &json!(" -3 ")).unwrap(), -3.0);
    let err = resolver.resolve(gain, &json!("loud")).unwrap_err().to_string();
    assert!(err.contains("`loud` is not a value of Gain (gain)") && err.contains("examples"), "{err}");
    assert!(!resolver.tried_instance, "a built-in parses and formats its own numbers");
}

#[test]
fn a_built_in_reads_back_the_text_its_display_shows() {
    // Every built-in parameter: the text `display` shows for min, default and max resolves to
    // that value again, within the display's precision. So text means the displayed units for
    // built-ins as it does for hosted plugins, and error examples are valid input.
    for info in soundcraft_dsp::plugins() {
        let mut resolver = ParameterValueResolver::new("mix.insert_param", &Insert::new(info.id), 48_000.0, TrackId(0), 0, None);
        for param in info.params {
            for value in [param.min, param.default, param.max] {
                let shown = param.format(value);
                let back = resolver.resolve(param, &json!(shown)).unwrap_or_else(|err| panic!("{}.{}: `{shown}`: {err}", info.id, param.id));
                assert_eq!(param.format(back), shown, "{}.{}: `{shown}` read back as {back}", info.id, param.id);
            }
        }
        assert!(!resolver.tried_instance, "{}: a built-in reads its own display", info.id);
    }
    let gain = soundcraft_dsp::plugin_info("gain").and_then(|info| info.param("gain")).unwrap();
    let mut resolver = ParameterValueResolver::new("mix.insert_param", &Insert::new("gain"), 48_000.0, TrackId(0), 0, None);
    assert_eq!(resolver.resolve(gain, &json!("-3 dB")).unwrap(), -3.0);
    assert_eq!(resolver.resolve(gain, &json!("-3 db")).unwrap(), -3.0, "units ignore case");
    let err = resolver.resolve(gain, &json!("-3 ms")).unwrap_err().to_string();
    assert!(err.contains("not a value of Gain (gain)"), "another unit's suffix is refused: {err}");
    let hz =
        soundcraft_dsp::plugins().iter().flat_map(|info| info.params.iter()).find(|p| p.unit == soundcraft_dsp::Unit::Hz && p.max >= 2000.0).unwrap();
    assert_eq!(resolver.resolve(hz, &json!("1.2 kHz")).unwrap(), 1200.0, "kHz is scaled, not read as 1.2 Hz");
}

/// The same Apple Delay unit rejects a string that is not one of its parameter values.
#[cfg(target_os = "macos")]
#[test]
fn apple_au_garbage_parameter_text_is_rejected() {
    let id = "au:aufx:dely:appl";
    let info = super::super::clap::plugin_info(id).expect("Apple Delay ships with macOS");
    let param = info
        .params
        .iter()
        .find(|param| !matches!(param.unit, soundcraft_dsp::Unit::Toggle | soundcraft_dsp::Unit::Choice))
        .expect("Apple Delay has continuous parameters");
    let mut e = Engine::default();
    e.execute("track.new", &json!({})).unwrap();
    let t = e.session().tracks[0].id.0;
    e.execute("mix.insert", &json!({"track": t, "slot": 0, "plugin": id})).unwrap();
    let marker = [param.max, param.min, param.default].into_iter().find(|value| value.is_finite() && *value != 0.0).unwrap_or(param.default);
    e.execute("mix.insert_param", &json!({"track": t, "slot": 0, "param": param.id, "value": marker})).unwrap();
    let before = insert_param_value(&e, t, param.id);
    let err = e.execute("mix.insert_param", &json!({"track": t, "slot": 0, "param": param.id, "value": "twintig"})).unwrap_err().to_string();
    assert!(err.contains("twintig") && err.contains(&format!("{} ({})", param.name, param.id)), "{err}");
    assert!(err.contains("is not a value of") || err.contains("out of range"), "{err}");
    assert_eq!(insert_param_value(&e, t, param.id), before, "garbage must not be stored as 0");
}

/// Apple's DLS synth ships with every Mac: a hosted instrument's parameters can be set by
/// numeric id on its slot, which used to answer "not a built-in instrument".
#[cfg(target_os = "macos")]
#[test]
fn a_hosted_instruments_parameters_are_set_by_numeric_id() {
    let id = "au:aumu:dls :appl";
    let pi = super::super::clap::plugin_info(id)
        .and_then(|i| i.params.iter().find(|p| p.unit != soundcraft_dsp::Unit::Choice))
        .expect("Apple DLS ships with macOS and reports parameters");
    let n: u32 = pi.id.parse().unwrap();
    let mut e = Engine::default();
    e.execute("track.new", &json!({"kind": "instrument", "instrument": id})).unwrap();
    let t = e.session().tracks[0].id;
    let v = (pi.min + pi.max) / 2.0;
    e.execute("mix.instrument_param", &json!({"track": t.0, "param": n, "value": v})).unwrap();
    assert_eq!(instrument_params(&e, t).get(pi.id), Some(&v));
    e.execute("mix.insert_param", &json!({"track": t.0, "slot": "instrument", "param": n, "value": pi.max})).unwrap();
    assert_eq!(instrument_params(&e, t).get(pi.id), Some(&pi.max));
    let auto = format!("plugin:instrument:{n}");
    e.execute("automation.set_point", &json!({"tracks": [t.0], "param": auto, "at": 0, "value": pi.min})).unwrap();
}

fn plugin_lanes(e: &Engine, t: TrackId) -> Vec<(u8, String)> {
    e.session()
        .track(t)
        .map(|tr| {
            tr.automation
                .iter()
                .filter_map(|lane| match &lane.param {
                    AutoParam::Plugin { slot, param } => Some((*slot, param.clone())),
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default()
}

/// `MF Gain` is eq_7band's name for `mid_gain`. Ids, numbers and that name all store the id.
#[test]
fn insert_parameters_are_set_by_name() {
    let mut e = Engine::default();
    e.execute("track.new", &json!({})).unwrap();
    let t = e.session().tracks[0].id.0;
    e.execute("mix.insert", &json!({"track": t, "slot": 0, "plugin": "eq_7band", "params": {"MF Gain": -3.0, "mid_freq": 2500.0}})).unwrap();
    assert_eq!(insert_param_value(&e, t, "mid_gain"), -3.0);
    assert_eq!(insert_param_value(&e, t, "mid_freq"), 2500.0);
    assert!(!e.session().tracks[0].mixer.inserts[0].as_ref().unwrap().params.contains_key("MF Gain"));

    let r = e.execute("mix.insert_param", &json!({"track": t, "slot": 0, "param": "  mf   gain ", "value": -6.0})).unwrap();
    assert_eq!(r["value"], -6.0);
    assert_eq!(r["param"], "mid_gain");
    assert_eq!(r["id"], "mid_gain");
    assert_eq!(r["name"], "MF Gain");
    assert_eq!(insert_param_value(&e, t, "mid_gain"), -6.0);

    // The id still addresses the same parameter.
    e.execute("mix.insert_param", &json!({"track": t, "slot": 0, "param": "mid_gain", "value": -1.5})).unwrap();
    assert_eq!(insert_param_value(&e, t, "mid_gain"), -1.5);

    let r = e.execute("mix.insert_params", &json!({"track": t, "slot": 0, "params": {"MF Gain": -4.0, "mid_freq": 1500.0}})).unwrap();
    assert_eq!(r["set"], 2);
    assert_eq!(insert_param_value(&e, t, "mid_gain"), -4.0);
    assert_eq!(insert_param_value(&e, t, "mid_freq"), 1500.0);

    // Two keys, one parameter: nothing is stored.
    let depth = e.revision;
    let err = e.execute("mix.insert_params", &json!({"track": t, "slot": 0, "params": {"mid_gain": 1.0, "MF Gain": 2.0}})).unwrap_err().to_string();
    assert!(err.contains("both set") && err.contains("MF Gain (mid_gain)"), "{err}");
    assert_eq!(insert_param_value(&e, t, "mid_gain"), -4.0);
    assert_eq!(e.revision, depth);

    // A later unknown key does not keep the earlier name.
    let err = e.execute("mix.insert_params", &json!({"track": t, "slot": 0, "params": {"MF Gain": -9.0, "nope": 1.0}})).unwrap_err().to_string();
    assert!(err.contains("nope is not a parameter of EQ 7-Band"), "{err}");
    assert_eq!(insert_param_value(&e, t, "mid_gain"), -4.0);
    assert_eq!(e.revision, depth);

    // mix.insert resolves every key before it writes the slot.
    let err = e
        .execute("mix.insert", &json!({"track": t, "slot": 1, "plugin": "eq_7band", "params": {"MF Gain": -3.0, "nope": 1.0}}))
        .unwrap_err()
        .to_string();
    assert!(err.contains("nope"), "{err}");
    assert!(e.session().tracks[0].mixer.inserts[1].is_none());
    assert_eq!(e.revision, depth);
}

#[test]
fn automation_lanes_store_the_parameter_id_for_a_name() {
    let mut e = Engine::default();
    e.execute("track.new", &json!({"kind": "instrument"})).unwrap();
    let t = e.session().tracks[0].id;
    // "Cutoff" lowercases to the id. "Filter Env" does not, so it has to match the name.
    e.execute("automation.set_point", &json!({"track": t.0, "param": "plugin:instrument:Cutoff", "at": 0, "value": 400.0})).unwrap();
    e.execute("automation.set_point", &json!({"track": t.0, "param": "plugin:instrument:Filter Env", "at": 100, "value": -10.0})).unwrap();
    assert!(plugin_lanes(&e, t).contains(&(INSTRUMENT_AUTO_SLOT, "cutoff".to_string())));
    assert!(plugin_lanes(&e, t).contains(&(INSTRUMENT_AUTO_SLOT, "filter_env_amount".to_string())));
    assert!(!plugin_lanes(&e, t).iter().any(|(_, param)| param == "filter env" || param == "Cutoff"));

    e.execute("track.new", &json!({})).unwrap();
    let audio = e.session().tracks[1].id;
    e.execute("mix.insert", &json!({"track": audio.0, "slot": 0, "plugin": "eq_7band"})).unwrap();
    e.execute("automation.set_point", &json!({"track": audio.0, "param": "plugin:a:MF Gain", "at": 10, "value": -4.0})).unwrap();
    e.execute("automation.write_range", &json!({"track": audio.0, "param": "plugin:a:MF   Gain", "start": 0, "end": 48_000, "value": -6.0})).unwrap();
    let lane = AutoParam::Plugin { slot: 0, param: "mid_gain".into() };
    let written = e.session().track(audio).and_then(|tr| tr.lane(&lane)).unwrap();
    assert_eq!(written.value_at(1000, f32::NAN), -6.0);
    assert!(!plugin_lanes(&e, audio).iter().any(|(_, param)| param.to_ascii_lowercase().contains("mf")));
    let cleared = e.execute("automation.clear", &json!({"track": audio.0, "param": "plugin:a:mf gain", "start": 0, "end": 48_000})).unwrap();
    assert!(cleared["removed"].as_u64().unwrap_or(0) > 0);
    assert_eq!(plugin_lanes(&e, audio), vec![(0, "mid_gain".to_string())]);

    // Slot B is empty, so the parsed key is stored as AutoParam::parse left it.
    e.execute("automation.set_point", &json!({"track": audio.0, "param": "plugin:b:MF   Gain", "at": 20, "value": 1.0})).unwrap();
    assert!(plugin_lanes(&e, audio).contains(&(0, "mid_gain".to_string())));
    assert!(plugin_lanes(&e, audio).contains(&(1, "MF   Gain".to_string())));

    let depth = e.revision;
    let before = plugin_lanes(&e, audio);
    let err =
        e.execute("automation.set_point", &json!({"track": audio.0, "param": "plugin:a:not-a-real-param", "at": 30, "value": 1.0})).unwrap_err();
    assert!(err.to_string().contains("not-a-real-param is not a parameter of EQ 7-Band"), "{err}");
    assert_eq!(plugin_lanes(&e, audio), before);
    assert_eq!(e.revision, depth);
}

#[test]
fn automation_names_resolve_per_track_before_anything_is_stored() {
    let mut e = Engine::default();
    e.execute("track.new", &json!({})).unwrap();
    e.execute("track.new", &json!({})).unwrap();
    let (eq, empty) = (e.session().tracks[0].id, e.session().tracks[1].id);
    e.execute("mix.insert", &json!({"track": eq.0, "slot": 0, "plugin": "eq_7band"})).unwrap();
    e.execute("automation.set_point", &json!({"tracks": [eq.0, empty.0], "param": "plugin:a:MF Gain", "at": 10, "value": -4.0})).unwrap();
    assert_eq!(plugin_lanes(&e, eq), vec![(0, "mid_gain".to_string())]);
    assert_eq!(plugin_lanes(&e, empty), vec![(0, "MF Gain".to_string())]);

    e.execute("track.new", &json!({})).unwrap();
    e.execute("track.new", &json!({})).unwrap();
    let (eq, gain) = (e.session().tracks[2].id, e.session().tracks[3].id);
    e.execute("mix.insert", &json!({"track": eq.0, "slot": 0, "plugin": "eq_7band"})).unwrap();
    e.execute("mix.insert", &json!({"track": gain.0, "slot": 0, "plugin": "gain"})).unwrap();
    let depth = e.revision;
    let err =
        e.execute("automation.set_point", &json!({"tracks": [eq.0, gain.0], "param": "plugin:a:MF Gain", "at": 10, "value": -4.0})).unwrap_err();
    assert!(err.to_string().contains("is not a parameter of Gain"), "{err}");
    assert!(plugin_lanes(&e, eq).is_empty());
    assert!(plugin_lanes(&e, gain).is_empty());
    assert_eq!(e.revision, depth);
}

/// Apple's Delay unit: a parameter is set by the name `plugin_info` reports.
#[cfg(target_os = "macos")]
#[test]
fn apple_au_parameter_is_set_by_name() {
    let id = "au:aufx:dely:appl";
    let info = super::super::clap::plugin_info(id).expect("Apple Delay ships with macOS");
    let norm = super::super::param_key::normalize_key;
    let param = info
        .params
        .iter()
        .find(|param| {
            let name = norm(param.name);
            !name.is_empty() && name != norm(param.id) && info.params.iter().filter(|other| norm(other.name) == name).count() == 1
        })
        .expect("Apple Delay has a uniquely named parameter");
    let target = [param.max, param.min]
        .into_iter()
        .find(|value| value.is_finite() && (*value - param.default).abs() > 1e-4)
        .expect("the parameter has a finite value other than its default");
    let expected = param.clamp(target);
    let mut e = Engine::default();
    e.execute("track.new", &json!({})).unwrap();
    let t = e.session().tracks[0].id.0;
    e.execute("mix.insert", &json!({"track": t, "slot": 0, "plugin": id})).unwrap();
    let result = e.execute("mix.insert_param", &json!({"track": t, "slot": 0, "param": param.name, "value": target})).unwrap();
    assert_eq!(result["id"], param.id);
    assert_eq!(result["param"], param.id);
    assert_eq!(result["name"], param.name);
    assert_eq!(insert_param_value(&e, t, param.id), expected);
    assert!(!e.session().tracks[0].mixer.inserts[0].as_ref().unwrap().params.contains_key(param.name));
}

#[test]
fn an_empty_slot_rejects_an_oversized_automation_key() {
    let mut e = Engine::default();
    e.execute("track.new", &json!({})).unwrap();
    let t = e.session().tracks[0].id;
    let key = "x".repeat(201);
    let depth = e.revision;
    let err =
        e.execute("automation.set_point", &json!({"track": t.0, "param": format!("plugin:b:{key}"), "at": 0, "value": 1.0})).unwrap_err().to_string();
    assert!(err.contains("at most 200"), "{err}");
    assert!(!err.contains(&key), "{err}");
    assert!(plugin_lanes(&e, t).is_empty());
    assert_eq!(e.revision, depth);
}

#[test]
fn an_occupied_slot_without_metadata_keeps_only_an_exact_clear() {
    let mut e = Engine::default();
    e.execute("track.new", &json!({})).unwrap();
    let t = e.session().tracks[0].id;
    let stored = AutoParam::Plugin { slot: 0, param: "7".into() };
    {
        let tr = e.session_mut().track_mut(t).unwrap();
        tr.mixer.inserts[0] = Some(Insert::new("clap:vendor.missing"));
        tr.lane_mut(&stored).set_point(0, 0.5);
    }
    let depth = e.revision;
    let err = e.execute("automation.set_point", &json!({"track": t.0, "param": "plugin:a:Gain", "at": 10, "value": 1.0})).unwrap_err().to_string();
    assert!(err.contains("unknown plugin clap:vendor.missing in slot A; use the parameter id"), "{err}");
    assert_eq!(plugin_lanes(&e, t), vec![(0, "7".to_string())]);
    let points = e.session().track(t).and_then(|tr| tr.lane(&stored)).map(|lane| lane.points.len());
    assert_eq!(points, Some(1));
    assert_eq!(e.revision, depth);

    // A name is not resolved without metadata, so it does not clear the id lane.
    let missed = e.execute("automation.clear", &json!({"track": t.0, "param": "plugin:a:Gain"})).unwrap();
    assert_eq!(missed["removed"], 0);
    assert_eq!(e.session().track(t).and_then(|tr| tr.lane(&stored)).map(|lane| lane.points.len()), Some(1));

    let cleared = e.execute("automation.clear", &json!({"track": t.0, "param": "plugin:a:7"})).unwrap();
    assert!(cleared["removed"].as_u64().unwrap_or(0) > 0, "{cleared}");
    let points = e.session().track(t).and_then(|tr| tr.lane(&stored)).map(|lane| lane.points.len());
    assert_eq!(points, Some(0));
    assert!(!plugin_lanes(&e, t).iter().any(|(_, param)| param == "gain"));
}

#[test]
fn clear_keeps_an_uppercase_id_when_the_plugin_has_no_metadata() {
    let mut e = Engine::default();
    e.execute("track.new", &json!({})).unwrap();
    let t = e.session().tracks[0].id;
    let stored = AutoParam::Plugin { slot: 0, param: "Gain".into() };
    {
        let tr = e.session_mut().track_mut(t).unwrap();
        tr.mixer.inserts[0] = Some(Insert::new("clap:vendor.missing"));
        tr.lane_mut(&stored).set_point(0, 0.25);
    }
    // The lowercased id is a different parameter. It clears nothing.
    let missed = e.execute("automation.clear", &json!({"track": t.0, "param": "plugin:a:gain"})).unwrap();
    assert_eq!(missed["removed"], 0);
    assert_eq!(e.session().track(t).and_then(|tr| tr.lane(&stored)).map(|lane| lane.points.len()), Some(1));

    // The prefix may be any case. The id text is the caller's.
    let cleared = e.execute("automation.clear", &json!({"track": t.0, "param": "PLUGIN:A:Gain"})).unwrap();
    assert!(cleared["removed"].as_u64().unwrap_or(0) > 0, "{cleared}");
    assert_eq!(e.session().track(t).and_then(|tr| tr.lane(&stored)).map(|lane| lane.points.len()), Some(0));

    let depth = e.revision;
    // Set and write still refuse a slot with no parameter list.
    let err = e.execute("automation.set_point", &json!({"track": t.0, "param": "plugin:a:Gain", "at": 0, "value": 1.0})).unwrap_err().to_string();
    assert!(err.contains("unknown plugin clap:vendor.missing in slot A; use the parameter id"), "{err}");
    let err = e
        .execute("automation.write_range", &json!({"track": t.0, "param": "plugin:a:Gain", "start": 0, "end": 10, "value": 1.0}))
        .unwrap_err()
        .to_string();
    assert!(err.contains("unknown plugin"), "{err}");
    assert_eq!(e.revision, depth);

    // The 200-character cap still applies to an exact clear.
    let key = "G".repeat(201);
    let err = e.execute("automation.clear", &json!({"track": t.0, "param": format!("plugin:a:{key}")})).unwrap_err().to_string();
    assert!(err.contains("at most 200"), "{err}");
    assert!(!err.contains(&key), "{err}");
}

#[test]
fn clear_removes_a_lane_left_behind_by_a_replaced_plugin() {
    let mut e = Engine::default();
    e.execute("track.new", &json!({})).unwrap();
    let t = e.session().tracks[0].id;
    e.execute("mix.insert", &json!({"track": t.0, "slot": 0, "plugin": "eq_7band"})).unwrap();
    let stored = AutoParam::Plugin { slot: 0, param: "LegacyParam".into() };
    e.session_mut().track_mut(t).unwrap().lane_mut(&stored).set_point(10, 0.5);
    let depth = e.revision;

    // A different case is not that stored id, and the new plugin does not know the name.
    let err = e.execute("automation.clear", &json!({"track": t.0, "param": "plugin:a:legacyparam"})).unwrap_err().to_string();
    assert!(err.contains("not a parameter of EQ 7-Band"), "{err}");
    assert_eq!(e.revision, depth);
    assert_eq!(e.session().track(t).and_then(|tr| tr.lane(&stored)).map(|lane| lane.points.len()), Some(1));

    let cleared = e.execute("automation.clear", &json!({"track": t.0, "param": "Plugin:a:LegacyParam"})).unwrap();
    assert!(cleared["removed"].as_u64().unwrap_or(0) > 0, "{cleared}");
    assert_eq!(e.session().track(t).and_then(|tr| tr.lane(&stored)).map(|lane| lane.points.len()), Some(0));

    // A key the current plugin does know still resolves, so clear removes that lane.
    e.execute("automation.set_point", &json!({"track": t.0, "param": "plugin:a:MF Gain", "at": 0, "value": -3.0})).unwrap();
    let cleared = e.execute("automation.clear", &json!({"track": t.0, "param": "plugin:a:mf gain"})).unwrap();
    assert!(cleared["removed"].as_u64().unwrap_or(0) > 0, "{cleared}");
    let mid = AutoParam::Plugin { slot: 0, param: "mid_gain".into() };
    assert_eq!(e.session().track(t).and_then(|tr| tr.lane(&mid)).map(|lane| lane.points.len()), Some(0));
    assert_eq!(e.session().track(t).and_then(|tr| tr.lane(&stored)).map(|lane| lane.points.len()), Some(0));
}
