use super::*;
use history::History;
use soundcraft_time::Range;

/// Six simulated hours of seeded A-6 tempo steps, with the same input jitter on every run.
#[test]
#[ignore = "six simulated hours, real silent-output snapshot publication"]
fn six_hour_clock_history_has_bounded_unreferenced_events() {
    let peak = six_hour_clock_history(true);
    assert!(peak <= 512, "{peak} unreferenced tempo events");
}

/// Uncapped control for a same-build comparison: identical wire observations, estimator,
/// grid edits and publications; only the history retention policy differs.
#[test]
#[ignore = "six simulated hours, same-build uncapped memory and publication control"]
fn six_hour_clock_history_uncapped_control() {
    six_hour_clock_history(false);
}

fn six_hour_clock_history(compact: bool) -> usize {
    let engine = soundcraft_engine::Engine::default();
    let player = simulated_player(&engine);
    let sr = SampleRate::HZ_48000;
    let inputs = MidiInputs::with_port_names(Vec::new(), vec!["DJ".into()]);
    let feed = inputs.feed();
    let id = feed.ports().assign("DJ");
    feed.set_clock_follow(Some(id), true, 0, false);
    let mut grid = TempoMap::new(120., 4, 4).unwrap();
    let mut actions = Vec::new();
    let mut bpm = 120.;
    let mut us = 0_f64;
    let mut seed = 41_u64;
    let mut next = 180_000_000.;
    let mut history = history::History::default();
    let mut total_edits = 0;
    let mut peak_count = 1;
    let mut changes = 0;
    feed.set_clock(0);
    feed.receive(id, &[0xfa]);
    let mut pulses = 0;
    while us < 21_600_000_000. {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
        let at = if pulses == 0 { 0 } else { us.round() as i64 + ((seed >> 32) % 2001) as i64 - 1_000 };
        feed.set_clock(at);
        feed.receive(id, &[0xf8]);
        if pulses % 6 == 0 {
            let snapshot = feed.clock_follow_status(&mut actions);
            actions.clear();
            if update_grid(&mut grid, Some((0, 0)), 0, snapshot, sr, sr.samples(at as f64 / 1e6 + 0.030)) {
                total_edits += 1;
                if compact && history.should_compact(&grid) {
                    history.compact(&mut grid, sr, 0, vec![]);
                }
                peak_count = peak_count.max(grid.tempos().len());
            }
        }
        if us >= next {
            bpm = 120. + ((seed >> 35) % 31) as f64;
            next = us + (180. + ((seed >> 32) % 301) as f64) * 1e6;
            changes += 1;
        }
        pulses += 1;
        us += 2_500_000. / bpm;
    }

    player.prepare_clock_session(engine.session_arc(), &grid);
    let mut timings = Vec::new();
    let mut publish_grid = grid.clone();
    let probe_tick = pulses * TICKS_PER_PULSE;
    for i in 0..101 {
        publish_grid.set_tempo(probe_tick, if i % 2 == 0 { 120.1 } else { 119.9 }).unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            let start = std::time::Instant::now();
            if player.update_follow_grid(&publish_grid) {
                timings.push(start.elapsed());
                break;
            }
            assert!(start < deadline, "silent callback failed to consume the published snapshot");
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }
    timings.sort_unstable();
    eprintln!(
        "A6 pulses={pulses} changes={changes} edits={total_edits} peak_events={peak_count} final_events={} end_publish_p95_us={:.3} compact={compact}",
        grid.tempos().len(),
        timings[95].as_secs_f64() * 1e6
    );
    peak_count
}

#[test]
fn an_open_clock_recording_retains_its_original_tempo_changes() {
    let sr = SampleRate::HZ_48000;
    let mut grid = TempoMap::default();
    for i in 0..900 {
        grid.set_tempo(i * 960, 100. + (i % 7) as f64).unwrap();
    }
    let before = grid.clone();
    let punch = 75 * 960;
    let mut history = history::History::default();
    history.compact(&mut grid, sr, 0, vec![Range::new(before.tick_to_samples(punch, sr), i64::MAX)]);
    assert!(grid.tempos().len() < before.tempos().len(), "pre-punch changes may be compacted");
    let preserved: Vec<_> = grid.tempos().iter().filter(|event| event.tick >= punch).collect();
    let original: Vec<_> = before.tempos().iter().filter(|event| event.tick >= punch).collect();
    assert_eq!(preserved, original);
    for event in original {
        assert_eq!(grid.tick_to_samples(event.tick, sr), before.tick_to_samples(event.tick, sr));
    }
}

fn historical_grid() -> TempoMap {
    let mut grid = TempoMap::default();
    for n in 0..1250 {
        grid.set_tempo(n * 960, 100. + (n % 13) as f64).unwrap();
    }
    grid
}

#[test]
fn nonzero_continue_keeps_its_sample_without_pinning_the_prior_hours() {
    let sr = SampleRate::HZ_48000;
    let mut grid = historical_grid();
    let before = grid.clone();
    let anchor = 990 * 960 + 321;
    History::default().compact(&mut grid, sr, anchor, vec![]);
    assert!(grid.tempos().len() <= 512, "{} events retained at nonzero SPP", grid.tempos().len());
    for tick in [anchor - 31, anchor, anchor + 93, 1101 * 960, 1800 * 960] {
        assert_eq!(grid.tick_to_samples(tick, sr), before.tick_to_samples(tick, sr));
    }
}

#[test]
fn completed_early_takes_do_not_increase_the_unreferenced_budget() {
    let sr = SampleRate::HZ_48000;
    let mut grid = historical_grid();
    // Each old take needs three complete tempo intervals from its own recording.
    let take_ticks: Vec<_> = (1..=20).map(|i| (i * 30 * 960 + 100, (i * 30 + 2) * 960 + 200)).collect();
    let spans: Vec<_> = take_ticks.iter().map(|&(a, b)| Range::new(grid.tick_to_samples(a, sr), grid.tick_to_samples(b, sr))).collect();
    let mut history = History::default();
    history.compact(&mut grid, sr, 0, spans.clone());
    for i in 1250..2050 {
        grid.set_tempo(i * 960, 100. + (i % 13) as f64).unwrap();
        if history.should_compact(&grid) {
            history.compact(&mut grid, sr, 0, spans.clone());
        }
        let protected = grid
            .tempos()
            .iter()
            .filter(|e| {
                take_ticks.iter().any(|&(a, b)| {
                    let start = a / 960 * 960;
                    (start..=b).contains(&e.tick)
                })
            })
            .count();
        let unused = grid.tempos().len() - protected;
        assert!(unused <= 512, "{unused} unreferenced events plus {protected} take events");
    }
}
