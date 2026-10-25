use serde_json::{Value, json};
use soundcraft_engine::Engine;
use soundcraft_engine::edit::SHUFFLE_LOCK;
use soundcraft_model::{AutoParam, Clip, ClipId, EditMode, Source, SourceId, TrackId};

fn source(id: u64, frames: u64) -> Source {
    Source {
        id: SourceId(id),
        name: "src".into(),
        path: "src.wav".into(),
        channels: 1,
        frames,
        sample_rate: 48_000,
        format: soundcraft_audio_io::FileFormat::Wav,
        time_reference: 0,
        unsaved: false,
    }
}

fn new_track(e: &mut Engine) -> TrackId {
    TrackId(e.execute("track.new", &json!({"count": 1})).unwrap()["tracks"][0].as_u64().unwrap())
}

fn engine(mode: &str) -> (Engine, TrackId, [ClipId; 2]) {
    let mut e = Engine::default();
    let t = new_track(&mut e);
    let s = e.session_mut();
    s.sources.push(source(7, 10_000));
    let ids = [s.new_clip_id(), s.new_clip_id()];
    s.track_mut(t).and_then(|t| t.playlist_mut()).unwrap().clips =
        vec![Clip::audio(ids[0], "a", SourceId(7), 0, 0, 1000), Clip::audio(ids[1], "b", SourceId(7), 1000, 2000, 1000)];
    e.execute("edit.mode", &json!({"mode": mode})).unwrap();
    (e, t, ids)
}

/// A track of clips (start, length, offset) on a long file, source 8.
fn long(mode: &str, spans: &[(i64, i64, i64)]) -> (Engine, TrackId, Vec<ClipId>) {
    let mut e = Engine::default();
    let t = new_track(&mut e);
    let s = e.session_mut();
    s.sources.push(source(8, 1_000_000));
    let ids: Vec<ClipId> = spans.iter().map(|_| s.new_clip_id()).collect();
    s.track_mut(t).and_then(|t| t.playlist_mut()).unwrap().clips =
        spans.iter().zip(&ids).map(|(&(start, len, offset), id)| Clip::audio(*id, "c", SourceId(8), offset, start, len)).collect();
    e.execute("edit.mode", &json!({"mode": mode})).unwrap();
    (e, t, ids)
}

fn clips(e: &Engine, t: TrackId) -> Vec<(i64, i64)> {
    e.session().track(t).unwrap().clips().iter().map(|c| (c.start, c.length)).collect()
}

/// Each clip's id, length and file offset, in id order: what a move must never change.
fn material(e: &Engine, t: TrackId) -> Vec<(u64, i64, i64)> {
    let mut v: Vec<_> = e.session().track(t).unwrap().clips().iter().map(|c| (c.id.0, c.length, c.source_offset())).collect();
    v.sort_unstable();
    v
}

fn start(e: &Engine, id: ClipId) -> i64 {
    e.session().find_clip(id).unwrap().1.start
}

fn points(e: &Engine, t: TrackId) -> Vec<i64> {
    e.session().track(t).unwrap().lane(&AutoParam::Volume).map(|l| l.points.iter().map(|p| p.at).collect()).unwrap_or_default()
}

fn set_points(e: &mut Engine, t: TrackId, at: &[i64]) {
    for a in at {
        run(e, "automation.set_point", json!({"track": t.0, "param": "volume", "at": a, "value": -6.0}));
    }
}

fn run(e: &mut Engine, id: &str, p: Value) {
    e.execute(id, &p).unwrap();
}

#[test]
fn insert_silence_pushes_in_shuffle_and_clears_otherwise() {
    for (mode, want) in [("shuffle", vec![(0, 500), (700, 500), (2200, 1000)]), ("slip", vec![(0, 500), (700, 300), (2000, 1000)])] {
        let (mut e, t, _) = engine(mode);
        run(&mut e, "edit.insert_silence", json!({"tracks": [t.0], "start": 500, "end": 700}));
        assert_eq!(clips(&e, t), want, "{mode}");
    }
}

#[test]
fn duplicate_and_repeat_make_room_in_shuffle() {
    let (mut e, t, _) = engine("shuffle");
    run(&mut e, "edit.duplicate", json!({"tracks": [t.0], "start": 0, "end": 1000}));
    assert_eq!(clips(&e, t), vec![(0, 1000), (1000, 1000), (3000, 1000)]);
    let (mut e, t, _) = engine("shuffle");
    run(&mut e, "edit.repeat", json!({"tracks": [t.0], "start": 0, "end": 1000, "count": 2}));
    assert_eq!(clips(&e, t), vec![(0, 1000), (1000, 1000), (2000, 1000), (4000, 1000)]);
    let (mut e, t, _) = engine("slip");
    run(&mut e, "edit.duplicate", json!({"tracks": [t.0], "start": 0, "end": 1000}));
    assert_eq!(clips(&e, t), vec![(0, 1000), (1000, 1000), (2000, 1000)]);
}

#[test]
fn shuffle_paste_replaces_the_selection() {
    let (mut e, t, _) = engine("shuffle");
    run(&mut e, "edit.copy", json!({"tracks": [t.0], "start": 0, "end": 1000}));
    run(&mut e, "edit.select", json!({"tracks": [t.0], "start": 2000, "end": 2500}));
    run(&mut e, "edit.paste", json!({}));
    assert_eq!(clips(&e, t), vec![(0, 1000), (2000, 1000), (3000, 500)]);
}

#[test]
fn clip_list_drops_land_on_a_clip_boundary_in_shuffle() {
    let (mut e, t, [_, b]) = engine("shuffle");
    run(&mut e, "clip.place_source", json!({"source": 7, "track": t.0, "at": 1900}));
    assert_eq!(clips(&e, t), vec![(0, 1000), (2000, 10_000), (12_000, 1000)]);
    e.session_mut().find_clip_mut(b).unwrap().time_locked = true;
    assert!(e.execute("clip.place_source", &json!({"source": 7, "track": t.0, "at": 0})).is_err());
    assert_eq!(clips(&e, t).len(), 3);
}

#[test]
fn grid_moves_to_a_position_snap_or_keep_their_offset() {
    let (mut e, t, [_, b]) = engine("grid");
    let s = e.session();
    let step = s.edit.grid.step_samples(0, s.sample_rate, &s.tempo, s.frame_rate);
    run(&mut e, "edit.move_clips", json!({"clips": [b.0], "to": step + 1000}));
    assert_eq!(clips(&e, t)[1].0, step);
    run(&mut e, "edit.mode", json!({"mode": "relative_grid"}));
    run(&mut e, "edit.move_clips", json!({"clips": [b.0], "to": 2 * step + 2000}));
    assert_eq!(clips(&e, t)[1].0, 2 * step);
    run(&mut e, "edit.move_clips", json!({"clips": [b.0], "by": 200}));
    run(&mut e, "edit.move_clips", json!({"clips": [b.0], "to": 3 * step}));
    assert_eq!(clips(&e, t)[1].0, 3 * step + 200);
}

#[test]
fn clips_cannot_move_onto_tracks_without_a_playlist() {
    let (mut e, t, [a, _]) = engine("slip");
    let aux = e.execute("track.new", &json!({"kind": "aux"})).unwrap()["tracks"][0].clone();
    assert!(e.execute("edit.move_clips", &json!({"clips": [a.0], "by": 0, "track": aux})).is_err());
    assert_eq!(clips(&e, t).len(), 2);
}

#[test]
fn trim_clip_needs_an_edge_and_an_unlocked_clip() {
    let (mut e, t, [a, b]) = engine("slip");
    assert!(e.execute("edit.trim_clip", &json!({"clip": a.0})).is_err());
    e.session_mut().find_clip_mut(b).unwrap().edit_locked = true;
    assert!(e.execute("edit.trim_clip", &json!({"clip": b.0, "end": 2500})).is_err());
    run(&mut e, "edit.trim_clip", json!({"clip": a.0, "end": 5000}));
    assert_eq!(clips(&e, t), vec![(0, 2000), (2000, 1000)]);
}

#[test]
fn shuffle_lock_keeps_shuffle_mode_out() {
    let (mut e, _, _) = engine("slip");
    run(&mut e, "edit.shuffle_lock", json!({}));
    assert!(e.execute("edit.mode", &json!({"mode": "shuffle"})).is_err());
    run(&mut e, "edit.mode", json!({"mode": "grid"}));
    run(&mut e, "edit.shuffle_lock", json!({"value": false}));
    run(&mut e, "edit.mode", json!({"mode": "shuffle"}));
    assert!(e.execute("edit.shuffle_lock", &json!({})).is_err());
}

#[test]
fn snap_to_grid_rides_along_with_the_other_modes() {
    let (mut e, _, _) = engine("slip");
    let snap = |e: &Engine| e.session().edit.flag(soundcraft_engine::edit::SNAP_TO_GRID);
    run(&mut e, "edit.snap_to_grid", json!({}));
    assert!(snap(&e));
    run(&mut e, "edit.mode", json!({"mode": "spot", "snap_to_grid": true}));
    assert!(snap(&e));
    run(&mut e, "edit.mode", json!({"mode": "shuffle"}));
    assert!(!snap(&e));
    run(&mut e, "edit.mode", json!({"mode": "grid", "snap_to_grid": true}));
    assert!(!snap(&e));
    assert!(e.execute("edit.snap_to_grid", &json!({})).is_err());
}

#[test]
fn shuffle_edits_that_would_push_a_locked_clip_are_refused() {
    let (mut e, t, [_, b]) = engine("shuffle");
    e.session_mut().find_clip_mut(b).unwrap().time_locked = true;
    for id in ["edit.insert_silence", "edit.duplicate"] {
        let err = e.execute(id, &json!({"tracks": [t.0], "start": 0, "end": 1500})).unwrap_err().to_string();
        assert!(err.contains("locked clip"), "{id}: {err}");
    }
    run(&mut e, "edit.insert_silence", json!({"tracks": [t.0], "start": 0, "end": 1000}));
    assert_eq!(clips(&e, t), vec![(1000, 1000), (2000, 1000)]);
    run(&mut e, "event.cut_time", json!({"start": 0, "end": 1000}));
    assert_eq!(clips(&e, t), vec![(0, 1000), (1000, 1000)]);
}

#[test]
fn clips_dragged_from_several_tracks_keep_their_tracks_apart() {
    let (mut e, t, [a, _]) = engine("slip");
    let new = e.execute("track.new", &json!({"count": 2})).unwrap()["tracks"].clone();
    let [t2, t3] = [0, 1].map(|i| TrackId(new[i].as_u64().unwrap()));
    let s = e.session_mut();
    let d = s.new_clip_id();
    s.track_mut(t2).and_then(|t| t.playlist_mut()).unwrap().clips.push(Clip::audio(d, "d", SourceId(7), 0, 5000, 500));
    run(&mut e, "edit.move_clips", json!({"clips": [a.0, d.0], "by": 0, "track": t2.0}));
    assert_eq!(clips(&e, t), vec![(2000, 1000)]);
    assert_eq!(clips(&e, t2), vec![(0, 1000)]);
    assert_eq!(clips(&e, t3), vec![(5000, 500)]);
    assert!(e.execute("edit.move_clips", &json!({"clips": [a.0, d.0], "by": 0, "track": t3.0})).is_err());
}

#[test]
fn relative_grid_pastes_keep_the_offset_from_the_grid() {
    let (mut e, t, [_, b]) = engine("relative_grid");
    let s = e.session();
    let step = s.edit.grid.step_samples(0, s.sample_rate, &s.tempo, s.frame_rate);
    run(&mut e, "edit.select", json!({"clips": [b.0]}));
    run(&mut e, "edit.copy", json!({}));
    run(&mut e, "edit.select", json!({"tracks": [t.0], "start": 3 * step}));
    run(&mut e, "edit.paste", json!({}));
    assert_eq!(clips(&e, t).last(), Some(&(3 * step + 2000, 1000)));
    run(&mut e, "edit.mode", json!({"mode": "slip"}));
    run(&mut e, "edit.select", json!({"tracks": [t.0], "start": 5 * step}));
    run(&mut e, "edit.paste", json!({}));
    assert_eq!(clips(&e, t).last(), Some(&(5 * step, 1000)));
}

#[test]
fn shuffle_moves_keep_crossfade_partners_whole() {
    let (mut e, t, ids) = long("slip", &[(0, 1000, 10_000), (1000, 1000, 20_000), (2000, 1000, 30_000)]);
    run(&mut e, "edit.fades_create", json!({"tracks": [t.0], "start": 900, "end": 1100}));
    run(&mut e, "edit.mode", json!({"mode": "shuffle"}));
    let whole = material(&e, t);
    let [a, b, c] = [ids[0], ids[1], ids[2]];
    run(&mut e, "edit.move_clips", json!({"clips": [a.0], "by": 5000}));
    assert_eq!(material(&e, t), whole);
    assert_eq!([start(&e, b), start(&e, c), start(&e, a)], [0, 1100, 2100]);
    let (mut e, t, ids) = long("slip", &[(0, 1000, 10_000), (1000, 1000, 20_000), (2000, 1000, 30_000)]);
    run(&mut e, "edit.fades_create", json!({"tracks": [t.0], "start": 900, "end": 1100}));
    run(&mut e, "edit.mode", json!({"mode": "shuffle"}));
    for by in [2500, -2500] {
        run(&mut e, "edit.move_clips", json!({"clips": [ids[0].0, ids[1].0], "by": by}));
        assert_eq!(material(&e, t), whole, "{by}");
        assert_eq!(start(&e, ids[1]) - start(&e, ids[0]), 900, "{by}");
    }
    assert_eq!([start(&e, ids[0]), start(&e, ids[2])], [0, 2000]);
    run(&mut e, "edit.mode", json!({"mode": "slip"}));
    run(&mut e, "edit.move_clips", json!({"clips": [ids[0].0, ids[1].0], "by": 5000}));
    assert_eq!(material(&e, t), whole);
}

#[test]
fn pasting_copied_automation_in_shuffle_leaves_the_clips_alone() {
    let (mut e, t, ids) = long("slip", &[(0, 1000, 0), (1000, 1000, 0), (2000, 1000, 0)]);
    set_points(&mut e, t, &[200]);
    run(&mut e, "edit.copy_all_automation", json!({"tracks": [t.0], "start": 0, "end": 500}));
    run(&mut e, "edit.mode", json!({"mode": "shuffle"}));
    run(&mut e, "edit.select", json!({"clips": [ids[1].0]}));
    run(&mut e, "edit.paste", json!({}));
    assert_eq!(clips(&e, t), vec![(0, 1000), (1000, 1000), (2000, 1000)]);
}

#[test]
fn automation_follows_shuffle_moves_trims_and_drops() {
    let setup = || {
        let (mut e, t, ids) = long("shuffle", &[(0, 1000, 0), (1000, 1000, 0), (2000, 1000, 0)]);
        set_points(&mut e, t, &[500, 1500, 2500]);
        (e, t, ids)
    };
    let (mut e, t, ids) = setup();
    run(&mut e, "edit.move_clips", json!({"clips": [ids[0].0], "by": 3000}));
    assert_eq!(points(&e, t), [500, 1500, 2500]);
    assert_eq!([start(&e, ids[1]), start(&e, ids[2]), start(&e, ids[0])], [0, 1000, 2000]);
    let (mut e, t, ids) = setup();
    run(&mut e, "edit.trim_clip", json!({"clip": ids[0].0, "end": 500}));
    assert_eq!(points(&e, t), [1000, 2000]);
    let (mut e, t, _) = setup();
    run(&mut e, "clip.place_source", json!({"source": 8, "track": t.0, "at": 1000}));
    assert_eq!(points(&e, t), [500, 1_001_500, 1_002_500]);
}

#[test]
fn shuffle_automation_stops_at_locked_clips_and_needs_automation_follows_edit() {
    let (mut e, t, ids) = long("shuffle", &[(0, 1000, 0), (1000, 1000, 0), (3000, 1000, 0)]);
    e.session_mut().find_clip_mut(ids[2]).unwrap().time_locked = true;
    set_points(&mut e, t, &[3500]);
    run(&mut e, "edit.insert_silence", json!({"tracks": [t.0], "start": 0, "end": 500}));
    run(&mut e, "edit.cut", json!({"tracks": [t.0], "start": 0, "end": 500}));
    assert_eq!((start(&e, ids[2]), points(&e, t)), (3000, vec![3500]));
    let (mut e, t, _) = long("shuffle", &[(0, 1000, 0), (1000, 1000, 0)]);
    set_points(&mut e, t, &[300, 1500, 5000]);
    e.session_mut().edit.automation_follows_edit = false;
    run(&mut e, "edit.duplicate", json!({"tracks": [t.0], "start": 0, "end": 1000}));
    run(&mut e, "edit.insert_silence", json!({"tracks": [t.0], "start": 0, "end": 100}));
    assert_eq!(points(&e, t), [300, 1500, 5000]);
}

#[test]
fn shuffle_clear_moves_automation_with_the_clips() {
    let (mut e, t, _) = long("shuffle", &[(0, 10_000, 0), (20_000, 10_000, 0)]);
    set_points(&mut e, t, &[0, 9_000, 20_000, 29_000]);
    run(&mut e, "edit.clear", json!({"track": t.0, "start": 4_000, "end": 6_000}));
    assert_eq!(clips(&e, t), [(0, 4_000), (4_000, 4_000), (18_000, 10_000)]);
    assert_eq!(points(&e, t), [0, 7_000, 18_000, 27_000]);
}

#[test]
fn relative_grid_pastes_land_on_grid_lines_at_any_tempo() {
    let (mut e, t, [a, _]) = engine("relative_grid");
    run(&mut e, "event.tempo", json!({"bpm": 90, "at": 0}));
    run(&mut e, "edit.select", json!({"clips": [a.0]}));
    run(&mut e, "edit.copy", json!({}));
    run(&mut e, "edit.select", json!({"tracks": [t.0], "start": 128_000}));
    run(&mut e, "edit.paste", json!({}));
    assert!(clips(&e, t).contains(&(128_000, 1000)), "{:?}", clips(&e, t));
    run(&mut e, "edit.paste", json!({"tracks": [t.0], "at": 50_000}));
    assert!(clips(&e, t).contains(&(50_000, 1000)), "{:?}", clips(&e, t));
}

#[test]
fn start_trims_keep_the_sync_point_on_its_audio() {
    let (mut e, t, ids) = long("slip", &[(0, 10_000, 0)]);
    let sync = |e: &Engine| e.session().find_clip(ids[0]).unwrap().1.sync_point;
    e.session_mut().find_clip_mut(ids[0]).unwrap().sync_point = 5000;
    run(&mut e, "edit.trim_clip", json!({"clip": ids[0].0, "start": 2000}));
    assert_eq!(sync(&e), 3000);
    run(&mut e, "edit.trim_clip", json!({"clip": ids[0].0, "start": 8000}));
    assert_eq!(sync(&e), 0);
    run(&mut e, "edit.mode", json!({"mode": "grid"}));
    run(&mut e, "edit.move_clips", json!({"clips": [ids[0].0], "to": 30_000}));
    assert_eq!(clips(&e, t), vec![(30_000, 2000)]);
}

#[test]
fn imported_time_stamps_count_at_the_session_rate() {
    use soundcraft_audio_io::{AudioBuffer, BwfInfo, EncodeOptions, FileFormat};
    let mut e = Engine::default();
    let bwf = BwfInfo { time_reference: 1_587_600_000, ..BwfInfo::default() };
    let opts = EncodeOptions { format: FileFormat::Wav, bwf: Some(bwf), ..EncodeOptions::default() };
    let bytes = soundcraft_audio_io::encode(&AudioBuffer::new(44_100, 1, 441), &opts).unwrap();
    soundcraft_engine::io::import_audio_bytes(&mut e, "stamped.wav", &bytes, None, None, 0).unwrap();
    assert_eq!(e.session().sources[0].time_reference, 1_728_000_000);
}

#[test]
fn shuffle_trims_stop_at_a_locked_crossfade_and_stay_inside_the_file() {
    let (mut e, t, ids) = long("slip", &[(0, 1000, 500), (1000, 1000, 5000)]);
    run(&mut e, "edit.fades_create", json!({"tracks": [t.0], "start": 950, "end": 1050}));
    e.session_mut().find_clip_mut(ids[1]).unwrap().time_locked = true;
    run(&mut e, "edit.mode", json!({"mode": "shuffle"}));
    let before = material(&e, t);
    run(&mut e, "edit.trim_clip", json!({"clip": ids[0].0, "end": 1500}));
    run(&mut e, "edit.trim_clip", json!({"clip": ids[0].0, "start": 0}));
    assert_eq!(material(&e, t), before);
    let (mut e, _, [_, b]) = engine("shuffle");
    run(&mut e, "edit.trim_clip", json!({"clip": b.0, "start": 2500, "end": 20_000}));
    let c = e.session().find_clip(b).unwrap().1;
    assert_eq!((c.source_offset(), c.source_offset() + c.length), (1500, 10_000));
}

#[test]
fn duplicate_and_extend_makes_room_in_shuffle() {
    let (mut e, t, _) = long("shuffle", &[(0, 1000, 0), (1000, 1000, 0)]);
    run(&mut e, "edit.duplicate_extend", json!({"tracks": [t.0], "start": 0, "end": 1000}));
    assert_eq!(clips(&e, t), vec![(0, 1000), (1000, 1000), (2000, 1000)]);
    let (mut e, t, ids) = long("shuffle", &[(0, 1000, 0), (1000, 1000, 0)]);
    e.session_mut().find_clip_mut(ids[1]).unwrap().time_locked = true;
    assert!(e.execute("edit.duplicate_extend", &json!({"tracks": [t.0], "start": 0, "end": 1000})).is_err());
    assert_eq!(clips(&e, t), vec![(0, 1000), (1000, 1000)]);
}

#[test]
fn undo_leaves_the_edit_mode_and_shuffle_lock_alone() {
    let (mut e, t, _) = engine("shuffle");
    run(&mut e, "edit.cut", json!({"tracks": [t.0], "start": 0, "end": 500}));
    run(&mut e, "edit.mode", json!({"mode": "slip"}));
    run(&mut e, "edit.shuffle_lock", json!({}));
    for step in [Engine::undo, Engine::redo] {
        assert!(step(&mut e));
        assert_eq!((e.session().edit.edit_mode, e.session().edit.flag(SHUFFLE_LOCK)), (EditMode::Slip, true));
    }
}

#[test]
fn shuffle_cuts_reaching_before_the_session_start_keep_the_spacing() {
    let (mut e, t, _) = long("shuffle", &[(0, 1000, 0), (1000, 1000, 0), (2000, 1000, 0)]);
    run(&mut e, "edit.cut", json!({"tracks": [t.0], "start": -5000, "end": 500}));
    assert_eq!(clips(&e, t), vec![(0, 500), (500, 1000), (1500, 1000)]);
}

#[test]
fn shuffle_never_pulls_a_crossfade_apart() {
    let (mut e, t, ids) = long("slip", &[(0, 9000, 0), (9000, 11_000, 100_000), (20_000, 5000, 200_000)]);
    run(&mut e, "edit.fades_create", json!({"tracks": [t.0], "start": 8000, "end": 10_000}));
    run(&mut e, "edit.mode", json!({"mode": "shuffle"}));
    let err = e.execute("edit.duplicate", &json!({"tracks": [t.0], "start": 5000, "end": 9000})).unwrap_err().to_string();
    assert!(err.contains("crossfade"), "{err}");
    run(&mut e, "clip.place_source", json!({"source": 8, "track": t.0, "at": 7900}));
    assert_eq!(clips(&e, t)[0], (0, 1_000_000));
    assert_eq!([start(&e, ids[0]), start(&e, ids[1])], [1_000_000, 1_008_000]);
}

#[test]
fn clips_move_to_and_from_hidden_tracks() {
    let (mut e, t, [a, _]) = engine("slip");
    let hidden = new_track(&mut e);
    run(&mut e, "track.hide", json!({"tracks": [hidden.0], "hidden": true}));
    run(&mut e, "edit.move_clips", json!({"clips": [a.0], "by": 0, "track": hidden.0}));
    assert_eq!(clips(&e, hidden), vec![(0, 1000)]);
    run(&mut e, "edit.move_clips", json!({"clips": [a.0], "by": 0, "track": t.0}));
    assert_eq!(clips(&e, t).len(), 2);
}

#[test]
fn shuffle_lock_and_snap_rules_hold_when_a_call_names_tracks() {
    let (mut e, t, _) = engine("shuffle");
    assert!(e.execute("edit.shuffle_lock", &json!({"value": true, "tracks": [t.0]})).is_err());
    run(&mut e, "edit.mode", json!({"mode": "grid"}));
    assert!(e.execute("edit.snap_to_grid", &json!({"tracks": [t.0]})).is_err());
}
