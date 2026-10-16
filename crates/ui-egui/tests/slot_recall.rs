//! Recall Inserts / Recall Sends (#639): an insert or send slot's menu lists the Track Presets
//! folder and recalls a preset's inserts or sends onto that track, starting at the clicked slot, and
//! offers Save Track Preset... for that track.

use egui::vec2;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use serde_json::json;
use soundcraft_model::ChannelFormat;
use soundcraft_ui_egui::dialogs::Dialog;
use soundcraft_ui_egui::{MainWindow, Services, SoundApp};
use std::path::PathBuf;

/// One track, Vox, with a compressor on A, a gain on B and a send A to FX at -3 dB, saved as the
/// "Vox Chain" preset in a fresh Track Presets folder.
fn harness(test: &str, window: MainWindow) -> (Harness<'static, SoundApp>, PathBuf) {
    let presets = std::env::temp_dir().join(format!("sc-slot-recall-{test}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&presets);
    std::fs::create_dir_all(presets.join("Track Presets")).unwrap();
    let mut engine = soundcraft_engine::Engine::default();
    engine.execute("track.new", &json!({"name": "Vox"})).unwrap();
    engine.session_mut().add_bus("FX", ChannelFormat::Stereo);
    engine.execute("mix.insert", &json!({"track": "Vox", "slot": 0, "plugin": "compressor"})).unwrap();
    engine.execute("mix.insert", &json!({"track": "Vox", "slot": 1, "plugin": "gain"})).unwrap();
    engine.execute("mix.send", &json!({"track": "Vox", "slot": 0, "bus": "FX", "level_db": -3.0})).unwrap();
    let path = presets.join("Track Presets").join("Vox Chain.json");
    engine.execute("track.save_preset", &json!({"path": path.to_string_lossy(), "track": "Vox"})).unwrap();
    let mut app = SoundApp::new(engine, None, Services::default());
    app.preset_dir = Some(presets.clone());
    app.ui.window = window;
    app.run("view.edit_inserts_ae", json!({})).unwrap();
    app.run("view.edit_sends_ae", json!({})).unwrap();
    app.run("track.height", json!({"tracks": ["Vox"], "height": "medium"})).unwrap();
    let mut h = Harness::builder().with_size(vec2(1400.0, 1000.0)).build_ui_state(
        |ui, app: &mut SoundApp| {
            app.logic(&ui.ctx().clone());
            app.ui(ui);
        },
        app,
    );
    h.run_steps(4);
    (h, presets)
}

fn recall(h: &mut Harness<SoundApp>, menu: &str) {
    h.get_by_label(menu).hover();
    h.run_steps(3);
    h.get_by_label("Vox Chain").click();
    h.run_steps(3);
}

fn inserts(h: &Harness<SoundApp>) -> Vec<Option<String>> {
    h.state().engine.session().tracks[0].mixer.inserts.iter().map(|i| i.as_ref().map(|i| i.plugin.clone())).collect()
}

fn s(x: &str) -> Option<String> {
    Some(x.to_string())
}

fn recall_inserts_on_c(window: MainWindow, test: &str) {
    let (mut h, presets) = harness(test, window);
    h.get_by_label("Insert C").click();
    h.run_steps(3);
    recall(&mut h, "Recall Inserts ⏵");
    assert_eq!(inserts(&h), vec![s("compressor"), s("gain"), s("compressor"), s("gain"), None, None, None, None, None, None]);
    assert!(h.state().ui.plugin_windows.is_empty(), "recalling doesn't open the plugins");
    h.state_mut().run("edit.undo", json!({})).unwrap();
    assert_eq!(inserts(&h)[2..4], [None, None], "one undo step");
    let _ = std::fs::remove_dir_all(&presets);
}

#[test]
fn recall_inserts_from_an_empty_slot_in_the_mix_window() {
    recall_inserts_on_c(MainWindow::Mix, "mix");
}

#[test]
fn recall_inserts_from_the_edit_windows_inserts_view() {
    recall_inserts_on_c(MainWindow::Edit, "edit");
}

#[test]
fn recall_inserts_from_a_filled_slots_menu() {
    let (mut h, presets) = harness("filled", MainWindow::Mix);
    h.get_by_label("Insert B").click_secondary();
    h.run_steps(3);
    recall(&mut h, "Recall Inserts ⏵");
    assert_eq!(inserts(&h), vec![s("compressor"), s("compressor"), s("gain"), None, None, None, None, None, None, None]);
    let _ = std::fs::remove_dir_all(&presets);
}

#[test]
fn recall_sends_from_an_empty_send_slot() {
    let (mut h, presets) = harness("sends", MainWindow::Mix);
    h.get_by_label("Send C").click();
    h.run_steps(3);
    recall(&mut h, "Recall Sends ⏵");
    let sends: Vec<Option<f32>> = h.state().engine.session().tracks[0].mixer.sends.iter().map(|x| x.as_ref().map(|x| x.level_db)).collect();
    assert_eq!(sends[..4], [Some(-3.0), None, Some(-3.0), None]);
    assert_eq!(inserts(&h)[2], None, "the inserts are left alone");
    let _ = std::fs::remove_dir_all(&presets);
}

#[test]
fn the_submenu_says_when_the_folder_is_empty() {
    let (mut h, presets) = harness("empty", MainWindow::Mix);
    std::fs::remove_file(presets.join("Track Presets").join("Vox Chain.json")).unwrap();
    h.get_by_label("Insert C").click();
    h.run_steps(3);
    h.get_by_label("Recall Inserts ⏵").hover();
    h.run_steps(3);
    assert!(h.query_by_label("No Track Presets").is_some());
    let _ = std::fs::remove_dir_all(&presets);
}

fn save_from(slot: &str) {
    let (mut h, presets) = harness(&format!("save-{slot}"), MainWindow::Mix);
    h.get_by_label(slot).click();
    h.run_steps(3);
    h.get_by_label("Save Track Preset...").click();
    h.run_steps(3);
    let vox = h.state().engine.session().tracks[0].id;
    match &h.state().dialogs.open {
        Some(Dialog::SaveTrackPreset { track, name, .. }) => assert_eq!((*track, name.as_str()), (vox, "Vox")),
        other => panic!("{other:?}"),
    }
    assert!(h.query_by_label("Recall Inserts ⏵").is_none() && h.query_by_label("Recall Sends ⏵").is_none(), "the menu closed");
    let _ = std::fs::remove_dir_all(&presets);
}

#[test]
fn save_track_preset_from_an_insert_slot() {
    save_from("Insert C");
}

#[test]
fn save_track_preset_from_a_send_slot() {
    save_from("Send C");
}
