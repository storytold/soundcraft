//! Dragging a crossfade with the Grabber slides the joint between its two clips, and the cut with
//! it; the clips stay where they are. Dragging a clip outside the crossfade still moves the clip.
//! The Trim tool (and the lower half of the Smart tool) lengthens or shortens the crossfade from one
//! edge of its zone, the cut staying where it is. A click on the zone selects the crossfade and nothing else, and
//! Delete removes the selected crossfade. The zone is a box over the whole clip with no line at the cut, and the later
//! clip's name starts where the zone ends.

use egui::epaint::{ClippedShape, Shape};
use egui::{Event, Key, Modifiers, PointerButton, Pos2, RawInput, Rect, pos2, vec2};
use serde_json::json;
use soundcraft_engine::Engine;
use soundcraft_model::{Clip, ClipId, Fade, FadeShape, Source, SourceId, TrackId, crossfade_cut};
use soundcraft_ui_egui::{Gesture, Services, SoundApp, edit_window};

const SEC: i64 = 48_000;
/// A crossfade 20 000 samples long (50 px at the zoom used here).
const XFADE: i64 = 20_000;
/// Clip A is 0..2 s; clip B starts [`XFADE`] before A ends.
const B_START: i64 = 2 * SEC - XFADE;

fn frame(ctx: &egui::Context, app: &mut SoundApp, events: Vec<Event>) {
    let raw = RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1600.0, 1000.0))), events, ..Default::default() };
    ctx.run_ui(raw, |ui| {
        app.logic(ui.ctx());
        app.ui(ui);
    })
    .textures_delta
    .clear();
}

/// A frame, and what it painted.
fn frame_shapes(ctx: &egui::Context, app: &mut SoundApp) -> Vec<egui::epaint::ClippedShape> {
    let raw = RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1600.0, 1000.0))), ..Default::default() };
    let mut out = ctx.run_ui(raw, |ui| {
        app.logic(ui.ctx());
        app.ui(ui);
    });
    out.textures_delta.clear();
    out.shapes
}

fn button(pos: Pos2, pressed: bool) -> Event {
    Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE }
}

/// Clip A (0..2 s) crossfaded into clip B (1.58 s..3.58 s) on one track; A has `after` samples of
/// file left past its end.
fn crossfade(tool: &str, after: i64) -> (SoundApp, egui::Context, TrackId, ClipId, ClipId) {
    let mut e = Engine::default();
    let t = TrackId(e.execute("track.new", &json!({"count": 1})).unwrap()["tracks"][0].as_u64().unwrap());
    let s = e.session_mut();
    for (id, frames) in [(7, SEC + 2 * SEC + after), (8, 30 * SEC)] {
        s.sources.push(Source {
            id: SourceId(id),
            name: "src".into(),
            path: "src.wav".into(),
            channels: 1,
            frames: u64::try_from(frames).unwrap(),
            sample_rate: 48_000,
            format: soundcraft_audio_io::FileFormat::Wav,
            time_reference: 0,
            unsaved: false,
        });
    }
    let (a, b) = (s.new_clip_id(), s.new_clip_id());
    let fade = Fade { len: XFADE, shape: FadeShape::EqualPower, curvature: 0.0 };
    let mut ca = Clip::audio(a, "a", SourceId(7), SEC, 0, 2 * SEC);
    ca.fade_out = fade;
    let mut cb = Clip::audio(b, "b", SourceId(8), 5 * SEC, B_START, 2 * SEC);
    cb.fade_in = fade;
    s.track_mut(t).and_then(|t| t.playlist_mut()).unwrap().clips = vec![ca, cb];
    e.execute("edit.mode", &json!({"mode": "slip"})).unwrap();
    e.execute("edit.tool", &json!({"tool": tool})).unwrap();
    e.execute("view.zoom_set", &json!({"samples_per_px": 400.0})).unwrap();
    let mut app = SoundApp::new(e, None, Services::default());
    let ctx = egui::Context::default();
    frame(&ctx, &mut app, vec![]);
    frame(&ctx, &mut app, vec![]);
    (app, ctx, t, a, b)
}

/// The point at sample `at` on the track, `down` of the way from the top of its row to the bottom.
fn point(app: &SoundApp, t: TrackId, at: i64, down: f32) -> Pos2 {
    let s = app.engine.session();
    let [x0, y0, x1, y1] = app.edit_layout.timeline;
    let row = app.edit_layout.rows.iter().find(|(id, _)| *id == t.0).unwrap().1;
    let x = edit_window::x_of(s, Rect::from_min_max(pos2(x0, y0), pos2(x1, y1)), at);
    pos2(x, row[1] + s.track(t).unwrap().height.points() * down)
}

fn drag_between(ctx: &egui::Context, app: &mut SoundApp, from: Pos2, to: Pos2) {
    frame(ctx, app, vec![Event::PointerMoved(from)]);
    frame(ctx, app, vec![button(from, true)]);
    let started = from + vec2(7.0_f32.copysign(to.x - from.x), 0.0);
    for k in 0..=5 {
        frame(ctx, app, vec![Event::PointerMoved(started + (to - started) * (k as f32 / 5.0))]);
    }
    frame(ctx, app, vec![button(to, false)]);
    frame(ctx, app, vec![]);
}

fn clip(app: &SoundApp, id: ClipId) -> Clip {
    app.engine.session().find_clip(id).unwrap().1.clone()
}

/// The pointer's travel in samples, as the timeline sees it.
fn travel(app: &SoundApp, from: Pos2, to: Pos2) -> i64 {
    let [x0, y0, x1, y1] = app.edit_layout.timeline;
    let (s, tl) = (app.engine.session(), Rect::from_min_max(pos2(x0, y0), pos2(x1, y1)));
    edit_window::sample_at(s, tl, to.x) - edit_window::sample_at(s, tl, from.x)
}

#[test]
fn dragging_a_crossfade_slides_the_joint_and_the_clips_stay_in_place() {
    for tool in ["grabber", "smart"] {
        let (mut app, ctx, t, a, b) = crossfade(tool, 10 * SEC);
        let (a0, b0) = (clip(&app, a), clip(&app, b));
        let before = app.engine.session().clone();
        let steps = app.engine.undo_history().len();
        let from = point(&app, t, B_START + XFADE / 2, 0.75);
        let to = from + vec2(40.0, 0.0);
        drag_between(&ctx, &mut app, from, to);
        let by = travel(&app, from, to);
        assert!(by > 10_000, "the drag should be long enough to mean something: {by}");
        let (a1, b1) = (clip(&app, a), clip(&app, b));
        // The joint moved with the pointer, and the cut with it...
        assert_eq!((a1.end(), b1.start), (a0.end() + by, b0.start + by), "{tool}");
        assert_eq!(crossfade_cut(&a1, &b1), crossfade_cut(&a0, &b0) + by, "{tool}");
        // ...and nothing else did: A's start, B's end, and where each clip's audio sits.
        assert_eq!((a1.start, b1.end()), (a0.start, b0.end()), "{tool}");
        assert_eq!((a1.start - a1.source_offset(), b1.start - b1.source_offset()), (a0.start - a0.source_offset(), b0.start - b0.source_offset()));
        // The crossfade kept its length: overlap, fade-out and fade-in still agree.
        assert_eq!((a1.end() - b1.start, a1.fade_out.len, b1.fade_in.len), (XFADE, XFADE, XFADE), "{tool}");
        // One gesture, one undo step.
        assert_eq!(app.engine.undo_history().len(), steps + 1, "{tool}");
        assert!(app.engine.undo());
        assert_eq!(app.engine.session(), &before, "{tool}");
    }
}

#[test]
fn a_dragged_crossfade_stops_where_a_clip_runs_out_of_file() {
    let (mut app, ctx, t, a, b) = crossfade("grabber", 3000);
    let from = point(&app, t, B_START + XFADE / 2, 0.75);
    drag_between(&ctx, &mut app, from, from + vec2(200.0, 0.0));
    assert_eq!((clip(&app, a).end(), clip(&app, b).start), (2 * SEC + 3000, B_START + 3000));
    assert_eq!(clip(&app, a).end() - clip(&app, b).start, XFADE);
    // The other way, B's start stops one sample after A's start (B has 5 s of file ahead of it).
    let (mut app, ctx, t, a, b) = crossfade("grabber", 3000);
    let from = point(&app, t, B_START + XFADE - 1000, 0.75);
    drag_between(&ctx, &mut app, from, from - vec2(225.0, 0.0));
    assert_eq!((clip(&app, a).end(), clip(&app, b).start), (XFADE + 1, 1));
}

#[test]
fn a_clip_dragged_outside_its_crossfade_still_moves_the_clip() {
    let (mut app, ctx, t, a, b) = crossfade("grabber", 10 * SEC);
    let (a0, b0) = (clip(&app, a), clip(&app, b));
    let from = point(&app, t, 3 * SEC, 0.75);
    drag_between(&ctx, &mut app, from, from + vec2(80.0, 0.0));
    let (a1, b1) = (clip(&app, a), clip(&app, b));
    assert_eq!(a1, a0);
    assert!(b1.start > b0.start, "B moved as a whole: {} -> {}", b0.start, b1.start);
    assert_eq!(b1.length, b0.length);
}

#[test]
fn the_upper_half_of_a_crossfade_still_selects_with_the_smart_tool() {
    let (mut app, ctx, t, a, b) = crossfade("smart", 10 * SEC);
    let (a0, b0) = (clip(&app, a), clip(&app, b));
    let zone0 = zone(&app, a, b);
    let steps = app.engine.undo_history().len();
    let from = point(&app, t, B_START + XFADE / 2, 0.2);
    drag_between(&ctx, &mut app, from, from + vec2(40.0, 0.0));
    assert_eq!((clip(&app, a), clip(&app, b)), (a0, b0));
    assert_eq!(zone(&app, a, b), zone0, "neither the zone nor its cut moved");
    assert!(!app.engine.session().edit.selection.is_empty(), "the drag selected a range instead");
    assert_eq!(app.engine.undo_history().len(), steps, "no edit was made");
}

#[test]
fn clips_that_only_overlap_are_not_a_crossfade_and_still_move_as_clips() {
    // B laid inside A: its end is before A's end, so nothing here crossfades.
    let (mut app, ctx, t, a, b) = crossfade("grabber", 10 * SEC);
    app.engine.session_mut().find_clip_mut(b).unwrap().trim_end_to(B_START + 14_000);
    let b0 = clip(&app, b);
    let from = point(&app, t, B_START + 5000, 0.75);
    drag_between(&ctx, &mut app, from, from + vec2(40.0, 0.0));
    assert!(clip(&app, b).start > b0.start, "B moved as a clip");
    assert_eq!(clip(&app, b).length, b0.length);
    let _ = a;
}

#[test]
fn a_crossfade_with_a_locked_clip_stays_put() {
    let (mut app, ctx, t, a, b) = crossfade("grabber", 10 * SEC);
    app.engine.session_mut().find_clip_mut(a).unwrap().edit_locked = true;
    let before = app.engine.session().clone();
    let zone0 = zone(&app, a, b);
    // A crossfade with a locked clip does not slide, so the press is on the clip under the pointer, as anywhere
    // else: here the locked earlier one, left of the cut, which does not move either.
    let from = point(&app, t, B_START + XFADE / 4, 0.75);
    drag_between(&ctx, &mut app, from, from + vec2(40.0, 0.0));
    assert_eq!(zone(&app, a, b), zone0, "the zone and its cut stay put");
    assert_eq!((clip(&app, a), clip(&app, b)), (before.find_clip(a).unwrap().1.clone(), before.find_clip(b).unwrap().1.clone()));
    assert!(app.gesture.is_none());
}

#[test]
fn the_document_waits_for_the_release_while_a_crossfade_is_dragged() {
    let (mut app, ctx, t, a, b) = crossfade("grabber", 10 * SEC);
    let before = app.engine.session().clone();
    let from = point(&app, t, B_START + XFADE / 2, 0.75);
    let to = from + vec2(40.0, 0.0);
    frame(&ctx, &mut app, vec![Event::PointerMoved(from)]);
    frame(&ctx, &mut app, vec![button(from, true)]);
    frame(&ctx, &mut app, vec![Event::PointerMoved(from + vec2(7.0, 0.0))]);
    frame(&ctx, &mut app, vec![Event::PointerMoved(to)]);
    assert!(matches!(app.gesture, Some(Gesture::MoveCrossfade { clip, by, .. }) if clip == b && by > 0), "{:?}", app.gesture);
    assert_eq!(app.engine.session(), &before, "the ghost shows the slide; the clips wait for the release");
    frame(&ctx, &mut app, vec![button(to, false)]);
    frame(&ctx, &mut app, vec![]);
    assert!(app.gesture.is_none());
    assert!(clip(&app, a).end() > before.find_clip(a).unwrap().1.end());
}

/// The sample under pointer position `x`, as the timeline sees it.
fn sample(app: &SoundApp, x: f32) -> i64 {
    let [x0, y0, x1, y1] = app.edit_layout.timeline;
    edit_window::sample_at(app.engine.session(), Rect::from_min_max(pos2(x0, y0), pos2(x1, y1)), x)
}

fn cut(app: &SoundApp, a: ClipId, b: ClipId) -> i64 {
    crossfade_cut(&clip(app, a), &clip(app, b))
}

fn use_tool(ctx: &egui::Context, app: &mut SoundApp, tool: &str) {
    app.engine.execute("edit.tool", &json!({"tool": tool})).unwrap();
    frame(ctx, app, vec![]);
}

/// Put the cut `offset` samples into the zone, off the middle (which is where an unset cut sits).
fn cut_at(ctx: &egui::Context, app: &mut SoundApp, b: ClipId, offset: i64) {
    app.engine.session_mut().find_clip_mut(b).unwrap().cut = Some(offset);
    frame(ctx, app, vec![]);
}

/// Click the track at sample `at`, `down` of the way from the top of its row to the bottom.
fn click_at(ctx: &egui::Context, app: &mut SoundApp, t: TrackId, at: i64, down: f32) {
    let p = point(app, t, at, down);
    click(ctx, app, p);
}

fn click(ctx: &egui::Context, app: &mut SoundApp, at: Pos2) {
    frame(ctx, app, vec![Event::PointerMoved(at)]);
    frame(ctx, app, vec![button(at, true)]);
    frame(ctx, app, vec![button(at, false)]);
    frame(ctx, app, vec![]);
}

/// What a crossfade has to keep true after any drag: both fades are the overlap, and the audio is
/// where it was.
fn holds(app: &SoundApp, a: ClipId, b: ClipId, before: &(Clip, Clip)) {
    let (a1, b1) = (clip(app, a), clip(app, b));
    assert_eq!(
        (a1.start - a1.source_offset(), b1.start - b1.source_offset()),
        (before.0.start - before.0.source_offset(), before.1.start - before.1.source_offset())
    );
    assert_eq!((a1.start, b1.end()), (before.0.start, before.1.end()), "the outer edges stay");
    assert_eq!((a1.fade_out.len, b1.fade_in.len), (a1.end() - b1.start, a1.end() - b1.start));
}

#[test]
fn a_crossfade_dragged_to_the_end_of_a_file_stops_and_can_then_only_grow_from_its_other_edge() {
    let (mut app, ctx, t, a, b) = crossfade("grabber", 3000);
    cut_at(&ctx, &mut app, b, 5000);
    let cut0 = cut(&app, a, b);
    assert_eq!(cut0, B_START + 5000);
    let before = (clip(&app, a), clip(&app, b));
    // Dragged far to the right, the zone stops where A runs out of file; the cut went with it.
    let from = point(&app, t, B_START + 2000, 0.75);
    drag_between(&ctx, &mut app, from, from + vec2(200.0, 0.0));
    let (a1, b1) = (clip(&app, a), clip(&app, b));
    assert_eq!((a1.end(), b1.start), (2 * SEC + 3000, B_START + 3000));
    assert_eq!(cut(&app, a, b), cut0 + 3000);
    holds(&app, a, b, &before);
    // Pushing on changes nothing, and leaves no undo step.
    let steps = app.engine.undo_history().len();
    let from = point(&app, t, b1.start + 8000, 0.75);
    drag_between(&ctx, &mut app, from, from + vec2(60.0, 0.0));
    assert_eq!((clip(&app, a), clip(&app, b)), (a1.clone(), b1.clone()));
    assert_eq!(app.engine.undo_history().len(), steps);
    // What is left is to lengthen the other edge, with the audio B has hidden: the zone grows, the cut stays.
    use_tool(&ctx, &mut app, "trim");
    let edge = point(&app, t, b1.start, 0.75);
    drag_between(&ctx, &mut app, edge, edge - vec2(50.0, 0.0));
    let (a2, b2) = (clip(&app, a), clip(&app, b));
    assert_eq!(b2.start, sample(&app, edge.x - 50.0), "the left edge follows the pointer");
    assert!(b2.start < b1.start - 10_000, "{} -> {}", b1.start, b2.start);
    assert_eq!(a2.end(), a1.end(), "the other edge stays");
    assert_eq!(cut(&app, a, b), cut0 + 3000, "the cut stays");
    holds(&app, a, b, &before);
    // Far enough, it stops one sample into A.
    let edge = point(&app, t, b2.start, 0.75);
    drag_between(&ctx, &mut app, edge, edge - vec2(160.0, 0.0));
    assert_eq!(clip(&app, b).start, 1);
    assert_eq!(cut(&app, a, b), cut0 + 3000);
    holds(&app, a, b, &before);
}

#[test]
fn an_edge_of_the_zone_never_crosses_the_cut() {
    let (mut app, ctx, t, a, b) = crossfade("trim", 10 * SEC);
    cut_at(&ctx, &mut app, b, 5000);
    let cut0 = cut(&app, a, b);
    let before = (clip(&app, a), clip(&app, b));
    // The left edge taken past the cut stops on it: the zone is now the cut to A's end.
    let edge = point(&app, t, B_START, 0.75);
    drag_between(&ctx, &mut app, edge, edge + vec2(100.0, 0.0));
    assert_eq!((clip(&app, b).start, clip(&app, a).end()), (cut0, 2 * SEC));
    assert_eq!(cut(&app, a, b), cut0);
    holds(&app, a, b, &before);
    // So does the right edge: A's end taken back past the cut leaves the clips butted on it.
    let edge = point(&app, t, 2 * SEC, 0.75);
    drag_between(&ctx, &mut app, edge, edge - vec2(100.0, 0.0));
    let (a1, b1) = (clip(&app, a), clip(&app, b));
    assert_eq!((a1.end(), b1.start), (cut0, cut0));
    assert_eq!((a1.fade_out.len, b1.fade_in.len, b1.cut), (0, 0, None));
    holds(&app, a, b, &before);
}

#[test]
fn the_right_edge_of_the_zone_is_the_earlier_clips_end_and_a_trim_is_one_undo_step() {
    let (mut app, ctx, t, a, b) = crossfade("trim", 10 * SEC);
    let before_session = app.engine.session().clone();
    let before = (clip(&app, a), clip(&app, b));
    let steps = app.engine.undo_history().len();
    let cut0 = cut(&app, a, b);
    let edge = point(&app, t, 2 * SEC, 0.75);
    drag_between(&ctx, &mut app, edge, edge + vec2(30.0, 0.0));
    assert_eq!(clip(&app, a).end(), sample(&app, edge.x + 30.0));
    assert_eq!(clip(&app, b).start, B_START, "the left edge stays");
    assert_eq!(cut(&app, a, b), cut0);
    holds(&app, a, b, &before);
    assert_eq!(app.engine.undo_history().len(), steps + 1);
    assert!(app.engine.undo());
    assert_eq!(app.engine.session(), &before_session);
}

#[test]
fn the_smart_tool_trims_a_zone_edge_from_its_lower_half() {
    let (mut app, ctx, t, a, b) = crossfade("smart", 10 * SEC);
    let before = (clip(&app, a), clip(&app, b));
    let edge = point(&app, t, B_START, 0.75);
    drag_between(&ctx, &mut app, edge, edge - vec2(20.0, 0.0));
    assert_eq!(clip(&app, b).start, sample(&app, edge.x - 20.0));
    assert_eq!(clip(&app, a).end(), 2 * SEC);
    holds(&app, a, b, &before);
    let edge = point(&app, t, 2 * SEC, 0.75);
    drag_between(&ctx, &mut app, edge, edge + vec2(20.0, 0.0));
    assert_eq!(clip(&app, a).end(), sample(&app, edge.x + 20.0));
    holds(&app, a, b, &before);
}

#[test]
fn a_locked_crossfade_does_not_trim() {
    let (mut app, ctx, t, a, b) = crossfade("trim", 10 * SEC);
    app.engine.session_mut().find_clip_mut(b).unwrap().edit_locked = true;
    let before = app.engine.session().clone();
    let steps = app.engine.undo_history().len();
    let edge = point(&app, t, B_START, 0.75);
    drag_between(&ctx, &mut app, edge, edge - vec2(30.0, 0.0));
    assert_eq!((clip(&app, a), clip(&app, b)), (before.find_clip(a).unwrap().1.clone(), before.find_clip(b).unwrap().1.clone()));
    assert_eq!(app.engine.undo_history().len(), steps);
    assert!(app.gesture.is_none());
}

#[test]
fn in_the_zone_of_a_pair_that_cannot_slide_the_earlier_clip_is_left_of_the_cut_and_the_later_one_right_of_it() {
    // A stretched pair is no crossfade the Grabber takes, so a click there is a click on the clip under it.
    let (mut app, ctx, t, a, b) = crossfade("grabber", 10 * SEC);
    app.engine.session_mut().find_clip_mut(a).unwrap().stretch = 1.5;
    // The zone is B_START..B_START + 20 000, cut at 5 000 into it.
    cut_at(&ctx, &mut app, b, 5000);
    for (at, expected) in
        [(B_START + 1000, a), (B_START + 4000, a), (B_START + 6000, b), (B_START + 15_000, b), (B_START - 5000, a), (2 * SEC + 5000, b)]
    {
        let at = point(&app, t, at, 0.75);
        click(&ctx, &mut app, at);
        assert_eq!(app.engine.session().edit.selected_clips, vec![expected], "click at {at}");
    }
}

/// The zone as (left edge, cut, right edge).
fn zone(app: &SoundApp, a: ClipId, b: ClipId) -> (i64, i64, i64) {
    (clip(app, b).start, cut(app, a, b), clip(app, a).end())
}

/// Press at `from`, pull `dx` points sideways and keep the button down: the gesture is what the press started.
fn press_and_pull(ctx: &egui::Context, app: &mut SoundApp, from: Pos2, dx: f32) {
    frame(ctx, app, vec![Event::PointerMoved(from)]);
    frame(ctx, app, vec![button(from, true)]);
    frame(ctx, app, vec![Event::PointerMoved(from + vec2(7.0_f32.copysign(dx), 0.0))]);
    frame(ctx, app, vec![Event::PointerMoved(from + vec2(dx, 0.0))]);
}

/// A crossfade `width` samples wide (400 to the point), cut in the middle: a 10 ms crossfade seen from afar.
fn narrow(tool: &str, width: i64) -> (SoundApp, egui::Context, TrackId, ClipId, ClipId, i64) {
    let (mut app, ctx, t, a, b) = crossfade(tool, 10 * SEC);
    let (cut0, half) = (cut(&app, a, b), width / 2);
    app.engine.execute("edit.trim_crossfade", &json!({"clip": b.0, "start": cut0 - half, "end": cut0 + half})).unwrap();
    frame(&ctx, &mut app, vec![]);
    frame(&ctx, &mut app, vec![]);
    assert_eq!(zone(&app, a, b), (cut0 - half, cut0, cut0 + half));
    (app, ctx, t, a, b, cut0)
}

#[test]
fn a_narrow_zone_can_still_be_slid_and_trimmed() {
    // (tool, zone width in samples, points from the cut, what the press starts): the middle of the zone slides it, the
    // sides of it trim its edges, and a few points further out is the clip. A zone of 2 points is narrower than
    // the bands of its edges; one of 14 points is not, but its edge bands would leave the cut only 2 points.
    let cases = [
        ("grabber", 800, 0.0, "slide"),
        ("grabber", 800, 2.5, "slide"),
        ("grabber", 800, -2.5, "slide"),
        ("grabber", 800, 5.0, "clip"),
        ("smart", 800, 0.0, "slide"),
        ("smart", 800, 2.0, "slide"),
        ("smart", 800, -2.0, "slide"),
        ("smart", 800, -5.0, "start"),
        ("smart", 800, 5.0, "end"),
        ("trim", 800, -2.0, "start"),
        ("trim", 800, 2.0, "end"),
        ("grabber", 5600, 0.0, "slide"),
        ("grabber", 5600, 6.0, "slide"),
        ("grabber", 5600, -6.0, "slide"),
        ("grabber", 5600, 8.5, "clip"),
        ("smart", 5600, 0.0, "slide"),
        ("smart", 5600, 2.5, "slide"),
        ("smart", 5600, -2.5, "slide"),
        ("smart", 5600, -4.0, "start"),
        ("smart", 5600, 4.0, "end"),
        ("smart", 5600, 6.0, "end"),
        ("trim", 5600, -1.0, "start"),
        ("trim", 5600, 1.0, "end"),
    ];
    for (tool, width, dx, want) in cases {
        let (mut app, ctx, t, _, _, cut0) = narrow(tool, width);
        let from = point(&app, t, cut0, 0.75) + vec2(dx, 0.0);
        press_and_pull(&ctx, &mut app, from, 20.0);
        let got = match app.gesture {
            Some(Gesture::MoveCrossfade { .. }) => "slide",
            Some(Gesture::TrimCrossfadeStart { .. }) => "start",
            Some(Gesture::TrimCrossfadeEnd { .. }) => "end",
            Some(Gesture::MoveClips { .. }) => "clip",
            _ => "other",
        };
        assert_eq!(got, want, "{tool}, zone of {width} samples, {dx} points from the cut: {:?}", app.gesture);
    }
}

#[test]
fn a_narrow_zone_slides_when_it_is_taken_beside_the_cut() {
    let (mut app, ctx, t, a, b, cut0) = narrow("grabber", 800);
    let from = point(&app, t, cut0, 0.75) + vec2(2.5, 0.0);
    drag_between(&ctx, &mut app, from, from + vec2(30.0, 0.0));
    assert!(cut(&app, a, b) > cut0 + 10_000, "the cut went with the zone: {}", cut(&app, a, b));
    assert_eq!(clip(&app, a).end() - clip(&app, b).start, 800);
}

#[test]
fn the_fades_of_a_crossfade_are_not_offered_one_by_one() {
    // The zone's fades are resized with its edges: changing one alone breaks fade-out == overlap == fade-in.
    for cut_offset in [5000, 15_000] {
        let mut spots = vec![(B_START, 0.12), (B_START + XFADE, 0.12), (B_START + 100, 0.1), (B_START + XFADE - 100, 0.1)];
        spots.extend((3..9).map(|k| (B_START + XFADE / 2, k as f32 * 0.1)));
        for (at, down) in spots {
            let (mut app, ctx, t, a, b) = crossfade("smart", 10 * SEC);
            cut_at(&ctx, &mut app, b, cut_offset);
            let fades = (clip(&app, a).fade_out, clip(&app, b).fade_in);
            let from = point(&app, t, at, down);
            press_and_pull(&ctx, &mut app, from, 12.0);
            assert!(
                !matches!(app.gesture, Some(Gesture::Fade { .. } | Gesture::FadeCurve { .. })),
                "cut {cut_offset}, {at}, {down}: {:?}",
                app.gesture
            );
            frame(&ctx, &mut app, vec![button(from + vec2(12.0, 0.0), false)]);
            frame(&ctx, &mut app, vec![]);
            assert_eq!((clip(&app, a).fade_out, clip(&app, b).fade_in), fades, "cut {cut_offset}, {at}, {down}");
        }
    }
}

#[test]
fn a_dragged_crossfade_puts_its_cut_on_the_grid() {
    let (mut app, ctx, t, a, b) = crossfade("grabber", 10 * SEC);
    cut_at(&ctx, &mut app, b, 5000);
    app.engine.execute("edit.mode", &json!({"mode": "grid"})).unwrap();
    frame(&ctx, &mut app, vec![]);
    let cut0 = cut(&app, a, b);
    assert_eq!(cut0, B_START + 5000);
    let from = point(&app, t, B_START + 2000, 0.75);
    let to = from + vec2(40.0, 0.0);
    drag_between(&ctx, &mut app, from, to);
    let s = app.engine.session();
    let on_grid = s.edit.grid.snap(cut0 + travel(&app, from, to), s.sample_rate, &s.tempo, s.frame_rate);
    // The default grid is a sixteenth at 120 bpm, a line every 6 000 samples: 97 000 is nearest to 96 000, where
    // snapping the zone's start (to 90 000, with the cut at 95 000) would not put the cut.
    assert_eq!(on_grid, 96_000);
    assert_eq!(cut(&app, a, b), on_grid);
    assert_eq!(clip(&app, a).end() - clip(&app, b).start, XFADE);
}

fn near(a: f32, b: f32) -> bool {
    (a - b).abs() < 0.6
}

/// The rectangles a frame painted, with their place in the painting order.
fn rects(shapes: &[ClippedShape]) -> Vec<(usize, egui::epaint::RectShape)> {
    shapes
        .iter()
        .enumerate()
        .filter_map(|(i, c)| match &c.shape {
            Shape::Rect(r) => Some((i, r.clone())),
            _ => None,
        })
        .collect()
}

/// The top and the height of the track's lane on screen.
fn lane_of(app: &SoundApp, t: TrackId) -> (f32, f32) {
    let row = app.edit_layout.rows.iter().find(|(id, _)| *id == t.0).unwrap().1;
    (row[1], app.engine.session().track(t).unwrap().height.points())
}

/// Where sample `at` is on screen.
fn x_at(app: &SoundApp, t: TrackId, at: i64) -> f32 {
    point(app, t, at, 0.0).x
}

/// The zone box: a rectangle from `left` to `right` outlined `width` points wide, and where it was painted.
fn zone_box(shapes: &[ClippedShape], left: f32, right: f32, width: f32) -> Option<(usize, Rect)> {
    rects(shapes)
        .into_iter()
        .find(|(_, r)| near(r.rect.min.x, left) && near(r.rect.max.x, right) && near(r.stroke.width, width))
        .map(|(i, r)| (i, r.rect))
}

/// The widths of the vertical line segments at `x` that are at least `span` points long.
fn verticals(shapes: &[ClippedShape], x: f32, span: f32) -> Vec<f32> {
    shapes
        .iter()
        .filter_map(|c| match &c.shape {
            Shape::LineSegment { points, stroke } if near(points[0].x, x) && near(points[1].x, x) && (points[0].y - points[1].y).abs() >= span => {
                Some(stroke.width)
            }
            _ => None,
        })
        .collect()
}

/// How many selection highlights (a fill with no outline) cover exactly the lane from `left` to `right`.
fn highlights(shapes: &[ClippedShape], left: f32, right: f32, lane: (f32, f32)) -> usize {
    let (top, height) = lane;
    rects(shapes)
        .iter()
        .filter(|(_, r)| {
            r.stroke.width < 0.1
                && near(r.rect.min.x, left)
                && near(r.rect.max.x, right)
                && near(r.rect.min.y, top)
                && near(r.rect.max.y, top + height)
        })
        .count()
}

#[test]
fn a_crossfade_is_a_box_over_the_whole_clip_with_no_line_at_the_cut_and_the_later_name_starts_at_its_end() {
    for view in ["clips", "volume"] {
        let (mut app, ctx, t, a, b) = crossfade("grabber", 10 * SEC);
        cut_at(&ctx, &mut app, b, 5000);
        // A mark that does lie on the cut, so that the search for a line there can be seen to find one: B's sync point.
        app.engine.session_mut().edit.set_flag("view.clip.sync_point", true);
        app.engine.session_mut().find_clip_mut(b).unwrap().sync_point = 5000;
        if view != "clips" {
            app.engine.execute("track.view", &json!({"tracks": [t.0], "view": view})).unwrap();
        }
        frame_shapes(&ctx, &mut app);
        let shapes = frame_shapes(&ctx, &mut app);
        let (top, height) = lane_of(&app, t);
        let (zone_start, zone_end) = (x_at(&app, t, B_START), x_at(&app, t, 2 * SEC));
        let cut_x = x_at(&app, t, cut(&app, a, b));
        assert!(cut_x > zone_start + 5.0 && cut_x < zone_end - 5.0, "the cut is inside the zone, off its edges");

        // The box runs from the later clip's start to the earlier clip's end, over the clip's whole height, name band included.
        let (box_at, bx) = zone_box(&shapes, zone_start, zone_end, 1.0).unwrap_or_else(|| panic!("{view}: no zone box"));
        assert!(bx.min.y <= top + 0.5 && bx.max.y >= top + height - 0.5, "{view}: box {bx:?} in a lane {top}..{}", top + height);

        // A's name bar ends where the zone starts, B's starts where it ends: not at the cut.
        let mut bars: Vec<(usize, Rect)> = rects(&shapes)
            .into_iter()
            .filter(|(_, r)| near(r.rect.height(), 13.0) && near(r.rect.min.y, top + 1.0))
            .map(|(i, r)| (i, r.rect))
            .collect();
        bars.sort_by(|p, q| p.1.min.x.total_cmp(&q.1.min.x));
        assert_eq!(bars.len(), 2, "{view}: a name bar per clip: {bars:?}");
        let (a_bar, b_bar) = (bars[0].1, bars[1].1);
        assert!(near(a_bar.min.x, x_at(&app, t, 0)) && near(a_bar.max.x, zone_start), "{view}: A's bar {a_bar:?}, the zone starts at {zone_start}");
        assert!(
            near(b_bar.min.x, zone_end) && near(b_bar.max.x, x_at(&app, t, B_START + 2 * SEC)),
            "{view}: B's bar {b_bar:?}, the zone ends at {zone_end}"
        );
        // The names sit at the start of their bars.
        let label = |name: &str| {
            shapes.iter().enumerate().find_map(|(i, c)| match &c.shape {
                Shape::Text(tx) if tx.galley.text() == name && tx.pos.y > top && tx.pos.y < top + height => Some((i, tx.pos.x)),
                _ => None,
            })
        };
        let ((a_at, a_x), (b_at, b_x)) = (label("a").expect("A's name"), label("b").expect("B's name"));
        assert!(near(a_x, a_bar.min.x + 4.0), "{view}: A's name at {a_x}");
        assert!(near(b_x, zone_end + 4.0), "{view}: B's name starts where the zone ends ({zone_end}), not at the cut ({cut_x}): {b_x}");
        // The box is drawn over the names and the bars.
        assert!([a_at, b_at, bars[0].0, bars[1].0].iter().all(|&i| box_at > i), "{view}: the box is painted last");

        // Nor do the clips' outlines: they stop at the cut instead of drawing a dark line across the zone where the clips
        // meet. The control: the outlines are there at the clips' far ends.
        let outline_edges =
            |x: f32| rects(&shapes).iter().filter(|(_, r)| r.stroke.width > 0.5 && (near(r.rect.min.x, x) || near(r.rect.max.x, x))).count();
        assert!(outline_edges(x_at(&app, t, 0)) >= 1 && outline_edges(x_at(&app, t, B_START + 2 * SEC)) >= 1, "{view}: the far outlines");
        assert_eq!(outline_edges(cut_x), 0, "{view}: a clip's outline on the cut");

        // Nothing marks the cut across the lane. The control: B's sync point is a line on the cut, shorter than the lane.
        assert!(!verticals(&shapes, cut_x, height * 0.4).is_empty(), "{view}: the search sees a line on the cut");
        assert!(verticals(&shapes, cut_x, height - 2.0).is_empty(), "{view}: a line across the lane at the cut");
    }
}

#[test]
fn the_ghost_of_a_dragged_crossfade_is_a_box_with_no_line_at_the_cut() {
    let (mut app, ctx, t, a, b) = crossfade("grabber", 10 * SEC);
    cut_at(&ctx, &mut app, b, 5000);
    let (_, height) = lane_of(&app, t);
    let cut0 = cut(&app, a, b);
    let from = point(&app, t, B_START + 2000, 0.75);
    press_and_pull(&ctx, &mut app, from, 40.0);
    let by = match &app.gesture {
        Some(Gesture::MoveCrossfade { by, .. }) => *by,
        other => panic!("{other:?}"),
    };
    assert!(by > 10_000, "the pointer has pulled the zone a good way: {by}");
    let shapes = frame_shapes(&ctx, &mut app);
    let ghost = zone_box(&shapes, x_at(&app, t, B_START + by), x_at(&app, t, 2 * SEC + by), 2.0);
    assert!(ghost.is_some(), "the ghost box is where the zone will be");
    let thick = |x: f32| verticals(&shapes, x, height - 2.0).into_iter().filter(|w| near(*w, 2.0)).count();
    assert_eq!(thick(x_at(&app, t, cut0 + by)), 0, "no line at the cut the zone will have");
    assert_eq!(thick(x_at(&app, t, cut0)), 0, "none at the cut it has");
    // The control: the Trim tool's ghost of a clip edge is a 2-point line across the lane, and the search sees it.
    let (mut app, ctx, t, _, _) = crossfade("trim", 10 * SEC);
    let edge = point(&app, t, B_START + 2 * SEC - 400, 0.75);
    press_and_pull(&ctx, &mut app, edge, -30.0);
    assert!(matches!(app.gesture, Some(Gesture::TrimEnd { .. })), "{:?}", app.gesture);
    let shapes = frame_shapes(&ctx, &mut app);
    assert!(!verticals(&shapes, edge.x - 30.0, height * 0.9).is_empty(), "the search sees a trim line");
}

#[test]
fn a_click_in_the_zone_selects_the_crossfade_only() {
    // The Grabber anywhere in the zone, the Smart tool in its lower half away from the zone's edges; either side of the cut.
    for (tool, down) in [("grabber", 0.75), ("grabber", 0.2), ("smart", 0.75)] {
        for at in [B_START + 3500, B_START + 15_000] {
            let (mut app, ctx, t, a, b) = crossfade(tool, 10 * SEC);
            cut_at(&ctx, &mut app, b, 5000);
            let lane = lane_of(&app, t);
            let (zone_start, zone_end) = (x_at(&app, t, B_START), x_at(&app, t, 2 * SEC));
            // A clip is selected first: the crossfade replaces it.
            click_at(&ctx, &mut app, t, 3 * SEC, 0.75);
            assert_eq!(app.engine.session().edit.selected_clips, vec![b], "{tool}: B first");
            frame_shapes(&ctx, &mut app);
            assert_eq!(highlights(&frame_shapes(&ctx, &mut app), zone_start, zone_end, lane), 0);
            let (a0, b0) = (clip(&app, a), clip(&app, b));
            let steps = app.engine.undo_history().len();
            click_at(&ctx, &mut app, t, at, down);
            let ed = &app.engine.session().edit;
            let why = format!("{tool} at {down} of the lane, {at}");
            assert_eq!((ed.selection.start, ed.selection.end), (B_START, 2 * SEC), "{why}: the zone");
            assert_eq!(ed.selected_tracks, vec![t], "{why}");
            assert!(ed.selected_clips.is_empty(), "{why}: no clip is selected: {:?}", ed.selected_clips);
            assert_eq!(ed.selected_crossfade, Some(b), "{why}: it remembers which crossfade");
            assert_eq!(ed.timeline_selection, ed.selection, "{why}: the timeline selection follows");
            assert_eq!(app.engine.undo_history().len(), steps, "{why}: a selection is no edit");
            assert_eq!((clip(&app, a), clip(&app, b)), (a0, b0), "{why}");
            // The selection is highlighted like any other.
            frame_shapes(&ctx, &mut app);
            assert_eq!(highlights(&frame_shapes(&ctx, &mut app), zone_start, zone_end, lane), 1, "{why}");
        }
    }
}

#[test]
fn selecting_a_clip_of_a_crossfade_highlights_only_what_shows_of_it() {
    let (mut app, ctx, t, a, b) = crossfade("grabber", 10 * SEC);
    cut_at(&ctx, &mut app, b, 5000);
    let lane = lane_of(&app, t);
    let (start_a, cut_x, end_a) = (x_at(&app, t, 0), x_at(&app, t, cut(&app, a, b)), x_at(&app, t, 2 * SEC));
    let (start_b, end_b) = (x_at(&app, t, B_START), x_at(&app, t, B_START + 2 * SEC));
    let highlighted = |app: &mut SoundApp, ctx: &egui::Context, from: f32, to: f32| {
        frame_shapes(ctx, app);
        highlights(&frame_shapes(ctx, app), from, to, lane)
    };
    // The earlier clip, from its start to the cut: not to its end, which lies under the zone.
    click_at(&ctx, &mut app, t, B_START - 5000, 0.75);
    assert_eq!(app.engine.session().edit.selected_clips, vec![a]);
    let ed = &app.engine.session().edit;
    assert_eq!((ed.selection.start, ed.selection.end), (0, cut(&app, a, b)));
    assert_eq!(highlighted(&mut app, &ctx, start_a, cut_x), 1, "the highlight stops at the cut");
    assert_eq!(highlighted(&mut app, &ctx, start_a, end_a), 0, "and not at the earlier clip's end");
    // The later clip, from the cut to its end: not from its start, which lies under the zone.
    click_at(&ctx, &mut app, t, 2 * SEC + 5000, 0.75);
    assert_eq!(app.engine.session().edit.selected_clips, vec![b]);
    let ed = &app.engine.session().edit;
    assert_eq!((ed.selection.start, ed.selection.end), (cut(&app, a, b), B_START + 2 * SEC));
    assert_eq!(highlighted(&mut app, &ctx, cut_x, end_b), 1, "the highlight starts at the cut");
    assert_eq!(highlighted(&mut app, &ctx, start_b, end_b), 0, "and not at the later clip's start");
}

#[test]
fn a_click_that_is_not_on_the_crossfade_does_not_select_it() {
    // The upper half of the Smart tool places the insertion point, its lower half within reach of a zone edge is the
    // Trim tool's, and the Selector places the insertion point.
    for (tool, at, down) in
        [("smart", B_START + 15_000, 0.2), ("smart", B_START + 1000, 0.75), ("smart", B_START + 19_000, 0.75), ("selector", B_START + 15_000, 0.75)]
    {
        let (mut app, ctx, t, _, _) = crossfade(tool, 10 * SEC);
        click_at(&ctx, &mut app, t, at, down);
        let ed = &app.engine.session().edit;
        assert_eq!(ed.selected_crossfade, None, "{tool} at {down}, {at}");
        assert!(ed.selected_clips.is_empty(), "{tool} at {down}, {at}");
        assert_ne!((ed.selection.start, ed.selection.end), (B_START, 2 * SEC), "{tool} at {down}, {at}");
    }
    // Outside the zone the Grabber still selects the clip, and that drops the crossfade.
    let (mut app, ctx, t, a, b) = crossfade("grabber", 10 * SEC);
    click_at(&ctx, &mut app, t, B_START + 15_000, 0.75);
    assert_eq!(app.engine.session().edit.selected_crossfade, Some(b));
    click_at(&ctx, &mut app, t, B_START - 5000, 0.75);
    let ed = &app.engine.session().edit;
    assert_eq!((ed.selected_clips.clone(), ed.selected_crossfade), (vec![a], None));
}

#[test]
fn double_clicking_a_crossfade_leaves_it_selected() {
    let (mut app, ctx, t, _, b) = crossfade("grabber", 10 * SEC);
    let at = point(&app, t, B_START + 15_000, 0.75);
    // A second of frames first: egui counts a click within 0.6 s of the one before last as a triple-click.
    for _ in 0..60 {
        frame(&ctx, &mut app, vec![]);
    }
    for pressed in [true, false, true, false] {
        frame(&ctx, &mut app, vec![Event::PointerMoved(at), button(at, pressed)]);
    }
    frame(&ctx, &mut app, vec![]);
    let ed = &app.engine.session().edit;
    assert_eq!(ed.selected_crossfade, Some(b));
    assert!(ed.selected_clips.is_empty());
    assert_eq!((ed.selection.start, ed.selection.end), (B_START, 2 * SEC));
}

#[test]
fn a_selected_crossfade_that_is_dragged_stays_selected_where_it_lands() {
    let (mut app, ctx, t, a, b) = crossfade("grabber", 10 * SEC);
    let from = point(&app, t, B_START + 15_000, 0.75);
    click(&ctx, &mut app, from);
    drag_between(&ctx, &mut app, from, from + vec2(40.0, 0.0));
    let (a1, b1) = (clip(&app, a), clip(&app, b));
    assert!(b1.start > B_START + 10_000, "the crossfade moved: {}", b1.start);
    let ed = &app.engine.session().edit;
    assert_eq!((ed.selection.start, ed.selection.end), (b1.start, a1.end()), "the selection took the new zone");
    assert_eq!(soundcraft_engine::edit::selected_crossfade(app.engine.session()).map(|c| c.0), Some(b));
}

/// A key press, and a frame after it.
fn press_key(ctx: &egui::Context, app: &mut SoundApp, key: Key) {
    frame(ctx, app, vec![Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE }]);
    frame(ctx, app, vec![]);
}

#[test]
fn delete_removes_the_selected_crossfade_and_leaves_the_clips_butted_at_the_cut() {
    for key in [Key::Delete, Key::Backspace] {
        let (mut app, ctx, t, a, b) = crossfade("grabber", 10 * SEC);
        // Fades on the far sides of the pair: A's fade-in, B's fade-out. Deleting the crossfade leaves them.
        let far = |len| Fade { len, shape: FadeShape::SCurve, curvature: 0.25 };
        app.engine.session_mut().find_clip_mut(a).unwrap().fade_in = far(5000);
        app.engine.session_mut().find_clip_mut(b).unwrap().fade_out = far(6000);
        cut_at(&ctx, &mut app, b, 5000);
        let (a0, b0) = (clip(&app, a), clip(&app, b));
        let cut0 = cut(&app, a, b);
        click_at(&ctx, &mut app, t, B_START + 15_000, 0.75);
        assert_eq!(app.engine.session().edit.selected_crossfade, Some(b));
        let selected = app.engine.session().clone();
        let steps = app.engine.undo_history().len();
        press_key(&ctx, &mut app, key);
        let (a1, b1) = (clip(&app, a), clip(&app, b));
        assert_eq!((a1.end(), b1.start), (cut0, cut0), "{key:?}: butted at the cut");
        assert_eq!((a1.fade_out.len, b1.fade_in.len, b1.cut), (0, 0, None), "{key:?}: the crossfade's fades are gone");
        assert_eq!((a1.fade_in, b1.fade_out), (a0.fade_in, b0.fade_out), "{key:?}: the others stay");
        assert_eq!((a1.fade_in.len, b1.fade_out.len), (5000, 6000), "{key:?}");
        assert_eq!((a1.start, b1.end()), (a0.start, b0.end()), "{key:?}: the outer edges stay");
        assert_eq!(a1.start - a1.source_offset(), a0.start - a0.source_offset(), "{key:?}: the audio stays where it is");
        assert_eq!(b1.start - b1.source_offset(), b0.start - b0.source_offset(), "{key:?}");
        // The cut is what is left selected, not the old zone for a second Delete to clear.
        let ed = &app.engine.session().edit;
        assert_eq!((ed.selection.start, ed.selection.end, ed.selected_crossfade), (cut0, cut0, None), "{key:?}");
        // One undo step, and it brings the crossfade back, selected.
        assert_eq!(app.engine.undo_history().len(), steps + 1, "{key:?}");
        press_key(&ctx, &mut app, key);
        assert_eq!(app.engine.undo_history().len(), steps + 1, "{key:?}: a second Delete has nothing to remove");
        assert!(app.engine.undo());
        assert_eq!(app.engine.session(), &selected, "{key:?}");
        assert_eq!((clip(&app, a), clip(&app, b)), (a0, b0), "{key:?}");
    }
}

#[test]
fn delete_does_nothing_unless_a_crossfade_is_selected() {
    // A plain range (in B, past the crossfade, and over the crossfade itself), a clip, an insertion point, nothing.
    for what in ["a range in B", "a range over the zone", "a clip", "an insertion point", "nothing"] {
        for key in [Key::Delete, Key::Backspace] {
            let (mut app, ctx, t, _, _) = crossfade("grabber", 10 * SEC);
            match what {
                "a range in B" => {
                    app.engine.execute("edit.select", &json!({"tracks": [t.0], "start": 3 * SEC, "end": 3 * SEC + 24_000})).unwrap();
                }
                "a range over the zone" => {
                    app.engine.execute("edit.select", &json!({"tracks": [t.0], "start": B_START, "end": 2 * SEC})).unwrap();
                }
                "a clip" => click_at(&ctx, &mut app, t, 3 * SEC, 0.75),
                "an insertion point" => {
                    app.engine.execute("edit.select", &json!({"tracks": [t.0], "start": 3 * SEC})).unwrap();
                }
                _ => {}
            }
            frame(&ctx, &mut app, vec![]);
            assert_eq!(app.engine.session().edit.selected_crossfade, None, "{what}");
            let before = app.engine.session().clone();
            let steps = app.engine.undo_history().len();
            press_key(&ctx, &mut app, key);
            assert_eq!(app.engine.session(), &before, "{what}, {key:?}: nothing is cleared");
            assert_eq!(app.engine.undo_history().len(), steps, "{what}, {key:?}");
        }
    }
}

#[test]
fn a_stale_midi_selection_does_not_block_delete_on_a_selected_crossfade() {
    for key in [Key::Delete, Key::Backspace] {
        let (mut app, ctx, t, a, b) = crossfade("grabber", 10 * SEC);
        // The MIDI editor is open, but there is no MIDI clip to edit on this track.
        app.ui.show_midi_editor = true;
        frame(&ctx, &mut app, vec![]);
        frame(&ctx, &mut app, vec![]);
        click_at(&ctx, &mut app, t, B_START + 15_000, 0.75);
        assert_eq!(app.engine.session().edit.selected_crossfade, Some(b));
        // Note numbers left over from a clip that is no longer the one edited.
        app.midi.selected = vec![0, 1];
        assert!(soundcraft_ui_egui::midi_editor::target_clip(&app).is_none());
        press_key(&ctx, &mut app, key);
        assert_eq!(clip(&app, a).end(), clip(&app, b).start, "{key:?}: the crossfade is removed");
        assert!(app.midi.selected.is_empty(), "{key:?}: the editor let go of the stale notes");
    }
}

#[test]
fn shift_clicking_in_the_zone_extends_the_clip_selection_as_anywhere_else() {
    let (mut app, ctx, t, a, b) = crossfade("grabber", 10 * SEC);
    click_at(&ctx, &mut app, t, B_START - 5000, 0.75);
    assert_eq!(app.engine.session().edit.selected_clips, vec![a]);
    // Right of the cut, B is the clip under the pointer.
    let at = point(&app, t, B_START + 15_000, 0.75);
    frame(&ctx, &mut app, vec![Event::ModifiersChanged(Modifiers::SHIFT), Event::PointerMoved(at)]);
    frame(&ctx, &mut app, vec![Event::PointerButton { pos: at, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::SHIFT }]);
    frame(&ctx, &mut app, vec![Event::PointerButton { pos: at, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::SHIFT }]);
    frame(&ctx, &mut app, vec![Event::ModifiersChanged(Modifiers::NONE)]);
    let ed = &app.engine.session().edit;
    assert_eq!(ed.selected_clips, vec![a, b], "the multi-selection is kept and extended");
    assert_eq!(ed.selected_crossfade, None);
}

#[test]
fn delete_does_nothing_while_a_text_field_or_a_dialog_has_the_keyboard() {
    for how in ["the search field", "the rename dialog"] {
        let (mut app, ctx, t, _, b) = crossfade("grabber", 10 * SEC);
        click_at(&ctx, &mut app, t, B_START + 15_000, 0.75);
        assert_eq!(app.engine.session().edit.selected_crossfade, Some(b));
        if how == "the search field" {
            app.ui.show_search = true;
        } else {
            app.dialogs.open_rename_track(t, "Audio 1");
        }
        frame(&ctx, &mut app, vec![]);
        frame(&ctx, &mut app, vec![]);
        let before = app.engine.session().clone();
        let steps = app.engine.undo_history().len();
        for key in [Key::Delete, Key::Backspace] {
            press_key(&ctx, &mut app, key);
        }
        assert_eq!(app.engine.session(), &before, "{how}: the crossfade is still there");
        assert_eq!(app.engine.undo_history().len(), steps, "{how}");
    }
}

#[test]
fn a_clip_laid_over_a_zone_is_grabbed_instead_of_the_crossfade() {
    // D lies inside the zone and B (80 000..90 000), on top of both: it is no crossfade.
    let layered = || {
        let (mut app, ctx, t, a, b) = crossfade("grabber", 10 * SEC);
        let d = app.engine.session_mut().new_clip_id();
        let pl = app.engine.session_mut().track_mut(t).and_then(|t| t.playlist_mut()).unwrap();
        pl.clips.push(Clip::audio(d, "d", SourceId(8), 0, B_START + 4000, 10_000));
        pl.sort();
        frame(&ctx, &mut app, vec![]);
        frame(&ctx, &mut app, vec![]);
        (app, ctx, t, a, b, d)
    };
    // On D, the clip is grabbed, not the crossfade under it.
    let (mut app, ctx, t, _, _, d) = layered();
    let from = point(&app, t, B_START + 9000, 0.75);
    press_and_pull(&ctx, &mut app, from, 20.0);
    assert!(matches!(&app.gesture, Some(Gesture::MoveClips { clips, .. }) if clips == &vec![d]), "{:?}", app.gesture);
    frame(&ctx, &mut app, vec![button(from + vec2(20.0, 0.0), false)]);
    assert!(clip(&app, d).start > B_START + 4000, "D moved as a clip: {}", clip(&app, d).start);
    // Beside D, still in the zone, the crossfade is the Grabber's.
    let (mut app, ctx, t, _, b, _) = layered();
    let from = point(&app, t, B_START + 1000, 0.75);
    press_and_pull(&ctx, &mut app, from, 20.0);
    assert!(matches!(app.gesture, Some(Gesture::MoveCrossfade { clip, .. }) if clip == b), "{:?}", app.gesture);
}

#[test]
fn a_zone_that_cannot_slide_is_grabbed_as_the_clips_it_is() {
    for how in ["stretched", "edit-locked"] {
        let (mut app, ctx, t, a, _) = crossfade("grabber", 10 * SEC);
        let c = app.engine.session_mut().find_clip_mut(a).unwrap();
        if how == "stretched" {
            c.stretch = 1.5;
        } else {
            c.edit_locked = true;
        }
        frame(&ctx, &mut app, vec![]);
        // Left of the cut (the middle of the zone), the earlier clip is the one under the pointer.
        let from = point(&app, t, B_START + 3000, 0.75);
        press_and_pull(&ctx, &mut app, from, 20.0);
        assert!(matches!(&app.gesture, Some(Gesture::MoveClips { clips, .. }) if clips == &vec![a]), "{how}: {:?}", app.gesture);
    }
}
