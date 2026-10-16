//! Dragging an insert moves it to the slot it's dropped on, on the same or another track;
//! Option (Alt) at the drop copies it, as in Pro Tools (#638).

use egui::{DragAndDrop, Event, Key, Modifiers, Rect};
use egui_kittest::{Harness, kittest::Queryable};
use serde_json::json;
use soundcraft_model::Insert;
use soundcraft_ui_egui::{InsertDrag, Services, SoundApp};

/// Vox has a compressor on A, Gtr another one on B with different settings.
fn mix() -> Harness<'static, SoundApp> {
    let mut e = soundcraft_engine::Engine::default();
    e.execute("track.new", &json!({"count": 1, "name": "Vox", "format": "mono"})).unwrap();
    e.execute("track.new", &json!({"count": 1, "name": "Gtr", "format": "mono"})).unwrap();
    e.execute("mix.insert", &json!({"track": "Vox", "slot": "a", "plugin": "compressor", "params": {"threshold": -30.0}})).unwrap();
    e.execute("mix.insert", &json!({"track": "Gtr", "slot": "b", "plugin": "compressor", "params": {"threshold": -10.0, "ratio": 2.0}})).unwrap();
    let mut app = SoundApp::new(e, None, Services::default());
    app.run("window.mix", json!({})).unwrap();
    let mut h = Harness::builder().with_size(egui::vec2(1600.0, 1000.0)).build_ui_state(
        |ui, app: &mut SoundApp| {
            app.logic(ui.ctx());
            app.ui(ui);
        },
        app,
    );
    h.run_steps(4);
    h
}

fn insert(h: &Harness<'_, SoundApp>, track: &str, slot: usize) -> Option<Insert> {
    h.state().engine.session().track_by_name(track).and_then(|t| t.mixer.inserts[slot].clone())
}

/// Insert `letter` of the `n`th strip from the left (Vox, then Gtr).
fn slot(h: &Harness<'_, SoundApp>, letter: char, n: usize) -> Rect {
    let mut slots: Vec<Rect> = h.get_all_by_label(&format!("Insert {letter}")).map(|node| node.rect()).collect();
    assert_eq!(slots.len(), 2, "one Insert {letter} per strip");
    slots.sort_by(|a, b| a.min.x.total_cmp(&b.min.x));
    slots[n]
}

/// Presses on `from` and moves onto `to` over a few frames, without releasing.
fn drag(h: &mut Harness<'_, SoundApp>, from: Rect, to: Rect) {
    h.hover_at(from.center());
    h.step();
    h.drag_at(from.center());
    h.step();
    for k in 1..=4 {
        h.hover_at(from.center().lerp(to.center(), k as f32 / 4.0));
        h.step();
    }
    assert!(DragAndDrop::has_payload_of_type::<InsertDrag>(&h.ctx), "the insert is being dragged");
}

fn drop(h: &mut Harness<'_, SoundApp>, at: Rect) {
    h.drop_at(at.center());
    h.run_steps(2);
}

#[test]
fn dragging_an_insert_moves_it_to_another_tracks_slot_in_one_undo_step() {
    let mut h = mix();
    let (a, b) = (insert(&h, "Vox", 0), insert(&h, "Gtr", 1));
    let (from, to) = (slot(&h, 'A', 0), slot(&h, 'B', 1));
    drag(&mut h, from, to);
    drop(&mut h, to);
    assert_eq!(insert(&h, "Vox", 0), None);
    assert_eq!(insert(&h, "Gtr", 1), a, "the dragged compressor replaces Gtr's");

    h.state_mut().engine.execute("edit.undo", &json!({})).unwrap();
    assert_eq!((insert(&h, "Vox", 0), insert(&h, "Gtr", 1)), (a, b));
}

#[test]
fn option_dragging_an_insert_copies_it() {
    let mut h = mix();
    let a = insert(&h, "Vox", 0);
    let (from, to) = (slot(&h, 'A', 0), slot(&h, 'B', 1));
    drag(&mut h, from, to);
    h.event(Event::ModifiersChanged(Modifiers::ALT));
    drop(&mut h, to);
    h.event(Event::ModifiersChanged(Modifiers::NONE));
    assert_eq!(insert(&h, "Vox", 0), a, "the original stays");
    assert_eq!(insert(&h, "Gtr", 1), a);
}

#[test]
fn escape_cancels_a_drag() {
    let mut h = mix();
    let (a, b) = (insert(&h, "Vox", 0), insert(&h, "Gtr", 1));
    let (from, to) = (slot(&h, 'A', 0), slot(&h, 'B', 1));
    drag(&mut h, from, to);
    h.key_press(Key::Escape);
    h.step();
    assert!(!DragAndDrop::has_any_payload(&h.ctx));
    drop(&mut h, to);
    assert_eq!((insert(&h, "Vox", 0), insert(&h, "Gtr", 1)), (a, b));
}

#[test]
fn a_click_still_opens_the_insert() {
    let mut h = mix();
    let a = insert(&h, "Vox", 0);
    let here = slot(&h, 'A', 0);
    h.hover_at(here.center());
    h.step();
    h.drag_at(here.center());
    h.step();
    drop(&mut h, here);
    let vox = h.state().engine.session().track_by_name("Vox").unwrap().id;
    assert!(h.state().ui.plugin_windows.contains(&(vox, 0)));
    assert_eq!(insert(&h, "Vox", 0), a);
}
