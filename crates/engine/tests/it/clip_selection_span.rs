//! Selecting a clip that is in a crossfade selects it as far as it shows: the two clips of a crossfade are butted at
//! the cut, so the earlier one is selected up to the cut, the later one from it, and one that is crossfaded on both
//! sides between its two cuts. The clip itself stays in `selected_clips`. Clear, Cut and Copy on such a selection act on
//! the clips whole, and what is left of their crossfade partners is rejoined at the cuts.

use serde_json::json;
use soundcraft_engine::{Engine, edit};
use soundcraft_model::{AutoParam, Clip, ClipId, Fade, FadeShape, Source, SourceId, TrackId};
use soundcraft_time::Range;

fn source(id: u64) -> Source {
    Source {
        id: SourceId(id),
        name: "src".into(),
        path: "src.wav".into(),
        channels: 1,
        frames: 50_000,
        sample_rate: 48_000,
        format: soundcraft_audio_io::FileFormat::Wav,
        time_reference: 0,
        unsaved: false,
    }
}

/// A on 0..1200, B on 1000..3000, C on 2800..4800: A crossfades into B over 1000..1200 with its cut at 1050, and B
/// into C over 2800..3000 with its cut at 2900. So B is crossfaded on both sides.
fn chain() -> (Engine, TrackId, ClipId, ClipId, ClipId) {
    let mut e = Engine::default();
    let t = TrackId(e.execute("track.new", &json!({"count": 1})).unwrap()["tracks"][0].as_u64().unwrap());
    let s = e.session_mut();
    s.sources.push(source(1));
    s.sources.push(source(2));
    let (a, b, c) = (s.new_clip_id(), s.new_clip_id(), s.new_clip_id());
    let fade = |len| Fade { len, shape: FadeShape::EqualPower, curvature: 0.0 };
    let mut ca = Clip::audio(a, "a", SourceId(1), 300, 0, 1200);
    ca.fade_out = fade(200);
    let mut cb = Clip::audio(b, "b", SourceId(2), 5000, 1000, 2000);
    cb.fade_in = fade(200);
    cb.fade_out = fade(200);
    cb.cut = Some(50);
    let mut cc = Clip::audio(c, "c", SourceId(2), 9000, 2800, 2000);
    cc.fade_in = fade(200);
    cc.cut = Some(100);
    s.track_mut(t).and_then(|t| t.playlist_mut()).unwrap().clips = vec![ca, cb, cc];
    (e, t, a, b, c)
}

fn select(e: &mut Engine, ids: &[ClipId]) {
    let ids: Vec<u64> = ids.iter().map(|c| c.0).collect();
    e.execute("edit.select", &json!({"clips": ids})).unwrap();
}

fn selection(e: &Engine) -> Range {
    e.session().edit.selection
}

#[test]
fn the_earlier_clip_is_selected_up_to_its_cut() {
    let (mut e, t, a, _, _) = chain();
    select(&mut e, &[a]);
    assert_eq!(selection(&e), Range::new(0, 1050), "A's own end (1200) is under the zone");
    assert_eq!(e.session().edit.selected_clips, vec![a], "the clip itself stays selected");
    assert_eq!(e.session().edit.selected_tracks, vec![t]);
    assert_eq!(e.session().edit.selected_crossfade, None);
}

#[test]
fn the_later_clip_is_selected_from_its_cut() {
    let (mut e, _, _, _, c) = chain();
    select(&mut e, &[c]);
    assert_eq!(selection(&e), Range::new(2900, 4800), "C's own start (2800) is under the zone");
    assert_eq!(e.session().edit.selected_clips, vec![c]);
}

#[test]
fn a_clip_crossfaded_on_both_sides_is_selected_between_its_two_cuts() {
    let (mut e, _, _, b, _) = chain();
    select(&mut e, &[b]);
    assert_eq!(selection(&e), Range::new(1050, 2900));
    assert_eq!(e.session().edit.selected_clips, vec![b]);
}

#[test]
fn the_cut_is_where_the_crossfade_has_it_even_when_none_is_stored() {
    // No stored cut: the middle of each zone.
    let (mut e, t, a, b, c) = chain();
    for clip in e.session_mut().track_mut(t).and_then(|t| t.playlist_mut()).unwrap().clips.iter_mut() {
        clip.cut = None;
    }
    select(&mut e, &[a]);
    assert_eq!(selection(&e), Range::new(0, 1100));
    select(&mut e, &[b]);
    assert_eq!(selection(&e), Range::new(1100, 2900));
    select(&mut e, &[c]);
    assert_eq!(selection(&e), Range::new(2900, 4800));
}

#[test]
fn several_clips_are_selected_over_the_union_of_what_shows_of_each() {
    let (mut e, _, a, b, c) = chain();
    select(&mut e, &[a, b]);
    assert_eq!(selection(&e), Range::new(0, 2900));
    select(&mut e, &[b, c]);
    assert_eq!(selection(&e), Range::new(1050, 4800));
    select(&mut e, &[c, a]);
    assert_eq!(selection(&e), Range::new(0, 4800), "the gap between them is in the range, as ever");
    assert_eq!(e.session().edit.selected_clips, vec![c, a], "in the order given");
}

#[test]
fn clips_that_are_not_in_a_crossfade_are_selected_whole() {
    let mut e = Engine::default();
    let t = TrackId(e.execute("track.new", &json!({"count": 1})).unwrap()["tracks"][0].as_u64().unwrap());
    let s = e.session_mut();
    s.sources.push(source(1));
    let (a, b, c) = (s.new_clip_id(), s.new_clip_id(), s.new_clip_id());
    // Butted, and apart: no crossfade.
    let clips = vec![
        Clip::audio(a, "a", SourceId(1), 0, 0, 1000),
        Clip::audio(b, "b", SourceId(1), 0, 1000, 1000),
        Clip::audio(c, "c", SourceId(1), 0, 5000, 2000),
    ];
    s.track_mut(t).and_then(|t| t.playlist_mut()).unwrap().clips = clips;
    for (ids, want) in [(vec![a], (0, 1000)), (vec![b], (1000, 2000)), (vec![c], (5000, 7000)), (vec![a, c], (0, 7000))] {
        select(&mut e, &ids);
        assert_eq!(selection(&e), Range::new(want.0, want.1), "{ids:?}");
    }
    let c = e.session().find_clip(a).unwrap().1.clone();
    let clips = e.session().track(t).unwrap().clips();
    assert_eq!(edit::clip_visible_span(clips, &c), c.range());
}

#[test]
fn the_helper_says_what_shows_of_each_clip() {
    let (e, t, a, b, c) = chain();
    let clips = e.session().track(t).unwrap().clips();
    let span = |id: ClipId| edit::clip_visible_span(clips, clips.iter().find(|x| x.id == id).unwrap());
    assert_eq!((span(a), span(b), span(c)), (Range::new(0, 1050), Range::new(1050, 2900), Range::new(2900, 4800)));
    // The spans of a chain tile it with no gap and no overlap.
    assert_eq!((span(a).end, span(b).start, span(b).end, span(c).start), (1050, 1050, 2900, 2900));
}

/// A on `base`..`base`+1100 (fade-in 50 at its far end), B on `base`+900..`base`+2000 (fade-out 60 at its far end),
/// crossfaded over 200 with the cut at `base`+1000; and D, a plain clip, further on.
fn pair(base: i64) -> (Engine, TrackId, ClipId, ClipId, ClipId) {
    let mut e = Engine::default();
    let t = TrackId(e.execute("track.new", &json!({"count": 1})).unwrap()["tracks"][0].as_u64().unwrap());
    let s = e.session_mut();
    s.sources.push(source(1));
    s.sources.push(source(2));
    let (a, b, d) = (s.new_clip_id(), s.new_clip_id(), s.new_clip_id());
    let fade = |len| Fade { len, shape: FadeShape::EqualPower, curvature: 0.0 };
    let mut ca = Clip::audio(a, "a", SourceId(1), 300, base, 1100);
    ca.fade_in = fade(50);
    ca.fade_out = fade(200);
    let mut cb = Clip::audio(b, "b", SourceId(2), 5000, base + 900, 1100);
    cb.fade_in = fade(200);
    cb.fade_out = fade(60);
    cb.cut = Some(100);
    let cd = Clip::audio(d, "d", SourceId(2), 0, base + 5000, 1000);
    s.track_mut(t).and_then(|t| t.playlist_mut()).unwrap().clips = vec![ca, cb, cd];
    (e, t, a, b, d)
}

fn clip(e: &Engine, id: ClipId) -> Clip {
    e.session().find_clip(id).unwrap().1.clone()
}

fn layout(e: &Engine, t: TrackId) -> Vec<(ClipId, i64, i64)> {
    e.session().track(t).unwrap().clips().iter().map(|c| (c.id, c.start, c.end())).collect()
}

/// Where a clip's file sits on the timeline: rejoining a partner must not move its audio.
fn anchor(c: &Clip) -> i64 {
    c.start - c.source_offset()
}

#[test]
fn clearing_the_earlier_clip_removes_it_and_rejoins_the_later_one_at_the_cut() {
    let (mut e, t, a, b, d) = pair(0);
    let (b0, d0) = (clip(&e, b), clip(&e, d));
    select(&mut e, &[a]);
    assert_eq!(selection(&e), Range::new(0, 1000));
    let selected = e.session().clone();
    let steps = e.undo_history().len();
    assert_eq!(e.execute("edit.clear", &json!({})).unwrap(), json!({"removed": 1}));
    assert!(e.session().find_clip(a).is_none(), "A and its crossfade are gone");
    let b1 = clip(&e, b);
    // B keeps all that showed of it, [1000, 2000): it starts at the cut, and its hidden head is dropped from view.
    assert_eq!((b1.start, b1.end()), (1000, 2000));
    assert_eq!(anchor(&b1), anchor(&b0), "its audio stayed where it was");
    assert_eq!((b1.fade_in.len, b1.cut), (0, None));
    assert_eq!(b1.fade_out, b0.fade_out, "its other fade stays");
    assert_eq!(clip(&e, d), d0, "nothing else is touched");
    assert_eq!(layout(&e, t).len(), 2);
    // One undo step, and the crossfade is back as it was.
    assert_eq!(e.undo_history().len(), steps + 1);
    assert!(e.undo());
    assert_eq!(e.session(), &selected);
}

#[test]
fn clearing_the_later_clip_removes_it_and_rejoins_the_earlier_one_at_the_cut() {
    let (mut e, _, a, b, d) = pair(0);
    let (a0, d0) = (clip(&e, a), clip(&e, d));
    select(&mut e, &[b]);
    assert_eq!(selection(&e), Range::new(1000, 2000));
    let selected = e.session().clone();
    let steps = e.undo_history().len();
    e.execute("edit.clear", &json!({})).unwrap();
    assert!(e.session().find_clip(b).is_none());
    let a1 = clip(&e, a);
    assert_eq!((a1.start, a1.end()), (0, 1000), "A ends at the cut: all that showed of it is kept");
    assert_eq!(anchor(&a1), anchor(&a0));
    assert_eq!(a1.fade_out.len, 0);
    assert_eq!(a1.fade_in, a0.fade_in, "its other fade stays");
    assert_eq!(clip(&e, d), d0);
    assert_eq!(e.undo_history().len(), steps + 1);
    assert!(e.undo());
    assert_eq!(e.session(), &selected);
}

#[test]
fn both_clips_of_a_crossfade_selected_are_both_removed() {
    let (mut e, t, a, b, d) = pair(0);
    let d0 = clip(&e, d);
    select(&mut e, &[a, b]);
    assert_eq!(selection(&e), Range::new(0, 2000));
    let selected = e.session().clone();
    assert_eq!(e.execute("edit.clear", &json!({})).unwrap(), json!({"removed": 2}));
    assert_eq!(layout(&e, t), vec![(d, 5000, 6000)]);
    assert_eq!(clip(&e, d), d0);
    assert!(e.undo());
    assert_eq!(e.session(), &selected);
}

#[test]
fn a_partner_crossfaded_on_both_sides_is_rejoined_on_both() {
    let (mut e, t, a, b, c) = chain();
    // A and C go; B, between them and inside their range, stays.
    select(&mut e, &[a, c]);
    assert_eq!(selection(&e), Range::new(0, 4800));
    let b0 = clip(&e, b);
    e.execute("edit.clear", &json!({})).unwrap();
    assert_eq!(layout(&e, t).len(), 1);
    let b1 = clip(&e, b);
    assert_eq!((b1.start, b1.end()), (1050, 2900), "from one cut to the other");
    assert_eq!(anchor(&b1), anchor(&b0));
    assert_eq!((b1.fade_in.len, b1.fade_out.len, b1.cut), (0, 0, None));
}

#[test]
fn cutting_puts_exactly_the_removed_clips_in_the_clipboard() {
    for (base, select_b) in [(0, false), (500, false), (500, true)] {
        let (mut e, t, a, b, _) = pair(base);
        let (a0, b0) = (clip(&e, a), clip(&e, b));
        let (picked, kept, whole) = if select_b { (b, a, b0.clone()) } else { (a, b, a0.clone()) };
        select(&mut e, &[picked]);
        let before = layout(&e, t);
        let selected = e.session().clone();
        let steps = e.undo_history().len();
        let out = e.execute("edit.cut", &json!({})).unwrap();
        assert_eq!(out["clips"], 1, "{base} {select_b}");
        // The clipboard holds the clip whole, hidden handles and fades as they were, from the start of the clip.
        let mut want = whole.clone();
        want.start = 0;
        assert_eq!(e.clipboard.tracks, vec![vec![want]], "{base} {select_b}");
        assert_eq!(e.clipboard.length, whole.length, "{base} {select_b}");
        // The clip is gone and its partner rejoined at the cut, exactly as Clear does it.
        assert!(e.session().find_clip(picked).is_none());
        let partner = clip(&e, kept);
        let cut = base + 1000;
        let expected = if select_b { (base, cut) } else { (cut, base + 2000) };
        assert_eq!((partner.start, partner.end()), expected, "{base} {select_b}");
        assert_eq!(e.undo_history().len(), steps + 1);
        assert!(e.undo());
        assert_eq!(e.session(), &selected);
        assert_eq!(layout(&e, t), before);
    }
}

#[test]
fn copying_copies_the_selected_clips_only() {
    let (mut e, t, a, b, _) = pair(500);
    let (a0, b0) = (clip(&e, a), clip(&e, b));
    // The earlier clip alone, then the later one alone, then both.
    for (ids, want) in [(vec![a], vec![0]), (vec![b], vec![0]), (vec![a, b], vec![0, 900])] {
        select(&mut e, &ids);
        let selected = e.session().clone();
        e.execute("edit.copy", &json!({})).unwrap();
        let starts: Vec<i64> = e.clipboard.tracks[0].iter().map(|c| c.start).collect();
        assert_eq!(starts, want, "{ids:?}");
        let copied: Vec<ClipId> = e.clipboard.tracks[0].iter().map(|c| c.id).collect();
        assert_eq!(copied, ids, "{ids:?}: the clips selected, whole and no other");
        assert_eq!(
            e.clipboard.tracks[0].iter().map(|c| c.length).collect::<Vec<_>>(),
            ids.iter().map(|i| if *i == a { a0.length } else { b0.length }).collect::<Vec<_>>()
        );
        assert_eq!(e.session(), &selected, "a copy changes nothing");
    }
    let _ = t;
}

#[test]
fn a_plain_clip_is_cleared_and_cut_as_before() {
    for command in ["edit.cut", "edit.clear"] {
        let (mut by_clip, t, _, _, d) = pair(0);
        select(&mut by_clip, &[d]);
        assert_eq!(selection(&by_clip), Range::new(5000, 6000));
        assert!(edit::crossfaded_selection(by_clip.session()).is_none());
        let (mut by_range, _, _, _, _) = pair(0);
        by_range.execute("edit.select", &json!({"tracks": [t.0], "start": 5000, "end": 6000})).unwrap();
        let (r1, r2) = (by_clip.execute(command, &json!({})).unwrap(), by_range.execute(command, &json!({})).unwrap());
        assert_eq!(r1, r2, "{command}");
        assert_eq!(layout(&by_clip, t), layout(&by_range, t), "{command}");
        assert_eq!(by_clip.clipboard.length, by_range.clipboard.length, "{command}");
        assert_eq!(layout(&by_clip, t).len(), 2, "{command}: the pair is still there");
    }
}

#[test]
fn any_other_selection_keeps_the_range_behaviour() {
    // The same range as A's span, typed: no clip is selected, so it is a range like any other.
    let (mut e, t, a, _, _) = pair(0);
    e.execute("edit.select", &json!({"tracks": [t.0], "start": 0, "end": 1000})).unwrap();
    assert!(edit::crossfaded_selection(e.session()).is_none());
    e.execute("edit.clear", &json!({})).unwrap();
    // Cleared as a range: what A had past the cut stays, a clip of its own from the cut to A's end.
    assert!(layout(&e, t).iter().any(|&(_, start, end)| (start, end) == (1000, 1100)));
    assert!(e.session().find_clip(a).is_none());
    // A clip selection that was moved on (Move Edit Right) is no longer what shows of the clip: a range, copied as it stands.
    let (mut moved, _, a, _, _) = pair(0);
    select(&mut moved, &[a]);
    moved.run("edit.move_selection_right").unwrap();
    assert_eq!(selection(&moved), Range::new(1000, 2000));
    assert!(edit::crossfaded_selection(moved.session()).is_none());
    moved.execute("edit.copy", &json!({})).unwrap();
    assert_eq!(moved.clipboard.length, 1000);
    // Explicit parameters are a range of their own, whatever is selected.
    let (mut explicit, t, a, _, _) = pair(0);
    select(&mut explicit, &[a]);
    explicit.execute("edit.copy", &json!({"tracks": [t.0], "start": 0, "end": 500})).unwrap();
    assert_eq!(explicit.clipboard.length, 500);
}

#[test]
fn a_partner_that_cannot_be_rejoined_leaves_the_selection_a_range() {
    for how in ["edit-locked", "stretched"] {
        let (mut e, _, a, b, _) = pair(0);
        let c = e.session_mut().find_clip_mut(b).unwrap();
        if how == "stretched" {
            c.stretch = 1.5;
        } else {
            c.edit_locked = true;
        }
        select(&mut e, &[a]);
        assert!(edit::crossfaded_selection(e.session()).is_none(), "{how}");
    }
}

fn points(e: &Engine, t: TrackId) -> Vec<i64> {
    e.session().track(t).unwrap().lane(&AutoParam::Volume).map(|l| l.points.iter().map(|p| p.at).collect()).unwrap_or_default()
}

/// `pair(base)` in the given edit mode, with volume automation at `base` + 500 (under A), 1500 (under B), 5500 (under D)
/// and 8000 (past everything).
fn with_automation(mode: &str, base: i64) -> (Engine, TrackId, ClipId, ClipId, ClipId) {
    let (mut e, t, a, b, d) = pair(base);
    e.execute("edit.mode", &json!({"mode": mode})).unwrap();
    let lane = e.session_mut().track_mut(t).unwrap().lane_mut(&AutoParam::Volume);
    for at in [500, 1500, 5500, 8000] {
        lane.set_point(base + at, -6.0);
    }
    (e, t, a, b, d)
}

#[test]
fn in_shuffle_removing_crossfaded_clips_closes_the_gap_and_the_automation_follows() {
    // What shows of A is [0, 1000), of B [1000, 2000): the gap is what shows of the clips removed. D (5000..6000) and the
    // points after it move left by its length, the points inside it go, and the partner that stays is rejoined first, so
    // it slides back to where the gap began. Cut and Clear do the same.
    type Case<'a> = (&'a [&'a str], &'a [(&'a str, i64, i64)], &'a [i64], i64);
    let cases: [Case; 3] = [
        (&["a"], &[("b", 0, 1000), ("d", 4000, 5000)], &[500, 4500, 7000], 0),
        (&["b"], &[("a", 0, 1000), ("d", 4000, 5000)], &[500, 4500, 7000], 1000),
        (&["a", "b"], &[("d", 3000, 4000)], &[3500, 6000], 0),
    ];
    for command in ["edit.clear", "edit.cut"] {
        for base in [0, 500] {
            for (picked, left, want_points, caret) in cases {
                let why = format!("{command} {picked:?} from {base}");
                let (mut e, t, a, b, d) = with_automation("shuffle", base);
                let id = |n: &str| match n {
                    "a" => a,
                    "b" => b,
                    _ => d,
                };
                select(&mut e, &picked.iter().map(|n| id(n)).collect::<Vec<_>>());
                let selected = e.session().clone();
                let steps = e.undo_history().len();
                e.execute(command, &json!({})).unwrap();
                assert_eq!(layout(&e, t), left.iter().map(|&(n, s, f)| (id(n), base + s, base + f)).collect::<Vec<_>>(), "{why}");
                assert_eq!(points(&e, t), want_points.iter().map(|p| base + p).collect::<Vec<_>>(), "{why}");
                assert_eq!(selection(&e), Range::point(base + caret), "{why}: the insertion point stays where the selection began");
                // One undo step brings everything back: clips, automation, selection.
                assert_eq!(e.undo_history().len(), steps + 1, "{why}");
                assert!(e.undo(), "{why}");
                assert_eq!(e.session(), &selected, "{why}");
            }
        }
    }
}

#[test]
fn in_shuffle_a_partner_that_stays_between_two_removed_clips_closes_up_on_both_sides() {
    // A (0..1050 shown) and C (2900..4800 shown) go, B stays between them and is rejoined at both cuts: 1050..2900. E, past
    // C, follows C's gap; then B and E follow A's. The gaps close from the last, so each is still where it was read.
    let (mut e, t, a, b, c) = chain();
    e.execute("edit.mode", &json!({"mode": "shuffle"})).unwrap();
    let id = e.session_mut().new_clip_id();
    let clips = e.session_mut().track_mut(t).and_then(|t| t.playlist_mut()).unwrap();
    clips.clips.push(Clip::audio(id, "e", SourceId(2), 0, 6000, 1000));
    let lane = e.session_mut().track_mut(t).unwrap().lane_mut(&AutoParam::Volume);
    for at in [500, 2000, 4000, 6500] {
        lane.set_point(at, -6.0);
    }
    select(&mut e, &[a, c]);
    assert_eq!(selection(&e), Range::new(0, 4800));
    let selected = e.session().clone();
    e.execute("edit.clear", &json!({})).unwrap();
    assert_eq!(layout(&e, t), vec![(b, 0, 1850), (id, 3050, 4050)]);
    // 500 and 4000 were inside the gaps; 2000 (under B) and 6500 (under E) moved with their clips.
    assert_eq!(points(&e, t), vec![950, 3550]);
    assert!(e.undo());
    assert_eq!(e.session(), &selected);
}

#[test]
fn outside_shuffle_removing_crossfaded_clips_leaves_what_follows_where_it_is() {
    for (command, left) in [("edit.clear", vec![500, 1500, 5500, 8000]), ("edit.cut", vec![1500, 5500, 8000])] {
        let (mut e, t, a, b, d) = with_automation("slip", 0);
        select(&mut e, &[a]);
        e.execute(command, &json!({})).unwrap();
        // B is rejoined at the cut and stays there; D is where it was. A cut also takes the automation it shows (as ever).
        assert_eq!(layout(&e, t), vec![(b, 1000, 2000), (d, 5000, 6000)], "{command}");
        assert_eq!(points(&e, t), left, "{command}");
        assert_eq!(selection(&e), Range::point(0), "{command}");
    }
}

#[test]
fn a_partner_that_stays_between_two_removed_clips_keeps_the_automation_under_it() {
    // A and C go, B stays between them (rejoined at both cuts). The selection is 0..4800, but only what showed of A
    // (0..1050) and of C (2900..4800) goes with a cut: the point under B (2000) stays, and in Shuffle follows B.
    // Clear never touched the automation outside Shuffle (and in Shuffle it is the existing test above).
    type Case<'a> = (&'a str, &'a str, (i64, i64), &'a [i64]);
    let cases: [Case; 3] = [
        ("slip", "edit.clear", (1050, 2900), &[500, 2000, 4000]),
        ("slip", "edit.cut", (1050, 2900), &[2000]),
        ("shuffle", "edit.cut", (0, 1850), &[950]),
    ];
    for (mode, command, (start, end), want) in cases {
        let (mut e, t, a, b, c) = chain();
        e.execute("edit.mode", &json!({"mode": mode})).unwrap();
        let lane = e.session_mut().track_mut(t).unwrap().lane_mut(&AutoParam::Volume);
        for at in [500, 2000, 4000] {
            lane.set_point(at, -6.0);
        }
        select(&mut e, &[a, c]);
        assert_eq!(selection(&e), Range::new(0, 4800), "{mode} {command}");
        e.execute(command, &json!({})).unwrap();
        assert_eq!(layout(&e, t), vec![(b, start, end)], "{mode} {command}");
        assert_eq!(points(&e, t), want, "{mode} {command}");
    }
}

#[test]
fn clips_removed_around_a_crossfaded_partner_and_a_gap_keep_the_automation_under_both() {
    // A (shows 0..1000) and D (5000..6000) go; B (rejoined at 1000..2000) and the gap between them stay. Points: 500 under
    // A, 1500 under B, 3000 in the gap, 5500 under D, 8000 past everything. A cut takes the first and the fourth only.
    for (command, want) in [("edit.clear", vec![500, 1500, 3000, 5500, 8000]), ("edit.cut", vec![1500, 3000, 8000])] {
        let (mut e, t, a, b, d) = with_automation("slip", 0);
        e.session_mut().track_mut(t).unwrap().lane_mut(&AutoParam::Volume).set_point(3000, -6.0);
        select(&mut e, &[a, d]);
        assert_eq!(selection(&e), Range::new(0, 6000), "{command}");
        e.execute(command, &json!({})).unwrap();
        assert_eq!(layout(&e, t), vec![(b, 1000, 2000)], "{command}");
        assert_eq!(points(&e, t), want, "{command}");
        assert_eq!(selection(&e), Range::point(0), "{command}");
    }
}

#[test]
fn in_shuffle_a_locked_clip_stops_the_gap_and_the_automation_needs_automation_follows_edit() {
    // D locked: B slides back, D stays, and so do the points from D on.
    let (mut e, t, a, b, d) = with_automation("shuffle", 0);
    e.session_mut().find_clip_mut(d).unwrap().time_locked = true;
    select(&mut e, &[a]);
    e.execute("edit.clear", &json!({})).unwrap();
    assert_eq!(layout(&e, t), vec![(b, 0, 1000), (d, 5000, 6000)]);
    assert_eq!(points(&e, t), vec![500, 5500, 8000]);
    // Without Automation Follows Edit the clips close up and the points stay.
    let (mut e, t, a, b, d) = with_automation("shuffle", 0);
    e.session_mut().edit.automation_follows_edit = false;
    select(&mut e, &[a]);
    e.execute("edit.clear", &json!({})).unwrap();
    assert_eq!(layout(&e, t), vec![(b, 0, 1000), (d, 4000, 5000)]);
    assert_eq!(points(&e, t), vec![500, 1500, 5500, 8000]);
}

/// `pair(0)` and, on a second track, the same crossfade again (A2, B2); volume automation at 500, 1500 and 3000 on both.
fn two_pairs() -> (Engine, [TrackId; 2], [ClipId; 4]) {
    let (mut e, t1, a1, b1, _) = pair(0);
    let t2 = TrackId(e.execute("track.new", &json!({"count": 1})).unwrap()["tracks"][0].as_u64().unwrap());
    let s = e.session_mut();
    let (a2, b2) = (s.new_clip_id(), s.new_clip_id());
    let copies: Vec<Clip> = [(a1, a2), (b1, b2)]
        .iter()
        .map(|&(from, id)| {
            let mut c = s.find_clip(from).unwrap().1.clone();
            c.id = id;
            c
        })
        .collect();
    s.track_mut(t2).and_then(|t| t.playlist_mut()).unwrap().clips = copies;
    for t in [t1, t2] {
        let lane = s.track_mut(t).unwrap().lane_mut(&AutoParam::Volume);
        for at in [500, 1500, 3000] {
            lane.set_point(at, -6.0);
        }
    }
    (e, [t1, t2], [a1, b1, a2, b2])
}

#[test]
fn narrowing_the_tracks_leaves_the_clips_of_the_other_track_out_of_a_crossfaded_cut_or_clear() {
    // Clips in a crossfade on two tracks, then the selection narrowed to the first track: what the screen shows is that
    // range on that track alone, so Cut and Clear act on it as a range and the clips of the other track are not touched.
    for mode in ["slip", "shuffle"] {
        for command in ["edit.cut", "edit.clear"] {
            let why = format!("{mode} {command}");
            let setup = || {
                let (mut e, tracks, ids) = two_pairs();
                e.execute("edit.mode", &json!({"mode": mode})).unwrap();
                (e, tracks, ids)
            };
            let (mut e, [t1, t2], ids) = setup();
            select(&mut e, &ids);
            assert!(edit::crossfaded_selection(e.session()).is_some(), "{why}: both tracks are selected");
            e.execute("edit.select", &json!({"tracks": [t1.0]})).unwrap();
            assert_eq!(selection(&e), Range::new(0, 2000), "{why}");
            let (other, other_layout, other_points) = (e.session().track(t2).cloned(), layout(&e, t2), points(&e, t2));
            // The same range typed on the first track.
            let (mut by_range, _, _) = setup();
            by_range.execute("edit.select", &json!({"tracks": [t1.0], "start": 0, "end": 2000})).unwrap();
            let (r1, r2) = (e.execute(command, &json!({})).unwrap(), by_range.execute(command, &json!({})).unwrap());
            assert_eq!(layout(&e, t2), other_layout, "{why}: the other track keeps its clips");
            assert_eq!(points(&e, t2), other_points, "{why}: and its automation");
            assert_eq!(e.session().track(t2), other.as_ref(), "{why}: and the rest of it");
            assert_eq!((r1, e.session().track(t1)), (r2, by_range.session().track(t1)), "{why}: the first track is cut as a range");
            assert_eq!(e.clipboard.tracks, by_range.clipboard.tracks, "{why}");
        }
    }
}
