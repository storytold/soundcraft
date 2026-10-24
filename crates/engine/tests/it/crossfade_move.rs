//! `edit.move_crossfade`: dragging a crossfade slides the joint between its two clips. Both clips
//! keep their audio where it is on the timeline; only the edit point (the end of the earlier clip
//! and the start of the later one) moves, by the same amount, so the crossfade keeps its length. The
//! cut goes with it: it keeps its place in the zone.

use serde_json::{Value, json};
use soundcraft_engine::Engine;
use soundcraft_model::{Clip, ClipId, Fade, FadeShape, Source, SourceId, TrackId, crossfade_cut};

const XFADE: i64 = 200;

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

/// Clip A on 0..1200 (200 of it under B), clip B on 1000..3000, crossfaded over 1000..1200.
/// A has `after` samples of file left past its end, B has `before` samples of file ahead of its start.
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
    s.track_mut(t).and_then(|t| t.playlist_mut()).unwrap().clips = vec![ca, cb];
    (e, t, a, b)
}

fn clip(e: &Engine, id: ClipId) -> Clip {
    e.session().find_clip(id).unwrap().1.clone()
}

/// Where the clip's file sits on the timeline: this must not change when a crossfade slides.
fn anchor(c: &Clip) -> i64 {
    c.start - c.source_offset()
}

fn moved(e: &mut Engine, b: ClipId, by: i64) -> Value {
    e.execute("edit.move_crossfade", &json!({"clip": b.0, "by": by})).unwrap()
}

#[test]
fn sliding_a_crossfade_moves_the_joint_and_leaves_the_audio_where_it_is() {
    for by in [100, -100, 1, -1] {
        let (mut e, _, a, b) = crossfade(10_000, 10_000);
        let (a0, b0) = (clip(&e, a), clip(&e, b));
        assert_eq!(moved(&mut e, b, by), json!({"by": by}));
        let (a1, b1) = (clip(&e, a), clip(&e, b));
        // The joint moves: A ends later, B starts later, by the same amount.
        assert_eq!((a1.end(), b1.start), (a0.end() + by, b0.start + by));
        // Nothing else moves: A's start, B's end, and every clip's file position.
        assert_eq!((a1.start, b1.end()), (a0.start, b0.end()));
        assert_eq!((anchor(&a1), anchor(&b1)), (anchor(&a0), anchor(&b0)));
        // The crossfade contract holds: overlap == fade-out == fade-in, same shape.
        assert_eq!(a1.end() - b1.start, XFADE);
        assert_eq!((a1.fade_out, b1.fade_in), (a0.fade_out, b0.fade_in));
    }
}

#[test]
fn the_cut_moves_with_the_crossfade() {
    // Off the middle of the zone (1000..1200), at its start, at its end, and not stored at all.
    for stored in [Some(50), Some(0), Some(XFADE), None] {
        for by in [100, -100, 1] {
            let (mut e, _, a, b) = crossfade(10_000, 10_000);
            e.session_mut().find_clip_mut(b).unwrap().cut = stored;
            let cut0 = crossfade_cut(&clip(&e, a), &clip(&e, b));
            moved(&mut e, b, by);
            let (a1, b1) = (clip(&e, a), clip(&e, b));
            assert_eq!(crossfade_cut(&a1, &b1), cut0 + by, "{stored:?} by {by}: the cut travels with the zone");
            // It keeps its place in the zone, and is stored the way it was.
            assert_eq!(b1.cut, stored);
            assert_eq!(crossfade_cut(&a1, &b1) - b1.start, cut0 - 1000);
        }
    }
    // Sliding there and back leaves the pair exactly as it was.
    let (mut e, _, _, b) = crossfade(10_000, 10_000);
    e.session_mut().find_clip_mut(b).unwrap().cut = Some(70);
    let before = e.session().clone();
    moved(&mut e, b, 300);
    moved(&mut e, b, -300);
    assert_eq!(e.session(), &before);
}

#[test]
fn a_crossfade_slides_no_further_than_the_clips_have_file_to_give() {
    // A has 80 samples of file past its end, B has 40 ahead of its start.
    let (mut e, _, a, b) = crossfade(80, 40);
    assert_eq!(moved(&mut e, b, 5000), json!({"by": 80}));
    assert_eq!(clip(&e, a).end(), 1280);
    assert_eq!(clip(&e, b).start, 1080);
    assert_eq!(moved(&mut e, b, -5000), json!({"by": -120}), "back to B's handle: 80 + 40");
    assert_eq!((clip(&e, a).end(), clip(&e, b).start), (1160, 960));
    assert_eq!(clip(&e, b).source_offset(), 0);
    // The contract still holds at both stops.
    assert_eq!(clip(&e, a).end() - clip(&e, b).start, XFADE);
}

#[test]
fn a_crossfade_keeps_both_clips_and_their_own_fades_in_one_piece() {
    // Far left: B's start may not reach A's start.
    let (mut e, _, a, b) = crossfade(100_000, 100_000);
    assert_eq!(moved(&mut e, b, i64::MIN), json!({"by": -999}));
    assert_eq!((clip(&e, a).start, clip(&e, b).start), (0, 1));
    assert_eq!(clip(&e, a).end() - clip(&e, b).start, XFADE);
    // Far right: B keeps room for the fade-in it already has, and a sample of its own past the zone.
    let (mut e, _, a, b) = crossfade(100_000, 100_000);
    assert_eq!(moved(&mut e, b, i64::MAX), json!({"by": 1799}));
    assert_eq!((clip(&e, b).start, clip(&e, b).end(), clip(&e, b).length), (2799, 3000, XFADE + 1));
    assert_eq!(clip(&e, a).end() - clip(&e, b).start, XFADE);
    assert_eq!(clip(&e, b).end() - clip(&e, a).end(), 1, "B is not wholly inside the zone");
    // A fade-out at the end of B is kept clear of the crossfade too, and a sample beyond it.
    let (mut e, _, _, b) = crossfade(100_000, 100_000);
    e.session_mut().find_clip_mut(b).unwrap().fade_out = Fade { len: 500, shape: FadeShape::Linear, curvature: 0.0 };
    assert_eq!(moved(&mut e, b, i64::MAX), json!({"by": 1299}));
    assert_eq!(clip(&e, b).fade_out.len, 500);
    // A's own fade-in keeps B's start after it, and A a sample of its own past the fade.
    let (mut e, _, a, b) = crossfade(100_000, 100_000);
    e.session_mut().find_clip_mut(a).unwrap().fade_in = Fade { len: 300, shape: FadeShape::Linear, curvature: 0.0 };
    assert_eq!(moved(&mut e, b, i64::MIN), json!({"by": -699}));
    assert_eq!(clip(&e, b).start, 301);
}

#[test]
fn a_crossfade_slide_is_one_undo_step() {
    let (mut e, _, a, b) = crossfade(10_000, 10_000);
    let before = e.session().clone();
    let steps = e.undo_history().len();
    moved(&mut e, b, 150);
    assert_eq!(e.undo_history().len(), steps + 1);
    let after = e.session().clone();
    assert_ne!(after, before);
    assert!(e.undo());
    assert_eq!(e.session(), &before);
    assert!(e.redo());
    assert_eq!(e.session(), &after);
    assert_eq!((clip(&e, a).end(), clip(&e, b).start), (1350, 1150));
}

#[test]
fn a_slide_that_goes_nowhere_changes_nothing_and_leaves_no_undo_step() {
    // Both clips are at the end of their file.
    let (mut e, _, _, b) = crossfade(0, 0);
    let before = e.session().clone();
    let steps = e.undo_history().len();
    assert_eq!(moved(&mut e, b, 50), json!({"by": 0}));
    assert_eq!(moved(&mut e, b, -50), json!({"by": 0}));
    assert_eq!(moved(&mut e, b, 0), json!({"by": 0}));
    assert_eq!(e.session(), &before);
    assert_eq!(e.undo_history().len(), steps);
}

#[test]
fn only_a_crossfade_between_audio_clips_that_can_be_trimmed_slides() {
    let (mut e, _, a, b) = crossfade(10_000, 10_000);
    let before = e.session().clone();
    let steps = e.undo_history().len();
    let mut fails = |p: Value| {
        assert!(e.execute("edit.move_crossfade", &p).is_err(), "{p}");
    };
    // The earlier clip of the pair is not "the clip the crossfade leads into".
    fails(json!({"clip": a.0, "by": 10}));
    fails(json!({"clip": 987_654, "by": 10}));
    fails(json!({"by": 10}));
    fails(json!({"clip": b.0}));
    fails(json!({"clip": b.0, "by": "far"}));
    assert_eq!(e.session(), &before);
    assert_eq!(e.undo_history().len(), steps);
    // The same call with the right clip works, so the refusals above are about the call.
    assert_eq!(moved(&mut e, b, 10), json!({"by": 10}));
    // An edit-locked clip on either side keeps the joint where it is.
    for pick in 0..2 {
        let (mut e, _, a, b) = crossfade(10_000, 10_000);
        e.session_mut().find_clip_mut(if pick == 0 { a } else { b }).unwrap().edit_locked = true;
        assert!(e.execute("edit.move_crossfade", &json!({"clip": b.0, "by": 10})).is_err());
    }
    // So does a stretched one: a start trim would shift its audio.
    for pick in 0..2 {
        let (mut e, _, a, b) = crossfade(10_000, 10_000);
        e.session_mut().find_clip_mut(if pick == 0 { a } else { b }).unwrap().stretch = 1.5;
        let before = e.session().clone();
        assert!(e.execute("edit.move_crossfade", &json!({"clip": b.0, "by": 10})).is_err());
        assert_eq!(e.session(), &before);
    }
    // A time lock only says the audio stays in time, which a slide respects.
    let (mut e, _, a, b) = crossfade(10_000, 10_000);
    e.session_mut().find_clip_mut(a).unwrap().time_locked = true;
    e.session_mut().find_clip_mut(b).unwrap().time_locked = true;
    assert_eq!(moved(&mut e, b, 10), json!({"by": 10}));
    // Two clips that only touch, or sit apart, are not a crossfade.
    let (mut e, _, _, b) = crossfade(10_000, 10_000);
    e.session_mut().find_clip_mut(b).unwrap().trim_start_to(1200);
    assert!(e.execute("edit.move_crossfade", &json!({"clip": b.0, "by": 10})).is_err());
}

#[test]
fn a_crossfade_keeps_clear_of_whatever_its_neighbours_already_claim() {
    // C crosses the end of B over 200 samples although B's fade-out says 1: A may not run into C.
    let (mut e, t, a, b) = crossfade(100_000, 100_000);
    let c = e.session_mut().new_clip_id();
    e.session_mut().find_clip_mut(b).unwrap().fade_out.len = 1;
    e.session_mut().track_mut(t).and_then(|t| t.playlist_mut()).unwrap().clips.push(Clip::audio(c, "c", SourceId(2), 9000, 2800, 2000));
    assert_eq!(moved(&mut e, b, i64::MAX), json!({"by": 1600}));
    assert_eq!(clip(&e, a).end(), clip(&e, c).start);
    // Likewise Z, which crosses the start of A over 100 samples although A's fade-in says 1.
    let (mut e, t, a, b) = crossfade(100_000, 100_000);
    let z = e.session_mut().new_clip_id();
    let ca = e.session_mut().find_clip_mut(a).unwrap();
    ca.trim_start_to(500);
    ca.fade_in.len = 1;
    let pl = e.session_mut().track_mut(t).and_then(|t| t.playlist_mut()).unwrap();
    pl.clips.push(Clip::audio(z, "z", SourceId(2), 9000, 0, 600));
    pl.sort();
    assert_eq!(moved(&mut e, b, i64::MIN), json!({"by": -400}));
    assert_eq!(clip(&e, b).start, clip(&e, z).end());
    let starts: Vec<i64> = e.session().tracks.iter().flat_map(|t| t.clips().iter().map(|c| c.start)).collect();
    assert!(starts.windows(2).all(|w| w[0] <= w[1]), "{starts:?}");
}

#[test]
fn a_fade_out_longer_than_the_overlap_still_fits_its_clip_after_a_slide() {
    let (mut e, _, a, b) = crossfade(100_000, 100_000);
    e.session_mut().find_clip_mut(a).unwrap().fade_out.len = 600;
    assert_eq!(moved(&mut e, b, i64::MIN), json!({"by": -600}));
    let a1 = clip(&e, a);
    assert_eq!((a1.length, a1.fade_out.len), (600, 600));
    assert_eq!(a1.end() - clip(&e, b).start, XFADE);
}
