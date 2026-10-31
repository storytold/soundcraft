use egui::accesskit::Role;
use egui::{Event, Key, Modifiers, vec2};
use egui_kittest::{Harness, kittest::Queryable};
use serde_json::json;
use soundcraft_model::TrackId;
use soundcraft_ui_egui::{Services, SoundApp, menus};

/// Selects the whole Kick track and opens Edit > Strip Silence from the menu.
fn strip_silence_dialog() -> (Harness<'static, SoundApp>, TrackId, Vec<(i64, i64)>) {
    let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
    let track = app.engine.session().track_by_name("Kick").unwrap().id;
    let end = app.engine.session().content_end();
    app.run("edit.select", json!({"tracks": [track.0], "start": 0, "end": end})).unwrap();
    let before = app.engine.session().track(track).unwrap().clips().iter().map(|c| (c.start, c.end())).collect();
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
    h.run_steps(3);
    (h, track, before)
}

fn clips(h: &Harness<'_, SoundApp>, track: TrackId) -> Vec<(i64, i64)> {
    h.state().engine.session().track(track).map(|t| t.clips().iter().map(|c| (c.start, c.end())).collect()).unwrap_or_default()
}

fn covered(clips: &[(i64, i64)]) -> i64 {
    clips.iter().map(|(a, b)| b - a).sum()
}

/// Strip on the same selection, run as a command, for comparison.
fn stripped(track: TrackId) -> Vec<(i64, i64)> {
    let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
    let end = app.engine.session().content_end();
    app.run("edit.select", json!({"tracks": [track.0], "start": 0, "end": end})).unwrap();
    app.run("edit.strip_silence", json!({})).unwrap();
    app.engine.session().track(track).unwrap().clips().iter().map(|c| (c.start, c.end())).collect()
}

#[test]
fn strip_silence_extract_keeps_what_strip_removes() {
    let (mut h, track, before) = strip_silence_dialog();
    h.get_by_role_and_label(Role::Button, "Extract").click();
    h.run_steps(2);
    assert_eq!(h.state().dialogs.open_name(), None);
    let extracted = clips(&h, track);
    let strip = stripped(track);
    assert!(!extracted.is_empty() && !strip.is_empty());
    assert_eq!(covered(&extracted) + covered(&strip), covered(&before));
}

#[test]
fn strip_silence_separate_cuts_the_clips_and_removes_nothing() {
    let (mut h, track, before) = strip_silence_dialog();
    h.get_by_role_and_label(Role::Button, "Separate").click();
    h.run_steps(2);
    assert_eq!(h.state().dialogs.open_name(), None);
    let separated = clips(&h, track);
    assert!(separated.len() > before.len());
    assert_eq!(covered(&separated), covered(&before));
}

#[test]
fn enter_on_a_focused_separate_separates_and_does_not_strip() {
    let (mut h, track, before) = strip_silence_dialog();
    h.get_by_role_and_label(Role::Button, "Separate").focus();
    h.run_steps(2);
    h.event(Event::Key { key: Key::Enter, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE });
    h.run_steps(2);
    assert_eq!(h.state().dialogs.open_name(), None);
    let separated = clips(&h, track);
    assert!(separated.len() > before.len());
    assert_eq!(covered(&separated), covered(&before), "Enter on Separate keeps all the audio");
}

#[test]
fn strip_silence_strip_removes_the_silence() {
    let (mut h, track, before) = strip_silence_dialog();
    h.get_by_role_and_label(Role::Button, "Strip").click();
    h.run_steps(2);
    assert_eq!(h.state().dialogs.open_name(), None);
    let strip = clips(&h, track);
    assert_eq!(strip, stripped(track));
    assert!(covered(&strip) < covered(&before));
}
