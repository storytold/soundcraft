//! The Edit window's Inserts columns take plugins as the Mix window's do: clicking an empty slot
//! opens the plugin menu and the pick lands in that slot; a used slot opens its plugin on click
//! and changes from its right-click menu. They used to show the plugin names only.

use egui::{Event, Modifiers, PointerButton, Pos2, pos2, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use serde_json::json;
use soundcraft_ui_egui::edit_window::{COLUMN_W, HEADER_W};
use soundcraft_ui_egui::{Services, SoundApp};

/// The demo session in the Edit window, with View › Edit Window Views › Inserts A-E on.
fn harness() -> Harness<'static, SoundApp> {
    let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
    app.engine.execute("view.edit_inserts_ae", &json!({"value": true})).unwrap();
    let mut h = Harness::builder().with_size(vec2(1600.0, 1000.0)).build_ui_state(
        |ui, app: &mut SoundApp| {
            let ctx = ui.ctx().clone();
            app.logic(&ctx);
            app.ui(ui);
        },
        app,
    );
    h.run_steps(6);
    h
}

/// Centre of insert slot `slot` (A = 0) in the named track's Inserts A-E column (its only column
/// here): slots are 13 points tall on a 15-point pitch, the first 3 points below the row's top.
fn slot_pos(h: &Harness<'_, SoundApp>, track: &str, slot: usize) -> Pos2 {
    let app = h.state();
    let id = app.engine.session().track_by_name(track).unwrap().id.0;
    let row = app.edit_layout.rows.iter().find(|(t, _)| *t == id).unwrap().1;
    pos2(row[0] + HEADER_W + COLUMN_W / 2.0, row[1] + 9.5 + 15.0 * slot as f32)
}

fn click_slot(h: &mut Harness<'_, SoundApp>, track: &str, slot: usize, button: PointerButton) {
    let pos = slot_pos(h, track, slot);
    for event in [
        Event::PointerMoved(pos),
        Event::PointerButton { pos, button, pressed: true, modifiers: Modifiers::NONE },
        Event::PointerButton { pos, button, pressed: false, modifiers: Modifiers::NONE },
    ] {
        h.input_mut().events.push(event);
        h.step();
    }
    h.run_steps(2);
}

fn insert(h: &Harness<'_, SoundApp>, track: &str, slot: usize) -> Option<String> {
    let t = h.state().engine.session().track_by_name(track).unwrap();
    t.mixer.inserts[slot].as_ref().map(|i| i.plugin.clone())
}

#[test]
fn an_empty_edit_window_insert_slot_adds_a_plugin() {
    let mut h = harness();
    assert_eq!(insert(&h, "Snare", 0), None);
    click_slot(&mut h, "Snare", 0, PointerButton::Primary);
    h.get_by_label_contains("Dynamics").click();
    h.run_steps(2);
    h.get_by_label("Compressor/Limiter").click();
    h.run_steps(2);
    assert_eq!(insert(&h, "Snare", 0).as_deref(), Some("compressor"));
}

#[test]
fn a_used_edit_window_insert_slot_opens_and_removes_its_plugin() {
    let mut h = harness();
    let kick = h.state().engine.session().track_by_name("Kick").unwrap().id;
    assert_eq!(insert(&h, "Kick", 1).as_deref(), Some("compressor"));
    click_slot(&mut h, "Kick", 1, PointerButton::Primary);
    assert!(h.state().ui.plugin_windows.contains(&(kick, 1)), "clicking a used slot should open its plugin window");
    click_slot(&mut h, "Kick", 1, PointerButton::Secondary);
    h.get_by_label("no insert").click();
    h.run_steps(2);
    assert_eq!(insert(&h, "Kick", 1), None);
}
