//! Meter changes stay on bar lines when an earlier meter is edited (issue #250).

use soundcraft_time::{BarBeat, MeterEvent, SampleRate, TempoMap};

fn assert_positions_round_trip(map: &TempoMap) {
    for bar in 1..=5 {
        let meter = map.meter_at_tick(map.bar_start_tick(bar));
        for beat in 1..=i64::from(meter.numerator) {
            let position = BarBeat { bar, beat, tick: 0 };
            assert_eq!(map.bar_beat_at_tick(map.tick_at_bar_beat(position)), position);
        }
    }
}

fn meter_bars(map: &TempoMap) -> Vec<(i64, u32, u32)> {
    map.meters()
        .iter()
        .map(|m| {
            let bb = map.bar_beat_at_tick(m.tick);
            assert_eq!((bb.beat, bb.tick), (1, 0), "meter at tick {} is off the bar line", m.tick);
            (bb.bar, m.numerator, m.denominator)
        })
        .collect()
}

#[test]
fn earlier_meter_edit_preserves_bar_positions() {
    let mut map = TempoMap::default();
    let at = map.samples_to_ticks(192_000, SampleRate::HZ_48000);
    assert_eq!(at, 7680);
    map.set_meter(at, 3, 4).unwrap();
    assert_positions_round_trip(&map);
    map.set_meter(0, 6, 4).unwrap();
    assert_positions_round_trip(&map);
    // The 3/4 change stays at bar 3, now two 6/4 bars in.
    assert_eq!(meter_bars(&map), vec![(1, 6, 4), (3, 3, 4)]);
    assert_eq!(map.meters().get(1).map(|m| m.tick), Some(2 * 5760));
}

#[test]
fn inserting_and_removing_meters_keeps_later_bars() {
    let mut map = TempoMap::default();
    map.set_meter(map.bar_start_tick(5), 5, 4).unwrap();
    map.set_meter(map.bar_start_tick(3), 3, 4).unwrap();
    assert_eq!(meter_bars(&map), vec![(1, 4, 4), (3, 3, 4), (5, 5, 4)]);
    // Inserting 7/8 at bar 2 keeps 3/4 at bar 3 and 5/4 at bar 5.
    map.set_meter(map.bar_start_tick(2), 7, 8).unwrap();
    assert_eq!(meter_bars(&map), vec![(1, 4, 4), (2, 7, 8), (3, 3, 4), (5, 5, 4)]);
    assert_positions_round_trip(&map);
    // Removing 3/4 leaves 5/4 at bar 5.
    let three_four = map.meters().iter().find(|m| m.numerator == 3).map(|m| m.tick).unwrap();
    assert!(map.remove_meter(three_four));
    assert_eq!(meter_bars(&map), vec![(1, 4, 4), (2, 7, 8), (5, 5, 4)]);
    assert_positions_round_trip(&map);
}

#[test]
fn initial_meter_first_and_restoring_controls() {
    // Setting the initial meter first, then the later change at the same raw tick, snaps to a bar line.
    let mut map = TempoMap::default();
    map.set_meter(0, 6, 4).unwrap();
    map.set_meter(7680, 3, 4).unwrap();
    assert_eq!(map.meters().get(1), Some(&MeterEvent { tick: 5760, numerator: 3, denominator: 4 }));
    assert_positions_round_trip(&map);
    // Restoring 4/4 after the edit puts the 3/4 change back where it started.
    let mut map = TempoMap::default();
    map.set_meter(7680, 3, 4).unwrap();
    map.set_meter(0, 6, 4).unwrap();
    map.set_meter(0, 4, 4).unwrap();
    assert_eq!(map.meters().get(1), Some(&MeterEvent { tick: 7680, numerator: 3, denominator: 4 }));
    assert_positions_round_trip(&map);
}
