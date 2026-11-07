//! `edit.trim_crossfade`: lengthening or shortening a crossfade from one edge of its zone. The zone
//! is laid over the cut, made from the audio each clip has beyond its edge; an edge never crosses the
//! cut, which stays where it is, and the audio never moves.

use serde_json::{Value, json};
use soundcraft_engine::Engine;
use soundcraft_engine::edit as ops;
use soundcraft_model::{Clip, ClipId, Fade, FadeShape, Source, SourceId, TrackId, crossfade_cut};

const XFADE: i64 = 200;
/// Where the cut sits: 50 samples into the zone of 1000..1200, off the middle (1100).
const CUT: i64 = 1050;

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

/// Clip A on 0..1200 (200 of it under B), clip B on 1000..3000, crossfaded over 1000..1200 and cut at
/// [`CUT`]. A has `after` samples of file left past its end, B has `before` samples ahead of its start.
fn crossfade(after: i64, before: i64) -> (Engine, TrackId, ClipId, ClipId) {
    let mut e = Engine::default();
    let t = TrackId(e.execute("track.new", &json!({"count": 1})).unwrap()["tracks"][0].as_u64().unwrap());
    let s = e.session_mut();
    s.sources.push(source(1, u64::try_from(300 + 1200 + after).unwrap()));
    s.sources.push(source(2, u64::try_from(before + 2000 + 1000).unwrap()));
    let (a, b) = (s.new_clip_id(), s.new_clip_id());
    let fade = Fade { len: XFADE, shape: FadeShape::EqualPower, curvature: 0.0 };
    let mut ca = Clip::audio(a, "a", SourceId(1), 300, 0, 1200);
    ca.fade_out = fade;
    let mut cb = Clip::audio(b, "b", SourceId(2), before, 1000, 2000);
    cb.fade_in = fade;
    cb.cut = Some(CUT - 1000);
    s.track_mut(t).and_then(|t| t.playlist_mut()).unwrap().clips = vec![ca, cb];
    (e, t, a, b)
}

fn clip(e: &Engine, id: ClipId) -> Clip {
    e.session().find_clip(id).unwrap().1.clone()
}

/// Where the clip's file sits on the timeline: this must not change when a crossfade is trimmed.
fn anchor(c: &Clip) -> i64 {
    c.start - c.source_offset()
}

fn cut(e: &Engine, a: ClipId, b: ClipId) -> i64 {
    crossfade_cut(&clip(e, a), &clip(e, b))
}

fn trimmed(e: &mut Engine, b: ClipId, p: Value) -> Value {
    let mut p = p;
    p["clip"] = json!(b.0);
    e.execute("edit.trim_crossfade", &p).unwrap()
}

/// The zone, as (left edge, right edge).
fn zone(e: &Engine, a: ClipId, b: ClipId) -> (i64, i64) {
    (clip(e, b).start, clip(e, a).end())
}

/// What a crossfade has to keep true after any trim: both fades are the overlap, the audio is where it
/// was, and the cut is where it was.
fn holds(e: &Engine, a: ClipId, b: ClipId, before: &(Clip, Clip)) {
    let (a1, b1) = (clip(e, a), clip(e, b));
    assert_eq!((anchor(&a1), anchor(&b1)), (anchor(&before.0), anchor(&before.1)), "the audio did not move");
    assert_eq!((a1.start, b1.end()), (before.0.start, before.1.end()), "the outer edges did not move");
    assert_eq!(cut(e, a, b), CUT, "the cut stayed");
    let overlap = a1.end() - b1.start;
    if overlap > 0 {
        assert_eq!((a1.fade_out.len, b1.fade_in.len), (overlap, overlap), "both fades are the overlap");
    } else {
        assert_eq!((a1.fade_out.len, b1.fade_in.len, b1.cut), (0, 0, None), "butted: no fade, no stored cut");
    }
}

fn pair(e: &Engine, a: ClipId, b: ClipId) -> (Clip, Clip) {
    (clip(e, a), clip(e, b))
}

#[test]
fn the_left_edge_goes_where_it_is_taken_and_the_cut_stays() {
    for (start, left) in [(900, 900), (1030, 1030), (1049, 1049), (1000, 1000)] {
        let (mut e, _, a, b) = crossfade(10_000, 10_000);
        let before = pair(&e, a, b);
        let r = trimmed(&mut e, b, json!({"start": start}));
        assert_eq!(r, json!({"start": left, "end": 1200, "cut": CUT}));
        assert_eq!(zone(&e, a, b), (left, 1200));
        holds(&e, a, b, &before);
    }
}

#[test]
fn the_right_edge_goes_where_it_is_taken_and_the_cut_stays() {
    for end in [1400, 1100, 1051, 1200] {
        let (mut e, _, a, b) = crossfade(10_000, 10_000);
        let before = pair(&e, a, b);
        let r = trimmed(&mut e, b, json!({"end": end}));
        assert_eq!(r, json!({"start": 1000, "end": end, "cut": CUT}));
        assert_eq!(zone(&e, a, b), (1000, end));
        holds(&e, a, b, &before);
    }
}

#[test]
fn both_edges_can_go_at_once_and_the_stored_cut_is_the_same_place() {
    let (mut e, _, a, b) = crossfade(10_000, 10_000);
    let before = pair(&e, a, b);
    trimmed(&mut e, b, json!({"start": 800, "end": 1500}));
    assert_eq!(zone(&e, a, b), (800, 1500));
    holds(&e, a, b, &before);
    // The cut is stored from the new start now, and read back at the same place.
    assert_eq!(clip(&e, b).cut, Some(CUT - 800));
    // A zone with no stored cut is cut in the middle: trimming makes that explicit and keeps it.
    let (mut e, _, a, b) = crossfade(10_000, 10_000);
    e.session_mut().find_clip_mut(b).unwrap().cut = None;
    assert_eq!(cut(&e, a, b), 1100);
    trimmed(&mut e, b, json!({"start": 900}));
    assert_eq!((cut(&e, a, b), zone(&e, a, b)), (1100, (900, 1200)));
}

#[test]
fn an_edge_stops_at_the_audio_its_clip_has() {
    // A has 80 samples of file past its end, B has 40 ahead of its start.
    let (mut e, _, a, b) = crossfade(80, 40);
    let before = pair(&e, a, b);
    assert_eq!(trimmed(&mut e, b, json!({"start": -5000, "end": 5000})), json!({"start": 960, "end": 1280, "cut": CUT}));
    assert_eq!(clip(&e, b).source_offset(), 0);
    holds(&e, a, b, &before);
    // Both clips at the end of their file: neither edge can grow, whatever the pointer does.
    let (mut e, _, a, b) = crossfade(0, 0);
    assert_eq!(trimmed(&mut e, b, json!({"start": 0, "end": 3000})), json!({"start": 1000, "end": 1200, "cut": CUT}));
    assert_eq!(zone(&e, a, b), (1000, 1200));
}

#[test]
fn an_edge_stops_where_a_neighbour_already_claims_the_room() {
    // A's own fade-in claims the first 300 samples; B's fade-out the last 500; each clip keeps a sample of its own past them.
    let (mut e, _, a, b) = crossfade(100_000, 100_000);
    e.session_mut().find_clip_mut(a).unwrap().fade_in = Fade { len: 300, shape: FadeShape::Linear, curvature: 0.0 };
    e.session_mut().find_clip_mut(b).unwrap().fade_out = Fade { len: 500, shape: FadeShape::Linear, curvature: 0.0 };
    let before = pair(&e, a, b);
    assert_eq!(trimmed(&mut e, b, json!({"start": -9999, "end": 99_999})), json!({"start": 301, "end": 2499, "cut": CUT}));
    holds(&e, a, b, &before);
    assert_eq!(clip(&e, b).fade_out.len, 500);
    assert_eq!(clip(&e, a).fade_in.len, 300);
    // A crossfade into A (Z, over A's first 100 samples) and one out of B (C, over its last 200).
    let (mut e, t, a, b) = crossfade(100_000, 100_000);
    let (z, c) = (e.session_mut().new_clip_id(), e.session_mut().new_clip_id());
    e.session_mut().find_clip_mut(a).unwrap().trim_start_to(500);
    e.session_mut().find_clip_mut(a).unwrap().fade_in.len = 1;
    let pl = e.session_mut().track_mut(t).and_then(|t| t.playlist_mut()).unwrap();
    pl.clips.push(Clip::audio(z, "z", SourceId(2), 9000, 0, 600));
    pl.clips.push(Clip::audio(c, "c", SourceId(2), 9000, 2800, 2000));
    pl.sort();
    assert_eq!(trimmed(&mut e, b, json!({"start": -9999, "end": 99_999})), json!({"start": 600, "end": 2800, "cut": CUT}));
}

#[test]
fn each_clip_keeps_a_sample_of_its_own_beyond_the_zone() {
    // Taken as far as they go, the edges stop one sample inside A's start and one before B's end.
    let (mut e, _, a, b) = crossfade(100_000, 100_000);
    let before = pair(&e, a, b);
    assert_eq!(trimmed(&mut e, b, json!({"start": i64::MIN, "end": i64::MAX})), json!({"start": 1, "end": 2999, "cut": CUT}));
    holds(&e, a, b, &before);
    assert_eq!((clip(&e, b).start - clip(&e, a).start, clip(&e, b).end() - clip(&e, a).end()), (1, 1));
    // With a fade on the far side of each clip, the sample comes after the fade.
    let (mut e, _, a, b) = crossfade(100_000, 100_000);
    e.session_mut().find_clip_mut(a).unwrap().fade_in = Fade { len: 40, shape: FadeShape::Linear, curvature: 0.0 };
    e.session_mut().find_clip_mut(b).unwrap().fade_out = Fade { len: 60, shape: FadeShape::Linear, curvature: 0.0 };
    assert_eq!(trimmed(&mut e, b, json!({"start": i64::MIN, "end": i64::MAX})), json!({"start": 41, "end": 2939, "cut": CUT}));
}

#[test]
fn an_edge_never_crosses_the_cut() {
    // The left edge taken past the cut stops on it: a zone from the cut to the old right edge.
    let (mut e, _, a, b) = crossfade(10_000, 10_000);
    let before = pair(&e, a, b);
    assert_eq!(trimmed(&mut e, b, json!({"start": i64::MAX})), json!({"start": CUT, "end": 1200, "cut": CUT}));
    assert_eq!(clip(&e, b).cut, Some(0));
    holds(&e, a, b, &before);
    // The same for the right edge, taken before the cut.
    let (mut e, _, a, b) = crossfade(10_000, 10_000);
    assert_eq!(trimmed(&mut e, b, json!({"end": i64::MIN})), json!({"start": 1000, "end": CUT, "cut": CUT}));
    assert_eq!(clip(&e, b).cut, Some(CUT - 1000));
    holds(&e, a, b, &before);
}

#[test]
fn an_overlap_that_reaches_zero_leaves_the_clips_butted_at_the_cut() {
    let (mut e, _, a, b) = crossfade(10_000, 10_000);
    let before = pair(&e, a, b);
    // Shape and curvature stay on the fades, whatever their length.
    e.session_mut().find_clip_mut(a).unwrap().fade_out = Fade { len: XFADE, shape: FadeShape::Linear, curvature: 0.4 };
    trimmed(&mut e, b, json!({"start": CUT, "end": CUT}));
    assert_eq!(zone(&e, a, b), (CUT, CUT));
    holds(&e, a, b, &before);
    let (a1, b1) = pair(&e, a, b);
    assert_eq!((a1.fade_out.shape, a1.fade_out.curvature), (FadeShape::Linear, 0.4));
    assert_eq!((a1.end(), b1.start, b1.cut), (CUT, CUT, None));
    // Butted clips are no crossfade any more: there is nothing left to trim.
    assert!(e.execute("edit.trim_crossfade", &json!({"clip": b.0, "start": 900})).is_err());
    // One edge after the other gets there too.
    let (mut e, _, a, b) = crossfade(10_000, 10_000);
    trimmed(&mut e, b, json!({"start": CUT}));
    assert_eq!((zone(&e, a, b), clip(&e, b).cut), ((CUT, 1200), Some(0)));
    trimmed(&mut e, b, json!({"end": CUT}));
    holds(&e, a, b, &pair(&e, a, b));
    assert_eq!(zone(&e, a, b), (CUT, CUT));
    assert_eq!(clip(&e, b).cut, None);
}

#[test]
fn shapes_and_curvature_of_both_fades_are_kept() {
    let (mut e, _, a, b) = crossfade(10_000, 10_000);
    e.session_mut().find_clip_mut(a).unwrap().fade_out = Fade { len: XFADE, shape: FadeShape::Linear, curvature: 0.3 };
    e.session_mut().find_clip_mut(b).unwrap().fade_in = Fade { len: XFADE, shape: FadeShape::Logarithmic, curvature: -0.2 };
    trimmed(&mut e, b, json!({"start": 700, "end": 1600}));
    let (a1, b1) = pair(&e, a, b);
    assert_eq!(a1.fade_out, Fade { len: 900, shape: FadeShape::Linear, curvature: 0.3 });
    assert_eq!(b1.fade_in, Fade { len: 900, shape: FadeShape::Logarithmic, curvature: -0.2 });
}

#[test]
fn a_trim_is_one_undo_step() {
    let (mut e, _, a, b) = crossfade(10_000, 10_000);
    let before = e.session().clone();
    let steps = e.undo_history().len();
    trimmed(&mut e, b, json!({"start": 900, "end": 1300}));
    assert_eq!(e.undo_history().len(), steps + 1);
    let after = e.session().clone();
    assert_ne!(after, before);
    assert!(e.undo());
    assert_eq!(e.session(), &before);
    assert!(e.redo());
    assert_eq!(e.session(), &after);
    assert_eq!(zone(&e, a, b), (900, 1300));
}

#[test]
fn an_edge_that_goes_nowhere_changes_nothing_and_leaves_no_undo_step() {
    let (mut e, _, a, b) = crossfade(0, 0);
    let before = e.session().clone();
    let steps = e.undo_history().len();
    // Both clips are at the end of their file, and these are the edges where they are already.
    for p in [json!({"start": 1000}), json!({"end": 1200}), json!({"start": 400, "end": 9000}), json!({"start": 1000, "end": 1200})] {
        assert_eq!(trimmed(&mut e, b, p), json!({"start": 1000, "end": 1200, "cut": CUT}));
    }
    assert_eq!(e.session(), &before);
    assert_eq!(e.undo_history().len(), steps);
    assert_eq!(zone(&e, a, b), (1000, 1200));
}

#[test]
fn only_a_crossfade_between_unlocked_unstretched_audio_clips_trims() {
    let (mut e, _, a, b) = crossfade(10_000, 10_000);
    let before = e.session().clone();
    let steps = e.undo_history().len();
    let mut fails = |p: Value| {
        assert!(e.execute("edit.trim_crossfade", &p).is_err(), "{p}");
    };
    // The earlier clip of the pair is not "the clip the crossfade leads into".
    fails(json!({"clip": a.0, "start": 900}));
    fails(json!({"clip": 987_654, "start": 900}));
    fails(json!({"start": 900}));
    fails(json!({"clip": b.0}));
    fails(json!({"clip": b.0, "start": "far"}));
    assert_eq!(e.session(), &before);
    assert_eq!(e.undo_history().len(), steps);
    // The same call with the right clip works, so the refusals above are about the call.
    trimmed(&mut e, b, json!({"start": 900}));
    // An edit-locked clip on either side keeps the zone as it is.
    for pick in 0..2 {
        let (mut e, _, a, b) = crossfade(10_000, 10_000);
        e.session_mut().find_clip_mut(if pick == 0 { a } else { b }).unwrap().edit_locked = true;
        let before = e.session().clone();
        assert!(e.execute("edit.trim_crossfade", &json!({"clip": b.0, "start": 900})).is_err(), "{pick}");
        assert!(e.execute("edit.trim_crossfade", &json!({"clip": b.0, "end": 1300})).is_err(), "{pick}");
        assert_eq!(e.session(), &before);
    }
    // A time lock only says the audio stays in time, which a trim respects (as for any trim).
    let (mut e, _, a, b) = crossfade(10_000, 10_000);
    e.session_mut().find_clip_mut(a).unwrap().time_locked = true;
    e.session_mut().find_clip_mut(b).unwrap().time_locked = true;
    trimmed(&mut e, b, json!({"start": 900, "end": 1300}));
    assert_eq!(zone(&e, a, b), (900, 1300));
    // Stretched clips on either side: a start trim would ignore the stretch.
    for pick in 0..2 {
        let (mut e, _, a, b) = crossfade(10_000, 10_000);
        e.session_mut().find_clip_mut(if pick == 0 { a } else { b }).unwrap().stretch = 1.5;
        let before = e.session().clone();
        assert!(e.execute("edit.trim_crossfade", &json!({"clip": b.0, "end": 1300})).is_err(), "{pick}");
        assert_eq!(e.session(), &before);
    }
    // Two clips that only touch, or sit apart, are not a crossfade.
    let (mut e, _, _, b) = crossfade(10_000, 10_000);
    e.session_mut().find_clip_mut(b).unwrap().trim_start_to(1200);
    assert!(e.execute("edit.trim_crossfade", &json!({"clip": b.0, "start": 900})).is_err());
}

#[test]
fn hostile_positions_are_held_never_refused() {
    for (start, end) in [(i64::MIN, i64::MAX), (i64::MAX, i64::MIN), (i64::MIN, i64::MIN), (i64::MAX, i64::MAX), (0, 0)] {
        let (mut e, _, a, b) = crossfade(10_000, 10_000);
        let before = pair(&e, a, b);
        let r = e.execute("edit.trim_crossfade", &json!({"clip": b.0, "start": start, "end": end})).unwrap();
        assert!(r["start"].as_i64().unwrap() <= CUT && r["end"].as_i64().unwrap() >= CUT, "{r}");
        holds(&e, a, b, &before);
    }
}

#[test]
fn the_pair_predicate_is_one_place() {
    let (e, t, a, b) = crossfade(10_000, 10_000);
    let clips = e.session().track(t).unwrap().clips().to_vec();
    let (ca, cb) = (clips[0].clone(), clips[1].clone());
    assert_eq!(ops::crossfade_earlier(&clips, &cb).map(|c| c.id), Some(a));
    assert_eq!(ops::crossfade_later(&clips, &ca).map(|c| c.id), Some(b));
    assert!(ops::crossfade_earlier(&clips, &ca).is_none() && ops::crossfade_later(&clips, &cb).is_none());
    // A clip laid inside another is no pair, nor is one that only touches.
    let mut layered = cb.clone();
    layered.trim_end_to(1100);
    assert!(ops::crossfade_earlier(&[ca.clone(), layered.clone()], &layered).is_none());
    let mut touching = cb.clone();
    touching.trim_start_to(1200);
    assert!(ops::crossfade_earlier(&[ca.clone(), touching.clone()], &touching).is_none());
    // Only audio crosses over.
    let mut midi = cb.clone();
    midi.content = soundcraft_model::ClipContent::Midi { sequence: Default::default() };
    assert!(ops::crossfade_earlier(&[ca.clone(), midi.clone()], &midi).is_none());
    assert!(ops::crossfade_later(&[ca, midi], &clips[0]).is_none());
}
