//! Bounce Mix renders across frames behind a progress window instead of freezing the app.
use egui::vec2;
use egui_kittest::{Harness, kittest::Queryable};
use serde_json::json;
use soundcraft_ui_egui::dialogs::Dialog;
use soundcraft_ui_egui::{Services, SoundApp};
use std::path::{Path, PathBuf};

fn harness() -> Harness<'static, SoundApp> {
    let app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
    let mut h = Harness::builder().with_size(vec2(1400.0, 900.0)).with_max_steps(1_000).build_ui_state(
        |ui, app: &mut SoundApp| {
            app.logic(&ui.ctx().clone());
            app.ui(ui);
        },
        app,
    );
    h.run_steps(5);
    h
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("soundcraft-ui-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn bounce_from_dialog(h: &mut Harness<'_, SoundApp>, file: &Path) {
    let app = h.state_mut();
    assert!(app.dialogs.open_for_command(&app.engine, "file.bounce_mix"));
    if let Some(Dialog::Bounce { path, .. }) = &mut app.dialogs.open {
        *path = file.to_string_lossy().into_owned();
    }
    h.run_steps(2);
    h.get_by_label("Bounce").click();
    h.run_steps(3);
    assert!(h.state().bounce.is_some(), "{}", h.state().ui.status);
    assert!(h.query_by_label("Bounce Mix").is_none());
}

#[test]
fn a_bounce_renders_across_frames_with_progress_until_its_file_is_written() {
    let dir = scratch("bounce-frames");
    let mut h = harness();
    bounce_from_dialog(&mut h, &dir.join("Mix.wav"));
    let frames = h.state().bounce.as_ref().map_or(0, |b| b.range().len());
    let mut progress = Vec::new();
    for _ in 0..4 {
        progress.push(h.state().bounce.as_ref().map_or(1.0, |b| b.progress()));
        h.get_by_label("Cancel");
        h.step();
    }
    assert!(progress.is_sorted() && progress[0] < progress[3] && progress[3] < 1.0, "{progress:?}");
    h.run();
    assert!(h.state().bounce.is_none());
    assert!(h.state().ui.status.starts_with("Bounced Mix.wav (peak"), "{}", h.state().ui.status);
    let (_, file) = soundcraft_audio_io::decode(&std::fs::read(dir.join("Mix.wav")).unwrap(), None).unwrap();
    assert_eq!(file.frames() as i64, frames);
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_bounce_keeps_going_while_the_window_is_hidden() {
    let dir = scratch("bounce-hidden");
    let mut h = harness();
    bounce_from_dialog(&mut h, &dir.join("Hidden.wav"));
    let ctx = h.ctx.clone();
    let mut calls = 0;
    while h.state().bounce.is_some() {
        let _ = ctx.run_logic(&egui::RawInput::default(), |ctx| h.state_mut().logic(ctx));
        assert!(h.state().bounce.is_none() || ctx.has_requested_repaint());
        calls += 1;
        assert!(calls < 10_000);
    }
    assert!(calls > 3, "{calls}");
    assert!(h.state().ui.status.starts_with("Bounced Hidden.wav"), "{}", h.state().ui.status);
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_session_cannot_be_replaced_while_it_bounces() {
    let dir = scratch("bounce-replace");
    let mut h = harness();
    let tracks = h.state().engine.session().tracks.len();
    bounce_from_dialog(&mut h, &dir.join("Mix.wav"));
    for id in ["session.new", "session.open", "session.close", "session.revert"] {
        let err = h.state_mut().run(id, json!({"path": dir.join("Other.scraft").to_string_lossy()})).unwrap_err();
        assert!(err.contains("bounce"), "{id}: {err}");
    }
    assert_eq!(h.state().engine.session().tracks.len(), tracks);
    assert!(h.state().bounce.is_some());
    h.key_press(egui::Key::Escape);
    h.run_steps(2);
    assert!(h.state_mut().run("session.close", json!({})).is_ok());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn cancel_or_escape_stops_a_bounce_and_leaves_no_file() {
    let dir = scratch("bounce-cancel");
    let mut h = harness();
    bounce_from_dialog(&mut h, &dir.join("Clicked.wav"));
    h.get_by_label("Cancel").click();
    h.run_steps(2);
    assert!(h.state().bounce.is_none());
    assert_eq!(h.state().ui.status, "Bounce cancelled");
    bounce_from_dialog(&mut h, &dir.join("Escaped.wav"));
    h.key_press(egui::Key::Escape);
    h.run_steps(2);
    assert!(h.state().bounce.is_none());
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
    let _ = std::fs::remove_dir_all(&dir);
}

/// The largest gap between `int` and `float` in 16-bit LSBs.
fn max_error_lsb(int: &Path, float: &Path) -> f64 {
    let (_, a) = soundcraft_audio_io::decode(&std::fs::read(int).unwrap(), None).unwrap();
    let (_, b) = soundcraft_audio_io::decode(&std::fs::read(float).unwrap(), None).unwrap();
    let mut worst = 0.0f64;
    for (i, f) in a.channels.iter().zip(&b.channels) {
        for (x, y) in i.iter().zip(f) {
            worst = worst.max((f64::from(*x) - f64::from(y.clamp(-1.0, 1.0))).abs() * 32768.0);
        }
    }
    worst
}

/// Bounces from the dialog at `bit_depth`, ticking Dither when `dither`, and waits for the file.
fn bounce_with(h: &mut Harness<'_, SoundApp>, file: &Path, bit_depth: &str, dither: bool) {
    let app = h.state_mut();
    assert!(app.dialogs.open_for_command(&app.engine, "file.bounce_mix"));
    if let Some(Dialog::Bounce { path, bit_depth: depth, .. }) = &mut app.dialogs.open {
        *path = file.to_string_lossy().into_owned();
        *depth = bit_depth.into();
    }
    h.run_steps(2);
    if dither {
        h.get_by_label("Dither (TPDF)").click();
        h.run_steps(2);
    }
    h.get_by_label("Bounce").click();
    h.run_steps(3);
    h.run();
    assert!(h.state().bounce.is_none());
    assert!(file.exists(), "{}", h.state().ui.status);
}

#[test]
fn the_bounce_dialog_dithers_only_when_dither_is_ticked() {
    let dir = scratch("bounce-dither");
    let mut h = harness();
    let app = h.state_mut();
    assert!(app.dialogs.open_for_command(&app.engine, "file.bounce_mix"));
    match &app.dialogs.open {
        Some(Dialog::Bounce { dither, .. }) => assert!(!dither, "Dither starts off"),
        other => panic!("{other:?}"),
    }
    app.dialogs.open = None;
    bounce_with(&mut h, &dir.join("Float.wav"), "32f", false);
    bounce_with(&mut h, &dir.join("Plain.wav"), "16", false);
    bounce_with(&mut h, &dir.join("Dithered.wav"), "16", true);
    let plain = max_error_lsb(&dir.join("Plain.wav"), &dir.join("Float.wav"));
    let dithered = max_error_lsb(&dir.join("Dithered.wav"), &dir.join("Float.wav"));
    assert!(plain <= 0.5 + 1e-3, "{plain}");
    assert!(dithered > 0.6, "{dithered}");
    let _ = std::fs::remove_dir_all(&dir);
}
