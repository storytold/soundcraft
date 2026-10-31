use egui::accesskit::Role;
use egui::{Event, Modifiers, PointerButton, Pos2, vec2};
use egui_kittest::{Harness, kittest::Queryable};
use serde_json::json;
use soundcraft_ui_egui::{Services, SoundApp, menus};

fn press(h: &mut Harness<'_, SoundApp>, pos: Pos2, pressed: bool) {
    h.event(Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE });
    h.run_steps(2);
}

/// #399: in Track > Group..., dragging down the Tracks list adds every track it passes, and OK
/// groups them all.
#[test]
fn dragging_down_the_group_dialogs_track_list_adds_each_track() {
    let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
    let tracks: Vec<(u64, String)> = app.engine.session().tracks.iter().map(|t| (t.id.0, t.name.clone())).collect();
    assert!(tracks.len() >= 4);
    app.run("edit.select", json!({"tracks": [tracks[0].0], "exact": true})).unwrap();
    menus::invoke_menu(&mut app, "track.group", "Track > Group...");
    let mut h = Harness::builder().with_size(vec2(1600.0, 1000.0)).build_ui_state(
        |ui, app: &mut SoundApp| {
            let ctx = ui.ctx().clone();
            app.logic(&ctx);
            app.ui(ui);
        },
        app,
    );
    h.run_steps(3);
    let at: Vec<Pos2> = tracks[1..4].iter().map(|(_, n)| h.get_by_role_and_label(Role::CheckBox, n).rect().center()).collect();
    h.event(Event::PointerMoved(at[0]));
    h.run_steps(1);
    press(&mut h, at[0], true);
    for p in &at[1..] {
        h.event(Event::PointerMoved(*p));
        h.run_steps(2);
    }
    press(&mut h, at[2], false);
    h.get_by_role_and_label(Role::Button, "OK").click();
    h.run_steps(2);
    let session = h.state().engine.session();
    let members: Vec<u64> = session.groups.last().map(|g| g.members.iter().map(|m| m.0).collect()).unwrap_or_default();
    assert_eq!(members, tracks[..4].iter().map(|t| t.0).collect::<Vec<_>>());
}

/// The Groups list's Modify Group… reopens the creation dialog on an existing group: it comes up
/// pre-filled, and OK edits that group in place instead of adding a second one.
#[test]
fn modify_group_dialog_edits_the_existing_group() {
    let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
    let tracks: Vec<(u64, String)> = app.engine.session().tracks.iter().map(|t| (t.id.0, t.name.clone())).collect();
    assert!(tracks.len() >= 2);
    let g = app.run("track.group", json!({"tracks": [tracks[0].0], "name": "Drums"})).unwrap()["group"].as_u64().unwrap();

    assert!(app.dialogs.open_modify_group(&app.engine, soundcraft_model::GroupId(g)));
    assert_eq!(app.dialogs.open_name(), Some("modify_group"));
    // Pre-filled from the group, not from the track selection.
    let Some(soundcraft_ui_egui::dialogs::Dialog::Group { id, name, edit, mix, members }) = &app.dialogs.open else {
        panic!("Modify Group… did not open the group dialog");
    };
    assert_eq!((id.map(|i| i.0), name.as_str(), *edit, *mix), (Some(g), "Drums", true, true));
    assert_eq!(members.iter().filter(|m| m.2).map(|m| m.0).collect::<Vec<_>>(), vec![tracks[0].0]);

    let mut h = Harness::builder().with_size(vec2(1600.0, 1000.0)).build_ui_state(
        |ui, app: &mut SoundApp| {
            let ctx = ui.ctx().clone();
            app.logic(&ctx);
            app.ui(ui);
        },
        app,
    );
    h.run_steps(3);
    // Add a second track and submit.
    h.get_by_role_and_label(Role::CheckBox, &tracks[1].1).click();
    h.run_steps(2);
    h.get_by_role_and_label(Role::Button, "OK").click();
    h.run_steps(2);

    let session = h.state().engine.session();
    assert_eq!(session.groups.len(), 1, "Modify edits the group in place rather than creating another");
    let gr = &session.groups[0];
    assert_eq!(gr.id.0, g);
    assert_eq!(gr.name, "Drums");
    assert_eq!(gr.members.iter().map(|m| m.0).collect::<Vec<_>>(), vec![tracks[0].0, tracks[1].0]);
}
