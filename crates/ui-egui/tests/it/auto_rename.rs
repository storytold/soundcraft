use egui::accesskit::Role;
use egui::{Event, Key, Modifiers, vec2};
use egui_kittest::{Harness, kittest::Queryable};
use serde_json::json;
use soundcraft_engine::io::StripNames;
use soundcraft_model::{ClipId, TrackId};
use soundcraft_ui_egui::dialogs::Dialog;
use soundcraft_ui_egui::{Services, SoundApp, menus};

/// Selects the first Kick clip in the Clip List and opens Auto Rename for it.
fn auto_rename_dialog() -> (Harness<'static, SoundApp>, TrackId, ClipId) {
    let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
    let kick = app.engine.session().track_by_name("Kick").unwrap();
    let (track, clip) = (kick.id, kick.clips()[0].id);
    app.run("edit.select", json!({"clips": [clip.0]})).unwrap();
    menus::invoke_menu(&mut app, "clip.auto_rename", "");
    assert_eq!(app.dialogs.open_name(), Some("auto_rename"));
    let mut h = Harness::builder().with_size(vec2(1600.0, 1000.0)).build_ui_state(
        |ui, app: &mut SoundApp| {
            let ctx = ui.ctx().clone();
            app.logic(&ctx);
            app.ui(ui);
        },
        app,
    );
    h.run_steps(3);
    (h, track, clip)
}

fn sfx() -> StripNames {
    StripNames { name: "SFX".into(), number: 23, zeros: 1, suffix: ".Reel1".into() }
}

/// Puts `names` in the open Auto Rename dialog, as typing them would.
fn type_names(h: &mut Harness<'_, SoundApp>, names: StripNames) {
    let Some(Dialog::AutoRename { names: open, .. }) = &mut h.state_mut().dialogs.open else { panic!("Auto Rename is not open") };
    *open = names;
    h.run_steps(2);
}

/// Separates the clip `clip` at its middle and returns the name of the piece after the cut.
fn separate_middle(h: &mut Harness<'_, SoundApp>, track: TrackId, clip: ClipId) -> String {
    let (start, len) = {
        let c = h.state().engine.session().find_clip(clip).unwrap().1;
        (c.start, c.length)
    };
    h.state_mut().run("edit.separate", json!({"tracks": [track.0], "at": start + len / 2})).unwrap();
    let s = h.state().engine.session();
    s.track(track).unwrap().clips().iter().find(|c| c.start == start + len / 2).unwrap().name.clone()
}

#[test]
fn ok_sets_the_names_of_the_clips_separated_from_it() {
    let (mut h, track, clip) = auto_rename_dialog();
    h.get_by_label("Starting Number");
    h.get_by_label("Number of Places");
    type_names(&mut h, sfx());
    h.get_by_label("First clip: SFX023.Reel1");
    h.get_by_role_and_label(Role::Button, "OK").click();
    h.run_steps(2);
    assert_eq!(h.state().dialogs.open_name(), None);
    assert_eq!(separate_middle(&mut h, track, clip), "SFX023.Reel1");
}

#[test]
fn the_dialog_opens_with_the_clips_scheme() {
    let (mut h, _, clip) = auto_rename_dialog();
    type_names(&mut h, sfx());
    h.get_by_role_and_label(Role::Button, "OK").click();
    h.run_steps(2);
    let app = h.state_mut();
    assert!(app.dialogs.open_auto_rename(&app.engine, vec![clip]));
    assert!(matches!(&app.dialogs.open, Some(Dialog::AutoRename { names, clips }) if *names == sfx() && *clips == vec![clip]));
    assert!(!app.dialogs.open_auto_rename(&app.engine, vec![ClipId(999_999)]), "no dialog for a clip that is gone");
}

#[test]
fn enter_on_a_focused_cancel_sets_no_scheme() {
    let (mut h, track, clip) = auto_rename_dialog();
    let old = h.state().engine.session().find_clip(clip).unwrap().1.name.clone();
    type_names(&mut h, sfx());
    h.get_by_role_and_label(Role::Button, "Cancel").focus();
    h.run_steps(2);
    h.event(Event::Key { key: Key::Enter, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE });
    h.run_steps(2);
    assert_eq!(h.state().dialogs.open_name(), None);
    assert!(h.state().engine.session().auto_names.is_empty());
    assert_eq!(separate_middle(&mut h, track, clip), old);
}
