use super::*;
use egui::accesskit::Role;
use egui_kittest::{Harness, kittest::Queryable};

/// The painted header selectors are interactive controls; their accessible name is the field
/// they edit, not the short value text ("Read", "Waveform") painted inside them.
#[test]
fn track_header_selectors_have_accessible_names() {
    let mut h = Harness::builder().with_context_setup(|ctx| crate::fonts::install(ctx, true)).build_ui(|ui| {
        let view = Rect::from_min_size(Pos2::ZERO, vec2(60.0, 17.0));
        super::header_selector(ui, view, egui::Id::unique("test-view"), "Waveform", Color32::WHITE, true, "Track view");
        let mode = Rect::from_min_size(pos2(0.0, 20.0), vec2(60.0, 17.0));
        super::header_selector(ui, mode, egui::Id::unique("test-mode"), "Read", Color32::WHITE, true, "Automation mode");
    });
    h.run();
    assert!(h.query_by_role_and_label(Role::ComboBox, "Track view").is_some());
    assert!(h.query_by_role_and_label(Role::ComboBox, "Automation mode").is_some());
}

/// Repeated per-track selectors and toggles must name their track and publish their current
/// value; a screen reader otherwise sees several identical "Solo" or "Track view" controls.
#[test]
fn repeated_track_controls_are_named_and_valued_per_track() {
    let mut h = harness_with(soundcraft_engine::demo::demo_engine());
    for name in ["Kick", "Snare"] {
        let view_label = format!("Track view for {name}");
        let view = h.query_by_role_and_label(Role::ComboBox, &view_label).unwrap_or_else(|| panic!("missing track view for {name}"));
        assert_eq!(view.value().as_deref(), Some("waveform"), "track view value for {name}");
        let voice_label = format!("Voice for {name}");
        let voice = h.query_by_role_and_label(Role::ComboBox, &voice_label).unwrap_or_else(|| panic!("missing voice for {name}"));
        assert_eq!(voice.value().as_deref(), Some("dyn"), "voice value for {name}");
        let mode_label = format!("Automation mode for {name}");
        let mode = h.query_by_role_and_label(Role::ComboBox, &mode_label).unwrap_or_else(|| panic!("missing automation mode for {name}"));
        assert_eq!(mode.value().as_deref(), Some("read"), "automation value for {name}");
        for label in ["Record enable", "Input monitoring", "Solo", "Mute"] {
            assert!(h.query_by_label(&format!("{label} for {name}")).is_some(), "missing {label} for {name}");
        }
    }
    assert!(h.query_all_by_label_contains("Track view for").count() >= 2, "both visible tracks expose their own control");

    // The Mix window repeats the same controls per strip; those names must be track-qualified
    // too, and the automation selector must publish its value rather than only a tooltip.
    h.state_mut().ui.window = crate::MainWindow::Mix;
    h.run_steps(5);
    for name in ["Kick", "Snare"] {
        let mode_label = format!("Automation mode for {name}");
        let mode = h.query_by_role_and_label(Role::ComboBox, &mode_label).unwrap_or_else(|| panic!("missing mix automation mode for {name}"));
        assert_eq!(mode.value().as_deref(), Some("auto read"), "mix automation value for {name}");
        for label in ["Input", "Output", "Group", "Record enable", "Input monitoring", "Solo", "Mute", "Insert A", "Send A"] {
            assert!(h.query_by_label(&format!("{label} for {name}")).is_some(), "missing mix {label} for {name}");
        }
    }
    // A used send's level is another per-track control; it must name its track too.
    assert!(h.query_by_label("Send A level for Snare").is_some(), "missing mix send A level for Snare");
}

/// Every track's comments editor carries its own name, so label queries and screen readers can
/// tell two tracks apart instead of seeing several identical "Comments" fields.
#[test]
fn every_track_comments_field_is_named_for_its_track() {
    let engine = soundcraft_engine::demo::demo_engine();
    let tracks: Vec<Track> = engine.session().tracks.iter().take(2).cloned().collect();
    assert_eq!(tracks.len(), 2);
    let names: Vec<String> = tracks.iter().map(|t| format!("Comments for {}", t.name)).collect();
    let app = SoundApp::new(engine, None, crate::Services::default());
    let mut h = Harness::new_ui_state(
        move |ui, app: &mut SoundApp| {
            for (i, track) in tracks.iter().enumerate() {
                let r = Rect::from_min_size(pos2(i as f32 * 200.0, 0.0), vec2(180.0, 80.0));
                header_column(app, ui, track, "comments", r, Color32::BLACK);
            }
        },
        app,
    );
    h.run();
    for name in &names {
        assert!(h.query_by_label(name).is_some(), "missing comments field {name}");
    }
}

#[test]
fn automation_id_round_trips_for_high_slots() {
    for slot in [0u8, 25, 26, 155, 156, 255] {
        for p in [AutoParam::SendLevel(slot), AutoParam::SendPan(slot), AutoParam::SendMute(slot), AutoParam::Plugin { slot, param: "trim".into() }] {
            assert_eq!(AutoParam::parse(&automation_id(&p)), Some(p));
        }
    }
}

#[test]
fn follow_jumps_to_offscreen_playhead_while_playing() {
    // Steady view, follow mode, playhead left the screen: jump to it.
    assert_eq!(
        follow_update(FollowFrame {
            playing: true,
            mode_follow: true,
            playhead_visible: false,
            cur: 1_000,
            last_scroll: 1_000,
            last_follow_to: None,
            hold: false,
            pos: 96_000
        }),
        (false, Some(96_000))
    );
}

#[test]
fn follow_leaves_onscreen_playhead_alone() {
    assert_eq!(
        follow_update(FollowFrame {
            playing: true,
            mode_follow: true,
            playhead_visible: true,
            cur: 1_000,
            last_scroll: 1_000,
            last_follow_to: None,
            hold: false,
            pos: 96_000
        }),
        (false, None)
    );
}

#[test]
fn follow_never_scrolls_when_stopped_nor_in_free_modes() {
    assert_eq!(
        follow_update(FollowFrame {
            playing: false,
            mode_follow: true,
            playhead_visible: false,
            cur: 1_000,
            last_scroll: 1_000,
            last_follow_to: None,
            hold: false,
            pos: 96_000
        }),
        (false, None)
    );
    // `none`/`after_playback` never follow, even with a steady view.
    assert_eq!(
        follow_update(FollowFrame {
            playing: true,
            mode_follow: false,
            playhead_visible: false,
            cur: 1_000,
            last_scroll: 1_000,
            last_follow_to: None,
            hold: false,
            pos: 96_000
        }),
        (false, None)
    );
    assert_eq!(
        follow_update(FollowFrame {
            playing: true,
            mode_follow: false,
            playhead_visible: false,
            cur: 9_000,
            last_scroll: 1_000,
            last_follow_to: None,
            hold: false,
            pos: 96_000
        }),
        (false, None)
    );
}

#[test]
fn outside_view_move_pauses_the_follow() {
    // The view moved and it was not our own jump: pause, no jump.
    assert_eq!(
        follow_update(FollowFrame {
            playing: true,
            mode_follow: true,
            playhead_visible: false,
            cur: 9_000,
            last_scroll: 1_000,
            last_follow_to: None,
            hold: false,
            pos: 96_000
        }),
        (true, None)
    );
    // A held view stays held while it sits still elsewhere.
    assert_eq!(
        follow_update(FollowFrame {
            playing: true,
            mode_follow: true,
            playhead_visible: false,
            cur: 9_000,
            last_scroll: 9_000,
            last_follow_to: None,
            hold: true,
            pos: 96_000
        }),
        (true, None)
    );
}

#[test]
fn own_follow_jump_is_not_mistaken_for_a_takeover() {
    // The scroll equals the target we issued last frame: keep following.
    assert_eq!(
        follow_update(FollowFrame {
            playing: true,
            mode_follow: true,
            playhead_visible: false,
            cur: 96_000,
            last_scroll: 1_000,
            last_follow_to: Some(96_000),
            hold: false,
            pos: 97_000
        }),
        (false, Some(97_000))
    );
}

#[test]
fn hold_clears_when_playhead_returns_or_transport_stops() {
    assert_eq!(
        follow_update(FollowFrame {
            playing: true,
            mode_follow: true,
            playhead_visible: true,
            cur: 9_000,
            last_scroll: 9_000,
            last_follow_to: None,
            hold: true,
            pos: 96_000
        }),
        (false, None)
    );
    assert_eq!(
        follow_update(FollowFrame {
            playing: false,
            mode_follow: true,
            playhead_visible: false,
            cur: 9_000,
            last_scroll: 9_000,
            last_follow_to: None,
            hold: true,
            pos: 96_000
        }),
        (false, None)
    );
}

fn harness(n: usize) -> Harness<'static, SoundApp> {
    let mut engine = soundcraft_engine::Engine::default();
    engine.execute("track.new", &json!({"count": n})).unwrap();
    harness_with(engine)
}

fn harness_with(engine: soundcraft_engine::Engine) -> Harness<'static, SoundApp> {
    let app = SoundApp::new(engine, None, crate::Services::default());
    let builder = Harness::builder().with_size(vec2(1400.0, 900.0)).with_step_dt(1.0 / 60.0);
    let builder = if std::env::var_os("SOUNDCRAFT_FADE_SHOTS").is_some() { builder.wgpu() } else { builder };
    let mut h = builder.build_ui_state(
        |ui, app: &mut SoundApp| {
            let ctx = ui.ctx().clone();
            app.logic(&ctx);
            app.ui(ui);
        },
        app,
    );
    h.input_mut().max_texture_side = Some(8192);
    h.run_steps(4);
    h
}

#[test]
fn a_file_time_stamp_at_the_end_of_the_range_draws() {
    let mut engine = soundcraft_engine::demo::demo_engine();
    let s = engine.session_mut();
    s.edit.flags.insert("view.clip.time.original".into());
    s.sources.iter_mut().for_each(|src| src.time_reference = i64::MAX as u64);
    harness_with(engine).run_steps(2);
}

fn smart_demo() -> Harness<'static, SoundApp> {
    let mut engine = soundcraft_engine::demo::demo_engine();
    engine.execute("edit.tool", &json!({"tool": "smart"})).unwrap();
    engine.execute("edit.mode", &json!({"mode": "slip"})).unwrap();
    harness_with(engine)
}

fn row(h: &Harness<SoundApp>, i: usize) -> Rect {
    rect(h.state().edit_layout.rows[i].1)
}

fn row_ids(h: &Harness<SoundApp>) -> Vec<TrackId> {
    h.state().edit_layout.rows.iter().map(|(i, _)| TrackId(*i)).collect()
}

fn timeline(h: &Harness<SoundApp>) -> Rect {
    rect(h.state().edit_layout.timeline)
}

fn lane_point(h: &Harness<SoundApp>, i: usize) -> Pos2 {
    pos2(timeline(h).min.x + 100.0, row(h, i).min.y + 10.0)
}

fn clip_on_screen(h: &Harness<SoundApp>, ok: impl Fn(f32, f32) -> bool) -> (Clip, Rect) {
    let (s, tl) = (h.state().engine.session(), timeline(h));
    h.state()
        .edit_layout
        .rows
        .iter()
        .filter_map(|(id, r)| s.track(TrackId(*id)).map(|t| (t, r)))
        .find_map(|(t, r)| {
            let c = t.clips().iter().find(|c| ok(x_of(s, tl, c.start), x_of(s, tl, c.end())))?;
            Some((c.clone(), lane_rect(t, rect(*r), tl)))
        })
        .expect("no such clip on screen")
}

fn start_drag(h: &mut Harness<SoundApp>, at: Pos2) {
    h.hover_at(at);
    h.drag_at(at);
    h.hover_at(at + vec2(20.0, 0.0));
    h.step();
}

fn drag(h: &mut Harness<SoundApp>, from: Pos2, to: Pos2) {
    h.hover_at(from);
    h.drag_at(from);
    for k in 1..=8 {
        h.hover_at(from + (to - from) * (k as f32 / 8.0));
    }
    h.drop_at(to);
    h.step();
}

fn quick_drag(h: &mut Harness<SoundApp>, at: Pos2, to: Pos2) {
    h.hover_at(at);
    h.drag_at(at);
    h.hover_at(at + (to - at) * 0.75);
    h.hover_at(to);
    h.drop_at(to);
    h.step();
}

fn click(h: &mut Harness<SoundApp>, at: Pos2) {
    h.hover_at(at);
    h.drag_at(at);
    h.drop_at(at);
    h.step();
}

fn click_with(h: &mut Harness<SoundApp>, at: Pos2, mods: egui::Modifiers) {
    h.event(egui::Event::ModifiersChanged(mods));
    click(h, at);
    h.event(egui::Event::ModifiersChanged(egui::Modifiers::NONE));
}

fn painted_rows(h: &Harness<SoundApp>, y_of: impl Fn(&Shape) -> Option<f32>) -> Vec<usize> {
    let rows = &h.state().edit_layout.rows;
    let mut out: Vec<usize> =
        h.output().shapes.iter().filter_map(|c| y_of(&c.shape)).filter_map(|y| rows.iter().position(|(_, r)| (r[1]..=r[3]).contains(&y))).collect();
    out.sort_unstable();
    out.dedup();
    out
}

fn highlighted_rows(h: &Harness<SoundApp>) -> Vec<usize> {
    painted_rows(h, |shape| match shape {
        Shape::Rect(r) if r.fill == Tokens::current().selection => Some(r.rect.center().y),
        _ => None,
    })
}

fn insertion_rows(h: &Harness<SoundApp>) -> Vec<usize> {
    painted_rows(h, |shape| match shape {
        Shape::LineSegment { points, stroke } if stroke.color == Tokens::current().insertion => Some((points[0].y + points[1].y) / 2.0),
        _ => None,
    })
}

#[test]
fn drag_selection_spans_the_rows_from_anchor_to_pointer() {
    let mut h = harness(4);
    let ids = row_ids(&h);
    for (from, to) in [(3, 2), (0, 1), (2, 0), (1, 3)] {
        let (a, b) = (lane_point(&h, from), lane_point(&h, to) + vec2(80.0, 0.0));
        drag(&mut h, a, b);
        let want = ids.get(from.min(to)..=from.max(to)).unwrap();
        assert_eq!(h.state().engine.session().edit.selected_tracks, want, "drag from row {from} to row {to}");
    }
}

#[test]
fn the_frame_a_drag_reaches_a_row_paints_it_selected() {
    let mut h = harness(4);
    let (a, b) = (lane_point(&h, 3), lane_point(&h, 2) + vec2(60.0, 0.0));
    start_drag(&mut h, a);
    h.hover_at(b);
    h.step();
    assert_eq!(highlighted_rows(&h), [2, 3]);
    h.drop_at(b);
    h.step();
}

#[test]
fn a_drag_paints_each_frame_once() {
    let mut h = harness(4);
    let (a, b) = (lane_point(&h, 3), lane_point(&h, 0) + vec2(300.0, 0.0));
    start_drag(&mut h, a);
    for k in 1..=10 {
        h.hover_at(a + (b - a) * (k as f32 / 10.0));
        h.step();
        assert_eq!(h.output().platform_output.num_completed_passes, 1, "move {k}");
    }
    h.drop_at(b);
    h.step();
}

#[test]
fn a_drag_frame_changes_the_document_once() {
    let mut h = harness(4);
    let (a, b) = (lane_point(&h, 3), lane_point(&h, 2) + vec2(60.0, 0.0));
    start_drag(&mut h, a);
    let rev = h.state().engine.revision;
    h.hover_at(b);
    h.step();
    assert_eq!(h.state().engine.revision, rev + 1);
    h.drop_at(b);
    h.step();
}

#[test]
fn a_drag_selection_starts_where_the_button_went_down() {
    let mut h = harness(4);
    let a = lane_point(&h, 1);
    drag(&mut h, a, a + vec2(200.0, 0.0));
    let s = h.state().engine.session();
    assert_eq!(s.edit.selection.start, sample_at(s, timeline(&h), a.x));
}

#[test]
fn pressing_in_a_lane_places_the_insertion_point_before_release() {
    let mut h = harness(4);
    let a = lane_point(&h, 2);
    h.hover_at(a);
    h.drag_at(a);
    h.step();
    let s = h.state().engine.session();
    assert_eq!(s.edit.selection, Range::point(sample_at(s, timeline(&h), a.x)));
    assert_eq!(s.edit.selected_tracks, row_ids(&h)[2..3]);
    h.drop_at(a);
    h.step();
}

#[test]
fn a_moved_insertion_point_shows_at_once_even_mid_blink() {
    let mut h = harness(4);
    let (a, b) = (lane_point(&h, 0), lane_point(&h, 2) + vec2(150.0, 0.0));
    click(&mut h, a);
    for _ in 0..90 {
        if insertion_rows(&h).is_empty() {
            break;
        }
        h.step();
    }
    assert!(insertion_rows(&h).is_empty(), "the insertion point never blinked off");
    click(&mut h, b);
    assert!(!insertion_rows(&h).is_empty());
}

#[test]
fn the_insertion_point_shows_only_on_selected_tracks() {
    let mut h = harness(4);
    let a = lane_point(&h, 2);
    click(&mut h, a);
    assert_eq!(insertion_rows(&h), [2]);
}

#[test]
fn shift_click_adds_another_track_to_the_selection() {
    let mut h = harness(4);
    let ids = row_ids(&h);
    let (a, b, c) = (lane_point(&h, 0), lane_point(&h, 2) + vec2(100.0, 0.0), lane_point(&h, 3) + vec2(300.0, 0.0));
    drag(&mut h, a, a + vec2(200.0, 0.0));
    let sel = h.state().engine.session().edit.selection;
    click_with(&mut h, b, egui::Modifiers::SHIFT);
    assert_eq!(h.state().engine.session().edit.selected_tracks, [ids[0], ids[2]]);
    assert_eq!(h.state().engine.session().edit.selection, sel);
    click_with(&mut h, c, egui::Modifiers::SHIFT);
    let s = h.state().engine.session();
    assert_eq!(s.edit.selected_tracks, [ids[0], ids[2], ids[3]]);
    assert_eq!(s.edit.selection, Range::new(sel.start, sample_at(s, timeline(&h), c.x)));
    assert_eq!(highlighted_rows(&h), [0, 2, 3]);
}

#[test]
fn shift_and_cmd_click_on_track_names_extend_the_selection() {
    let mut h = harness(5);
    let ids = row_ids(&h);
    let (none, shift, cmd) = (egui::Modifiers::NONE, egui::Modifiers::SHIFT, egui::Modifiers::COMMAND);
    for (i, mods, want) in [(1, none, vec![1]), (3, shift, vec![1, 2, 3]), (2, cmd, vec![1, 3]), (0, cmd, vec![0, 1, 3]), (4, none, vec![4])] {
        let at = row(&h, i).min + vec2(60.0, 10.0);
        click_with(&mut h, at, mods);
        let want: Vec<TrackId> = want.into_iter().map(|k| ids[k]).collect();
        assert_eq!(h.state().engine.session().edit.selected_tracks, want, "row {i} clicked with {mods:?}");
    }
}

#[test]
fn a_quick_drag_from_a_clip_edge_trims_it() {
    let mut h = smart_demo();
    let tl = timeline(&h);
    let (clip, lane) = clip_on_screen(&h, |xs, xe| xe > tl.min.x + 100.0 && xe < tl.max.x - 20.0 && xe - xs > 100.0);
    let press = pos2(x_of(h.state().engine.session(), tl, clip.end()) - 2.0, lane.min.y + lane.height() * 0.75);
    quick_drag(&mut h, press, press - vec2(40.0, 0.0));
    let after = h.state().engine.session().find_clip(clip.id).unwrap().1.clone();
    assert_eq!(after.start, clip.start, "the clip moved instead of being trimmed");
    assert!(after.end() < clip.end(), "the clip end did not move");
}

#[test]
fn a_quick_clip_drag_keeps_the_clip_under_the_pointer() {
    let mut h = smart_demo();
    let tl = timeline(&h);
    let (clip, lane) = clip_on_screen(&h, |xs, xe| xe - xs > 120.0 && (xs + xe) / 2.0 > tl.min.x + 60.0 && (xs + xe) / 2.0 < tl.max.x - 60.0);
    let s = h.state().engine.session();
    let grab = pos2((x_of(s, tl, clip.start) + x_of(s, tl, clip.end())) / 2.0, lane.min.y + lane.height() * 0.75);
    let by = sample_at(s, tl, grab.x + 40.0) - sample_at(s, tl, grab.x);
    quick_drag(&mut h, grab, grab + vec2(40.0, 0.0));
    let after = h.state().engine.session().find_clip(clip.id).unwrap().1.clone();
    assert_eq!(after.start - clip.start, by, "the clip trails the pointer");
}

#[test]
fn fade_drag_ranges_handle_empty_short_and_saturated_clips() {
    let mut c = Clip::audio(ClipId(1), "test", soundcraft_model::SourceId(1), 0, 100, 1);
    for side in [true, false] {
        for to in [i64::MIN, 100, 101, i64::MAX] {
            assert_eq!(fade_drag_range(&c, side, to), Some(Range::new(100, 101)));
        }
        c.length = 0;
        assert_eq!(fade_drag_range(&c, side, 100), None);
        c.length = -1;
        assert_eq!(fade_drag_range(&c, side, 100), None);
        c.length = 1;
    }
    c.start = i64::MAX - 1;
    c.length = 10;
    for side in [true, false] {
        assert_eq!(fade_drag_range(&c, side, i64::MAX), Some(Range::new(i64::MAX - 1, i64::MAX)));
        c.start = i64::MAX;
        assert_eq!(fade_drag_range(&c, side, i64::MAX), None);
        c.start = i64::MAX - 1;
    }
}

fn fade_shot(h: &mut Harness<SoundApp>, name: &str) {
    if let Some(dir) = std::env::var_os("SOUNDCRAFT_FADE_SHOTS") {
        let dir = std::path::PathBuf::from(dir);
        std::fs::create_dir_all(&dir).unwrap();
        h.render().unwrap().save(dir.join(format!("{name}.png"))).unwrap();
    }
}

fn fade_curves(h: &Harness<SoundApp>, clip_rect: Rect) -> Vec<(egui::epaint::PathShape, Rect)> {
    h.output()
        .shapes
        .iter()
        .filter_map(|shape| match &shape.shape {
            Shape::Path(path)
                if path.stroke == egui::epaint::PathStroke::new(1.2, Color32::from_rgba_unmultiplied(255, 255, 255, 90))
                    && path.points.iter().all(|point| clip_rect.contains(*point)) =>
            {
                Some((path.clone(), shape.clip_rect))
            }
            _ => None,
        })
        .collect()
}

fn waveform_geometry(h: &Harness<SoundApp>, clip_rect: Rect) -> Vec<Vec<Pos2>> {
    h.output()
        .shapes
        .iter()
        .filter_map(|shape| match &shape.shape {
            // The waveform's columns snap to physical pixels and fade out over half of one.
            Shape::Mesh(mesh) if !mesh.vertices.is_empty() && mesh.vertices.iter().all(|vertex| clip_rect.expand(1.0).contains(vertex.pos)) => {
                Some(mesh.vertices.iter().map(|vertex| vertex.pos).collect())
            }
            _ => None,
        })
        .collect()
}

/// Replace the clip's mono source with a stereo one whose right channel is half the left, and make the track stereo.
fn make_stereo(h: &mut Harness<'static, SoundApp>, tid: soundcraft_model::TrackId, source: soundcraft_model::SourceId) {
    let audio = h.state().engine.session().pool.get(source).unwrap().clone();
    let left = audio.buffer.channels[0].clone();
    let right = left.iter().map(|sample| sample * 0.5).collect();
    let audio =
        soundcraft_model::SourceAudio::new(soundcraft_audio_io::AudioBuffer { sample_rate: audio.buffer.sample_rate, channels: vec![left, right] });
    let s = h.state_mut().engine.session_mut();
    s.pool.insert(source, std::sync::Arc::new(audio));
    s.track_mut(tid).unwrap().format = soundcraft_model::ChannelFormat::Stereo;
}

/// Where a fade drag presses and where it releases: the curve midpoint (bend) or the fade's end (length).
fn fade_drag_points(
    session: &soundcraft_model::Session,
    tl: Rect,
    lane: Rect,
    clip: soundcraft_model::ClipId,
    fade_in: bool,
    bend: bool,
) -> (Pos2, Pos2) {
    let c = session.find_clip(clip).unwrap().1;
    let (xs, xe) = (x_of(session, tl, c.start), x_of(session, tl, c.end()));
    if bend {
        let midpoint = fade_midpoint(session, tl, lane, c, fade_in).unwrap();
        (midpoint, midpoint - vec2(0.0, 16.0))
    } else {
        (pos2(if fade_in { xs + 6.0 } else { xe - 6.0 }, lane.min.y + 4.0), pos2(xs + (xe - xs) * if fade_in { 0.7 } else { 0.3 }, lane.min.y + 4.0))
    }
}

fn fade_waveform_case(channels: usize, fade_in: bool, bend: bool, mode: &str) {
    let mut h = smart_demo();
    let tl = timeline(&h);
    let (clip, lane) = clip_on_screen(&h, |xs, xe| xs > tl.min.x + 30.0 && xe < tl.max.x - 100.0 && xe - xs > 100.0);
    let (tid, _) = h.state().engine.session().find_clip(clip.id).unwrap();
    let neighbour = h.state().engine.session().track(tid).unwrap().clips().iter().find(|c| c.id != clip.id && c.start >= clip.end()).unwrap().clone();
    let source = clip.source().unwrap();
    if channels == 2 {
        make_stereo(&mut h, tid, source);
    }
    if mode == "power" {
        h.state_mut().engine.execute("view.waveform_power", &json!({})).unwrap();
    }
    h.state_mut().engine.execute("view.waveform_rectified", &json!({"value": mode == "rectified"})).unwrap();
    h.state_mut().engine.execute("view.waveform_outlines", &json!({"value": mode == "outlines"})).unwrap();
    h.state_mut().engine.execute("view.waveform_zoom", &json!({"value": 1.4})).unwrap();
    let c = h.state_mut().engine.session_mut().find_clip_mut(clip.id).unwrap();
    c.gain_db = -3.0;
    c.gain_env = vec![(0, -3.0), (c.length, 3.0)];
    let target = soundcraft_model::Fade {
        len: if bend { c.length * 2 / 3 } else { c.length / 4 },
        shape: soundcraft_model::FadeShape::SCurve,
        curvature: 0.0,
    };
    let other = soundcraft_model::Fade { len: c.length / 8, shape: soundcraft_model::FadeShape::Linear, curvature: 0.2 };
    (c.fade_in, c.fade_out) = if fade_in { (target, other) } else { (other, target) };
    h.step();
    let before = h.state().engine.session().clone();
    let undo_len = h.state().engine.undo_history().len();
    let audio = before.pool.get(source).unwrap().clone();
    let (xs, xe) = (x_of(&before, tl, clip.start), x_of(&before, tl, clip.end()));
    let clip_rect = Rect::from_min_max(pos2(xs, lane.min.y), pos2(xe, lane.max.y)).intersect(tl);
    let neighbour_rect =
        Rect::from_min_max(pos2(x_of(&before, tl, neighbour.start), lane.min.y), pos2(x_of(&before, tl, neighbour.end()), lane.max.y)).intersect(tl);
    let original = waveform_geometry(&h, clip_rect);
    let neighbour_wave = waveform_geometry(&h, neighbour_rect);
    assert_eq!(original.len(), channels);
    let side = if fade_in { "in" } else { "out" };
    let operation = if bend { "bend" } else { "length" };
    let screenshot = channels == 1 && mode == "peak";
    if screenshot {
        fade_shot(&mut h, &format!("waveform-{side}-{operation}-before"));
    }
    let (press, to) = fade_drag_points(&before, tl, lane, clip.id, fade_in, bend);
    h.hover_at(press);
    h.drag_at(press);
    h.hover_at(to);
    h.step();
    h.step();
    assert!(if bend {
        matches!(h.state().gesture, Some(Gesture::FadeCurve { .. }))
    } else {
        matches!(h.state().gesture, Some(Gesture::Fade { .. }))
    });
    let preview = waveform_geometry(&h, clip_rect);
    assert_eq!(preview.len(), channels);
    for channel in 0..channels {
        assert_ne!(
            preview[channel], original[channel],
            "the waveform must respond while the button is held: {side}, {operation}, {mode}, channel={channel}"
        );
    }
    assert_eq!(waveform_geometry(&h, neighbour_rect), neighbour_wave);
    assert_eq!(h.state().engine.session(), &before);
    assert_eq!(h.state().engine.undo_history().len(), undo_len);
    assert!(std::sync::Arc::ptr_eq(h.state().engine.session().pool.get(source).unwrap(), &audio), "preview must retain cached media and peaks");
    if screenshot {
        fade_shot(&mut h, &format!("waveform-{side}-{operation}-preview"));
    }
    h.drop_at(to);
    h.step();
    assert!(h.state().gesture.is_none());
    assert_eq!(waveform_geometry(&h, clip_rect), preview, "release must not cause a waveform jump");
    assert_eq!(waveform_geometry(&h, neighbour_rect), neighbour_wave);
    assert_eq!(h.state().engine.undo_history().len(), undo_len + 1);
    assert!(std::sync::Arc::ptr_eq(h.state().engine.session().pool.get(source).unwrap(), &audio));
    if screenshot {
        fade_shot(&mut h, &format!("waveform-{side}-{operation}-released"));
    }
    assert!(h.state_mut().engine.undo());
    assert_eq!(h.state().engine.session(), &before);
    h.step();
    assert_eq!(waveform_geometry(&h, clip_rect), original);
}

#[test]
fn waveforms_follow_fade_drags_before_release_and_match_the_committed_waveform() {
    for channels in [1, 2] {
        for fade_in in [true, false] {
            for bend in [false, true] {
                for mode in ["peak", "power", "rectified", "outlines"] {
                    fade_waveform_case(channels, fade_in, bend, mode);
                }
            }
        }
    }
}

#[test]
fn fade_drag_previews_and_commits_stay_inside_the_clip() {
    for fade_in in [true, false] {
        for release_outside in [false, true] {
            let mut h = smart_demo();
            let tl = timeline(&h);
            let (clip, lane) = clip_on_screen(&h, |xs, xe| xs > tl.min.x + 30.0 && xe < tl.max.x - 100.0 && xe - xs > 100.0);
            let s = h.state().engine.session();
            let (xs, xe) = (x_of(s, tl, clip.start), x_of(s, tl, clip.end()));
            let corner = pos2(if fade_in { xs + 6.0 } else { xe - 6.0 }, lane.min.y + 4.0);
            let before = s.clone();
            let undo_len = h.state().engine.undo_history().len();
            start_drag(&mut h, corner);
            assert!(matches!(h.state().gesture, Some(Gesture::Fade { fade_in: side, .. }) if side == fade_in));
            let name = if fade_in { "fade-in" } else { "fade-out" };
            for (label, x) in [("left", xs - 60.0), ("right", xe + 60.0), ("inside", (xs + xe) / 2.0)] {
                h.hover_at(pos2(x, corner.y));
                h.step();
                let s = h.state().engine.session();
                let range = fade_drag_range(&clip, fade_in, sample_at(s, tl, x)).unwrap();
                let curves = fade_curves(&h, Rect::from_min_max(pos2(xs, lane.min.y), pos2(xe, lane.max.y)));
                assert_eq!(curves.len(), 1, "only one fade curve should be painted");
                let (curve, clip_rect) = &curves[0];
                let points = &curve.points;
                assert_eq!(points.first().unwrap().x, x_of(s, tl, range.start));
                assert_eq!(points.last().unwrap().x, x_of(s, tl, range.end));
                assert!(points.iter().all(|p| p.x >= xs && p.x <= xe && lane.contains(*p)));
                assert!(clip_rect.min.x >= xs && clip_rect.max.x <= xe);
                assert_eq!(h.state().engine.session(), &before, "drag must only preview");
                if !release_outside {
                    fade_shot(&mut h, &format!("{name}-{label}"));
                }
            }
            // Releasing at a new point without an intervening move must use that point.
            let release = pos2(if release_outside { if fade_in { xe + 60.0 } else { xs - 60.0 } } else { xs + (xe - xs) * 0.3 }, corner.y);
            h.drop_at(release);
            h.step();
            assert!(h.state().gesture.is_none());
            let range = fade_drag_range(&clip, fade_in, sample_at(&before, tl, release.x)).unwrap();
            let mut expected = before.clone();
            let c = expected.find_clip_mut(clip.id).unwrap();
            let fade = soundcraft_model::Fade { len: range.end - range.start, shape: soundcraft_model::FadeShape::SCurve, curvature: 0.0 };
            if fade_in {
                c.fade_in = fade;
            } else {
                c.fade_out = fade;
            }
            assert_eq!(h.state().engine.session(), &expected);
            assert_eq!(h.state().engine.undo_history().len(), undo_len + 1);
            if release_outside {
                assert_eq!(fade.len, clip.length);
                fade_shot(&mut h, &format!("{name}-committed"));
            }
            assert!(h.state_mut().engine.undo());
            assert_eq!(h.state().engine.session(), &before);
            assert!(h.state_mut().engine.redo());
            assert_eq!(h.state().engine.session(), &expected);
        }
    }
}

#[test]
fn a_fade_drag_replaces_the_old_curve_and_matches_the_released_curve() {
    for fade_in in [true, false] {
        for show_name in [true, false] {
            let mut h = smart_demo();
            h.state_mut().engine.execute("view.clip_name", &json!({"value": show_name})).unwrap();
            h.step();
            let tl = timeline(&h);
            let (clip, lane) = clip_on_screen(&h, |xs, xe| xs > tl.min.x + 30.0 && xe < tl.max.x - 100.0 && xe - xs > 100.0);
            let c = h.state_mut().engine.session_mut().find_clip_mut(clip.id).unwrap();
            c.fade_in = soundcraft_model::Fade { len: c.length / 8, shape: soundcraft_model::FadeShape::Linear, curvature: 0.0 };
            c.fade_out = soundcraft_model::Fade { len: c.length / 8, shape: soundcraft_model::FadeShape::EqualPower, curvature: 0.0 };
            h.step();
            let s = h.state().engine.session();
            let (xs, xe) = (x_of(s, tl, clip.start), x_of(s, tl, clip.end()));
            let clip_rect = Rect::from_min_max(pos2(xs, lane.min.y), pos2(xe, lane.max.y));
            let original = fade_curves(&h, clip_rect);
            assert_eq!(original.len(), 2);
            let (edited, opposite) = if fade_in { (0, 1) } else { (1, 0) };
            let before = s.clone();
            let corner = pos2(if fade_in { xs + 6.0 } else { xe - 6.0 }, lane.min.y + 4.0);
            start_drag(&mut h, corner);
            let to = pos2(if fade_in { xs + (xe - xs) * 0.65 } else { xs + (xe - xs) * 0.35 }, corner.y);
            h.hover_at(to);
            h.step();
            let preview = fade_curves(&h, clip_rect);
            assert_eq!(preview.len(), 2, "one curve per side, without the old curve underneath");
            assert_ne!(preview[edited], original[edited]);
            assert_eq!(preview[opposite], original[opposite]);
            assert_eq!(h.state().engine.session(), &before);
            assert!(!h.output().shapes.iter().any(|shape| {
                matches!(&shape.shape, Shape::LineSegment { points, stroke } if stroke.width == 2.0 && stroke.color == Color32::WHITE && points.iter().all(|p| clip_rect.contains(*p)))
            }), "the straight preview must be gone");
            let side = if fade_in { "in" } else { "out" };
            if show_name {
                fade_shot(&mut h, &format!("fade-{side}-curve-dragging"));
            }
            h.drop_at(to);
            h.step();
            assert_eq!(fade_curves(&h, clip_rect), preview, "curve geometry, thickness and colour must be identical after release");
            if show_name {
                fade_shot(&mut h, &format!("fade-{side}-curve-released"));
            }
        }
    }
}

#[test]
fn fade_curve_sampling_keeps_steep_bends_below_a_quarter_pixel_error() {
    for (width, height) in [(420.0, 180.0), (75.0, 320.0), (10_000.0, 80.0)] {
        let body = Rect::from_min_size(Pos2::ZERO, vec2(width, height));
        for scale in [1.0, 2.0, 4.0] {
            for shape in soundcraft_model::FadeShape::ALL {
                for curvature in [-1.0, 1.0] {
                    for fade_in in [true, false] {
                        let fade = soundcraft_model::Fade { len: 1000, shape, curvature };
                        let points = fade_curve_points(fade, fade_in, 0.0, width, body, scale);
                        assert!(points.len() > 17 && points.len() <= 4097);
                        assert_eq!((points.len() - 1) % 2, 0);
                        let segments = (points.len() - 1) as f32;
                        for (i, pair) in points.windows(2).enumerate() {
                            let line = pair[1] - pair[0];
                            for fraction in [0.25, 0.5, 0.75] {
                                let t = (i as f32 + fraction) / segments;
                                let actual = pos2(width * t, height - height * fade.gain(if fade_in { t } else { 1.0 - t }));
                                let along = ((actual - pair[0]).dot(line) / line.length_sq()).clamp(0.0, 1.0);
                                let distance = actual.distance(pair[0] + line * along) * scale;
                                assert!(
                                    distance < 0.25,
                                    "{width}x{height}, scale={scale}, {shape:?}, curve={curvature}, side={fade_in}, error={distance}"
                                );
                            }
                        }
                    }
                }
            }
        }
    }
    let body = Rect::from_min_size(Pos2::ZERO, vec2(1e20, 1e20));
    for scale in [f32::NAN, f32::INFINITY, 1.0] {
        let points = fade_curve_points(soundcraft_model::Fade::default(), true, 0.0, 1e20, body, scale);
        assert!(points.len() <= 4097);
    }
}

#[test]
fn narrow_fades_take_few_points_and_stay_within_a_quarter_pixel() {
    for width in [0.2f32, 1.0, 3.0, 12.0] {
        for height in [40.0f32, 320.0] {
            let body = Rect::from_min_size(Pos2::ZERO, vec2(width, height));
            for scale in [1.0, 2.0] {
                for shape in soundcraft_model::FadeShape::ALL {
                    for curvature in [-1.0, 0.0, 1.0] {
                        let fade = soundcraft_model::Fade { len: 1000, shape, curvature };
                        let points = fade_curve_points(fade, true, 0.0, width, body, scale);
                        assert!(points.len() <= ((width * scale * 4.0).ceil() as usize + 4), "{width} wide: {} points", points.len());
                        assert_eq!((points.len() - 1) % 2, 0, "an exact midpoint");
                        let segments = (points.len() - 1) as f32;
                        for (i, pair) in points.windows(2).enumerate() {
                            let line = pair[1] - pair[0];
                            for fraction in [0.1, 0.25, 0.5, 0.75, 0.9] {
                                let t = (i as f32 + fraction) / segments;
                                let actual = pos2(width * t, height - height * fade.gain(t));
                                let along =
                                    if line.length_sq() > 0.0 { ((actual - pair[0]).dot(line) / line.length_sq()).clamp(0.0, 1.0) } else { 0.0 };
                                let distance = actual.distance(pair[0] + line * along) * scale;
                                assert!(distance < 0.25, "{width}x{height}, scale={scale}, {shape:?}, curve={curvature}, error={distance}");
                            }
                        }
                    }
                }
            }
        }
    }
}

fn painted_fade_control(h: &Harness<SoundApp>, clip_rect: Rect) -> Pos2 {
    let controls: Vec<_> = h
        .output()
        .shapes
        .iter()
        .filter_map(|shape| match &shape.shape {
            Shape::Circle(circle) if circle.radius == 3.5 && clip_rect.contains(circle.center) => {
                let radius = circle.radius + circle.stroke.width * 0.5;
                for offset in [vec2(radius, 0.0), vec2(-radius, 0.0), vec2(0.0, radius), vec2(0.0, -radius)] {
                    assert!(shape.clip_rect.contains(circle.center + offset), "the complete control must remain visible");
                }
                Some(circle.center)
            }
            _ => None,
        })
        .collect();
    assert_eq!(controls.len(), 1, "the midpoint must stay visible without hover or a held mouse button");
    controls[0]
}

#[test]
fn fade_midpoint_handles_remain_visible_and_can_be_grabbed_again() {
    for fade_in in [true, false] {
        for show_name in [true, false] {
            for shape in [soundcraft_model::FadeShape::SCurve, soundcraft_model::FadeShape::Exponential] {
                for height in ["micro", "jumbo"] {
                    let mut h = smart_demo();
                    let tl = timeline(&h);
                    let (clip, _) = clip_on_screen(&h, |xs, xe| xs > tl.min.x + 30.0 && xe < tl.max.x - 100.0 && xe - xs > 100.0);
                    let (tid, _) = h.state().engine.session().find_clip(clip.id).unwrap();
                    let spp = h.state().engine.session().edit.zoom.samples_per_px * 0.5;
                    h.state_mut().engine.execute("view.clip_name", &json!({"value": show_name})).unwrap();
                    h.state_mut().engine.execute("track.height", &json!({"tracks": [tid.0], "height": height})).unwrap();
                    h.state_mut().engine.execute("view.zoom_set", &json!({"samples_per_px": spp})).unwrap();
                    h.state_mut().engine.execute("view.scroll", &json!({"to": clip.start - clip.length / 5})).unwrap();
                    let c = h.state_mut().engine.session_mut().find_clip_mut(clip.id).unwrap();
                    let fade = soundcraft_model::Fade { len: c.length * 2 / 3, shape, curvature: 0.0 };
                    if fade_in {
                        c.fade_in = fade;
                    } else {
                        c.fade_out = fade;
                    }
                    h.step();
                    let row = h.state().edit_layout.rows.iter().find(|(id, _)| *id == tid.0).unwrap().1;
                    let lane = lane_rect(h.state().engine.session().track(tid).unwrap(), rect(row), tl);
                    let s = h.state().engine.session();
                    let clip_rect = Rect::from_min_max(pos2(x_of(s, tl, clip.start), lane.min.y), pos2(x_of(s, tl, clip.end()), lane.max.y));
                    let control = painted_fade_control(&h, clip_rect);
                    let undo_len = h.state().engine.undo_history().len();
                    start_drag(&mut h, control);
                    let outside = control + vec2(0.0, 500.0);
                    h.hover_at(outside);
                    h.step();
                    h.drop_at(outside);
                    h.step();
                    let c = h.state().engine.session().find_clip(clip.id).unwrap().1;
                    assert_eq!(if fade_in { c.fade_in.curvature } else { c.fade_out.curvature }, -1.0);
                    assert!(h.state().gesture.is_none());
                    painted_fade_control(&h, clip_rect);
                    let side = if fade_in { "in" } else { "out" };
                    let screenshot = show_name && height == "jumbo" && shape == soundcraft_model::FadeShape::SCurve;
                    if screenshot {
                        fade_shot(&mut h, &format!("fade-{side}-smooth-released"));
                    }
                    for iteration in 0..2 {
                        let before = h.state().engine.session().clone();
                        let control = painted_fade_control(&h, clip_rect);
                        h.hover_at(control);
                        h.step();
                        assert_eq!(h.output().platform_output.cursor_icon, egui::CursorIcon::ResizeVertical);
                        start_drag(&mut h, control);
                        let to = control - vec2(0.0, (lane.height() - 2.0 - clip_name_height(&before, lane)) * 0.1);
                        h.hover_at(to);
                        h.step();
                        assert!(
                            matches!(h.state().gesture, Some(Gesture::FadeCurve { clip: id, fade_in: side, .. }) if id == clip.id && side == fade_in)
                        );
                        h.drop_at(to);
                        h.step();
                        let c = h.state().engine.session().find_clip(clip.id).unwrap().1;
                        let current = if fade_in { c.fade_in } else { c.fade_out };
                        let previous = before.find_clip(clip.id).unwrap().1;
                        let previous = if fade_in { previous.fade_in } else { previous.fade_out };
                        assert!(current.curvature > previous.curvature);
                        assert_eq!((current.len, current.shape), (previous.len, previous.shape));
                        painted_fade_control(&h, clip_rect);
                        assert_eq!(h.state().engine.undo_history().len(), undo_len + iteration + 2);
                        if screenshot && iteration == 1 {
                            fade_shot(&mut h, &format!("fade-{side}-smooth-regrabbed"));
                        }
                        let after = h.state().engine.session().clone();
                        assert!(h.state_mut().engine.undo());
                        assert_eq!(h.state().engine.session(), &before);
                        assert!(h.state_mut().engine.redo());
                        assert_eq!(h.state().engine.session(), &after);
                    }
                }
            }
        }
    }
}

/// `session` with the clip's fade-in (or fade-out) curvature set.
fn with_curvature(session: &soundcraft_model::Session, clip: soundcraft_model::ClipId, fade_in: bool, curvature: f32) -> soundcraft_model::Session {
    let mut expected = session.clone();
    let c = expected.find_clip_mut(clip).unwrap();
    if fade_in {
        c.fade_in.curvature = curvature;
    } else {
        c.fade_out.curvature = curvature;
    }
    expected
}

/// The screenshot name for a fade bend drag.
fn bend_shot_name(fade_in: bool, dy: f32, stage: &str) -> String {
    let side = if fade_in { "in" } else { "out" };
    let direction = if dy < 0.0 { "up" } else { "down" };
    format!("fade-{side}-bend-{direction}-{stage}")
}

fn fade_midpoint_bend_case(fade_in: bool, show_name: bool, dy: f32) {
    let mut h = smart_demo();
    h.state_mut().engine.execute("view.clip_name", &json!({"value": show_name})).unwrap();
    h.step();
    let tl = timeline(&h);
    let (clip, lane) = clip_on_screen(&h, |xs, xe| xs > tl.min.x + 30.0 && xe < tl.max.x - 100.0 && xe - xs > 100.0);
    let c = h.state_mut().engine.session_mut().find_clip_mut(clip.id).unwrap();
    let target = soundcraft_model::Fade { len: c.length * 2 / 3, shape: soundcraft_model::FadeShape::SCurve, curvature: 0.0 };
    let other = soundcraft_model::Fade { len: c.length / 8, shape: soundcraft_model::FadeShape::Linear, curvature: -0.2 };
    (c.fade_in, c.fade_out) = if fade_in { (target, other) } else { (other, target) };
    h.step();
    let before = h.state().engine.session().clone();
    let clip_rect = Rect::from_min_max(pos2(x_of(&before, tl, clip.start), lane.min.y), pos2(x_of(&before, tl, clip.end()), lane.max.y));
    let original = fade_curves(&h, clip_rect);
    let (edited, opposite) = if fade_in { (0, 1) } else { (1, 0) };
    let middle = original[edited].0.points.len() / 2;
    let midpoint = original[edited].0.points[middle];
    let undo_len = h.state().engine.undo_history().len();
    h.hover_at(midpoint);
    h.step();
    assert_eq!(h.output().platform_output.cursor_icon, egui::CursorIcon::ResizeVertical);
    click(&mut h, midpoint);
    assert_eq!(h.state().engine.session(), &before, "clicking the curve control should not change the selection");
    h.drag_at(midpoint);
    let to = midpoint + vec2(0.0, dy);
    h.hover_at(to);
    h.step();
    h.step(); // Paint the frame requested when the drag gesture starts.
    let curvature = match h.state().gesture {
        Some(Gesture::FadeCurve { clip: id, fade_in: side, curvature, .. }) if id == clip.id && side == fade_in => curvature,
        _ => panic!("the midpoint must start a curve gesture"),
    };
    assert_eq!(h.output().platform_output.cursor_icon, egui::CursorIcon::ResizeVertical);
    assert!(if dy < 0.0 { curvature > 0.0 } else { curvature < 0.0 });
    let preview = fade_curves(&h, clip_rect);
    assert_eq!(preview.len(), 2);
    assert_eq!(preview[opposite], original[opposite]);
    assert_eq!(preview[edited].0.points.first(), original[edited].0.points.first());
    assert_eq!(preview[edited].0.points.last(), original[edited].0.points.last());
    assert_eq!(preview[edited].0.points[middle].x, midpoint.x);
    assert!(
        (preview[edited].0.points[middle].y - (midpoint.y + dy)).abs() < 0.001,
        "side={fade_in}, names={show_name}, dy={dy}, midpoint={midpoint:?}, drawn={:?}, gesture={:?}",
        preview[edited].0.points[middle],
        h.state().gesture
    );
    assert_eq!(h.state().engine.session(), &before);
    if show_name {
        fade_shot(&mut h, &bend_shot_name(fade_in, dy, "preview"));
    }
    h.drop_at(to);
    h.step();
    let expected = with_curvature(&before, clip.id, fade_in, curvature);
    assert_eq!(h.state().engine.session(), &expected);
    assert_eq!(fade_curves(&h, clip_rect), preview, "the released envelope must match the preview");
    let rel = if fade_in { target.len / 2 } else { clip.length - target.len / 2 };
    let previous_gain = before.find_clip(clip.id).unwrap().1.gain_at(rel);
    let new_gain = expected.find_clip(clip.id).unwrap().1.gain_at(rel);
    assert!(if dy < 0.0 { new_gain > previous_gain } else { new_gain < previous_gain });
    assert_eq!(h.state().engine.undo_history().len(), undo_len + 1);
    if show_name {
        fade_shot(&mut h, &bend_shot_name(fade_in, dy, "released"));
    }
    assert!(h.state_mut().engine.undo());
    assert_eq!(h.state().engine.session(), &before);
    assert!(h.state_mut().engine.redo());
    assert_eq!(h.state().engine.session(), &expected);
}

#[test]
fn fade_midpoints_drag_vertically_without_moving_the_endpoints() {
    for fade_in in [true, false] {
        for show_name in [true, false] {
            for dy in [-16.0, 16.0] {
                fade_midpoint_bend_case(fade_in, show_name, dy);
            }
        }
    }
}

#[test]
fn fade_curve_drags_cancel_safely_or_return_to_the_original_curve() {
    for invalidate in [0, 1, 2] {
        let mut h = smart_demo();
        let tl = timeline(&h);
        let (clip, lane) = clip_on_screen(&h, |xs, xe| xs > tl.min.x + 30.0 && xe < tl.max.x - 100.0 && xe - xs > 100.0);
        let c = h.state_mut().engine.session_mut().find_clip_mut(clip.id).unwrap();
        c.fade_out = soundcraft_model::Fade { len: c.length * 2 / 3, shape: soundcraft_model::FadeShape::SCurve, curvature: 0.0 };
        h.step();
        let midpoint = fade_midpoint(h.state().engine.session(), tl, lane, h.state().engine.session().find_clip(clip.id).unwrap().1, false).unwrap();
        let before = h.state().engine.session().clone();
        let undo_len = h.state().engine.undo_history().len();
        start_drag(&mut h, midpoint);
        h.hover_at(midpoint - vec2(0.0, 16.0));
        h.step();
        assert!(matches!(h.state().gesture, Some(Gesture::FadeCurve { .. })));
        if invalidate == 1 {
            h.state_mut().engine.session_mut().find_clip_mut(clip.id).unwrap().length = 0;
        } else if invalidate == 2 {
            let (tid, _) = before.find_clip(clip.id).unwrap();
            h.state_mut().engine.session_mut().track_mut(tid).unwrap().playlist_mut().unwrap().clips.retain(|c| c.id != clip.id);
        }
        h.hover_at(midpoint);
        h.step();
        h.drop_at(midpoint);
        h.step();
        assert!(h.state().gesture.is_none());
        assert_eq!(h.state().engine.undo_history().len(), undo_len);
        if invalidate == 0 {
            assert_eq!(h.state().engine.session(), &before);
        }
    }
}

#[test]
fn existing_fades_drag_from_their_upper_curve_endpoints() {
    for fade_in in [true, false] {
        for show_name in [true, false] {
            for full_length in [false, true] {
                let mut h = smart_demo();
                h.state_mut().engine.execute("view.clip_name", &json!({"value": show_name})).unwrap();
                h.step();
                let tl = timeline(&h);
                let (clip, lane) = clip_on_screen(&h, |xs, xe| xs > tl.min.x + 30.0 && xe < tl.max.x - 100.0 && xe - xs > 100.0);
                let len = if full_length { clip.length } else { clip.length * 2 / 3 };
                let c = h.state_mut().engine.session_mut().find_clip_mut(clip.id).unwrap();
                let fade = soundcraft_model::Fade { len, shape: soundcraft_model::FadeShape::SCurve, curvature: 0.35 };
                if fade_in {
                    c.fade_in = fade;
                } else {
                    c.fade_out = fade;
                }
                h.step();
                let s = h.state().engine.session();
                let (xs, xe) = (x_of(s, tl, clip.start), x_of(s, tl, clip.end()));
                let curves = fade_curves(&h, Rect::from_min_max(pos2(xs, lane.min.y), pos2(xe, lane.max.y)));
                assert_eq!(curves.len(), 1);
                let curve = &curves[0].0;
                // Take the handle from the painted curve, rather than duplicating hit geometry.
                let head = *if fade_in { curve.points.last() } else { curve.points.first() }.unwrap();
                let press = head + vec2(3.0, 2.0);
                if fade_in && full_length {
                    let (tid, _) = s.find_clip(clip.id).unwrap();
                    assert_ne!(
                        clip_at(s.track(tid).unwrap(), sample_at(s, tl, press.x)).unwrap().id,
                        clip.id,
                        "the pointer should be over the adjacent clip"
                    );
                }
                let before = s.clone();
                let undo_len = h.state().engine.undo_history().len();
                h.hover_at(press);
                h.step();
                assert_eq!(h.output().platform_output.cursor_icon, egui::CursorIcon::ResizeNwSe);
                let side = if fade_in { "in" } else { "out" };
                if show_name && !full_length {
                    fade_shot(&mut h, &format!("fade-{side}-endpoint-hover"));
                }
                h.drag_at(press);
                let to = head + vec2(if fade_in { -35.0 } else { 35.0 }, 0.0);
                h.hover_at(to);
                h.step();
                assert!(matches!(h.state().gesture, Some(Gesture::Fade { clip: id, fade_in: side, .. }) if id == clip.id && side == fade_in));
                assert_eq!(h.output().platform_output.cursor_icon, egui::CursorIcon::ResizeNwSe);
                assert_eq!(h.state().engine.session(), &before, "pressing the handle must not move the edit selection");
                if show_name && !full_length {
                    fade_shot(&mut h, &format!("fade-{side}-endpoint-dragging"));
                }
                h.drop_at(to);
                h.step();
                let range = fade_drag_range(&clip, fade_in, sample_at(&before, tl, to.x)).unwrap();
                let mut expected = before.clone();
                let c = expected.find_clip_mut(clip.id).unwrap();
                let fade = soundcraft_model::Fade { len: range.end - range.start, shape: soundcraft_model::FadeShape::SCurve, curvature: 0.35 };
                assert!(fade.len < len);
                if fade_in {
                    c.fade_in = fade;
                } else {
                    c.fade_out = fade;
                }
                assert_eq!(h.state().engine.session(), &expected);
                assert_eq!(h.state().engine.undo_history().len(), undo_len + 1);
                assert!(h.state_mut().engine.undo());
                assert_eq!(h.state().engine.session(), &before);
            }
        }
    }
}

#[test]
fn existing_fade_handles_preserve_selector_and_other_smart_tool_zones() {
    let mut h = smart_demo();
    let tl = timeline(&h);
    let (clip, lane) = clip_on_screen(&h, |xs, xe| xs > tl.min.x + 30.0 && xe < tl.max.x - 100.0 && xe - xs > 100.0);
    let c = h.state_mut().engine.session_mut().find_clip_mut(clip.id).unwrap();
    c.fade_out = soundcraft_model::Fade { len: c.length / 2, shape: soundcraft_model::FadeShape::SCurve, curvature: 0.0 };
    h.step();
    let s = h.state().engine.session();
    let rect = Rect::from_min_max(pos2(x_of(s, tl, clip.start), lane.min.y), pos2(x_of(s, tl, clip.end()), lane.max.y));
    let curves = fade_curves(&h, rect);
    let head = *curves[0].0.points.first().unwrap();
    h.hover_at(head + vec2(0.0, 16.0));
    h.step();
    assert_eq!(h.output().platform_output.cursor_icon, egui::CursorIcon::Text);
    h.state_mut().engine.execute("edit.tool", &json!({"tool": "selector"})).unwrap();
    h.step();
    h.hover_at(head);
    h.step();
    assert_eq!(h.output().platform_output.cursor_icon, egui::CursorIcon::Text);
    start_drag(&mut h, head);
    assert!(matches!(h.state().gesture, Some(Gesture::Select { .. })));
    h.drop_at(head + vec2(20.0, 0.0));
    h.step();
}

#[test]
fn a_fade_drag_handles_its_clip_shrinking_to_one_sample() {
    for fade_in in [true, false] {
        let mut h = smart_demo();
        let tl = timeline(&h);
        let (clip, lane) = clip_on_screen(&h, |xs, xe| xs > tl.min.x + 20.0 && xe < tl.max.x - 20.0 && xe - xs > 100.0);
        let s = h.state().engine.session();
        let corner = pos2(x_of(s, tl, if fade_in { clip.start } else { clip.end() }) + if fade_in { 6.0 } else { -6.0 }, lane.min.y + 4.0);
        start_drag(&mut h, corner);
        assert!(matches!(h.state().gesture, Some(Gesture::Fade { .. })));
        let c = h.state_mut().engine.session_mut().find_clip_mut(clip.id).unwrap();
        c.length = 1;
        c.clamp_fades();
        let undo_len = h.state().engine.undo_history().len();
        h.drop_at(corner + vec2(40.0, 0.0));
        h.step();
        let c = h.state().engine.session().find_clip(clip.id).unwrap().1;
        assert_eq!(if fade_in { c.fade_in.len } else { c.fade_out.len }, 1);
        assert_eq!(h.state().engine.undo_history().len(), undo_len + 1);
        assert!(h.state().gesture.is_none());
    }
}

#[test]
fn a_fade_drag_cancels_if_its_clip_disappears() {
    let mut h = smart_demo();
    let tl = timeline(&h);
    let (clip, lane) = clip_on_screen(&h, |xs, xe| xs > tl.min.x + 20.0 && xe - xs > 100.0);
    let corner = pos2(x_of(h.state().engine.session(), tl, clip.start) + 6.0, lane.min.y + 4.0);
    start_drag(&mut h, corner);
    assert!(matches!(h.state().gesture, Some(Gesture::Fade { .. })));
    let (tid, _) = h.state().engine.session().find_clip(clip.id).unwrap();
    h.state_mut().engine.session_mut().track_mut(tid).unwrap().playlist_mut().unwrap().clips.retain(|c| c.id != clip.id);
    let undo_len = h.state().engine.undo_history().len();
    h.hover_at(corner + vec2(40.0, 0.0));
    h.step();
    h.drop_at(corner + vec2(40.0, 0.0));
    h.step();
    assert!(h.state().gesture.is_none());
    assert_eq!(h.state().engine.undo_history().len(), undo_len);
}

#[test]
fn a_fade_drag_survives_its_clip_shrinking_to_nothing() {
    let mut h = smart_demo();
    let tl = timeline(&h);
    let (clip, lane) = clip_on_screen(&h, |xs, xe| xs > tl.min.x + 20.0 && xe - xs > 100.0);
    let corner = pos2(x_of(h.state().engine.session(), tl, clip.start) + 6.0, lane.min.y + 4.0);
    start_drag(&mut h, corner);
    assert!(matches!(h.state().gesture, Some(Gesture::Fade { .. })));
    if let Some(c) = h.state_mut().engine.session_mut().find_clip_mut(clip.id) {
        c.length = 0;
    }
    let undo_len = h.state().engine.undo_history().len();
    h.hover_at(corner + vec2(20.0, 0.0));
    h.step();
    assert!(h.state().gesture.is_none());
    h.drop_at(corner + vec2(20.0, 0.0));
    h.step();
    assert!(h.state().gesture.is_none());
    assert_eq!(h.state().engine.undo_history().len(), undo_len);
}

#[test]
fn a_drag_keeps_going_when_its_row_scrolls_away() {
    let mut h = harness(20);
    let (ids, a) = (row_ids(&h), lane_point(&h, 0));
    start_drag(&mut h, a);
    h.state_mut().edit_layout.scroll_y = 500.0;
    h.step();
    assert!(row(&h, 0).max.y < timeline(&h).min.y, "the first row is still on screen");
    let b = lane_point(&h, 8);
    h.hover_at(b);
    h.drop_at(b);
    h.step();
    assert_eq!(h.state().engine.session().edit.selected_tracks, ids[..=8]);
    assert!(h.state().gesture.is_none(), "the drag outlived the release");
}

#[test]
fn a_gesture_left_over_after_the_release_is_dropped() {
    let mut h = harness(1);
    h.state_mut().gesture = Some(Gesture::Scrub { last: 0 });
    h.step();
    assert!(h.state().gesture.is_none());
}

#[test]
fn a_flick_past_the_last_track_selects_down_to_it() {
    let mut h = harness(4);
    let ids = row_ids(&h);
    let a = lane_point(&h, 1);
    let below = pos2(a.x + 60.0, row(&h, 3).max.y + 100.0);
    start_drag(&mut h, a);
    h.hover_at(below);
    h.drop_at(below);
    h.step();
    assert_eq!(h.state().engine.session().edit.selected_tracks, ids[1..]);
}

#[test]
fn dragging_past_the_bottom_scrolls_and_keeps_selecting() {
    let mut h = harness(20);
    let first = row_ids(&h)[0];
    let a = lane_point(&h, 0);
    let below = pos2(a.x + 60.0, timeline(&h).max.y + 40.0);
    start_drag(&mut h, a);
    h.hover_at(below);
    h.step();
    h.run_steps(60);
    assert!(h.state().edit_layout.scroll_y > 300.0, "scrolled {} px", h.state().edit_layout.scroll_y);
    h.drop_at(below);
    h.step();
    let tl = timeline(&h);
    let last_seen = h.state().edit_layout.rows.iter().filter(|(_, r)| r[1] < tl.max.y).count();
    let sel = &h.state().engine.session().edit.selected_tracks;
    assert_eq!((sel.first(), sel.len()), (Some(&first), last_seen));
}

#[test]
fn dragging_past_the_right_edge_scrolls_the_timeline() {
    let mut h = harness(2);
    let a = lane_point(&h, 0);
    let right = pos2(timeline(&h).max.x + 40.0, a.y);
    let s = h.state().engine.session();
    let (before, pointed) = (s.edit.zoom.scroll, sample_at(s, timeline(&h), right.x));
    start_drag(&mut h, a);
    h.hover_at(right);
    h.step();
    h.run_steps(30);
    h.drop_at(right);
    h.step();
    let s = h.state().engine.session();
    assert!(s.edit.zoom.scroll > before, "the timeline did not scroll");
    assert!(s.edit.selection.end > pointed, "the selection did not follow the scroll");
}

/// A pass redrawn by egui must still contribute one frame-time sample, measured from the host clock rather than
/// UI input time.
#[test]
fn a_redrawn_frame_counts_once_in_the_frame_time() {
    let mut h = harness(4);
    // The monotonic host clock, not the UI input time, drives frame statistics.
    let frame = |h: &mut Harness<SoundApp>| {
        h.state_mut().advance_test_clock(std::time::Duration::from_secs_f64(1.0 / 60.0));
        h.step();
    };
    for _ in 0..120 {
        frame(&mut h);
    }
    let button = |pos: Pos2, pressed: bool| egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    };
    for k in 0..30 {
        let at = lane_point(&h, k % 4) + vec2(k as f32 * 7.0, 0.0);
        h.hover_at(at);
        frame(&mut h);
        h.event(button(at, true));
        frame(&mut h);
        h.event(button(at, false));
        frame(&mut h);
    }
    let ms = h.state().frame_ms;
    assert!((ms - 1000.0 / 60.0).abs() < 0.5, "{ms} ms/frame");
}

#[test]
fn dragging_a_name_plate_down_drops_it_where_released() {
    let mut h = harness(4);
    let order = |h: &Harness<SoundApp>| -> Vec<TrackId> { h.state().engine.session().tracks.iter().map(|t| t.id).collect() };
    let before = order(&h);
    let grab = row(&h, 0).min + vec2(40.0, 12.0);
    let release = pos2(grab.x, row(&h, 2).min.y);
    drag(&mut h, grab, release);
    let want: Vec<TrackId> = [1, 0, 2, 3].iter().map(|&i| before[i]).collect();
    assert_eq!(order(&h), want);
}

#[test]
fn a_pencil_click_on_a_plugin_lane_uses_the_plugin_range() {
    let mut engine = soundcraft_engine::demo::demo_engine();
    engine.execute("track.view", &json!({"track": "Kick", "view": "plugin:a:input_gain"})).unwrap();
    engine.execute("edit.tool", &json!({"tool": "pencil"})).unwrap();
    let mut h = harness_with(engine);
    let s = h.state().engine.session_arc();
    let kick = s.track_by_name("Kick").unwrap();
    let i = h.state().edit_layout.rows.iter().position(|(id, _)| *id == kick.id.0).unwrap();
    let lane = lane_rect(kick, row(&h, i), timeline(&h));
    // A quarter up the lane: -12 dB in the Input range of -24..24 dB.
    let at = pos2(lane.min.x + 100.0, lane.max.y - 4.0 - (lane.height() - 8.0) * 0.25);
    click(&mut h, at);
    let s = h.state().engine.session_arc();
    let param = AutoParam::Plugin { slot: 0, param: "input_gain".into() };
    let points = s.track_by_name("Kick").and_then(|t| t.lane(&param)).map(|l| l.points.clone()).unwrap_or_default();
    assert_eq!(points.len(), 1, "status: {}", h.state().ui.status);
    assert!((points[0].value + 12.0).abs() < 0.01, "{}", points[0].value);
}

/// A volume lane over a clip, with two breakpoints low enough that the Smart tool would grab
/// the clip there: the screen position of each, and the clips as they were.
fn breakpoints_over_a_clip(tool: &str) -> (Harness<'static, SoundApp>, TrackId, [Pos2; 2], Vec<Clip>) {
    let mut engine = soundcraft_engine::demo::demo_engine();
    engine.execute("edit.tool", &json!({"tool": tool})).unwrap();
    engine.execute("edit.mode", &json!({"mode": "shuffle"})).unwrap();
    let mut h = harness_with(engine);
    let tl = timeline(&h);
    let (clip, _) = clip_on_screen(&h, |xs, xe| xs > tl.min.x + 30.0 && xe < tl.max.x - 100.0 && xe - xs > 200.0);
    let track = h.state().engine.session().find_clip(clip.id).unwrap().0;
    let (xs, xe) = (x_of(h.state().engine.session(), tl, clip.start), x_of(h.state().engine.session(), tl, clip.end()));
    let xs_at = [xs + (xe - xs) * 0.3, xs + (xe - xs) * 0.6];
    for x in xs_at {
        let at = sample_at(h.state().engine.session(), tl, x);
        h.state_mut().engine.execute("automation.set_point", &json!({"track": track.0, "param": "volume", "at": at, "value": -40.0})).unwrap();
    }
    h.state_mut().engine.execute("track.view", &json!({"track": track.0, "view": "volume"})).unwrap();
    h.run_steps(2);
    let s = h.state().engine.session_arc();
    let t = s.track(track).unwrap();
    let i = h.state().edit_layout.rows.iter().position(|(id, _)| *id == track.0).unwrap();
    let lane = lane_rect(t, row(&h, i), tl);
    let y = lane.max.y - 4.0 - (lane.height() - 8.0) * (20.0 / 72.0);
    assert!(y > lane.center().y, "the breakpoints sit where the Smart tool grabs");
    let at = t.lane(&AutoParam::Volume).unwrap().points.iter().map(|p| pos2(x_of(&s, tl, p.at), y)).collect::<Vec<_>>();
    (h, track, [at[0], at[1]], t.clips().to_vec())
}

fn volume_points(h: &Harness<SoundApp>, track: TrackId) -> Vec<(Samples, f32)> {
    let s = h.state().engine.session();
    s.track(track).and_then(|t| t.lane(&AutoParam::Volume)).map(|l| l.points.iter().map(|p| (p.at, p.value)).collect()).unwrap_or_default()
}

#[test]
fn dragging_a_breakpoint_moves_it_not_the_clip_under_it() {
    for tool in ["smart", "grabber"] {
        let (mut h, track, [a, b], clips) = breakpoints_over_a_clip(tool);
        let before = volume_points(&h, track);
        let to = b + vec2(-40.0, -15.0);
        drag(&mut h, b, to);
        let s = h.state().engine.session_arc();
        assert_eq!(s.track(track).unwrap().clips(), clips.as_slice(), "{tool}: the clip stays");
        let points = volume_points(&h, track);
        assert_eq!(points.len(), 2, "{tool}: {points:?}");
        assert_eq!(points[0], before[0], "{tool}: the other breakpoint stays");
        let want = sample_at(&s, timeline(&h), to.x);
        assert!((points[1].0 - want).abs() as f64 <= s.edit.zoom.samples_per_px, "{tool}: {} vs {want}", points[1].0);
        assert!(points[1].1 > -40.0 + 1.0, "{tool}: dragged up, {}", points[1].1);
        assert!(x_of(&s, timeline(&h), points[1].0) > a.x, "{tool}");
        h.state_mut().engine.execute("edit.undo", &json!({})).unwrap();
        assert_eq!(volume_points(&h, track), before, "{tool}: one undo step");
    }
}

#[test]
fn alt_click_on_a_breakpoint_deletes_it() {
    for tool in ["smart", "grabber", "pencil"] {
        let (mut h, track, [_, b], clips) = breakpoints_over_a_clip(tool);
        let before = volume_points(&h, track);
        click_with(&mut h, b, egui::Modifiers::ALT);
        assert_eq!(volume_points(&h, track), before[..1], "{tool}");
        assert_eq!(h.state().engine.session().track(track).unwrap().clips(), clips.as_slice(), "{tool}");
    }
}
