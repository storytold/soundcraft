use serde_json::{Value, json};
use soundcraft_engine::Engine;
use soundcraft_engine::edit::SHUFFLE_LOCK;
use soundcraft_model::{AutoParam, Clip, ClipId, EditMode, Source, SourceId, TrackId, crossfade_cut};

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

/// A crossfade is two clips that overlap for exactly as long as the earlier one fades out and the
/// later one fades in. Every overlap on the track, as (the earlier clip's fade-out length, overlap
/// start, overlap end, the later clip's fade-in length): a sound crossfade has both fades as long as
/// its overlap.
fn crossfades(e: &Engine, t: TrackId) -> Vec<(i64, i64, i64, i64)> {
    let c = e.session().track(t).unwrap().clips();
    c.windows(2).filter(|w| w[0].end() > w[1].start).map(|w| (w[0].fade_out.len, w[1].start, w[0].end(), w[1].fade_in.len)).collect()
}

/// The cut of every crossfade on the track, as an absolute position.
fn cuts(e: &Engine, t: TrackId) -> Vec<i64> {
    let c = e.session().track(t).unwrap().clips();
    c.windows(2).filter(|w| w[0].end() > w[1].start).map(|w| crossfade_cut(&w[0], &w[1])).collect()
}

/// What each clip plays at each point of the timeline: the file offset minus the start, per clip.
fn slide(e: &Engine, t: TrackId) -> Vec<i64> {
    e.session().track(t).unwrap().clips().iter().map(|c| c.source_offset() - c.start).collect()
}

/// Two clips joined at 1000 on a long file, crossfaded by F over 800..1050: off-centre around the
/// joint (the middle of the overlap would be 925).
fn off_centre() -> (Engine, TrackId, Vec<ClipId>) {
    let (mut e, t, ids) = long("slip", &[(0, 1000, 10_000), (1000, 1000, 20_000)]);
    run(&mut e, "edit.fades_create", json!({"tracks": [t.0], "start": 800, "end": 1050}));
    (e, t, ids)
}

#[test]
fn fades_created_over_a_crossfade_replace_it() {
    let (mut e, t, _) = long("slip", &[(0, 1000, 10_000), (1000, 1000, 20_000)]);
    let placed = slide(&e, t);
    run(&mut e, "edit.fades_create", json!({"tracks": [t.0], "start": 900, "end": 1100}));
    assert_eq!(crossfades(&e, t), [(200, 900, 1100, 200)]);
    assert_eq!(cuts(&e, t), [1000], "the cut is where the clips were joined");
    run(&mut e, "edit.fades_create", json!({"tracks": [t.0], "start": 500, "end": 1500}));
    // The overlap is the selection and both fades span all of it...
    assert_eq!(crossfades(&e, t), [(1000, 500, 1500, 1000)]);
    // ...with no hole (same stretch of timeline) and no audio moved (each clip plays what it played)...
    assert_eq!(clips(&e, t), vec![(0, 1500), (500, 1500)]);
    assert_eq!(slide(&e, t), placed);
    // ...around the same cut.
    assert_eq!(cuts(&e, t), [1000]);
}

#[test]
fn fades_store_the_cut_where_the_clips_were_joined() {
    let (e, t, ids) = off_centre();
    let made = e.session().track(t).unwrap().clips().to_vec();
    assert_eq!(crossfades(&e, t), [(250, 800, 1050, 250)]);
    assert_eq!((made[0].cut, made[1].cut), (None, Some(200)), "stored on the later clip, as an offset from its start");
    assert_eq!(cuts(&e, t), [1000], "not the middle of the overlap (925)");
    // Agents see it when they inspect the session.
    let seen = |id| soundcraft_engine::inspect::clip(&e, id).unwrap().get("cut").cloned();
    assert_eq!((seen(ids[0]), seen(ids[1])), (None, Some(json!(200))));
    // No audio past either edge: the clips stay butted and nothing is stored.
    let (mut e, t, _) = long("slip", &[(0, 1000, 999_000), (1000, 1000, 0)]);
    run(&mut e, "edit.fades_create", json!({"tracks": [t.0], "start": 900, "end": 1100}));
    assert_eq!(e.session().track(t).unwrap().clips().iter().map(|c| c.cut).collect::<Vec<_>>(), [None, None]);
    // The cut is edit metadata: the sound of the pair does not depend on it.
    let (mut e, _, ids) = off_centre();
    let sound = |e: &Engine| {
        let at = |id: ClipId, x: i64| {
            let c = e.session().find_clip(id).unwrap().1.clone();
            c.gain_at(x - c.start)
        };
        (700..1200).map(|x| (at(ids[0], x), at(ids[1], x))).collect::<Vec<_>>()
    };
    let before = sound(&e);
    e.session_mut().find_clip_mut(ids[1]).unwrap().cut = None;
    assert_eq!(sound(&e), before);
}

#[test]
fn a_longer_or_shorter_range_replaces_the_crossfade_around_the_same_cut() {
    // The middle of the overlap is 925, the cut 1000: the new crossfade is the range, whichever side
    // it grows or shrinks on, around the cut and not the middle, with no audio moved. One undo
    // brings the old crossfade back.
    for (start, end, want) in [
        (700, 1500, (800, 700, 1500, 800)),
        (990, 1030, (40, 990, 1030, 40)),
        (999, 1001, (2, 999, 1001, 2)),
        (950, 1100, (150, 950, 1100, 150)),
        (800, 1100, (300, 800, 1100, 300)),
        (900, 1050, (150, 900, 1050, 150)),
        (800, 1050, (250, 800, 1050, 250)),
    ] {
        let (mut e, t, _) = off_centre();
        let (made, placed) = (e.session().track(t).unwrap().clips().to_vec(), slide(&e, t));
        run(&mut e, "edit.fades_create", json!({"tracks": [t.0], "start": start, "end": end}));
        assert_eq!(crossfades(&e, t), [want], "{start}..{end}");
        assert_eq!(cuts(&e, t), [1000], "{start}..{end}");
        assert_eq!(e.session().track(t).unwrap().clips()[1].cut, Some(1000 - start), "{start}..{end}");
        assert_eq!(slide(&e, t), placed, "{start}..{end}");
        assert!(e.undo());
        assert_eq!(e.session().track(t).unwrap().clips(), made, "{start}..{end} undone");
    }
}

#[test]
fn a_crossfade_short_of_audio_is_clamped_around_its_cut() {
    // A has no audio past its end: the range asks for 200 more of it after the cut, and gets none.
    let (mut e, t, _) = long("slip", &[(0, 1000, 999_000), (1000, 1000, 20_000)]);
    run(&mut e, "edit.fades_create", json!({"tracks": [t.0], "start": 900, "end": 1100}));
    assert_eq!((crossfades(&e, t), cuts(&e, t)), (vec![(100, 900, 1000, 100)], vec![1000]));
    run(&mut e, "edit.fades_create", json!({"tracks": [t.0], "start": 800, "end": 1300}));
    assert_eq!((crossfades(&e, t), cuts(&e, t)), (vec![(200, 800, 1000, 200)], vec![1000]));
    assert_eq!(clips(&e, t), vec![(0, 1000), (800, 1200)]);
}

#[test]
fn a_crossfade_without_a_stored_cut_is_cut_in_the_middle() {
    // A session saved before the cut was stored: the middle of the overlap (1000) is the cut.
    let (mut e, t, ids) = long("slip", &[(0, 1100, 10_000), (900, 1100, 19_100)]);
    e.session_mut().find_clip_mut(ids[0]).unwrap().fade_out.len = 200;
    e.session_mut().find_clip_mut(ids[1]).unwrap().fade_in.len = 200;
    assert_eq!((cuts(&e, t), e.session().track(t).unwrap().clips()[1].cut), (vec![1000], None));
    run(&mut e, "edit.fades_create", json!({"tracks": [t.0], "start": 950, "end": 1500}));
    assert_eq!((crossfades(&e, t), cuts(&e, t)), (vec![(550, 950, 1500, 550)], vec![1000]));
    assert_eq!(e.session().track(t).unwrap().clips()[1].cut, Some(50), "now stored");
    // Deleting the fades of an old crossfade cuts it back in the middle.
    let (mut e, t, ids) = long("slip", &[(0, 1100, 10_000), (900, 1100, 19_100)]);
    run(&mut e, "edit.fades_delete", json!({"tracks": [t.0], "start": 900, "end": 1100}));
    assert_eq!(clips(&e, t), vec![(0, 1000), (1000, 1000)]);
    assert_eq!(e.session().find_clip(ids[1]).unwrap().1.cut, None);
}

#[test]
fn deleting_the_fades_cuts_a_crossfade_back_to_its_stored_cut() {
    let (mut e, t, ids) = off_centre();
    let placed = slide(&e, t);
    run(&mut e, "edit.fades_delete", json!({"tracks": [t.0], "start": 800, "end": 1050}));
    // Joined at 1000 again (not 925), the audio each gave up is its handle, nothing moved, nothing stored.
    assert_eq!((clips(&e, t), slide(&e, t)), (vec![(0, 1000), (1000, 1000)], placed));
    let made: Vec<_> = ids.iter().map(|id| e.session().find_clip(*id).unwrap().1).map(|c| (c.cut, c.fade_in.len, c.fade_out.len)).collect();
    assert_eq!(made, [(None, 0, 0), (None, 0, 0)]);
    assert!(e.undo());
    assert_eq!((crossfades(&e, t), cuts(&e, t)), (vec![(250, 800, 1050, 250)], vec![1000]));
}

/// Each clip's (fade-in, fade-out) length.
fn fade_lens(e: &Engine, t: TrackId) -> Vec<(i64, i64)> {
    e.session().track(t).unwrap().clips().iter().map(|c| (c.fade_in.len, c.fade_out.len)).collect()
}

fn deleted(e: &mut Engine, t: TrackId, start: i64, end: i64) -> u64 {
    e.execute("edit.fades_delete", &json!({"tracks": [t.0], "start": start, "end": end})).unwrap()["deleted"].as_u64().unwrap()
}

#[test]
fn deleting_the_fades_far_from_a_crossfade_leaves_it_whole() {
    // Crossfade 900..1100 around the cut at 1000, and a clip far away with a fade-in.
    let (mut e, t, _) = long("slip", &[(0, 1000, 10_000), (1000, 1000, 20_000), (5000, 1000, 30_000)]);
    run(&mut e, "edit.fades_create", json!({"tracks": [t.0], "start": 900, "end": 1100}));
    run(&mut e, "edit.fades_create", json!({"tracks": [t.0], "start": 5000, "end": 5200}));
    assert_eq!(fade_lens(&e, t), [(0, 200), (200, 0), (200, 0)]);
    let made = e.session().track(t).unwrap().clips().to_vec();
    // Over a gap nothing happens; over the far clip only its own fade goes.
    assert_eq!(deleted(&mut e, t, 3000, 4000), 0);
    assert_eq!(e.session().track(t).unwrap().clips(), made);
    assert_eq!(deleted(&mut e, t, 5000, 6000), 1);
    assert_eq!(e.session().track(t).unwrap().clips()[..2], made[..2], "the crossfade is as it was");
    assert_eq!((fade_lens(&e, t), crossfades(&e, t), cuts(&e, t)), (vec![(0, 200), (200, 0), (0, 0)], vec![(200, 900, 1100, 200)], vec![1000]));
    assert!(e.undo());
    assert_eq!(e.session().track(t).unwrap().clips(), made);
}

#[test]
fn deleting_the_fades_over_one_clip_of_a_crossfade_rejoins_the_pair_with_no_orphan_fade() {
    // Over the earlier clip only, over the later one only: the pair meets at the cut with no fade on
    // either, the clip the range did not touch included.
    for (start, end) in [(0, 500), (1500, 1800)] {
        let (mut e, t, ids) = long("slip", &[(0, 1000, 10_000), (1000, 1000, 20_000)]);
        let placed = slide(&e, t);
        run(&mut e, "edit.fades_create", json!({"tracks": [t.0], "start": 900, "end": 1100}));
        assert_eq!(deleted(&mut e, t, start, end), 2, "{start}..{end}");
        assert_eq!((clips(&e, t), slide(&e, t), fade_lens(&e, t)), (vec![(0, 1000), (1000, 1000)], placed, vec![(0, 0), (0, 0)]), "{start}..{end}");
        assert!(ids.iter().all(|id| e.session().find_clip(*id).unwrap().1.cut.is_none()), "{start}..{end}");
        assert!(e.undo());
        assert_eq!((crossfades(&e, t), cuts(&e, t)), (vec![(200, 900, 1100, 200)], vec![1000]), "{start}..{end} undone");
    }
    // A second crossfade the range does not touch stays whole.
    let (mut e, t, _) = long("slip", &[(0, 1000, 10_000), (1000, 1000, 20_000), (2000, 1000, 30_000)]);
    run(&mut e, "edit.fades_create", json!({"tracks": [t.0], "start": 900, "end": 1100}));
    run(&mut e, "edit.fades_create", json!({"tracks": [t.0], "start": 1900, "end": 2100}));
    assert_eq!(deleted(&mut e, t, 0, 500), 2);
    assert_eq!(clips(&e, t), vec![(0, 1000), (1000, 1100), (1900, 1100)]);
    assert_eq!((fade_lens(&e, t), crossfades(&e, t), cuts(&e, t)), (vec![(0, 0), (0, 200), (200, 0)], vec![(200, 1900, 2100, 200)], vec![2000]));
}

#[test]
fn a_crossfade_built_over_a_whole_clip_stays_a_crossfade() {
    // A is 1000..2000, B 2000..3000, both with plenty of audio around them. Select All reaches the
    // start of A: B may go back under A, but not as far as A's start (then they would no longer be a
    // crossfade, and every later command would misread the pair).
    let (mut e, t, _) = long("slip", &[(1000, 1000, 10_000), (2000, 1000, 20_000)]);
    run(&mut e, "edit.fades_create", json!({"tracks": [t.0], "start": 0, "end": 2100}));
    assert_eq!((clips(&e, t), crossfades(&e, t), cuts(&e, t)), (vec![(1000, 1100), (1001, 1999)], vec![(1099, 1001, 2100, 1099)], vec![2000]));
    // F replaces it around the same cut, and delete cuts it back there.
    run(&mut e, "edit.fades_create", json!({"tracks": [t.0], "start": 1500, "end": 2500}));
    assert_eq!((crossfades(&e, t), cuts(&e, t)), (vec![(1000, 1500, 2500, 1000)], vec![2000]));
    run(&mut e, "edit.fades_delete", json!({"tracks": [t.0], "start": 1000, "end": 3000}));
    assert_eq!((clips(&e, t), fade_lens(&e, t)), (vec![(1000, 1000), (2000, 1000)], vec![(0, 0), (0, 0)]));
    // Not even as far as the end of the fade A has coming in.
    let (mut e, t, ids) = long("slip", &[(1000, 1000, 10_000), (2000, 1000, 20_000)]);
    e.session_mut().find_clip_mut(ids[0]).unwrap().fade_in.len = 300;
    run(&mut e, "edit.fades_create", json!({"tracks": [t.0], "start": 0, "end": 2100}));
    assert_eq!((clips(&e, t), crossfades(&e, t), cuts(&e, t)), (vec![(1000, 1100), (1301, 1699)], vec![(799, 1301, 2100, 799)], vec![2000]));
    assert_eq!(fade_lens(&e, t)[0].0, 300, "the fade-in of A is as it was");
    // The other side: A may not reach the end of B, nor the start of the fade B has going out.
    let (mut e, t, _) = long("slip", &[(0, 1000, 10_000), (1000, 1000, 20_000)]);
    run(&mut e, "edit.fades_create", json!({"tracks": [t.0], "start": 900, "end": 3000}));
    assert_eq!((clips(&e, t), crossfades(&e, t), cuts(&e, t)), (vec![(0, 1999), (900, 1100)], vec![(1099, 900, 1999, 1099)], vec![1000]));
    run(&mut e, "edit.fades_delete", json!({"tracks": [t.0], "start": 0, "end": 3000}));
    assert_eq!((clips(&e, t), fade_lens(&e, t)), (vec![(0, 1000), (1000, 1000)], vec![(0, 0), (0, 0)]));
    let (mut e, t, ids) = long("slip", &[(0, 1000, 10_000), (1000, 1000, 20_000)]);
    e.session_mut().find_clip_mut(ids[1]).unwrap().fade_out.len = 300;
    run(&mut e, "edit.fades_create", json!({"tracks": [t.0], "start": 900, "end": 3000}));
    assert_eq!((clips(&e, t), crossfades(&e, t), cuts(&e, t)), (vec![(0, 1699), (900, 1100)], vec![(799, 900, 1699, 799)], vec![1000]));
    assert_eq!(fade_lens(&e, t)[1].1, 300, "the fade-out of B is as it was");
}

#[test]
fn a_joint_next_to_a_crossfade_is_faded_without_reaching_into_it() {
    // A 0..1100 and B 900..2000 are crossfaded around 1000, C is joined to B at 2000. A range from
    // 1050 holds the joint of B and C and could take C back to 1050, under A: C stops short of the
    // crossfade (and of the fade B has coming in).
    let (mut e, t, _) = long("slip", &[(0, 1000, 10_000), (1000, 1000, 20_000), (2000, 1000, 30_000)]);
    run(&mut e, "edit.fades_create", json!({"tracks": [t.0], "start": 900, "end": 1100}));
    run(&mut e, "edit.fades_create", json!({"tracks": [t.0], "start": 1050, "end": 2500}));
    assert_eq!(clips(&e, t), vec![(0, 1100), (900, 1600), (1101, 1899)]);
    assert_eq!(crossfades(&e, t), [(200, 900, 1100, 200), (1399, 1101, 2500, 1399)]);
    assert_eq!(cuts(&e, t), [1000, 2000]);
    // Clips that overlap with no fade (layered) bound the zone too: Z 0..800 lies over the start of A,
    // so the joint of A and B (1500) cannot be crossfaded back past the end of Z...
    let (mut e, t, _) = long("slip", &[(0, 800, 10_000), (500, 1000, 20_000), (1500, 1000, 30_000)]);
    run(&mut e, "edit.fades_create", json!({"tracks": [t.0], "start": 600, "end": 1600}));
    assert_eq!(clips(&e, t), vec![(0, 800), (500, 1100), (800, 1700)]);
    assert_eq!(crossfades(&e, t)[1], (800, 800, 1600, 800));
    // ...and R 1500..4000 lies over the end of Q, so the joint of P and Q (1000) cannot be crossfaded
    // on past the start of R.
    let (mut e, t, _) = long("slip", &[(0, 1000, 10_000), (1000, 1000, 20_000), (1500, 2500, 30_000)]);
    run(&mut e, "edit.fades_create", json!({"tracks": [t.0], "start": 900, "end": 2500}));
    assert_eq!(clips(&e, t), vec![(0, 1500), (900, 1100), (1500, 2500)]);
    assert_eq!(crossfades(&e, t)[0], (600, 900, 1500, 600));
}

#[test]
fn fades_created_over_a_crossfade_can_use_the_audio_the_cut_gives_back() {
    // 150 samples of each file are left past the crossfade, and the cut gives 100 more back: a
    // crossfade 500 long can be built, which it could not if the handles were measured before the cut.
    let (mut e, t, _) = long("slip", &[(0, 1000, 998_750), (1000, 1000, 250)]);
    run(&mut e, "edit.fades_create", json!({"tracks": [t.0], "start": 900, "end": 1100}));
    let made = e.session().track(t).unwrap().clips().to_vec();
    assert_eq!((1_000_000 - (made[0].source_offset() + made[0].length), made[1].source_offset()), (150, 150));
    assert_eq!(crossfades(&e, t), [(200, 900, 1100, 200)]);
    run(&mut e, "edit.fades_create", json!({"tracks": [t.0], "start": 900, "end": 1100}));
    assert_eq!(e.session().track(t).unwrap().clips(), made, "exactly the crossfade");
    run(&mut e, "edit.fades_create", json!({"tracks": [t.0], "start": 750, "end": 1250}));
    assert_eq!((crossfades(&e, t), cuts(&e, t)), (vec![(500, 750, 1250, 500)], vec![1000]), "wider than the crossfade");
}

#[test]
fn selecting_a_clip_or_fading_to_a_point_in_it_leaves_its_crossfade_sound() {
    let (mut e, t, _) = long("slip", &[(0, 1000, 10_000), (1000, 1000, 20_000)]);
    run(&mut e, "edit.fades_create", json!({"tracks": [t.0], "start": 900, "end": 1100}));
    let made = clips(&e, t);
    // A range that is a clip, reaches past the pair, or does not hold the joint (the middle of the
    // overlap): no audio is dragged under the other clip, and both fades stay as long as the overlap.
    let intact = |e: &Engine, what: &str| assert_eq!((clips(e, t), crossfades(e, t)), (made.clone(), vec![(200, 900, 1100, 200)]), "{what}");
    for (start, end) in [(900, 2000), (0, 1100), (1000, 1100), (900, 1000), (500, 950), (-100, 1500), (500, 2500)] {
        let _ = e.execute("edit.fades_create", &json!({"tracks": [t.0], "start": start, "end": end}));
        intact(&e, &format!("{start}..{end}"));
    }
    run(&mut e, "edit.select", json!({"tracks": [t.0], "start": 1500}));
    run(&mut e, "edit.fade_to_start", json!({"tracks": [t.0]}));
    intact(&e, "fade to start");
    run(&mut e, "edit.select", json!({"tracks": [t.0], "start": 500}));
    run(&mut e, "edit.fade_to_end", json!({"tracks": [t.0]}));
    intact(&e, "fade to end");
}

#[test]
fn a_range_that_holds_the_cut_makes_the_crossfade_shorter_or_longer() {
    // Crossfade 900..1100 around the cut at 1000: the new one is the range, whichever side it grows
    // or shrinks on, around the same cut, with no audio moved, and one undo brings the old one back.
    for (start, end, want) in [
        (950, 1050, (100, 950, 1050, 100)),
        (990, 1100, (110, 990, 1100, 110)),
        (900, 1500, (600, 900, 1500, 600)),
        (500, 1100, (600, 500, 1100, 600)),
    ] {
        let (mut e, t, _) = long("slip", &[(0, 1000, 10_000), (1000, 1000, 20_000)]);
        let placed = slide(&e, t);
        run(&mut e, "edit.fades_create", json!({"tracks": [t.0], "start": 900, "end": 1100}));
        let made = e.session().track(t).unwrap().clips().to_vec();
        run(&mut e, "edit.fades_create", json!({"tracks": [t.0], "start": start, "end": end}));
        assert_eq!((crossfades(&e, t), cuts(&e, t)), (vec![want], vec![1000]), "{start}..{end}");
        assert_eq!(slide(&e, t), placed, "{start}..{end}");
        assert!(e.undo());
        assert_eq!(e.session().track(t).unwrap().clips(), made, "{start}..{end} undone");
    }
}

#[test]
fn a_crossfade_short_of_audio_keeps_both_fades_as_long_as_its_overlap() {
    let fades = |e: &Engine, t: TrackId| e.session().track(t).unwrap().clips().iter().map(|c| (c.fade_in.len, c.fade_out.len)).collect::<Vec<_>>();
    // A has no audio after its end, B none before its start: crossfaded over 100, rebuilt over a wider
    // range, the pair reaches only the 50 + 50 the cut gives back.
    let (mut e, t, _) = long("slip", &[(0, 1000, 999_000), (900, 1100, 0)]);
    e.session_mut().track_mut(t).and_then(|t| t.playlist_mut()).unwrap().clips[0].fade_out.len = 100;
    e.session_mut().track_mut(t).and_then(|t| t.playlist_mut()).unwrap().clips[1].fade_in.len = 100;
    run(&mut e, "edit.fades_create", json!({"tracks": [t.0], "start": 800, "end": 1100}));
    assert_eq!((clips(&e, t), crossfades(&e, t)), (vec![(0, 1000), (900, 1100)], vec![(100, 900, 1000, 100)]));
    assert_eq!(cuts(&e, t), [950], "no cut was stored: the middle of the overlap, which is now stored");
    // Joined clips, A short of audio: B alone reaches back under A.
    let (mut e, t, _) = long("slip", &[(0, 1000, 999_000), (1000, 1000, 20_000)]);
    run(&mut e, "edit.fades_create", json!({"tracks": [t.0], "start": 900, "end": 1100}));
    assert_eq!((crossfades(&e, t), cuts(&e, t)), (vec![(100, 900, 1000, 100)], vec![1000]));
    // No audio on either side: the clips stay butted and each fades to the joint.
    let (mut e, t, _) = long("slip", &[(0, 1000, 999_000), (1000, 1000, 0)]);
    run(&mut e, "edit.fades_create", json!({"tracks": [t.0], "start": 900, "end": 1100}));
    assert_eq!((clips(&e, t), fades(&e, t)), (vec![(0, 1000), (1000, 1000)], vec![(0, 100), (100, 0)]));
}

#[test]
fn a_clip_layered_on_another_is_no_crossfade_partner() {
    let fade = |e: &Engine, id: ClipId| e.session().find_clip(id).map(|(_, c)| (c.fade_in.len, c.fade_out.len)).unwrap();
    let (mut e, t, ids) = long("slip", &[(0, 10_000, 10_000), (2000, 1000, 50_000)]);
    run(&mut e, "edit.fades_create", json!({"tracks": [t.0], "start": 9000, "end": 10_000}));
    assert_eq!(fade(&e, ids[0]), (0, 1000));
    run(&mut e, "edit.fades_create", json!({"tracks": [t.0], "start": 2000, "end": 2100}));
    assert_eq!(fade(&e, ids[1]), (100, 0));
    run(&mut e, "edit.select", json!({"tracks": [t.0], "start": 5000}));
    run(&mut e, "edit.fade_to_end", json!({"tracks": [t.0]}));
    assert_eq!(fade(&e, ids[0]), (0, 5000));
    assert_eq!(clips(&e, t), vec![(0, 10_000), (2000, 1000)]);
}

#[test]
fn fades_created_over_one_crossfade_leave_the_others_alone() {
    let (mut e, t, _) = long("slip", &[(0, 1000, 10_000), (1000, 1000, 20_000), (2000, 1000, 30_000)]);
    run(&mut e, "edit.fades_create", json!({"tracks": [t.0], "start": 900, "end": 1100}));
    run(&mut e, "edit.fades_create", json!({"tracks": [t.0], "start": 1900, "end": 2100}));
    assert_eq!(crossfades(&e, t), [(200, 900, 1100, 200), (200, 1900, 2100, 200)]);
    run(&mut e, "edit.fades_create", json!({"tracks": [t.0], "start": 500, "end": 1500}));
    assert_eq!(crossfades(&e, t), [(1000, 500, 1500, 1000), (200, 1900, 2100, 200)]);
}

#[test]
fn select_all_then_create_fades_still_finds_nothing_to_fade_on_crossfaded_clips() {
    let (mut e, t, _) = long("slip", &[(0, 1000, 10_000), (1000, 1000, 20_000), (2000, 1000, 30_000)]);
    run(&mut e, "edit.fades_create", json!({"tracks": [t.0], "start": 900, "end": 1100}));
    run(&mut e, "edit.fades_create", json!({"tracks": [t.0], "start": 1900, "end": 2100}));
    let made = e.session().track(t).unwrap().clips().to_vec();
    run(&mut e, "edit.select_all", json!({}));
    let err = e.execute("edit.fades_create", &json!({"tracks": [t.0]})).unwrap_err().to_string();
    assert!(err.contains("must cover"), "{err}");
    assert_eq!(e.session().track(t).unwrap().clips(), made);
    assert_eq!(crossfades(&e, t), [(200, 900, 1100, 200), (200, 1900, 2100, 200)]);
    // Two clips only: nothing else bounds the range, but it still is not inside the pair's outer edges.
    let (mut e, t, _) = long("slip", &[(0, 1000, 10_000), (1000, 1000, 20_000)]);
    run(&mut e, "edit.fades_create", json!({"tracks": [t.0], "start": 900, "end": 1100}));
    let made = e.session().track(t).unwrap().clips().to_vec();
    run(&mut e, "edit.select_all", json!({}));
    assert!(e.execute("edit.fades_create", &json!({"tracks": [t.0]})).is_err());
    assert_eq!(e.session().track(t).unwrap().clips(), made);
}

#[test]
fn a_locked_clip_keeps_its_crossfade_as_it_is() {
    for time_locked in [false, true] {
        let (mut e, t, ids) = long("slip", &[(0, 1000, 10_000), (1000, 1000, 20_000)]);
        run(&mut e, "edit.fades_create", json!({"tracks": [t.0], "start": 900, "end": 1100}));
        let c = e.session_mut().find_clip_mut(ids[1]).unwrap();
        (c.time_locked, c.edit_locked) = (time_locked, !time_locked);
        let made = e.session().track(t).unwrap().clips().to_vec();
        assert!(e.execute("edit.fades_create", &json!({"tracks": [t.0], "start": 500, "end": 1500})).is_err(), "nothing to fade");
        assert_eq!(e.session().track(t).unwrap().clips(), made, "time_locked {time_locked}");
        assert_eq!((clips(&e, t), crossfades(&e, t)), (vec![(0, 1100), (900, 1100)], vec![(200, 900, 1100, 200)]), "time_locked {time_locked}");
    }
}

#[test]
fn a_stretched_clip_keeps_its_crossfade_as_it_is() {
    let (mut e, t, ids) = long("slip", &[(0, 1000, 10_000), (1000, 1000, 20_000)]);
    run(&mut e, "edit.fades_create", json!({"tracks": [t.0], "start": 900, "end": 1100}));
    e.session_mut().find_clip_mut(ids[1]).unwrap().stretch = 0.5;
    let made = e.session().track(t).unwrap().clips().to_vec();
    assert!(e.execute("edit.fades_create", &json!({"tracks": [t.0], "start": 500, "end": 1500})).is_err(), "nothing to fade");
    assert_eq!(e.session().track(t).unwrap().clips(), made);
    assert_eq!((clips(&e, t), crossfades(&e, t)), (vec![(0, 1100), (900, 1100)], vec![(200, 900, 1100, 200)]));
}

#[test]
fn a_crossfade_is_only_replaced_by_a_range_that_reaches_no_other_clip() {
    // Three clips playing at one point are not a crossfade: nothing is cut.
    let (mut e, t, ids) = long("slip", &[(0, 1100, 10_000), (900, 1100, 20_000), (950, 2050, 30_000)]);
    e.session_mut().find_clip_mut(ids[0]).unwrap().fade_out.len = 200;
    e.session_mut().find_clip_mut(ids[1]).unwrap().fade_in.len = 200;
    assert!(e.execute("edit.fades_create", &json!({"tracks": [t.0], "start": 500, "end": 1500})).is_err(), "nothing to fade");
    assert_eq!(clips(&e, t), vec![(0, 1100), (900, 1100), (950, 2050)]);
    assert_eq!(crossfades(&e, t)[0], (200, 900, 1100, 200));
    // Two crossfades in a row: a range that reaches the next one (or the previous one) would pull a
    // third clip under the pair, so it rebuilds neither, and every fade stays as long as its overlap.
    let (mut e, t, _) = long("slip", &[(0, 1000, 10_000), (1000, 1000, 20_000), (2000, 1000, 30_000)]);
    run(&mut e, "edit.fades_create", json!({"tracks": [t.0], "start": 900, "end": 1100}));
    run(&mut e, "edit.fades_create", json!({"tracks": [t.0], "start": 1900, "end": 2100}));
    let made = clips(&e, t);
    for (start, end) in [(500, 2000), (1050, 2500)] {
        assert!(e.execute("edit.fades_create", &json!({"tracks": [t.0], "start": start, "end": end})).is_err(), "{start}..{end}");
        assert_eq!((clips(&e, t), crossfades(&e, t)), (made.clone(), vec![(200, 900, 1100, 200), (200, 1900, 2100, 200)]), "{start}..{end}");
    }
}

#[test]
fn a_range_inside_the_zone_that_does_not_hold_the_cut_changes_nothing() {
    // Crossfade 800..1050, cut at 1000. The later clip's start (800) and the earlier clip's end (1050)
    // are hidden inside it, not a clip start or end to fade: a range that reaches one of them without
    // holding the cut (strictly inside) has nothing to fade, and the crossfade stays as it is.
    let (mut e, t, _) = off_centre();
    let made = e.session().track(t).unwrap().clips().to_vec();
    for (start, end) in [(800, 1000), (800, 950), (850, 990), (900, 1000), (1000, 1050), (1000, 1100), (1010, 1050), (1010, 1040)] {
        assert!(e.execute("edit.fades_create", &json!({"tracks": [t.0], "start": start, "end": end})).is_err(), "{start}..{end}");
        assert_eq!(e.session().track(t).unwrap().clips(), made, "{start}..{end}");
    }
}

#[test]
fn a_range_inside_a_plain_clip_or_in_a_gap_changes_nothing() {
    let (mut e, t, _) = long("slip", &[(0, 1000, 10_000), (1500, 1000, 20_000)]);
    let made = e.session().track(t).unwrap().clips().to_vec();
    for (start, end) in [(100, 200), (400, 900), (1200, 1300), (1600, 2400), (1000, 1500)] {
        assert!(e.execute("edit.fades_create", &json!({"tracks": [t.0], "start": start, "end": end})).is_err(), "{start}..{end}");
        assert_eq!(e.session().track(t).unwrap().clips(), made, "{start}..{end}");
    }
    // A clip start (or end) is where a fade-in (fade-out) goes.
    run(&mut e, "edit.fades_create", json!({"tracks": [t.0], "start": 1500, "end": 1600}));
    run(&mut e, "edit.fades_create", json!({"tracks": [t.0], "start": 900, "end": 1100}));
    let fades = e.session().track(t).unwrap().clips().iter().map(|c| (c.fade_in.len, c.fade_out.len)).collect::<Vec<_>>();
    assert_eq!(fades, [(0, 100), (100, 0)]);
}

#[test]
fn whatever_the_range_a_crossfade_keeps_its_cut_and_its_fades_as_long_as_its_overlap() {
    // Every range with ends on a 100 grid, over a crossfade 800..1050 cut at 1000: the command may
    // refuse, replace the crossfade or fade the outer ends of the pair, but it never moves the cut,
    // never splits the pair and never leaves a fade on the crossfade that is not as long as the overlap.
    let marks: Vec<i64> = (-1..=21).map(|i| i * 100).collect();
    for &start in &marks {
        for &end in marks.iter().filter(|&&end| end > start) {
            let (mut e, t, _) = off_centre();
            let _ = e.execute("edit.fades_create", &json!({"tracks": [t.0], "start": start, "end": end}));
            let c = e.session().track(t).unwrap().clips().to_vec();
            assert_eq!(c.len(), 2, "{start}..{end}");
            let overlap = c[0].end() - c[1].start;
            assert!(overlap > 0, "{start}..{end}");
            assert_eq!((c[0].fade_out.len, c[1].fade_in.len), (overlap, overlap), "{start}..{end}");
            assert_eq!(crossfade_cut(&c[0], &c[1]), 1000, "{start}..{end}");
        }
    }
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
