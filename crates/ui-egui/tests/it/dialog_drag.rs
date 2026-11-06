//! Dialog title bars move their forms without editing the session or form values.

use egui::{Event, Id, Modifiers, PointerButton, Rect, vec2};
use egui_kittest::Harness;
use soundcraft_ui_egui::{Services, SoundApp, dialogs::Dialog, menus};

fn window_rect(h: &Harness<'_, SoundApp>, title: &str) -> Rect {
    h.ctx.memory(|m| m.area_rect(Id::unique(Some(title)))).unwrap_or_else(|| panic!("{title} window is visible"))
}

fn assert_draggable(app: SoundApp, title: &str) {
    let mut h = Harness::builder().with_size(vec2(1600.0, 1000.0)).with_accessibility_check(false).build_ui_state(
        |ui, app: &mut SoundApp| {
            app.logic(ui.ctx());
            app.ui(ui);
        },
        app,
    );
    h.run_steps(5);
    let before = window_rect(&h, title);
    let form = format!("{:?}", h.state().dialogs.open);
    let session = serde_json::to_value(h.state().engine.session()).unwrap();
    let leftovers = h.state().leftovers.clone();
    let undo = h.state().engine.undo_history().len();
    assert!((before.center() - egui::pos2(800.0, 420.0)).length() <= 2.0, "{title} opens centered: {before:?}");
    let start = before.center_top() + vec2(0.0, 12.0);
    let delta = vec2(120.0, 90.0);
    h.event(Event::PointerMoved(start));
    h.event(Event::PointerButton { pos: start, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE });
    h.run_steps(1);
    for step in 1..=4 {
        h.event(Event::PointerMoved(start + delta * (step as f32 / 4.0)));
        h.run_steps(2);
    }
    h.event(Event::PointerButton { pos: start + delta, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
    h.run_steps(4);
    let moved = window_rect(&h, title);
    assert!((moved.min - before.min - delta).length() <= 2.0, "{title} should follow its title bar: {before:?} -> {moved:?}");
    assert_eq!(format!("{:?}", h.state().dialogs.open), form, "{title} drag preserves the form");
    assert_eq!(serde_json::to_value(h.state().engine.session()).unwrap(), session, "{title} drag preserves the session");
    assert_eq!(h.state().engine.undo_history().len(), undo, "{title} drag creates no edit");
    assert_eq!(h.state().leftovers, leftovers, "{title} drag preserves recoverable work");
}

#[test]
fn menu_dialogs_can_be_dragged_by_their_title_bars() {
    for (command, title) in [("track.new", "New Tracks"), ("session.new", "New Session"), ("file.bounce_mix", "Bounce Mix")] {
        let mut app = SoundApp::new(soundcraft_engine::Engine::default(), None, Services::default());
        menus::invoke_menu(&mut app, command, "");
        assert!(app.dialogs.open.is_some(), "{command} opens a dialog");
        assert_draggable(app, title);
    }
}

#[test]
fn ruler_tempo_dialog_can_be_dragged_by_its_title_bar() {
    let mut app = SoundApp::new(soundcraft_engine::Engine::default(), None, Services::default());
    app.dialogs.open = Some(Dialog::TempoChange { at: 96_000, bpm: 123.5 });
    assert_draggable(app, "Tempo Change (BPM)");
}

#[test]
fn recovery_dialog_can_be_dragged_by_its_title_bar() {
    let mut app = SoundApp::new(soundcraft_engine::Engine::default(), None, Services::default());
    app.leftovers.push(soundcraft_engine::autosave::Leftover {
        dir: "unused-drag-test-autosave".into(),
        name: "Recoverable session".into(),
        origin: None,
        saved: std::time::SystemTime::UNIX_EPOCH,
        recording: true,
    });
    assert_draggable(app, "Recover Unsaved Work");
}
