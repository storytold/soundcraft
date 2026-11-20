use egui::accesskit::Role;
use egui::{Event, Key, Modifiers, vec2};
use egui_kittest::{Harness, kittest::Queryable};
use serde_json::json;
use soundcraft_engine::io::StripNames;
use soundcraft_model::TrackId;
use soundcraft_ui_egui::dialogs::Dialog;
use soundcraft_ui_egui::{Services, SoundApp, menus};

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
    h.run_steps(3);
    (h, track)
}

fn sfx() -> StripNames {
    StripNames { name: "SFX".into(), number: 23, zeros: 1, suffix: ".Reel1".into() }
}

/// Puts `names` in the open Rename Selected Clips dialog, as typing them would.
fn type_names(h: &mut Harness<'_, SoundApp>, names: StripNames) {
    let Some(Dialog::StripSilenceRename { names: open, .. }) = &mut h.state_mut().dialogs.open else { panic!("Rename Selected Clips is not open") };
    *open = names;
    h.run_steps(2);
}

/// Clicks a button, then lets the window that comes up settle where it is drawn.
fn click(h: &mut Harness<'_, SoundApp>, label: &str) {
    h.get_by_role_and_label(Role::Button, label).click();
    h.run_steps(3);
}

#[test]
fn rename_names_the_clips_strip_silence_leaves() {
    let (mut h, track) = strip_silence_dialog();
    click(&mut h, "Rename…");
    assert_eq!(h.state().dialogs.open_name(), Some("strip_silence_rename"));
    type_names(&mut h, sfx());
    click(&mut h, "OK");
    assert_eq!(h.state().dialogs.open_name(), Some("strip_silence"), "OK goes back to Strip Silence");
    assert_eq!(h.state().dialogs.strip_names, sfx());
    click(&mut h, "Strip");
    assert_eq!(h.state().dialogs.open_name(), None);
    let names: Vec<String> = h.state().engine.session().track(track).unwrap().clips().iter().map(|c| c.name.clone()).collect();
    assert!(names.len() > 1);
    assert_eq!(names[..2], ["SFX023.Reel1", "SFX024.Reel1"]);
    assert_eq!(h.state().dialogs.strip_names, sfx(), "the naming scheme is kept for the next Strip Silence");
}

#[test]
fn cancel_in_rename_goes_back_and_keeps_the_names_it_had() {
    let (mut h, _) = strip_silence_dialog();
    if let Some(Dialog::StripSilence { threshold, .. }) = &mut h.state_mut().dialogs.open {
        *threshold = -30.0;
    }
    click(&mut h, "Rename…");
    type_names(&mut h, sfx());
    click(&mut h, "Cancel");
    assert!(matches!(h.state().dialogs.open, Some(Dialog::StripSilence { threshold, .. }) if threshold == -30.0));
    assert_eq!(h.state().dialogs.strip_names, StripNames::default());
}

#[test]
fn clear_in_rename_empties_the_naming_scheme() {
    let (mut h, track) = strip_silence_dialog();
    h.state_mut().dialogs.strip_names = sfx();
    h.run_steps(2);
    h.get_by_label("New clips are named SFX023.Reel1, SFX024.Reel1 and so on.");
    click(&mut h, "Rename…");
    assert!(matches!(&h.state().dialogs.open, Some(Dialog::StripSilenceRename { names, .. }) if *names == sfx()), "the dialog shows the kept names");
    click(&mut h, "Clear");
    assert!(matches!(&h.state().dialogs.open, Some(Dialog::StripSilenceRename { names, .. }) if *names == StripNames::default()));
    click(&mut h, "OK");
    assert_eq!(h.state().dialogs.strip_names, StripNames::default());
    let before: Vec<String> = h.state().engine.session().track(track).unwrap().clips().iter().map(|c| c.name.clone()).collect();
    click(&mut h, "Separate");
    let after = h.state().engine.session().track(track).unwrap().clips().iter().map(|c| c.name.clone()).collect::<Vec<_>>();
    assert!(after.len() > before.len() && after.iter().all(|n| before.contains(n)), "without a name the clips keep their names");
}

#[test]
fn enter_on_a_focused_cancel_goes_back_without_the_new_names() {
    let (mut h, _) = strip_silence_dialog();
    click(&mut h, "Rename…");
    type_names(&mut h, sfx());
    h.get_by_role_and_label(Role::Button, "Cancel").focus();
    h.run_steps(2);
    h.event(Event::Key { key: Key::Enter, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE });
    h.run_steps(2);
    assert_eq!(h.state().dialogs.open_name(), Some("strip_silence"));
    assert_eq!(h.state().dialogs.strip_names, StripNames::default());
}

#[test]
fn enter_on_a_focused_clear_clears_and_leaves_rename_open() {
    let (mut h, _) = strip_silence_dialog();
    click(&mut h, "Rename…");
    type_names(&mut h, sfx());
    h.get_by_role_and_label(Role::Button, "Clear").focus();
    h.run_steps(2);
    h.event(Event::Key { key: Key::Enter, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE });
    h.run_steps(2);
    assert!(matches!(&h.state().dialogs.open, Some(Dialog::StripSilenceRename { names, .. }) if *names == StripNames::default()));
    assert_eq!(h.state().dialogs.strip_names, StripNames::default());
}

#[test]
fn enter_on_a_focused_rename_opens_it_and_does_not_strip() {
    let (mut h, track) = strip_silence_dialog();
    let before = h.state().engine.session().track(track).unwrap().clips().len();
    h.get_by_role_and_label(Role::Button, "Rename…").focus();
    h.run_steps(2);
    h.event(Event::Key { key: Key::Enter, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE });
    h.run_steps(2);
    assert_eq!(h.state().dialogs.open_name(), Some("strip_silence_rename"));
    assert_eq!(h.state().engine.session().track(track).unwrap().clips().len(), before);
}
