//! `edit.fades_create` aimed at one crossfade: `{clip: the later clip, side: "crossfade", shape}`
//! gives both fades of the pair the chosen shape, over the overlap, in one undo step.

use serde_json::json;
use soundcraft_engine::Engine;
use soundcraft_model::{Clip, ClipId, Fade, FadeShape, Source, SourceId, TrackId};

/// Clip A on 0..1200 and clip B on 1000..3000: a crossfade over 1000..1200, fades of a different
/// shape and bend than the ones the tests ask for.
fn crossfade() -> (Engine, TrackId, ClipId, ClipId) {
    let mut e = Engine::default();
    let t = TrackId(e.execute("track.new", &json!({"count": 1})).unwrap()["tracks"][0].as_u64().unwrap());
    let s = e.session_mut();
    s.sources.push(Source {
        id: SourceId(1),
        name: "src".into(),
        path: "src.wav".into(),
        channels: 1,
        frames: 100_000,
        sample_rate: 48_000,
        format: soundcraft_audio_io::FileFormat::Wav,
        time_reference: 0,
        unsaved: false,
    });
    let (a, b) = (s.new_clip_id(), s.new_clip_id());
    let fade = Fade { len: 200, shape: FadeShape::Linear, curvature: 0.4 };
    let mut ca = Clip::audio(a, "a", SourceId(1), 300, 0, 1200);
    ca.fade_out = fade;
    let mut cb = Clip::audio(b, "b", SourceId(1), 5000, 1000, 2000);
    cb.fade_in = fade;
    s.track_mut(t).and_then(|t| t.playlist_mut()).unwrap().clips = vec![ca, cb];
    (e, t, a, b)
}

fn clip(e: &Engine, id: ClipId) -> Clip {
    e.session().find_clip(id).unwrap().1.clone()
}

#[test]
fn a_crossfade_takes_one_shape_on_both_fades_over_its_overlap_in_one_undo_step() {
    let (mut e, _, a, b) = crossfade();
    let before = e.session().clone();
    let steps = e.undo_history().len();
    assert_eq!(e.execute("edit.fades_create", &json!({"clip": b.0, "side": "crossfade", "shape": "s-curve"})).unwrap(), json!({"fades": 2}));
    let fade = Fade { len: 200, shape: FadeShape::SCurve, curvature: 0.0 };
    let mut expected = before.clone();
    expected.find_clip_mut(a).unwrap().fade_out = fade;
    expected.find_clip_mut(b).unwrap().fade_in = fade;
    assert_eq!(e.session(), &expected, "only the two fades change: the clips, the overlap and the other fades stay");
    assert_eq!(e.undo_history().len(), steps + 1);
    assert!(e.undo());
    assert_eq!(e.session(), &before);
    assert!(e.redo());
    assert_eq!(e.session(), &expected);
}

#[test]
fn each_fade_of_a_crossfade_keeps_its_own_length_and_a_missing_one_takes_the_overlap() {
    let (mut e, _, a, b) = crossfade();
    // Fades that do not match the overlap (Create can leave them so when a clip's file is short) stay as long as they were.
    e.session_mut().find_clip_mut(a).unwrap().fade_out.len = 500;
    e.session_mut().find_clip_mut(b).unwrap().fade_in.len = 50;
    e.execute("edit.fades_create", &json!({"clip": b.0, "side": "crossfade", "shape": "s-curve", "curvature": -0.25})).unwrap();
    let (fo, fi) = (clip(&e, a).fade_out, clip(&e, b).fade_in);
    assert_eq!((fo.len, fi.len), (500, 50));
    assert_eq!((fo.shape, fi.shape, fo.curvature, fi.curvature), (FadeShape::SCurve, FadeShape::SCurve, -0.25, -0.25));
    // Overlapping clips with no fade on one side get one over the overlap.
    let (mut e, _, a, b) = crossfade();
    e.session_mut().find_clip_mut(a).unwrap().fade_out = Fade::default();
    e.execute("edit.fades_create", &json!({"clip": b.0, "side": "crossfade", "shape": "linear"})).unwrap();
    assert_eq!(clip(&e, a).fade_out, Fade { len: 200, shape: FadeShape::Linear, curvature: 0.0 });
    assert_eq!(clip(&e, b).fade_in.len, 200);
}

#[test]
fn a_fade_that_already_has_the_shape_is_left_alone_unless_a_bend_is_asked_for() {
    let (mut e, _, a, b) = crossfade();
    let before = e.session().clone();
    let steps = e.undo_history().len();
    // Both fades are Linear, bent 0.4: asking for Linear changes nothing, and leaves no undo step.
    assert_eq!(e.execute("edit.fades_create", &json!({"clip": b.0, "side": "crossfade", "shape": "linear"})).unwrap(), json!({"fades": 0}));
    assert_eq!(e.session(), &before);
    assert_eq!(e.undo_history().len(), steps);
    // One fade of another shape is the only one that changes.
    e.session_mut().find_clip_mut(a).unwrap().fade_out.shape = FadeShape::Exponential;
    assert_eq!(e.execute("edit.fades_create", &json!({"clip": b.0, "side": "crossfade", "shape": "linear"})).unwrap(), json!({"fades": 1}));
    assert_eq!(clip(&e, a).fade_out, Fade { len: 200, shape: FadeShape::Linear, curvature: 0.0 });
    assert_eq!(clip(&e, b).fade_in.curvature, 0.4);
    // An explicit bend is applied to both.
    assert_eq!(
        e.execute("edit.fades_create", &json!({"clip": b.0, "side": "crossfade", "shape": "linear", "curvature": 0.1})).unwrap(),
        json!({"fades": 2})
    );
    assert_eq!((clip(&e, a).fade_out.curvature, clip(&e, b).fade_in.curvature), (0.1, 0.1));
}

#[test]
fn only_the_later_clip_of_a_crossfade_can_be_aimed_at() {
    let (mut e, _, a, b) = crossfade();
    let before = e.session().clone();
    let steps = e.undo_history().len();
    let mut fails = |p: serde_json::Value| assert!(e.execute("edit.fades_create", &p).is_err(), "{p}");
    fails(json!({"clip": a.0, "side": "crossfade"}));
    fails(json!({"clip": 987_654, "side": "crossfade"}));
    fails(json!({"clip": b.0, "side": "crossfade", "shape": "wobbly"}));
    fails(json!({"clip": b.0, "side": "crossfade", "shape": 7}));
    fails(json!({"clip": b.0, "side": "crossfade", "curvature": 2.0}));
    fails(json!({"clip": b.0, "side": "crossfade", "tracks": [1]}));
    fails(json!({"clip": b.0, "side": "both"}));
    assert_eq!(e.session(), &before);
    assert_eq!(e.undo_history().len(), steps);
    // The right call works, so the refusals above are about the calls.
    e.execute("edit.fades_create", &json!({"clip": b.0, "side": "crossfade"})).unwrap();
    // Two clips that only touch are not a crossfade.
    let (mut e, _, _, b) = crossfade();
    e.session_mut().find_clip_mut(b).unwrap().trim_start_to(1200);
    assert!(e.execute("edit.fades_create", &json!({"clip": b.0, "side": "crossfade"})).is_err());
}

#[test]
fn a_crossfade_takes_an_in_shape_on_the_later_clip_and_an_out_shape_on_the_earlier_one() {
    let (mut e, _, a, b) = crossfade();
    let before = e.session().clone();
    let steps = e.undo_history().len();
    let params =
        json!({"clip": b.0, "side": "crossfade", "shape": "linear", "in_shape": "s-curve equal power", "out_shape": "logarithmic", "curvature": 0});
    assert_eq!(e.execute("edit.fades_create", &params).unwrap(), json!({"fades": 2}));
    assert_eq!(clip(&e, a).fade_out, Fade { len: 200, shape: FadeShape::Logarithmic, curvature: 0.0 });
    assert_eq!(clip(&e, b).fade_in, Fade { len: 200, shape: FadeShape::SCurvePower, curvature: 0.0 });
    assert_eq!(e.undo_history().len(), steps + 1, "both fades in one undo step");
    assert!(e.undo());
    assert_eq!(e.session(), &before);
}

#[test]
fn a_crossfade_with_one_new_shape_leaves_the_other_fade_and_its_bend_alone() {
    let (mut e, _, a, b) = crossfade();
    // Only the fade-in is asked to change: the fade-out already has the shape it is asked for (Linear, bent).
    assert_eq!(
        e.execute("edit.fades_create", &json!({"clip": b.0, "side": "crossfade", "in_shape": "exponential", "out_shape": "linear"})).unwrap(),
        json!({"fades": 1})
    );
    assert_eq!(clip(&e, a).fade_out, Fade { len: 200, shape: FadeShape::Linear, curvature: 0.4 });
    assert_eq!(clip(&e, b).fade_in, Fade { len: 200, shape: FadeShape::Exponential, curvature: 0.0 });
}

#[test]
fn a_crossfade_rejects_a_bad_in_shape_or_out_shape_without_an_edit() {
    let (mut e, _, _, b) = crossfade();
    for extra in [json!({"in_shape": "bad"}), json!({"out_shape": "bad"}), json!({"in_shape": null}), json!({"out_shape": 4})] {
        let mut params = json!({"clip": b.0, "side": "crossfade", "shape": "s-curve"});
        params.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        let (before, history) = (e.session().clone(), e.undo_history());
        assert!(e.execute("edit.fades_create", &params).is_err(), "{params}");
        assert_eq!((e.session(), e.undo_history()), (&before, history), "{params}");
    }
}
