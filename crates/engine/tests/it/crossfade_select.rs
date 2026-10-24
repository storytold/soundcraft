//! `edit.select {crossfade}`: a crossfade is selected as itself, its zone on its track and no clip, and the
//! selection remembers which crossfade it is. Any other selection drops that; a slide or a trim of the selected
//! crossfade takes the selection along; deleting its fades leaves the clips butted at the cut, in one undo step.

use serde_json::json;
use soundcraft_engine::{Engine, edit};
use soundcraft_model::{Clip, ClipId, Fade, FadeShape, Source, SourceId, TrackId, crossfade_cut};
use soundcraft_time::Range;

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

/// Clip A on 0..1200 (200 of it under B), clip B on 1000..3000, crossfaded over 1000..1200, cut at 1050.
fn crossfade() -> (Engine, TrackId, ClipId, ClipId) {
    let mut e = Engine::default();
    let t = TrackId(e.execute("track.new", &json!({"count": 1})).unwrap()["tracks"][0].as_u64().unwrap());
    let s = e.session_mut();
    s.sources.push(source(1, 20_000));
    s.sources.push(source(2, 20_000));
    let (a, b) = (s.new_clip_id(), s.new_clip_id());
    let fade = Fade { len: XFADE, shape: FadeShape::EqualPower, curvature: 0.0 };
    let mut ca = Clip::audio(a, "a", SourceId(1), 300, 0, 1200);
    ca.fade_out = fade;
    let mut cb = Clip::audio(b, "b", SourceId(2), 5000, 1000, 2000);
    cb.fade_in = fade;
    cb.cut = Some(50);
    s.track_mut(t).and_then(|t| t.playlist_mut()).unwrap().clips = vec![ca, cb];
    (e, t, a, b)
}

fn clip(e: &Engine, id: ClipId) -> Clip {
    e.session().find_clip(id).unwrap().1.clone()
}

fn select(e: &mut Engine, b: ClipId) {
    e.execute("edit.select", &json!({"crossfade": b.0})).unwrap();
}

/// The crossfade the selection is, by its later clip.
fn selected(e: &Engine) -> Option<ClipId> {
    edit::selected_crossfade(e.session()).map(|(later, ..)| later)
}

#[test]
fn selecting_a_crossfade_selects_its_zone_on_its_track_and_no_clip() {
    let (mut e, t, a, b) = crossfade();
    // A clip and a Clip List file selected first: the crossfade replaces them.
    e.execute("edit.select", &json!({"clips": [a.0], "sources": [1]})).unwrap();
    assert_eq!(e.session().edit.selected_clips, vec![a]);
    let steps = e.undo_history().len();
    let out = e.execute("edit.select", &json!({"crossfade": b.0})).unwrap();
    let ed = &e.session().edit;
    assert_eq!(ed.selection, Range::new(1000, 1200), "the zone: B's start to A's end");
    assert_eq!(ed.selected_tracks, vec![t]);
    assert!(ed.selected_clips.is_empty() && ed.selected_sources.is_empty());
    assert_eq!(ed.selected_crossfade, Some(b));
    assert_eq!(out["crossfade"], json!(b.0));
    assert_eq!(selected(&e), Some(b));
    // Linked, the timeline selection follows; a selection is no undo step.
    assert_eq!(ed.timeline_selection, ed.selection);
    assert_eq!(e.undo_history().len(), steps);
    // The scriptable state says so too.
    assert_eq!(soundcraft_engine::inspect::session(&e, false)["selection"]["crossfade"], json!(b.0));
}

#[test]
fn the_timeline_selection_stays_when_it_is_not_linked() {
    let (mut e, _, _, b) = crossfade();
    e.execute("edit.select", &json!({"tracks": [e.session().tracks[0].id.0], "start": 5, "end": 9})).unwrap();
    e.session_mut().edit.link_timeline_edit = false;
    let timeline = e.session().edit.timeline_selection;
    select(&mut e, b);
    assert_eq!(e.session().edit.timeline_selection, timeline);
    assert_eq!(e.session().edit.selection, Range::new(1000, 1200));
}

#[test]
fn only_the_later_clip_of_a_crossfade_can_be_named() {
    let (mut e, t, a, b) = crossfade();
    let before = e.session().clone();
    for params in [
        json!({"crossfade": a.0}),             // the earlier clip
        json!({"crossfade": 9999}),            // no such clip
        json!({"crossfade": "b"}),             // not an id
        json!({"crossfade": b.0, "start": 0}), // stands alone
        json!({"crossfade": b.0, "tracks": [t.0]}),
        json!({"crossfade": b.0, "clips": [b.0]}),
    ] {
        assert!(e.execute("edit.select", &params).is_err(), "{params}");
        assert_eq!(e.session(), &before, "{params}: nothing changed");
    }
    // Butted clips are no crossfade.
    e.session_mut().track_mut(t).and_then(|t| t.playlist_mut()).unwrap().clips[1].trim_start_to(1200);
    assert!(e.execute("edit.select", &json!({"crossfade": b.0})).is_err());
}

/// Select something else, whichever way.
fn select_otherwise(e: &mut Engine, how: &str) {
    let t = e.session().tracks[0].id.0;
    let first_clip = e.session().tracks[0].clips()[0].id.0;
    let (id, params) = match how {
        "a range" => ("edit.select", json!({"tracks": [t], "start": 0, "end": 5000})),
        "the same range, typed" => ("edit.select", json!({"tracks": [t], "start": 1000, "end": 1200})),
        "an insertion point" => ("edit.select", json!({"start": 3})),
        "a clip" => ("edit.select", json!({"clips": [first_clip]})),
        "a file" => ("edit.select", json!({"sources": [1]})),
        "all" => ("edit.select_all", json!({})),
        _ => ("edit.select_none", json!({})),
    };
    e.execute(id, &params).unwrap();
}

#[test]
fn any_other_selection_drops_the_crossfade() {
    for how in ["a range", "the same range, typed", "an insertion point", "a clip", "a file", "all", "none"] {
        let (mut e, _, _, b) = crossfade();
        select(&mut e, b);
        assert_eq!(selected(&e), Some(b));
        select_otherwise(&mut e, how);
        assert_eq!(e.session().edit.selected_crossfade, None, "{how}");
        assert_eq!(selected(&e), None, "{how}");
    }
}

#[test]
fn a_stale_crossfade_is_not_selected() {
    // The pair reshaped by another edit: the zone no longer is the selection.
    let (mut e, _, _, b) = crossfade();
    select(&mut e, b);
    e.session_mut().find_clip_mut(b).unwrap().trim_start_to(1100);
    assert_eq!(selected(&e), None);
    // The clip gone.
    let (mut e, t, _, b) = crossfade();
    select(&mut e, b);
    e.session_mut().track_mut(t).and_then(|t| t.playlist_mut()).unwrap().clips.retain(|c| c.id != b);
    assert_eq!(selected(&e), None);
    // A clip selected on top of it.
    let (mut e, _, a, b) = crossfade();
    select(&mut e, b);
    e.session_mut().edit.selected_clips = vec![a];
    assert_eq!(selected(&e), None);
}

#[test]
fn deleting_the_selected_crossfade_butts_the_clips_at_the_cut_and_undo_brings_it_back() {
    let (mut e, t, a, b) = crossfade();
    let (a0, b0) = (clip(&e, a), clip(&e, b));
    assert_eq!(crossfade_cut(&a0, &b0), 1050);
    select(&mut e, b);
    let selected_state = e.session().clone();
    let steps = e.undo_history().len();
    e.execute("edit.fades_delete", &json!({"tracks": [t.0], "start": 1000, "end": 1200})).unwrap();
    let (a1, b1) = (clip(&e, a), clip(&e, b));
    assert_eq!((a1.end(), b1.start), (1050, 1050), "butted at the cut");
    assert_eq!((a1.fade_out.len, b1.fade_in.len, b1.cut), (0, 0, None));
    assert_eq!((a1.start, b1.end()), (a0.start, b0.end()), "the outer edges stay");
    assert_eq!((a1.start - a1.source_offset(), b1.start - b1.source_offset()), (a0.start - a0.source_offset(), b0.start - b0.source_offset()));
    // What is left selected is the cut, not the old zone as a range a second Delete would clear.
    let ed = &e.session().edit;
    assert_eq!((ed.selection, ed.selected_crossfade), (Range::point(1050), None));
    assert_eq!(ed.timeline_selection, ed.selection);
    // One step, and undoing it brings the crossfade back, selected, as it was.
    assert_eq!(e.undo_history().len(), steps + 1);
    assert!(e.undo());
    assert_eq!(e.session(), &selected_state);
    assert_eq!(selected(&e), Some(b));
    assert_eq!((clip(&e, a), clip(&e, b)), (a0, b0));
}

/// The pair with fades on its far sides (A's fade-in, B's fade-out), and a clip C after it with fades of its own.
fn crossfade_with_far_fades() -> (Engine, TrackId, ClipId, ClipId, ClipId) {
    let (mut e, t, a, b) = crossfade();
    let fade = |len| Fade { len, shape: FadeShape::SCurve, curvature: 0.25 };
    let c = e.session_mut().new_clip_id();
    let s = e.session_mut();
    s.find_clip_mut(a).unwrap().fade_in = fade(100);
    s.find_clip_mut(b).unwrap().fade_out = fade(300);
    let mut clip_c = Clip::audio(c, "c", SourceId(2), 0, 4000, 1000);
    clip_c.fade_in = fade(50);
    clip_c.fade_out = fade(60);
    s.track_mut(t).and_then(|t| t.playlist_mut()).unwrap().clips.push(clip_c);
    (e, t, a, b, c)
}

#[test]
fn deleting_a_crossfade_removes_only_that_crossfade() {
    // Named, with nothing selected (so it needs no selection), and as the selection with nothing named.
    for how in ["named", "selected"] {
        let (mut e, _, a, b, c) = crossfade_with_far_fades();
        let (a0, b0, c0) = (clip(&e, a), clip(&e, b), clip(&e, c));
        let cut = crossfade_cut(&a0, &b0);
        if how == "selected" {
            select(&mut e, b);
        }
        let before = e.session().clone();
        let steps = e.undo_history().len();
        let params = if how == "named" { json!({"crossfade": b.0}) } else { json!({}) };
        let out = e.execute("edit.fades_delete", &params).unwrap();
        assert_eq!(out["cut"], json!(cut), "{how}");
        let (a1, b1, c1) = (clip(&e, a), clip(&e, b), clip(&e, c));
        // The clips are butted at the cut and the crossfade's own two fades are gone...
        assert_eq!((a1.end(), b1.start), (cut, cut), "{how}");
        assert_eq!((a1.fade_out.len, b1.fade_in.len, b1.cut), (0, 0, None), "{how}");
        // ...the fades on their far sides, their outer edges, where their audio is, and every other clip are as they were.
        assert_eq!((a1.fade_in, b1.fade_out), (a0.fade_in, b0.fade_out), "{how}");
        assert_eq!((a1.start, b1.end()), (a0.start, b0.end()), "{how}");
        assert_eq!((a1.start - a1.source_offset(), b1.start - b1.source_offset()), (a0.start - a0.source_offset(), b0.start - b0.source_offset()));
        assert_eq!(c1, c0, "{how}");
        if how == "selected" {
            assert_eq!(e.session().edit.selection, Range::point(cut));
            assert_eq!(e.session().edit.selected_crossfade, None);
        }
        // One undo step.
        assert_eq!(e.undo_history().len(), steps + 1, "{how}");
        assert!(e.undo());
        assert_eq!(e.session(), &before, "{how}");
    }
}

#[test]
fn a_crossfade_that_is_gone_or_cannot_be_edited_is_not_deleted() {
    let (mut e, _, a, b, _) = crossfade_with_far_fades();
    let before = e.session().clone();
    let steps = e.undo_history().len();
    for params in [
        json!({"crossfade": a.0}),  // the earlier clip
        json!({"crossfade": 9999}), // no such clip
        json!({"crossfade": "b"}),  // not an id
        json!({"crossfade": b.0, "start": 0}),
        json!({"crossfade": b.0, "tracks": [1]}),
    ] {
        assert!(e.execute("edit.fades_delete", &params).is_err(), "{params}");
        assert_eq!(e.session(), &before, "{params}");
    }
    assert_eq!(e.undo_history().len(), steps);
    // Edit-locked or stretched clips are not edited.
    for how in ["edit-locked", "stretched"] {
        let (mut e, _, a, b, _) = crossfade_with_far_fades();
        let c = e.session_mut().find_clip_mut(a).unwrap();
        if how == "stretched" {
            c.stretch = 1.5;
        } else {
            c.edit_locked = true;
        }
        let before = e.session().clone();
        assert!(e.execute("edit.fades_delete", &json!({"crossfade": b.0})).is_err(), "{how}");
        assert_eq!(e.session(), &before, "{how}");
        assert_eq!(e.undo_history().len(), steps, "{how}");
    }
    // Once deleted, it is no crossfade to delete again.
    e.execute("edit.fades_delete", &json!({"crossfade": b.0})).unwrap();
    let after = e.session().clone();
    assert!(e.execute("edit.fades_delete", &json!({"crossfade": b.0})).is_err());
    assert_eq!(e.session(), &after);
}

#[test]
fn deleting_fades_elsewhere_leaves_the_selected_crossfade_selected() {
    let (mut e, t, _, b) = crossfade();
    select(&mut e, b);
    // No clip lies in this range: nothing is touched.
    e.execute("edit.fades_delete", &json!({"tracks": [t.0], "start": 5000, "end": 6000})).unwrap();
    assert_eq!(selected(&e), Some(b));
}

#[test]
fn a_selected_crossfade_stays_selected_when_it_slides_or_is_trimmed() {
    let (mut e, _, a, b) = crossfade();
    select(&mut e, b);
    e.execute("edit.move_crossfade", &json!({"clip": b.0, "by": 100})).unwrap();
    assert_eq!(e.session().edit.selection, Range::new(1100, 1300));
    assert_eq!(selected(&e), Some(b), "the selection took the new zone");
    e.execute("edit.trim_crossfade", &json!({"clip": b.0, "start": 1080})).unwrap();
    assert_eq!(e.session().edit.selection, Range::new(1080, 1300));
    assert_eq!(selected(&e), Some(b));
    assert_eq!(e.session().edit.timeline_selection, Range::new(1080, 1300));
    // Trimmed to nothing, the crossfade is gone and the selection is an insertion point at its cut.
    let cut = crossfade_cut(&clip(&e, a), &clip(&e, b));
    e.execute("edit.trim_crossfade", &json!({"clip": b.0, "start": cut, "end": cut})).unwrap();
    assert_eq!(clip(&e, a).end(), clip(&e, b).start);
    assert_eq!(e.session().edit.selection, Range::point(cut));
    assert_eq!(e.session().edit.selected_crossfade, None);
}

#[test]
fn sliding_a_crossfade_that_is_not_selected_leaves_the_selection_alone() {
    let (mut e, t, _, b) = crossfade();
    e.execute("edit.select", &json!({"tracks": [t.0], "start": 2000, "end": 2500})).unwrap();
    e.execute("edit.move_crossfade", &json!({"clip": b.0, "by": 100})).unwrap();
    assert_eq!(e.session().edit.selection, Range::new(2000, 2500));
    // A range that happens to be the zone is a plain range too.
    e.execute("edit.select", &json!({"tracks": [t.0], "start": 1100, "end": 1300})).unwrap();
    e.execute("edit.move_crossfade", &json!({"clip": b.0, "by": -50})).unwrap();
    assert_eq!(e.session().edit.selection, Range::new(1100, 1300));
}

#[test]
fn selecting_a_crossfade_does_not_dirty_the_document() {
    let (mut e, _, _, b) = crossfade();
    e.mark_clean();
    select(&mut e, b);
    assert!(!e.is_dirty());
}
