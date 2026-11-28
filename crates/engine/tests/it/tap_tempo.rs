//! Tapping a beat sets the tempo from the average of the recent intervals (#1483).

use serde_json::json;
use soundcraft_engine::Engine;

fn tap(e: &mut Engine, ms: f64) {
    e.execute("event.tap_tempo", &json!({"time_ms": ms})).unwrap();
}

fn bpm(e: &Engine) -> f64 {
    e.session().tempo.tempo_at_tick(0)
}

#[test]
fn taps_at_a_steady_interval_set_the_matching_tempo() {
    let mut e = Engine::default();
    for k in 0..4 {
        tap(&mut e, 1_000.0 + 600.0 * k as f64);
    }
    assert!((bpm(&e) - 100.0).abs() < 1e-6, "bpm {}", bpm(&e));
}

#[test]
fn a_single_tap_leaves_the_tempo_and_the_history_alone() {
    let mut e = Engine::default();
    let before = bpm(&e);
    tap(&mut e, 5_000.0);
    assert_eq!(bpm(&e), before);
    assert!(e.undo_history().is_empty(), "history: {:?}", e.undo_history());
}

#[test]
fn a_long_gap_starts_a_new_series() {
    let mut e = Engine::default();
    tap(&mut e, 0.0);
    tap(&mut e, 400.0);
    let first = bpm(&e);
    // The first tap after a pause only starts the count again.
    tap(&mut e, 10_000.0);
    assert_eq!(bpm(&e), first);
    tap(&mut e, 10_750.0);
    assert!((bpm(&e) - 80.0).abs() < 1e-6, "the new series must ignore the old taps: bpm {}", bpm(&e));
}

#[test]
fn recent_taps_outweigh_old_ones_in_a_long_series() {
    let mut e = Engine::default();
    let mut t = 0.0;
    for _ in 0..12 {
        tap(&mut e, t);
        t += 600.0;
    }
    for _ in 0..12 {
        tap(&mut e, t);
        t += 400.0;
    }
    assert!((bpm(&e) - 150.0).abs() < 1e-6, "bpm {}", bpm(&e));
}

#[test]
fn tapping_faster_than_the_tempo_range_clamps_to_its_top() {
    let mut e = Engine::default();
    for k in 0..4 {
        tap(&mut e, 10.0 * k as f64);
    }
    let top = bpm(&e);
    assert!(top > 120.0);
    assert!(e.execute("event.tempo", &json!({"bpm": top + 0.001})).is_err(), "{top} is not the top of the range");
}

#[test]
fn a_series_of_taps_is_one_undo_step() {
    let mut e = Engine::default();
    let before = bpm(&e);
    for k in 0..6 {
        tap(&mut e, 400.0 * k as f64);
    }
    assert_ne!(bpm(&e), before);
    assert_eq!(e.undo_history().len(), 1, "history: {:?}", e.undo_history());
    assert!(e.undo());
    assert_eq!(bpm(&e), before);
}

#[test]
fn a_new_series_is_its_own_undo_step() {
    let mut e = Engine::default();
    tap(&mut e, 0.0);
    tap(&mut e, 500.0);
    let first = bpm(&e);
    tap(&mut e, 10_000.0);
    tap(&mut e, 10_750.0);
    assert!(e.undo());
    assert_eq!(bpm(&e), first);
}

#[test]
fn tapping_without_a_time_is_refused() {
    let mut e = Engine::default();
    assert!(e.execute("event.tap_tempo", &json!({})).is_err());
}

#[test]
fn a_tap_after_undoing_another_edit_is_its_own_step_and_clears_redo() {
    let mut e = Engine::default();
    tap(&mut e, 0.0);
    tap(&mut e, 600.0);
    e.execute("event.tempo", &json!({"bpm": 80, "at": 0})).unwrap();
    assert!(e.undo());
    assert!(e.can_redo());
    // The document is back to what the taps left, but the 80 BPM edit still waits in redo.
    tap(&mut e, 1_100.0);
    assert!(!e.can_redo(), "a new tap must drop the undone edit from redo");
    assert_eq!(e.undo_history().len(), 2, "history: {:?}", e.undo_history());
    assert!(e.undo());
    assert!((bpm(&e) - 100.0).abs() < 1e-6, "undo must bring back the first series: bpm {}", bpm(&e));
}

#[test]
fn taps_in_a_replaced_session_do_not_count_for_the_new_one() {
    let mut e = Engine::default();
    let fresh = e.session().clone();
    let start = bpm(&e);
    tap(&mut e, 0.0);
    tap(&mut e, 600.0);
    e.replace_session(fresh);
    tap(&mut e, 1_100.0);
    assert_eq!(bpm(&e), start, "the first tap in the new session only starts the count");
}
