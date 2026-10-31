//! Regression: the clip context menu must stay open while the pointer moves onto it, so its
//! Delete item can be used. It used to vanish once the pointer left the lane, because the lane
//! returned early (no hover position) before the menu was drawn. A right-click on a clip of the
//! edit selection acts on the whole selection (#1191).

use egui::{Event, Modifiers, PointerButton, pos2, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use soundcraft_ui_egui::{Services, SoundApp};

/// One UI frame, with queued synthetic pointer input applied first (as the control channel does).
fn step(h: &mut Harness<'_, SoundApp>) {
    let mut raw = std::mem::take(h.input_mut());
    let ctx = h.ctx.clone();
    h.state_mut().raw_input_hook(&ctx, &mut raw);
    *h.input_mut() = raw;
    h.step();
}

fn press(h: &mut Harness<'_, SoundApp>, x: f32, y: f32, button: PointerButton) {
    let pos = pos2(x, y);
    let s = &mut h.state_mut().synthetic;
    s.push(Event::PointerMoved(pos));
    s.push(Event::PointerButton { pos, button, pressed: true, modifiers: Modifiers::NONE });
    s.push(Event::PointerButton { pos, button, pressed: false, modifiers: Modifiers::NONE });
}

fn clip_count(h: &Harness<'_, SoundApp>, track: &str) -> usize {
    let s = h.state().engine.session();
    s.tracks.iter().find(|t| t.name == track).map_or(0, |t| t.clips().len())
}

fn kick_clip_count(h: &Harness<'_, SoundApp>) -> usize {
    clip_count(h, "Kick")
}

fn demo() -> Harness<'static, SoundApp> {
    let engine = soundcraft_engine::demo::demo_engine();
    let app = SoundApp::new(engine, None, Services::default());
    let mut h = Harness::builder().with_size(vec2(1600.0, 1000.0)).with_pixels_per_point(2.0).build_ui_state(
        |ui, app: &mut SoundApp| {
            let ctx = ui.ctx().clone();
            app.logic(&ctx);
            app.ui(ui);
        },
        app,
    );
    for _ in 0..6 {
        step(&mut h);
    }
    h
}

/// Selects the whole session on `tracks`, as a drag across their clips does.
fn select_all_of(h: &mut Harness<'_, SoundApp>, tracks: &[&str]) {
    h.state_mut().engine.execute("edit.select", &serde_json::json!({"tracks": tracks, "start": 0, "end": 48_000 * 600})).unwrap();
}

/// Right-clicks the Kick clip and clicks `item` in its menu, moving onto the item first as a real pointer would.
fn kick_menu(h: &mut Harness<'_, SoundApp>, item: &str) {
    press(h, 880.0, 240.0, PointerButton::Secondary);
    for _ in 0..10 {
        step(h);
    }
    // Ask the newly laid-out menu where the item is instead of pinning coordinates that change with egui's item metrics.
    let at = h.get_by_label(item).rect().center();
    h.state_mut().synthetic.push(Event::PointerMoved(at));
    for _ in 0..10 {
        step(h);
    }
    press(h, at.x, at.y, PointerButton::Primary);
    for _ in 0..15 {
        step(h);
    }
}

/// Moving the pointer from a right-clicked clip onto the menu's Delete item keeps the menu open; clicking Delete
/// removes one Kick clip.
#[test]
fn clip_menu_stays_open_while_moving_onto_delete() {
    let mut h = demo();
    let before = kick_clip_count(&h);
    assert!(before > 0, "the demo session should have a recorded Kick clip");

    kick_menu(&mut h, "Delete");

    assert_eq!(kick_clip_count(&h), before - 1, "clicking Delete in the clip menu should remove the clip");
}

/// Delete on a clip of a drag selection clears the whole selection, as Edit > Clear does, in one undo step (#1191).
#[test]
fn delete_on_a_selected_clip_clears_the_whole_selection() {
    let mut h = demo();
    let (kick, snare) = (clip_count(&h, "Kick"), clip_count(&h, "Snare"));
    assert!(snare > 1, "the demo session should have several Snare clips");
    select_all_of(&mut h, &["Kick", "Snare"]);

    kick_menu(&mut h, "Delete");

    assert_eq!((clip_count(&h, "Kick"), clip_count(&h, "Snare")), (0, 0), "every selected clip should be cleared");
    h.state_mut().engine.execute("edit.undo", &serde_json::json!({})).unwrap();
    assert_eq!((clip_count(&h, "Kick"), clip_count(&h, "Snare")), (kick, snare), "one undo should bring them all back");
}

/// A clip outside the selection is still deleted on its own, and the selection's clips stay.
#[test]
fn delete_on_a_clip_outside_the_selection_removes_only_that_clip() {
    let mut h = demo();
    let (kick, snare) = (clip_count(&h, "Kick"), clip_count(&h, "Snare"));
    select_all_of(&mut h, &["Snare"]);

    kick_menu(&mut h, "Delete");

    assert_eq!((clip_count(&h, "Kick"), clip_count(&h, "Snare")), (kick - 1, snare));
}

/// With clips selected one by one (Grabber shift-click), Delete removes exactly those clips, not the other clips
/// their span covers.
#[test]
fn delete_on_selected_clips_keeps_the_unselected_clips_between_them() {
    let mut h = demo();
    let ids = |h: &Harness<'_, SoundApp>, track: &str| -> Vec<u64> {
        let s = h.state().engine.session();
        s.tracks.iter().filter(|t| t.name == track).flat_map(|t| t.clips()).map(|c| c.id.0).collect()
    };
    let (kick, snare) = (ids(&h, "Kick"), ids(&h, "Snare"));
    let last_snare = *snare.last().unwrap();
    let clips: Vec<u64> = kick.iter().copied().chain([last_snare]).collect();
    h.state_mut().engine.execute("edit.select", &serde_json::json!({ "clips": clips })).unwrap();

    kick_menu(&mut h, "Delete");

    assert!(ids(&h, "Kick").is_empty(), "the selected Kick clips should be removed");
    let left: Vec<u64> = snare.iter().copied().filter(|id| *id != last_snare).collect();
    assert_eq!(ids(&h, "Snare"), left, "only the selected Snare clip should be removed");
}

/// Selected clips are deleted even when the track selection was emptied after selecting them (Edit > Selection >
/// Remove Edit from Top keeps the clip selection).
#[test]
fn delete_on_selected_clips_works_without_a_track_selection() {
    let mut h = demo();
    let kick = clip_count(&h, "Kick");
    let clips: Vec<u64> = {
        let s = h.state().engine.session();
        s.tracks.iter().filter(|t| t.name == "Kick").flat_map(|t| t.clips()).map(|c| c.id.0).collect()
    };
    let e = &mut h.state_mut().engine;
    e.execute("edit.select", &serde_json::json!({ "clips": clips })).unwrap();
    e.execute("edit.remove_selection_top", &serde_json::json!({})).unwrap();
    assert!(e.session().edit.selected_tracks.is_empty() && !e.session().edit.selected_clips.is_empty());

    kick_menu(&mut h, "Delete");

    assert_eq!(clip_count(&h, "Kick"), kick - clips.len(), "the selected Kick clips should be removed");
}

/// The menu's other entries act on the whole selection too.
#[test]
fn mute_on_a_selected_clip_mutes_every_selected_clip() {
    let mut h = demo();
    select_all_of(&mut h, &["Kick", "Snare"]);

    kick_menu(&mut h, "Mute/Unmute");

    let s = h.state().engine.session();
    let clips: Vec<_> = s.tracks.iter().filter(|t| t.name == "Kick" || t.name == "Snare").flat_map(|t| t.clips()).collect();
    assert!(clips.len() > 2);
    assert!(clips.iter().all(|c| c.muted), "every selected clip should be muted");
}
