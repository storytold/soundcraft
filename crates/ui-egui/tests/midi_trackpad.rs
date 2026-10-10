use egui::{Event, Modifiers, MouseWheelUnit, PointerButton, Pos2, TouchPhase, Vec2, pos2, vec2};
use egui_kittest::Harness;
use serde_json::json;
use soundcraft_model::ClipContent;
use soundcraft_ui_egui::{Services, SoundApp};

fn harness() -> Harness<'static, SoundApp> {
    let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
    app.run("edit.select", json!({"tracks": ["Keys"]})).unwrap();
    app.run("window.midi_editor", json!({})).unwrap();
    let mut h = Harness::builder().with_size(vec2(1600.0, 1000.0)).build_ui_state(
        |ui, app: &mut SoundApp| {
            let ctx = ui.ctx().clone();
            app.logic(&ctx);
            app.ui(ui);
        },
        app,
    );
    for _ in 0..6 {
        h.step();
    }
    h
}

fn point(h: &Harness<'_, SoundApp>) -> Pos2 {
    let r = h.state().midi.roll;
    pos2(r[0] + 300.0, (r[1] + r[3]) * 0.5)
}

fn wheel(h: &mut Harness<'_, SoundApp>, p: Pos2, phase: TouchPhase, delta: Vec2, modifiers: Modifiers) {
    h.input_mut().events.extend([
        Event::PointerMoved(p),
        Event::ModifiersChanged(modifiers),
        Event::MouseWheel { unit: MouseWheelUnit::Point, delta, phase, modifiers },
    ]);
    h.step();
}

fn tick_at(h: &Harness<'_, SoundApp>, p: Pos2) -> f64 {
    let m = &h.state().midi;
    m.scroll_ticks + f64::from(p.x - m.roll[0]) * m.len_ticks / (f64::from(m.roll[2] - m.roll[0]) * m.zoom)
}

fn pinch(h: &mut Harness<'_, SoundApp>, p: Pos2, factor: f32) {
    h.input_mut().events.extend([Event::PointerMoved(p), Event::Zoom(factor)]);
    h.step();
}

#[test]
fn pinch_keeps_the_tick_under_the_pointer_and_leaves_the_timeline_and_notes_unchanged() {
    let mut h = harness();
    let session = h.state().engine.session().clone();
    let p = point(&h);
    let before = tick_at(&h, p);
    let pitch = h.state().midi.top_pitch;
    for factor in [1.25, 1.6, 0.5] {
        pinch(&mut h, p, factor);
        assert!((tick_at(&h, p) - before).abs() < 0.01);
    }
    assert!((h.state().midi.zoom - 1.0).abs() < 0.00001);
    assert_eq!(h.state().midi.top_pitch, pitch);
    assert_eq!(h.state().edit_layout.scroll_y, 0.0);
    assert_eq!(*h.state().engine.session(), session);
}

#[test]
fn diagonal_scroll_preserves_subpixel_momentum_after_finger_release() {
    let mut h = harness();
    let p = point(&h);
    pinch(&mut h, p, 2.0);
    let scroll = h.state().midi.scroll_ticks;
    let pitch = h.state().midi.top_pitch;
    let ticks_per_px = h.state().midi.len_ticks / (f64::from(h.state().midi.roll[2] - h.state().midi.roll[0]) * 2.0);
    wheel(&mut h, p, TouchPhase::Start, Vec2::ZERO, Modifiers::NONE);
    for delta in [vec2(-20.0, -15.0), vec2(-10.0, -8.0), vec2(-2.0, -1.0), vec2(-0.25, -0.25)] {
        wheel(&mut h, p, TouchPhase::Move, delta, Modifiers::NONE);
    }
    wheel(&mut h, p, TouchPhase::End, Vec2::ZERO, Modifiers::NONE);
    wheel(&mut h, p, TouchPhase::Move, vec2(-0.25, -0.25), Modifiers::NONE);
    wheel(&mut h, p, TouchPhase::End, Vec2::ZERO, Modifiers::NONE);
    assert!((h.state().midi.scroll_ticks - scroll - 32.5 * ticks_per_px).abs() < 0.001);
    assert!((h.state().midi.top_pitch - pitch + 24.5 / 9.0).abs() < 0.001);
    let stopped = (h.state().midi.scroll_ticks, h.state().midi.top_pitch);
    for _ in 0..8 {
        h.step();
    }
    assert_eq!((h.state().midi.scroll_ticks, h.state().midi.top_pitch), stopped);
}

#[test]
fn shift_scroll_pans_time_and_command_scroll_zooms_at_pointer() {
    let mut h = harness();
    let p = point(&h);
    pinch(&mut h, p, 2.0);
    let pitch = h.state().midi.top_pitch;
    let scroll = h.state().midi.scroll_ticks;
    let shift = Modifiers { shift: true, ..Modifiers::NONE };
    wheel(&mut h, p, TouchPhase::Start, Vec2::ZERO, shift);
    wheel(&mut h, p, TouchPhase::Move, vec2(0.0, -12.0), shift);
    wheel(&mut h, p, TouchPhase::End, Vec2::ZERO, shift);
    assert!(h.state().midi.scroll_ticks > scroll);
    assert_eq!(h.state().midi.top_pitch, pitch);
    let before = tick_at(&h, p);
    let cmd = Modifiers { command: true, mac_cmd: true, ..Modifiers::NONE };
    wheel(&mut h, p, TouchPhase::Start, Vec2::ZERO, cmd);
    wheel(&mut h, p, TouchPhase::Move, vec2(0.0, 12.0), cmd);
    wheel(&mut h, p, TouchPhase::End, Vec2::ZERO, cmd);
    assert!(h.state().midi.zoom > 2.0 && h.state().midi.zoom < 4.0);
    assert!((tick_at(&h, p) - before).abs() < 0.01);
    assert_eq!(h.state().midi.top_pitch, pitch);
}

#[test]
fn keys_and_velocity_lane_accept_gestures_but_other_panels_do_not() {
    let mut h = harness();
    let p = point(&h);
    let key = pos2(h.state().midi.roll[0] - 20.0, p.y);
    let pitch = h.state().midi.top_pitch;
    wheel(&mut h, key, TouchPhase::Start, Vec2::ZERO, Modifiers::NONE);
    wheel(&mut h, key, TouchPhase::Move, vec2(0.0, -0.25), Modifiers::NONE);
    wheel(&mut h, key, TouchPhase::End, Vec2::ZERO, Modifiers::NONE);
    assert!((h.state().midi.top_pitch - pitch + 0.25 / 9.0).abs() < 0.001);
    let v = h.state().midi.velocity;
    let velocity = pos2(p.x, (v[1] + v[3]) * 0.5);
    pinch(&mut h, velocity, 2.0);
    let before = tick_at(&h, velocity);
    pinch(&mut h, velocity, 1.25);
    assert!((tick_at(&h, velocity) - before).abs() < 0.01);
    let unchanged = (h.state().midi.zoom, h.state().midi.scroll_ticks, h.state().midi.top_pitch);
    let tl = h.state().edit_layout.timeline;
    let other = pos2(tl[0] + 200.0, tl[1] + 20.0);
    pinch(&mut h, other, 1.25);
    wheel(&mut h, other, TouchPhase::Start, Vec2::ZERO, Modifiers::NONE);
    wheel(&mut h, other, TouchPhase::Move, vec2(-20.0, -10.0), Modifiers::NONE);
    wheel(&mut h, other, TouchPhase::End, Vec2::ZERO, Modifiers::NONE);
    assert_eq!((h.state().midi.zoom, h.state().midi.scroll_ticks, h.state().midi.top_pitch), unchanged);
    let header = pos2(p.x, h.state().midi.roll[1] - 10.0);
    let before = tick_at(&h, header);
    pinch(&mut h, header, 1.25);
    assert!(h.state().midi.zoom > unchanged.0);
    assert!((tick_at(&h, header) - before).abs() < 0.01);
}

#[test]
fn velocity_editing_after_zoom_and_pan_targets_the_visible_note() {
    let mut h = harness();
    let p = point(&h);
    pinch(&mut h, p, 4.0);
    h.state_mut().run("ui.midi_scroll", json!({"by_px": 75})).unwrap();
    h.step();
    let m = &h.state().midi;
    let cid = m.clip.unwrap();
    let ClipContent::Midi { sequence } = &h.state().engine.session().find_clip(cid).unwrap().1.content else { panic!("expected MIDI") };
    let (index, note) = sequence.notes.iter().enumerate().find(|(_, n)| n.start as f64 > m.scroll_ticks + 300.0).unwrap();
    let scale = f64::from(m.roll[2] - m.roll[0]) * m.zoom / m.len_ticks;
    let x = m.roll[0] + ((note.start as f64 - m.scroll_ticks) * scale) as f32 + 1.0;
    let y = m.velocity[3] - 2.0 - (m.velocity[3] - m.velocity[1] - 4.0) * 64.0 / 127.0;
    let p = pos2(x, y);
    assert!(x < m.velocity[2]);
    let count = sequence.notes.len();
    h.input_mut()
        .events
        .extend([Event::PointerMoved(p), Event::PointerButton { pos: p, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE }]);
    h.step();
    h.input_mut().events.push(Event::PointerButton { pos: p, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
    h.step();
    let ClipContent::Midi { sequence } = &h.state().engine.session().find_clip(cid).unwrap().1.content else { panic!("expected MIDI") };
    assert_eq!(sequence.notes.len(), count);
    assert_eq!(sequence.notes[index].velocity, 64);
}

#[test]
fn reopening_preserves_the_view_and_changing_clips_resets_it() {
    let mut h = harness();
    let p = point(&h);
    pinch(&mut h, p, 4.0);
    h.state_mut().midi.selected = vec![0];
    let before = (h.state().midi.zoom, h.state().midi.scroll_ticks, h.state().midi.top_pitch);
    let old_clip = h.state().midi.clip;
    h.state_mut().run("window.midi_editor", json!({})).unwrap();
    h.step();
    assert!(h.state_mut().run("ui.midi_fit", json!({})).is_err());
    h.state_mut().run("window.midi_editor", json!({})).unwrap();
    h.step();
    assert_eq!((h.state().midi.zoom, h.state().midi.scroll_ticks, h.state().midi.top_pitch), before);
    h.state_mut().run("edit.select", json!({"tracks": ["Lead"]})).unwrap();
    h.step();
    assert_ne!(h.state().midi.clip, old_clip);
    assert_eq!(h.state().midi.zoom, 1.0);
    assert_eq!(h.state().midi.scroll_ticks, 0.0);
    assert!(h.state().midi.selected.is_empty());
}

#[test]
fn view_commands_bound_extremes_reject_bad_input_and_fit_the_clip() {
    let mut h = harness();
    let before = (h.state().midi.zoom, h.state().midi.scroll_ticks, h.state().midi.top_pitch);
    for (id, p) in [
        ("ui.midi_zoom_at", json!({"factor": 0})),
        ("ui.midi_zoom_at", json!({"factor": -1})),
        ("ui.midi_zoom_at", json!({"anchor_px": "NaN"})),
        ("ui.midi_scroll", json!({"by_px": 20, "by_y_px": "NaN"})),
    ] {
        assert!(h.state_mut().run(id, p).is_err());
        assert_eq!((h.state().midi.zoom, h.state().midi.scroll_ticks, h.state().midi.top_pitch), before);
    }
    for factor in [f64::MAX, f64::MIN_POSITIVE, 2.0] {
        h.state_mut().run("ui.midi_zoom_at", json!({"factor": factor, "anchor_px": f64::MAX})).unwrap();
        for delta in [f64::MAX, -f64::MAX] {
            h.state_mut().run("ui.midi_scroll", json!({"by_px": delta, "by_y_px": delta})).unwrap();
            h.step();
            let m = &h.state().midi;
            assert!((1.0..=1024.0).contains(&m.zoom));
            assert!((0.0..=m.len_ticks - m.len_ticks / m.zoom).contains(&m.scroll_ticks));
            assert!((0.0..=127.0).contains(&m.top_pitch));
        }
    }
    h.state_mut().run("ui.midi_fit", json!({})).unwrap();
    assert_eq!(h.state().midi.zoom, 1.0);
    assert_eq!(h.state().midi.scroll_ticks, 0.0);
    h.state_mut().run("edit.select", json!({"tracks": ["Kick"]})).unwrap();
    h.step();
    assert!(h.state_mut().run("ui.midi_zoom_at", json!({"factor": 2})).is_err());
}

#[test]
fn adding_and_selecting_notes_after_panning_uses_the_visible_tick_and_pitch() {
    let mut h = harness();
    let p = point(&h);
    pinch(&mut h, p, 4.0);
    h.state_mut().run("ui.midi_scroll", json!({"by_px": 75, "by_y_px": 3.25})).unwrap();
    h.step();
    let m = &h.state().midi;
    let p = pos2(m.roll[0] + 500.0, m.roll[1] + 12.0);
    let pitch = (m.top_pitch - 12.0 / 9.0).ceil() as u8;
    let tick = tick_at(&h, p) as i64;
    let grid = soundcraft_time::TICKS_PER_QUARTER / 4;
    let expected = tick / grid * grid;
    let cid = m.clip.unwrap();
    let before = match &h.state().engine.session().find_clip(cid).unwrap().1.content {
        ClipContent::Midi { sequence } => sequence.notes.len(),
        _ => panic!("expected MIDI"),
    };
    h.input_mut()
        .events
        .extend([Event::PointerMoved(p), Event::PointerButton { pos: p, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE }]);
    h.step();
    h.input_mut().events.push(Event::PointerButton { pos: p, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
    h.step();
    let ClipContent::Midi { sequence } = &h.state().engine.session().find_clip(cid).unwrap().1.content else { panic!("expected MIDI") };
    assert_eq!(sequence.notes.len(), before + 1);
    assert!(sequence.notes.iter().any(|n| n.pitch == pitch && n.start == expected));
    // Clicking that same visible note now selects it instead of adding another.
    h.input_mut().events.push(Event::PointerButton { pos: p, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE });
    h.step();
    h.input_mut().events.push(Event::PointerButton { pos: p, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
    h.step();
    assert_eq!(h.state().midi.selected.len(), 1);
}
