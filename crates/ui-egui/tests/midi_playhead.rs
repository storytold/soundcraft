//! Cursor geometry and follow are verified from the editor's actual rendered shapes.
use egui::{Color32, Shape, vec2};
use egui_kittest::{Harness, kittest::Queryable};
use serde_json::json;
use soundcraft_ui_egui::{Services, SoundApp, midi_playback::MidiScrollingMode, theme::ThemeMode};

// Tokens use a process-wide palette. Separate test apps must not switch it
// while another test is rendering or comparing cursor colors.
static THEME_TESTS: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn harness(rate: u32, tempo_change: bool) -> Harness<'static, SoundApp> {
    let mut app = SoundApp::new(soundcraft_engine::Engine::default(), None, Services::default());
    app.engine.session_mut().sample_rate = soundcraft_time::SampleRate::new(rate).unwrap();
    if tempo_change {
        app.engine.session_mut().tempo.set_tempo(1920, 90.0).unwrap();
    }
    app.run("track.new", json!({"name": "Cursor", "kind": "midi"})).unwrap();
    let clip = app.run("midi.new_clip", json!({"track": "Cursor", "start": rate, "length": rate * 8})).unwrap()["clip"].clone();
    for tick in [0, 960, 1920, 3840] {
        app.run("midi.note_add", json!({"clip": clip, "pitch": 60, "start_ticks": tick, "length_ticks": 480})).unwrap();
    }
    app.run("edit.select", json!({"clips": [clip], "start": rate})).unwrap();
    app.ui.show_midi_editor = true;
    app.engine.mark_clean();
    let mut h = Harness::builder().with_size(vec2(1600.0, 1000.0)).build_ui_state(
        |ui, app: &mut SoundApp| {
            app.logic(ui.ctx());
            app.ui(ui);
        },
        app,
    );
    h.run_steps(6);
    h
}

fn set_position(h: &mut Harness<'_, SoundApp>, sample: i64) {
    h.state_mut().engine.transport.playing = true;
    h.state_mut().engine.transport.position = sample;
    h.step();
}

fn cursor(h: &Harness<'_, SoundApp>, color: Color32) -> Option<f32> {
    let m = &h.state().midi;
    h.output().shapes.iter().find_map(|shape| match &shape.shape {
        Shape::LineSegment { points: [a, b], stroke } if a.x == b.x && a.y == m.roll[1] && stroke.width == 1.5 && stroke.color == color => {
            assert_eq!(b.y, m.roll[3], "the cursor must stop at the bottom of the piano roll");
            assert!(shape.clip_rect.max.y <= m.roll[3], "the cursor must not paint into the Velocity lane");
            Some(a.x)
        }
        _ => None,
    })
}

fn expected_x(h: &Harness<'_, SoundApp>, sample: i64) -> f32 {
    let app = h.state();
    let s = app.engine.session();
    let m = &app.midi;
    let clip = s.find_clip(m.clip.unwrap()).unwrap().1;
    let tick = s.tempo.samples_to_ticks_f(sample, s.sample_rate) - s.tempo.samples_to_ticks(clip.start, s.sample_rate) as f64;
    m.roll[0] + ((tick - m.scroll_ticks) * f64::from(m.roll[2] - m.roll[0]) * m.zoom / m.len_ticks) as f32
}

#[test]
fn playback_line_matches_clip_offset_tempo_zoom_and_pan_in_both_themes() {
    let _theme = THEME_TESTS.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    for rate in [44_100, 48_000, 96_000] {
        for change in [false, true] {
            let mut h = harness(rate, change);
            h.state_mut().run("ui.midi_scrolling", json!({"mode": "none"})).unwrap();
            h.state_mut().run("ui.midi_zoom_at", json!({"factor": 3, "anchor_px": 0})).unwrap();
            h.state_mut().run("ui.midi_scroll", json!({"by_px": 80})).unwrap();
            let before = h.state().engine.session().clone();
            let undo = h.state().engine.undo_history().len();
            for (mode, height) in [(ThemeMode::Dark, 32.0), (ThemeMode::Light, 120.0)] {
                h.state_mut().run("ui.theme", json!({"mode": mode.id()})).unwrap();
                h.state_mut().run("ui.midi_velocity_height", json!({"height": height})).unwrap();
                set_position(&mut h, i64::from(rate) * 2);
                let line = cursor(&h, soundcraft_ui_egui::theme::Tokens::current().playhead).unwrap();
                assert!((line - expected_x(&h, i64::from(rate) * 2)).abs() < 0.01);
            }
            assert_eq!(h.state().engine.session(), &before);
            assert_eq!(h.state().engine.undo_history().len(), undo);
            assert!(!h.state().engine.is_dirty());
        }
    }
}

#[test]
fn page_follows_skips_and_loop_wraps_without_mutating_notes() {
    let _theme = THEME_TESTS.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut h = harness(48_000, false);
    assert_eq!(h.state().ui.midi_scrolling, MidiScrollingMode::Page);
    h.state_mut().run("ui.midi_zoom_at", json!({"factor": 4, "anchor_px": 0})).unwrap();
    let before = h.state().engine.session().clone();
    set_position(&mut h, 48_000 + 120_000);
    let first = h.state().midi.scroll_ticks;
    assert!(first > 0.0);
    assert!(cursor(&h, soundcraft_ui_egui::theme::Tokens::current().playhead).is_some());
    set_position(&mut h, 48_000 + 300_000);
    assert!(h.state().midi.scroll_ticks > first);
    set_position(&mut h, 48_000);
    assert_eq!(h.state().midi.scroll_ticks, 0.0);
    assert_eq!(h.state().engine.session(), &before);
}

#[test]
fn centered_modes_use_blank_time_and_follow_edit_resolves_live() {
    let _theme = THEME_TESTS.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut h = harness(48_000, true);
    h.state_mut().run("ui.midi_scrolling", json!({"mode": "continuous"})).unwrap();
    set_position(&mut h, 0);
    let m = &h.state().midi;
    assert!(m.scroll_ticks < 0.0);
    let x = cursor(&h, soundcraft_ui_egui::theme::Tokens::current().playhead).unwrap();
    assert!((x - (m.roll[0] + m.roll[2]) * 0.5).abs() < 0.01);
    h.state_mut().run("ui.midi_scrolling", json!({"mode": "follow_edit"})).unwrap();
    for edit_mode in ["none", "after_playback", "page", "continuous", "center"] {
        h.state_mut().run("options.scrolling", json!({"mode": edit_mode})).unwrap();
        h.step();
        assert_eq!(soundcraft_ui_egui::midi_playback::effective(h.state()).id(), edit_mode);
    }
}

#[test]
fn hidden_cursor_is_not_pinned_to_the_edge_and_manual_pan_holds_follow() {
    let _theme = THEME_TESTS.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut h = harness(48_000, false);
    h.state_mut().run("ui.midi_scrolling", json!({"mode": "none"})).unwrap();
    set_position(&mut h, 0);
    assert!(cursor(&h, soundcraft_ui_egui::theme::Tokens::current().playhead).is_none());
    h.state_mut().run("ui.midi_scrolling", json!({"mode": "continuous"})).unwrap();
    set_position(&mut h, 144_000);
    h.state_mut().run("ui.midi_scroll", json!({"by_px": 40})).unwrap();
    let view = h.state().midi.scroll_ticks;
    set_position(&mut h, 145_000);
    assert!(h.state().midi.follow.hold);
    assert_eq!(h.state().midi.scroll_ticks, view);
    h.state_mut().run("ui.midi_scrolling", json!({"mode": "continuous"})).unwrap();
    h.step();
    assert!(!h.state().midi.follow.hold);
    let m = &h.state().midi;
    assert!((expected_x(&h, 145_000) - (m.roll[0] + m.roll[2]) * 0.5).abs() < 0.01);

    // Control-channel batches can change mode and manually pan before the next drawing pass.
    h.state_mut().run("ui.midi_scrolling", json!({"mode": "continuous"})).unwrap();
    h.state_mut().run("ui.midi_scroll", json!({"by_px": 40})).unwrap();
    let manual = h.state().midi.scroll_ticks;
    h.step();
    assert!(h.state().midi.follow.hold);
    assert_eq!(h.state().midi.scroll_ticks, manual);
}

#[test]
fn scrolling_menu_and_old_preferences_are_compatible_and_invalid_modes_are_atomic() {
    let _theme = THEME_TESTS.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let old: soundcraft_ui_egui::UiState = serde_json::from_str("{}").unwrap();
    assert_eq!(old.midi_scrolling, MidiScrollingMode::Page);
    let mut h = harness(48_000, false);
    h.get_by_label("Scrolling: Page").click();
    h.run_steps(2);
    h.get_by_label("Center Playhead").click();
    h.run_steps(2);
    assert_eq!(h.state().ui.midi_scrolling, MidiScrollingMode::Center);
    let prefs = serde_json::to_string(&h.state().ui).unwrap();
    let restored: soundcraft_ui_egui::UiState = serde_json::from_str(&prefs).unwrap();
    assert_eq!(restored.midi_scrolling, MidiScrollingMode::Center);
    let mode = h.state().ui.midi_scrolling;
    let session = h.state().engine.session().clone();
    for params in [json!({"mode": "invalid"}), json!({"mode": null}), json!({"mode": 0})] {
        assert!(h.state_mut().run("ui.midi_scrolling", params).is_err());
        assert_eq!(h.state().ui.midi_scrolling, mode);
        assert_eq!(h.state().engine.session(), &session);
    }
}

#[test]
fn high_zoom_center_pan_keeps_fractional_ticks_and_space_plays_from_the_cue() {
    let _theme = THEME_TESTS.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut h = harness(48_000, false);
    h.state_mut().run("ui.midi_scrolling", json!({"mode": "center"})).unwrap();
    h.step();
    h.state_mut().run("ui.midi_zoom_at", json!({"factor": 1024})).unwrap();
    h.step();
    let before = h.state().engine.session().edit.playhead;
    let m = &h.state().midi;
    let expected_delta = 19.25 * m.len_ticks / m.zoom / f64::from(m.roll[2] - m.roll[0]) * 25.0;
    h.state_mut().run("ui.midi_scroll", json!({"by_px": 19.25})).unwrap();
    let cue = h.state().engine.session().edit.playhead;
    assert_ne!(cue, before);
    assert!(((cue - before) as f64 - expected_delta).abs() <= 0.51);
    h.step();
    h.input_mut().events.push(egui::Event::Key {
        key: egui::Key::Space,
        physical_key: Some(egui::Key::Space),
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    });
    h.step();
    // Shortcuts queue a transport request in the UI pass; logic drains it next frame.
    h.input_mut().events.push(egui::Event::Key {
        key: egui::Key::Space,
        physical_key: Some(egui::Key::Space),
        pressed: false,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    });
    h.step();
    assert!(h.state().is_playing());
    assert!((cue..cue + 48_000).contains(&h.state().position()));
}

#[test]
fn centered_record_cursor_is_red_without_opening_an_input_device() {
    let _theme = THEME_TESTS.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut h = harness(48_000, false);
    h.state_mut().run("ui.midi_scrolling", json!({"mode": "center"})).unwrap();
    set_position(&mut h, 96_000);
    assert!(cursor(&h, soundcraft_ui_egui::theme::Tokens::current().accent).is_some());
    // Render the recording status only; never arm a track or request hardware capture.
    h.state_mut().engine.transport.recording = true;
    h.step();
    assert!(cursor(&h, soundcraft_ui_egui::theme::Tokens::current().playhead).is_some());
    assert!(h.state().recorder.is_none());
}
