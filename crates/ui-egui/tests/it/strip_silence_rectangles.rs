use egui::accesskit::Role;
use egui::{Shape, vec2};
use egui_kittest::{Harness, kittest::Queryable};
use serde_json::json;
use soundcraft_model::TrackId;
use soundcraft_time::Range;
use soundcraft_ui_egui::dialogs::Dialog;
use soundcraft_ui_egui::{Services, SoundApp, edit_window, menus};

/// Selects the whole Kick track and opens Edit > Strip Silence from the menu.
fn strip_silence_dialog() -> (Harness<'static, SoundApp>, TrackId) {
    let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
    let track = app.engine.session().track_by_name("Kick").unwrap().id;
    let end = app.engine.session().content_end();
    app.run("edit.select", json!({"tracks": [track.0], "start": 0, "end": end})).unwrap();
    menus::invoke_menu(&mut app, "edit.strip_silence", "Edit > Strip Silence");
    assert_eq!(app.dialogs.open_name(), Some("strip_silence"));
    let mut h = Harness::builder().with_size(vec2(1600.0, 1000.0)).build_ui_state(
        |ui, app: &mut SoundApp| {
            let ctx = ui.ctx().clone();
            app.logic(&ctx);
            app.ui(ui);
        },
        app,
    );
    settle(&mut h);
    (h, track)
}

/// Runs frames until no Strip Silence scan is running, then one more so the Edit window draws what it found.
fn settle(h: &mut Harness<'_, SoundApp>) {
    h.run_steps(3);
    for _ in 0..2000 {
        if !h.state().dialogs.strip_scan_running() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
        h.run_steps(1);
    }
    assert!(!h.state().dialogs.strip_scan_running(), "the scan did not end");
    h.run_steps(2);
}

/// The Strip Silence rectangles in the last frame.
fn rectangles(h: &Harness<'_, SoundApp>) -> usize {
    let stroke = edit_window::strip_silence_stroke();
    h.output().shapes.iter().filter(|c| matches!(&c.shape, Shape::Rect(r) if r.stroke == stroke)).count()
}

/// The ranges `edit.strip_silence_preview` finds on `track` with the dialog's default values.
fn preview(h: &mut Harness<'_, SoundApp>, track: TrackId) -> Vec<Range> {
    let p = json!({"threshold_db": -48.0, "min_length_ms": 50.0, "pad_before_ms": 5.0, "pad_after_ms": 20.0});
    let r = h.state_mut().engine.execute("edit.strip_silence_preview", &p).unwrap();
    let ranges = r["tracks"].as_array().unwrap().iter().find(|t| t["track"] == track.0).unwrap()["ranges"].clone();
    ranges.as_array().unwrap().iter().map(|x| Range::new(x[0].as_i64().unwrap(), x[1].as_i64().unwrap())).collect()
}

fn clips(h: &Harness<'_, SoundApp>, track: TrackId) -> Vec<(i64, i64)> {
    h.state().engine.session().track(track).map(|t| t.clips().iter().map(|c| (c.start, c.end())).collect()).unwrap_or_default()
}

#[test]
fn strip_silence_draws_rectangles_around_what_strip_keeps() {
    let (mut h, track) = strip_silence_dialog();
    let before = clips(&h, track);
    let ctx = h.ctx.clone();
    let shown = soundcraft_ui_egui::dialogs::strip_silence_rectangles(h.state_mut(), &ctx);
    let ranges = preview(&mut h, track);
    assert!(!ranges.is_empty());
    assert_eq!(shown.iter().find(|(t, _)| *t == track).map(|(_, r)| r.clone()), Some(ranges.clone()));
    let drawn = rectangles(&h);
    assert!(drawn > 0 && drawn <= ranges.len(), "{drawn} rectangles for {} ranges", ranges.len());
    assert_eq!(clips(&h, track), before, "the rectangles changed the clips");
    assert!(!h.state().engine.can_undo(), "the rectangles made an undo step");
}

#[test]
fn the_rectangles_follow_the_sliders() {
    let (mut h, _) = strip_silence_dialog();
    assert!(rectangles(&h) > 0);
    if let Some(Dialog::StripSilence { threshold, .. }) = &mut h.state_mut().dialogs.open {
        *threshold = 0.0;
    }
    settle(&mut h);
    assert_eq!(rectangles(&h), 0, "nothing is louder than 0 dB, so Strip would keep nothing");
    if let Some(Dialog::StripSilence { threshold, .. }) = &mut h.state_mut().dialogs.open {
        *threshold = -48.0;
    }
    settle(&mut h);
    assert!(rectangles(&h) > 0);
}

#[test]
fn the_rectangles_stay_while_rename_is_open_and_go_when_the_window_closes() {
    let (mut h, track) = strip_silence_dialog();
    let before = clips(&h, track);
    let drawn = rectangles(&h);
    assert!(drawn > 0);
    h.get_by_role_and_label(Role::Button, "Rename…").click();
    h.run_steps(3);
    assert_eq!(h.state().dialogs.open_name(), Some("strip_silence_rename"));
    assert_eq!(rectangles(&h), drawn);
    h.get_by_role_and_label(Role::Button, "Cancel").click();
    h.run_steps(3);
    assert_eq!(h.state().dialogs.open_name(), Some("strip_silence"));
    h.get_by_role_and_label(Role::Button, "Cancel").click();
    h.run_steps(3);
    assert_eq!(h.state().dialogs.open_name(), None);
    assert_eq!(rectangles(&h), 0);
    assert_eq!(clips(&h, track), before);
}
