//! Track presets: Save Track Preset... asks for a name and saves into the Track Presets folder, and
//! Recall Track Preset (Track menu and a track name's menu) lists it and recalls a preset as one
//! undo step.

use egui::{Event, Modifiers, PointerButton, Pos2, pos2, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
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
    let mut h = Harness::builder().with_size(vec2(1400.0, 900.0)).with_step_dt(1.0 / 60.0).build_ui_state(
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
    h.get_by_label("Recall Track Preset ⏵").hover();
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
    assert!(!folder.exists(), "nothing is written before OK");
    let mut h = harness(app);
    h.get_by_label("OK").click();
    h.run_steps(3);
    assert!(h.state().dialogs.open.is_none());
    assert!(folder.join("Kick.json").is_file());
    let _ = std::fs::remove_dir_all(&presets);
}

#[test]
fn saving_over_a_preset_asks_first() {
    let (mut app, presets) = app("replace");
    let hats = app.engine.session().track_by_name("Hats").unwrap().id;
    app.dialogs.open = Some(Dialog::SaveTrackPreset { track: hats, name: "Bass Chain".into(), dir: presets.join("Track Presets") });
    let mut h = harness(app);
    assert!(h.query_by_label("OK").is_none());
    h.get_by_label("Replace").click();
    h.run_steps(3);
    let saved = std::fs::read_to_string(presets.join("Track Presets").join("Bass Chain.json")).unwrap();
    assert!(saved.contains("\"name\": \"Hats\""), "replaced with Hats's preset: {saved}");
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
