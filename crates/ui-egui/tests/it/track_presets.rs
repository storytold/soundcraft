//! Track presets: Save Track Preset... asks for a name and saves into the Track Presets folder, and
//! Recall Track Preset (Track menu and a track name's menu) lists it and recalls a preset as one
//! undo step.

use egui::accesskit::Role;
use egui::{Event, Modifiers, PointerButton, Pos2, pos2, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use serde_json::json;
use soundcraft_model::TrackId;
use soundcraft_ui_egui::dialogs::Dialog;
use soundcraft_ui_egui::{Services, SoundApp, menus};
use std::path::PathBuf;

/// The demo session, with presets kept in a fresh folder and a "Bass Chain" preset saved from Bass
/// (its compressor and fader) into it.
fn app(test: &str) -> (SoundApp, PathBuf) {
    let presets = std::env::temp_dir().join(format!("sc-track-presets-{test}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&presets);
    let folder = presets.join("Track Presets");
    std::fs::create_dir_all(&folder).unwrap();
    let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
    app.preset_dir = Some(presets.clone());
    let path = folder.join("Bass Chain.json");
    app.run("track.save_preset", json!({"path": path.to_string_lossy(), "tracks": ["Bass"]})).unwrap();
    (app, presets)
}

fn harness(app: SoundApp) -> Harness<'static, SoundApp> {
    let mut h = Harness::builder()
        .with_context_setup(soundcraft_ui_egui::framework::install_headless)
        .with_size(vec2(1400.0, 900.0))
        .with_step_dt(1.0 / 60.0)
        .build_ui_state(
            |ui, app: &mut SoundApp| {
                app.logic(&ui.ctx().clone());
                app.ui(ui);
            },
            app,
        );
    h.run_steps(4);
    h
}

fn compressor(app: &SoundApp, track: &str) -> bool {
    let t = app.engine.session().track_by_name(track).unwrap();
    t.mixer.inserts.iter().flatten().any(|i| i.plugin == "compressor")
}

fn recall(h: &mut Harness<SoundApp>) {
    h.get_by_label("Recall Track Preset").hover();
    h.run_steps(3);
    h.get_by_label("Bass Chain").click();
    h.run_steps(3);
}

fn save_dialog(app: &SoundApp) -> (TrackId, String) {
    match &app.dialogs.open {
        Some(Dialog::SaveTrackPreset { track, name, .. }) => (*track, name.clone()),
        other => panic!("{other:?}"),
    }
}

fn right_click_name(h: &mut Harness<SoundApp>, track: &str) {
    let id = h.state().engine.session().track_by_name(track).unwrap().id;
    let row = h.state().edit_layout.rows.iter().find(|(t, _)| *t == id.0).unwrap().1;
    let at: Pos2 = pos2(row[0] + 60.0, row[1] + 10.0);
    h.hover_at(at);
    for pressed in [true, false] {
        h.event(Event::PointerButton { pos: at, button: PointerButton::Secondary, pressed, modifiers: Modifiers::NONE });
    }
    h.run_steps(3);
}

#[test]
fn save_track_preset_asks_for_a_name_and_saves_into_the_folder() {
    let (mut app, presets) = app("save");
    let folder = presets.join("Track Presets");
    let _ = std::fs::remove_dir_all(&folder);
    // Selecting clips keeps their order, so Hats comes first in the selection; Kick sits above it and
    // is the track `track.save_preset` writes.
    let clip = |app: &SoundApp, track: &str| app.engine.session().track_by_name(track).unwrap().clips()[0].id.0;
    let (hats, kick) = (clip(&app, "Hats"), clip(&app, "Kick"));
    app.run("edit.select", json!({"clips": [hats, kick]})).unwrap();
    assert_eq!(app.engine.session().track_by_name("Hats").map(|t| t.id), app.engine.session().edit.selected_tracks.first().copied());
    menus::invoke_menu(&mut app, "track.save_preset", "Track > Save Track Preset...");
    let kick = app.engine.session().track_by_name("Kick").unwrap().id;
    assert_eq!(save_dialog(&app), (kick, "Kick".into()));
    assert!(!folder.exists(), "nothing is written before Save");
    let mut h = harness(app);
    h.get_by_label("Save").click();
    h.run_steps(3);
    assert!(h.state().dialogs.open.is_none());
    assert!(folder.join("Kick.json").is_file());
    let _ = std::fs::remove_dir_all(&presets);
}

/// The Save Track Preset dialog for Hats, named `name`, saving into `dir`.
fn save_hats(app: &mut SoundApp, name: &str, dir: PathBuf) {
    let track = app.engine.session().track_by_name("Hats").unwrap().id;
    app.dialogs.open = Some(Dialog::SaveTrackPreset { track, name: name.into(), dir, confirm: false, error: None });
}

fn enter(h: &mut Harness<SoundApp>) {
    h.key_press(egui::Key::Enter);
    h.run_steps(3);
}

#[test]
fn saving_over_a_preset_asks_first_and_enter_answers() {
    let (mut app, presets) = app("replace");
    let file = presets.join("Track Presets").join("Bass Chain.json");
    let bass = std::fs::read_to_string(&file).unwrap();
    save_hats(&mut app, "Bass Chain", presets.join("Track Presets"));
    let mut h = harness(app);
    enter(&mut h);
    assert!(h.query_by_label("“Bass Chain” already exists. Do you want to replace it?").is_some());
    assert_eq!(std::fs::read_to_string(&file).unwrap(), bass, "nothing is written before Replace");
    h.get_by_label("Cancel").click();
    h.run_steps(3);
    assert_eq!(save_dialog(h.state()).1, "Bass Chain", "Cancel goes back to the name");
    h.get_by_label("Save").click();
    h.run_steps(3);
    enter(&mut h);
    assert!(h.state().dialogs.open.is_none());
    let saved = std::fs::read_to_string(&file).unwrap();
    assert!(saved.contains("\"name\": \"Hats\""), "replaced with Hats's preset: {saved}");
    let _ = std::fs::remove_dir_all(&presets);
}

#[test]
fn enter_saves_a_new_name_at_once() {
    let (mut app, presets) = app("enter-new");
    save_hats(&mut app, "Hats Chain", presets.join("Track Presets"));
    let mut h = harness(app);
    enter(&mut h);
    assert!(h.state().dialogs.open.is_none());
    assert!(presets.join("Track Presets").join("Hats Chain.json").is_file());
    let _ = std::fs::remove_dir_all(&presets);
}

#[test]
fn a_blank_name_cannot_be_saved() {
    let (mut app, presets) = app("blank");
    let folder = presets.join("Track Presets");
    save_hats(&mut app, "   ", folder.clone());
    let mut h = harness(app);
    assert!(h.get_by_label("Save").accesskit_node().is_disabled());
    enter(&mut h);
    assert!(h.state().dialogs.open.is_some());
    assert_eq!(std::fs::read_dir(&folder).unwrap().count(), 1, "only Bass Chain");
    let _ = std::fs::remove_dir_all(&presets);
}

#[test]
fn a_folder_that_cannot_be_made_is_shown_in_the_dialog() {
    let (mut app, presets) = app("no-folder");
    let blocker = presets.join("a file");
    std::fs::write(&blocker, "").unwrap();
    save_hats(&mut app, "Hats", blocker.join("Track Presets"));
    let mut h = harness(app);
    h.get_by_label("Save").click();
    h.run_steps(3);
    match &h.state().dialogs.open {
        Some(Dialog::SaveTrackPreset { error: Some(e), .. }) => assert!(e.contains("Couldn't create"), "{e}"),
        other => panic!("the dialog stays open with the error: {other:?}"),
    }
    h.event(Event::Text("2".into()));
    h.run_steps(3);
    assert!(matches!(&h.state().dialogs.open, Some(Dialog::SaveTrackPreset { error: None, .. })), "typing a new name clears the error");
    let _ = std::fs::remove_dir_all(&presets);
}

#[test]
fn replacing_the_session_closes_a_pending_save() {
    let (mut app, presets) = app("replaced");
    save_hats(&mut app, "Hats", presets.join("Track Presets"));
    app.run("session.new", json!({"name": "Other"})).unwrap();
    assert!(app.dialogs.open.is_none(), "a track of the new session can have the old track's id");
    let _ = std::fs::remove_dir_all(&presets);
}

#[test]
fn a_track_names_menu_saves_that_track() {
    let (mut app, presets) = app("name-save");
    app.run("edit.select", json!({"tracks": ["Kick"]})).unwrap();
    let mut h = harness(app);
    right_click_name(&mut h, "Hats");
    h.get_by_label("Save Track Preset...").click();
    h.run_steps(3);
    let hats = h.state().engine.session().track_by_name("Hats").unwrap().id;
    assert_eq!(save_dialog(h.state()), (hats, "Hats".into()));
    let _ = std::fs::remove_dir_all(&presets);
}

#[test]
fn recall_track_preset_from_the_palette_starts_in_the_folder() {
    let (mut app, presets) = app("palette");
    menus::invoke_menu(&mut app, "track.load_preset", "");
    match &app.dialogs.open {
        Some(Dialog::PathPrompt { cmd, path, .. }) => {
            assert_eq!(cmd, "track.load_preset");
            assert_eq!(PathBuf::from(path), presets.join("Track Presets"));
        }
        other => panic!("{other:?}"),
    }
    let _ = std::fs::remove_dir_all(&presets);
}

#[test]
fn the_track_menu_recalls_a_preset_onto_the_selected_tracks_in_one_undo_step() {
    let (mut app, presets) = app("menu");
    app.run("edit.select", json!({"tracks": ["Hats"]})).unwrap();
    let volume = |app: &SoundApp| app.engine.session().track_by_name("Hats").unwrap().mixer.volume_db;
    let before = volume(&app);
    let mut h = harness(app);
    h.get_by_label("Track").click();
    h.run_steps(3);
    recall(&mut h);
    assert!(compressor(h.state(), "Hats"));
    assert_eq!(volume(h.state()), -5.0, "the whole preset, fader included");
    assert_eq!(menus::live_label(h.state(), Some("edit.undo")).as_deref(), Some("Undo Recall Track Preset"));
    h.state_mut().run("edit.undo", json!({})).unwrap();
    assert!(!compressor(h.state(), "Hats"));
    assert_eq!(volume(h.state()), before);
    let _ = std::fs::remove_dir_all(&presets);
}

#[test]
fn a_track_names_menu_recalls_onto_that_track_unless_it_is_selected() {
    let (mut app, presets) = app("name");
    app.run("edit.select", json!({"tracks": ["Kick"]})).unwrap();
    let kick = |app: &SoundApp| app.engine.session().track_by_name("Kick").unwrap().mixer.clone();
    let before = kick(&app);
    let mut h = harness(app);
    right_click_name(&mut h, "Hats");
    recall(&mut h);
    assert!(compressor(h.state(), "Hats"));
    assert_eq!(kick(h.state()), before, "the selection is left alone");
    let _ = std::fs::remove_dir_all(&presets);
}

#[test]
fn a_track_names_menu_recalls_onto_the_selection_it_is_part_of() {
    let (mut app, presets) = app("name-selected");
    app.run("edit.select", json!({"tracks": ["Kick", "Hats"]})).unwrap();
    let mut h = harness(app);
    right_click_name(&mut h, "Hats");
    recall(&mut h);
    assert!(compressor(h.state(), "Hats") && compressor(h.state(), "Kick"));
    let _ = std::fs::remove_dir_all(&presets);
}

#[test]
fn recall_lists_only_presets_ignoring_case_and_reports_a_broken_one() {
    let (mut app, presets) = app("listing");
    let folder = presets.join("Track Presets");
    for f in [".DS_Store", "notes.txt", "apple.json"] {
        std::fs::write(folder.join(f), "not a preset").unwrap();
    }
    app.run("edit.select", json!({"tracks": ["Hats"]})).unwrap();
    let mut h = harness(app);
    right_click_name(&mut h, "Hats");
    h.get_by_label("Recall Track Preset").hover();
    h.run_steps(3);
    assert!(h.query_by_label(".DS_Store").is_none() && h.query_by_label("notes").is_none());
    let y = |h: &Harness<SoundApp>, label: &str| h.get_by_label(label).rect().min.y;
    assert!(y(&h, "apple") < y(&h, "Bass Chain"), "sorted ignoring case");
    h.get_by_label("apple").click();
    h.run_steps(3);
    assert!(h.state().ui.status.contains("apple.json"), "the status line names the broken preset: {}", h.state().ui.status);
    let _ = std::fs::remove_dir_all(&presets);
}

#[test]
fn save_track_preset_opened_over_another_dialog_ignores_its_first_enter() {
    let (mut app, presets) = app("over-dialog");
    app.run("edit.select", json!({"tracks": ["Hats"]})).unwrap();
    menus::invoke_menu(&mut app, "track.load_preset", "");
    let mut h = harness(app);
    menus::invoke_menu(h.state_mut(), "track.save_preset", "Track > Save Track Preset...");
    enter(&mut h);
    assert!(!presets.join("Track Presets").join("Hats.json").exists(), "the Enter that opened it doesn't save");
    assert!(matches!(h.state().dialogs.open, Some(Dialog::SaveTrackPreset { .. })));
    let _ = std::fs::remove_dir_all(&presets);
}

#[test]
fn recall_from_file_opened_over_another_dialog_ignores_its_first_enter() {
    let (mut app, presets) = app("recall-over-dialog");
    app.run("edit.select", json!({"tracks": ["Hats"]})).unwrap();
    save_hats(&mut app, "Hats", presets.join("Track Presets"));
    let mut h = harness(app);
    menus::invoke_menu(h.state_mut(), "track.load_preset", "");
    if let Some(Dialog::PathPrompt { path, .. }) = &mut h.state_mut().dialogs.open {
        *path = presets.join("Track Presets").join("Bass Chain.json").to_string_lossy().into_owned();
    }
    enter(&mut h);
    assert!(!compressor(h.state(), "Hats"), "the Enter that opened it doesn't recall");
    assert!(matches!(h.state().dialogs.open, Some(Dialog::PathPrompt { .. })));
    let _ = std::fs::remove_dir_all(&presets);
}

/// Makes a folder unreadable until dropped, so a failing test still leaves it removable. None
/// when it stays readable anyway (as for root).
#[cfg(unix)]
struct Unreadable(PathBuf);

#[cfg(unix)]
impl Unreadable {
    fn new(dir: PathBuf) -> Option<Self> {
        use std::os::unix::fs::PermissionsExt;
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o000)).unwrap();
        let guard = Self(dir);
        std::fs::read_dir(&guard.0).is_err().then_some(guard)
    }
}

#[cfg(unix)]
impl Drop for Unreadable {
    fn drop(&mut self) {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&self.0, std::fs::Permissions::from_mode(0o755));
    }
}

/// A Track Presets folder that can't be read says so in Recall Track Preset, not "No Track Presets".
#[cfg(unix)]
#[test]
fn a_folder_that_cannot_be_read_is_named_in_the_recall_menu() {
    let (mut app, presets) = app("unreadable");
    let Some(locked) = Unreadable::new(presets.join("Track Presets")) else { return };
    app.run("edit.select", json!({"tracks": ["Hats"]})).unwrap();
    let mut h = harness(app);
    right_click_name(&mut h, "Hats");
    h.get_by_label("Recall Track Preset").hover();
    h.run_steps(3);
    assert!(h.query_by_label_contains("Couldn't read").is_some() && h.query_by_label("No Track Presets").is_none());
    drop(locked);
    let _ = std::fs::remove_dir_all(&presets);
}

/// The same for a plugin's preset folder in its Presets menu.
#[cfg(unix)]
#[test]
fn a_plugin_preset_folder_that_cannot_be_read_is_named_in_its_menu() {
    let (mut app, presets) = app("plugin-unreadable");
    let Some(locked) = Unreadable::new(presets.join(soundcraft_ui_egui::preset_folder_name("compressor"))) else { return };
    app.run("mix.insert", json!({"track": "Hats", "slot": 0, "plugin": "compressor"})).unwrap();
    let hats = app.engine.session().track_by_name("Hats").unwrap().id;
    app.ui.plugin_windows.push((hats, 0));
    let mut h = harness(app);
    h.get_by_label("Presets v").click();
    h.run_steps(3);
    assert!(h.query_by_label_contains("Couldn't read").is_some());
    drop(locked);
    let _ = std::fs::remove_dir_all(&presets);
}

/// As in Pro Tools, recalling a send whose bus isn't in the session makes the bus, and the status
/// line says so.
#[test]
fn a_bus_made_on_recall_is_named_in_the_status_line() {
    let (mut app, presets) = app("bus-made");
    let file = presets.join("Track Presets").join("Pad Room.json");
    app.run("track.save_preset", json!({"path": file.to_string_lossy(), "tracks": ["Pad"]})).unwrap();
    // As if saved in a session whose bus was called Room, which this one doesn't have.
    let mut v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
    assert_eq!(v["send_buses"][0]["name"], "Verb", "the demo's Pad sends A to Verb");
    v["send_buses"][0]["name"] = json!("Room");
    std::fs::write(&file, v.to_string()).unwrap();
    app.run("edit.select", json!({"tracks": ["Hats"]})).unwrap();
    let mut h = harness(app);
    right_click_name(&mut h, "Hats");
    h.get_by_label("Recall Track Preset").hover();
    h.run_steps(3);
    h.get_by_label("Pad Room").click();
    h.run_steps(3);
    assert_eq!(h.state().ui.status, "Created bus Room");
    let s = h.state().engine.session();
    let room = s.bus_by_name("Room").map(|b| b.id).expect("the bus is made");
    assert_eq!(s.track_by_name("Hats").unwrap().mixer.sends[0].as_ref().map(|x| x.target.clone()), Some(soundcraft_model::Route::Bus(room)));
    let _ = std::fs::remove_dir_all(&presets);
}

#[test]
fn choosing_save_track_preset_in_the_palette_with_enter_opens_the_dialog() {
    let (mut app, presets) = app("palette-save");
    app.run("edit.select", json!({"tracks": ["Hats"]})).unwrap();
    app.ui.show_search = true;
    let mut h = harness(app);
    h.event(Event::Text("save track preset".into()));
    h.run_steps(2);
    h.key_press(egui::Key::Enter);
    h.run_steps(3);
    let hats = h.state().engine.session().track_by_name("Hats").unwrap().id;
    assert!(!presets.join("Track Presets").join("Hats.json").exists(), "the palette's Enter doesn't press OK");
    assert_eq!(save_dialog(h.state()), (hats, "Hats".into()));
    let _ = std::fs::remove_dir_all(&presets);
}

#[test]
fn without_a_presets_folder_save_falls_back_to_the_path_prompt() {
    let (mut app, presets) = app("web");
    app.preset_dir = None;
    app.run("edit.select", json!({"tracks": ["Hats"]})).unwrap();
    menus::invoke_menu(&mut app, "track.save_preset", "Track > Save Track Preset...");
    assert!(matches!(&app.dialogs.open, Some(Dialog::PathPrompt { cmd, .. }) if cmd == "track.save_preset"));
    let _ = std::fs::remove_dir_all(&presets);
}

#[test]
fn a_plugin_preset_saved_from_its_menu_is_listed_when_it_opens_again() {
    let (mut app, presets) = app("plugin-save");
    app.run("mix.insert", json!({"track": "Hats", "slot": 0, "plugin": "compressor"})).unwrap();
    let hats = app.engine.session().track_by_name("Hats").unwrap().id;
    app.ui.plugin_windows.push((hats, 0));
    let mut h = harness(app);
    h.get_by_label("Presets v").click();
    h.run_steps(3);
    h.get_by_label("Save").click();
    h.run_steps(3);
    let status = h.state().ui.status.clone();
    let name = status.strip_prefix("Saved preset ").unwrap_or_else(|| panic!("{status}")).to_string();
    h.get_by_label("Presets v").click();
    h.run_steps(3);
    assert!(h.query_by_role_and_label(Role::Button, &name).is_some(), "{name} is listed");
    let _ = std::fs::remove_dir_all(&presets);
}
