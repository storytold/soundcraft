use soundcraft_midi::ops::*;
use soundcraft_midi::*;

fn n(pitch: u8, start: i64, length: i64) -> Note {
    Note { pitch, velocity: 100, release_velocity: 64, channel: 0, start, length }
}

fn starts(notes: &[Note]) -> Vec<i64> {
    notes.iter().map(|n| n.start).collect()
}

#[test]
fn quantize_full_strength() {
    let mut notes = vec![n(60, 10, 200), n(60, 230, 100), n(60, 365, 50), n(60, -20, 40)];
    quantize(&mut notes, &QuantizeOptions { grid_ticks: 240, ..Default::default() });
    assert_eq!(starts(&notes), vec![0, 240, 480, 0]);
    // Durations preserved by default.
    assert_eq!(notes.iter().map(|n| n.length).collect::<Vec<_>>(), vec![200, 100, 50, 40]);
}

#[test]
fn quantize_half_strength() {
    let mut notes = vec![n(60, 20, 100), n(60, 220, 100)];
    quantize(&mut notes, &QuantizeOptions { grid_ticks: 240, strength: 0.5, ..Default::default() });
    assert_eq!(starts(&notes), vec![10, 230]);
}

#[test]
fn quantize_swing() {
    // 100% swing on a 1/8 (480) grid moves odd lines by 480/3 = 160 (triplet feel).
    let mut notes = vec![n(60, 0, 10), n(60, 470, 10), n(60, 650, 10), n(60, 950, 10)];
    quantize(&mut notes, &QuantizeOptions { grid_ticks: 480, swing: 1.0, ..Default::default() });
    assert_eq!(starts(&notes), vec![0, 640, 640, 960]);
    let mut notes = vec![n(60, 480, 10)];
    quantize(&mut notes, &QuantizeOptions { grid_ticks: 480, swing: 0.5, ..Default::default() });
    assert_eq!(starts(&notes), vec![560]);
}

#[test]
fn quantize_include_exclude() {
    let opts = QuantizeOptions { grid_ticks: 240, include_within: 0.5, ..Default::default() };
    // Half grid = 120; include within 60 ticks.
    let mut notes = vec![n(60, 50, 10), n(60, 100, 10)];
    quantize(&mut notes, &opts);
    assert_eq!(starts(&notes), vec![0, 100]);

    let opts = QuantizeOptions { grid_ticks: 240, exclude_within: 0.25, ..Default::default() };
    // Exclude closer than 30 ticks.
    let mut notes = vec![n(60, 20, 10), n(60, 50, 10)];
    quantize(&mut notes, &opts);
    assert_eq!(starts(&notes), vec![20, 0]);
}

#[test]
fn quantize_ends_and_duration() {
    let mut notes = vec![n(60, 10, 220)];
    quantize(&mut notes, &QuantizeOptions { grid_ticks: 240, quantize_ends: true, ..Default::default() });
    assert_eq!((notes[0].start, notes[0].length), (0, 240));

    let mut notes = vec![n(60, 10, 220)];
    quantize(&mut notes, &QuantizeOptions { grid_ticks: 240, preserve_duration: false, ..Default::default() });
    assert_eq!((notes[0].start, notes[0].length), (0, 230));

    // Ends quantized onto the start line still keep a length of at least 1.
    let mut notes = vec![n(60, 10, 5)];
    quantize(&mut notes, &QuantizeOptions { grid_ticks: 240, quantize_ends: true, ..Default::default() });
    assert_eq!(notes[0].length, 1);
}

#[test]
fn quantize_random_is_deterministic_and_bounded() {
    let opts = QuantizeOptions { grid_ticks: 240, random: 0.5, ..Default::default() };
    let mut a: Vec<Note> = (0..50).map(|i| n(60, i * 240 + 7, 10)).collect();
    let mut b = a.clone();
    quantize(&mut a, &opts);
    quantize(&mut b, &opts);
    assert_eq!(a, b);
    for (i, note) in a.iter().enumerate() {
        assert!((note.start - i as i64 * 240).abs() <= 60);
    }
    let mut c: Vec<Note> = (0..50).map(|i| n(60, i * 240 + 7, 10)).collect();
    quantize_with_seed(&mut c, &opts, 42);
    assert_ne!(a, c);
}

#[test]
fn quantize_degenerate_options() {
    let mut notes = vec![n(60, 17, 10)];
    quantize(&mut notes, &QuantizeOptions { grid_ticks: 0, ..Default::default() });
    quantize(&mut notes, &QuantizeOptions { grid_ticks: -5, ..Default::default() });
    assert_eq!(notes[0].start, 17);
    quantize(&mut notes, &QuantizeOptions { grid_ticks: 240, strength: f32::NAN, swing: f32::INFINITY, ..Default::default() });
    assert_eq!(notes[0].start, 17);
    let mut extreme = vec![n(60, i64::MAX - 10, 1), n(60, i64::MIN + 10, 1)];
    quantize(&mut extreme, &QuantizeOptions { grid_ticks: i64::MAX, random: 1.0, ..Default::default() });
}

#[test]
fn transpose_clamps() {
    let mut notes = vec![n(0, 0, 1), n(60, 0, 1), n(127, 0, 1)];
    transpose(&mut notes, 12);
    assert_eq!(notes.iter().map(|n| n.pitch).collect::<Vec<_>>(), vec![12, 72, 127]);
    transpose(&mut notes, -100);
    assert_eq!(notes.iter().map(|n| n.pitch).collect::<Vec<_>>(), vec![0, 0, 27]);
    transpose(&mut notes, i32::MAX);
    assert!(notes.iter().all(|n| n.pitch == 127));
}

#[test]
fn ramp_full_tick_range() {
    let mut notes = vec![n(60, i64::MIN, 1), n(60, 0, 1), n(60, i64::MAX, 1)];
    change_velocity(&mut notes, VelocityOp::Ramp { from: 1, to: 127 });
    assert_eq!(notes.iter().map(|n| n.velocity).collect::<Vec<_>>(), vec![1, 64, 127]);
}

#[test]
fn velocity_ops() {
    let base = vec![n(60, 0, 1), n(60, 100, 1), n(60, 200, 1)];
    let vels = |v: &[Note]| v.iter().map(|n| n.velocity).collect::<Vec<_>>();

    let mut v = base.clone();
    change_velocity(&mut v, VelocityOp::Set(0));
    assert_eq!(vels(&v), vec![1, 1, 1]);
    change_velocity(&mut v, VelocityOp::Set(200));
    assert_eq!(vels(&v), vec![127, 127, 127]);

    let mut v = base.clone();
    change_velocity(&mut v, VelocityOp::Add(50));
    assert_eq!(vels(&v), vec![127; 3]);
    change_velocity(&mut v, VelocityOp::Add(-500));
    assert_eq!(vels(&v), vec![1; 3]);

    let mut v = base.clone();
    change_velocity(&mut v, VelocityOp::Scale(0.5));
    assert_eq!(vels(&v), vec![50; 3]);
    change_velocity(&mut v, VelocityOp::Scale(f32::NAN));
    assert_eq!(vels(&v), vec![50; 3]);

    let mut v = vec![n(60, 0, 1), n(60, 0, 1)];
    v[0].velocity = 10;
    v[1].velocity = 120;
    change_velocity(&mut v, VelocityOp::Limit { min: 100, max: 30 });
    assert_eq!(vels(&v), vec![30, 100]);

    let mut v = base.clone();
    change_velocity(&mut v, VelocityOp::Ramp { from: 20, to: 120 });
    assert_eq!(vels(&v), vec![20, 70, 120]);

    let mut a = base.clone();
    let mut b = base.clone();
    change_velocity(&mut a, VelocityOp::Humanize(10));
    change_velocity(&mut b, VelocityOp::Humanize(10));
    assert_eq!(a, b);
    assert!(a.iter().all(|n| (90..=110).contains(&n.velocity)));
}

#[test]
fn duration_ops() {
    let lens = |v: &[Note]| v.iter().map(|n| n.length).collect::<Vec<_>>();
    let base = vec![n(60, 0, 100), n(62, 480, 100), n(64, 480, 50), n(60, 960, 100)];

    let mut v = base.clone();
    change_duration(&mut v, DurationOp::Set(0));
    assert_eq!(lens(&v), vec![1; 4]);

    let mut v = base.clone();
    change_duration(&mut v, DurationOp::Add(-75));
    assert_eq!(lens(&v), vec![25, 25, 1, 25]);

    let mut v = base.clone();
    change_duration(&mut v, DurationOp::Scale(2.0));
    assert_eq!(lens(&v), vec![200, 200, 100, 200]);

    let mut v = base.clone();
    change_duration(&mut v, DurationOp::Legato { gap: 10 });
    assert_eq!(lens(&v), vec![470, 470, 470, 100]);

    let mut v = vec![n(60, 0, 500), n(60, 200, 500), n(61, 100, 500), Note { channel: 1, ..n(60, 100, 500) }];
    change_duration(&mut v, DurationOp::RemoveOverlap);
    assert_eq!(lens(&v), vec![200, 500, 500, 500]);
}

#[test]
fn split_by_pitch() {
    let notes = vec![n(36, 0, 1), n(60, 1, 1), n(72, 2, 1), n(48, 3, 1)];
    let (inside, outside) = split_notes(&notes, 60, 40);
    assert_eq!(inside.iter().map(|n| n.pitch).collect::<Vec<_>>(), vec![60, 48]);
    assert_eq!(outside.iter().map(|n| n.pitch).collect::<Vec<_>>(), vec![36, 72]);
}

#[test]
fn sequence_helpers() {
    let mut seq = Sequence {
        notes: vec![n(64, 100, 50), n(60, 100, 10), n(60, 100, 30), n(60, 0, 10), Note { channel: 2, ..n(60, 0, 10) }],
        ctrls: vec![
            CtrlEvent { tick: 500, channel: 0, kind: CtrlKind::Cc(1), value: 1 },
            CtrlEvent { tick: 5, channel: 0, kind: CtrlKind::Cc(1), value: 2 },
            CtrlEvent { tick: 500, channel: 0, kind: CtrlKind::Cc(1), value: 3 },
            CtrlEvent { tick: 500, channel: 0, kind: CtrlKind::Cc(2), value: 4 },
        ],
    };
    assert_eq!(seq.end_tick(), 500);
    seq.sort();
    assert_eq!(starts(&seq.notes), vec![0, 0, 100, 100, 100]);
    assert_eq!(seq.notes[2].pitch, 60);
    assert_eq!(seq.notes_in(0, 100).count(), 2);
    assert_eq!(seq.notes_overlapping(105, 106).count(), 3);

    let removed = seq.remove_duplicates();
    assert_eq!(removed, 2);
    assert_eq!(seq.notes.len(), 4);
    let kept = seq.notes.iter().find(|n| n.start == 100 && n.pitch == 60).unwrap();
    assert_eq!(kept.length, 30);
    assert_eq!(seq.ctrls.iter().map(|c| c.value).collect::<Vec<_>>(), vec![2, 3, 4]);
    assert_eq!(seq.remove_duplicates(), 0);
    assert_eq!(Sequence::default().end_tick(), 0);
}

#[test]
fn sequence_serde_round_trip() {
    let seq = Sequence { notes: vec![n(60, 0, 1)], ctrls: vec![CtrlEvent { tick: 1, channel: 0, kind: CtrlKind::PolyPressure(60), value: 9 }] };
    let json = serde_json::to_string(&seq).unwrap();
    assert_eq!(serde_json::from_str::<Sequence>(&json).unwrap(), seq);
}

#[test]
fn remove_duplicates_handles_a_large_same_tick_controller_group() {
    // Every channel x controller number at one tick, twice: the second copy of each wins. A
    // rescan of the tick's kept events per incoming event would make this quadratic.
    let keys: Vec<(u8, u8)> = (0..=255u8).flat_map(|ch| (0..=255u8).map(move |cc| (ch, cc))).collect();
    let ctrls = [0, 1]
        .iter()
        .flat_map(|&pass| keys.iter().map(move |&(channel, cc)| CtrlEvent { tick: 7, channel, kind: CtrlKind::Cc(cc), value: pass }))
        .collect();
    let mut seq = Sequence { notes: Vec::new(), ctrls };
    let start = std::time::Instant::now();
    assert_eq!(seq.remove_duplicates(), keys.len());
    assert!(start.elapsed() < std::time::Duration::from_secs(10), "{:?}", start.elapsed());
    assert_eq!(seq.ctrls.len(), keys.len());
    assert!(seq.ctrls.iter().all(|c| c.value == 1));
}

#[test]
fn remove_duplicates_matches_a_per_tick_rescan_on_mixed_ticks() {
    // The straightforward per-tick rescan the indexed version replaced, kept as an oracle.
    fn reference(mut ctrls: Vec<CtrlEvent>) -> Vec<CtrlEvent> {
        ctrls.sort_by_key(|c| c.tick);
        let key = |k: CtrlKind| match k {
            CtrlKind::Cc(n) => (0, n),
            CtrlKind::PitchBend => (1, 0),
            CtrlKind::ChannelPressure => (2, 0),
            CtrlKind::PolyPressure(p) => (3, p),
            CtrlKind::Program => (4, 0),
        };
        let mut kept: Vec<CtrlEvent> = Vec::new();
        for c in ctrls {
            match kept.iter_mut().rev().take_while(|k| k.tick == c.tick).find(|k| k.channel == c.channel && key(k.kind) == key(c.kind)) {
                Some(slot) => *slot = c,
                None => kept.push(c),
            }
        }
        kept
    }

    // Few ticks, channels and kinds, so the same key recurs both within a tick (interleaved with
    // other keys) and across ticks; unsorted input from a fixed xorshift stream.
    let mut state = 0x9E37_79B9_7F4A_7C15u64;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    let kinds = |r: u64| match r % 5 {
        0 => CtrlKind::Cc((r >> 8) as u8 % 4),
        1 => CtrlKind::PitchBend,
        2 => CtrlKind::ChannelPressure,
        3 => CtrlKind::PolyPressure((r >> 8) as u8 % 4),
        _ => CtrlKind::Program,
    };
    for round in 0..200 {
        let mut ctrls = Vec::new();
        for i in 0..(next() % 64) {
            let r = next();
            ctrls.push(CtrlEvent { tick: (r % 6) as i64 - 2, channel: (r >> 16) as u8 % 3, kind: kinds(r >> 24), value: (round * 100 + i) as i32 });
        }
        let mut seq = Sequence { notes: Vec::new(), ctrls: ctrls.clone() };
        let removed = seq.remove_duplicates();
        let expected = reference(ctrls.clone());
        assert_eq!(seq.ctrls, expected, "round {round}");
        assert_eq!(removed, ctrls.len() - expected.len(), "round {round}");
    }
}
