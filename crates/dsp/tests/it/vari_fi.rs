use soundcraft_dsp::offline::{self, VariFiChange, VariFiSelection};

/// A channel whose samples rise from 0 to 1, so each output sample shows where it was read.
fn ramp(len: usize) -> Vec<f32> {
    (0..len).map(|i| i as f32 / len as f32).collect()
}

/// The fraction of the played input reached at `u` (0 to 1) of the result.
fn slow_down_pos(u: f64) -> f64 {
    (3.0 * u - u * u * u) / 2.0
}

fn speed_up_pos(u: f64) -> f64 {
    1.0 - slow_down_pos(1.0 - u)
}

fn near(got: f32, want: f64, what: &str) {
    assert!((f64::from(got) - want).abs() < 2e-3, "{what}: got {got}, want {want}");
}

#[test]
fn vari_fi_fit_to_keeps_the_length_and_plays_two_thirds() {
    let len = 30_000;
    let x = vec![ramp(len)];
    let slow = offline::vari_fi(&x, VariFiChange::SlowDown, VariFiSelection::FitTo, false);
    assert_eq!(slow[0].len(), len);
    // Slow Down plays the last two thirds: it starts at normal speed one third in and stops at the end.
    for u in [0.0, 0.25, 0.5, 0.75, 0.9] {
        let n = (u * len as f64) as usize;
        near(slow[0][n], 1.0 / 3.0 + 2.0 / 3.0 * slow_down_pos(u), &format!("slow down at {u}"));
    }
    let step = |a: &[f32], n: usize| f64::from(a[n + 1] - a[n]) * len as f64;
    assert!((step(&slow[0], 100) - 1.0).abs() < 0.01, "slow down starts at normal speed: {}", step(&slow[0], 100));
    assert!(step(&slow[0], len - 200) < 0.02, "slow down ends near a stop: {}", step(&slow[0], len - 200));

    // Speed Up plays the first two thirds: it starts from a stop and reaches normal speed at the end.
    let fast = offline::vari_fi(&x, VariFiChange::SpeedUp, VariFiSelection::FitTo, false);
    assert_eq!(fast[0].len(), len);
    for u in [0.1, 0.25, 0.5, 0.75, 0.99] {
        let n = (u * len as f64) as usize;
        near(fast[0][n], 2.0 / 3.0 * speed_up_pos(u), &format!("speed up at {u}"));
    }
    assert!(step(&fast[0], 200) < 0.02, "speed up starts near a stop: {}", step(&fast[0], 200));
    assert!((step(&fast[0], len - 100) - 1.0).abs() < 0.01, "speed up ends at normal speed: {}", step(&fast[0], len - 100));
}

#[test]
fn vari_fi_extend_plays_everything_over_150_percent() {
    let len = 30_000;
    let x = vec![ramp(len), ramp(len)];
    for change in [VariFiChange::SlowDown, VariFiChange::SpeedUp] {
        let out = offline::vari_fi(&x, change, VariFiSelection::Extend, false);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].len(), 45_000, "{change:?}");
        assert_eq!(out[0], out[1], "{change:?}: both channels are read at the same positions");
        for u in [0.1, 0.25, 0.5, 0.75, 0.97] {
            let n = (u * 45_000.0) as usize;
            let pos = if change == VariFiChange::SlowDown { slow_down_pos(u) } else { speed_up_pos(u) };
            near(out[0][n], pos, &format!("{change:?} at {u}"));
        }
    }
}

#[test]
fn vari_fi_fades_follow_the_speed() {
    let len = 30_000;
    let x = vec![vec![0.5; len]];
    let slow = offline::vari_fi(&x, VariFiChange::SlowDown, VariFiSelection::FitTo, true);
    let fast = offline::vari_fi(&x, VariFiChange::SpeedUp, VariFiSelection::FitTo, true);
    for u in [0.25, 0.5, 0.75] {
        let n = (u * len as f64) as usize;
        near(slow[0][n], 0.5 * (1.0 - u * u), &format!("slow down fades out at {u}"));
        near(fast[0][n], 0.5 * (1.0 - (1.0 - u) * (1.0 - u)), &format!("speed up fades in at {u}"));
    }
    assert!(slow[0][len - 1].abs() < 1e-3 && fast[0][0].abs() < 1e-3, "the fades reach silence at the stop");
    // Fades Off keeps the level.
    let off = offline::vari_fi(&x, VariFiChange::SlowDown, VariFiSelection::FitTo, false);
    for u in [0.25, 0.5, 0.75, 0.9] {
        near(off[0][(u * len as f64) as usize], 0.5, &format!("slow down without fades at {u}"));
    }
}

#[test]
fn vari_fi_pitch_falls_as_the_tape_slows() {
    let sr = 48_000.0;
    let len = 96_000;
    let x = vec![(0..len).map(|i| (2.0 * std::f32::consts::PI * 1000.0 * i as f32 / sr).sin() * 0.5).collect::<Vec<f32>>()];
    let out = offline::vari_fi(&x, VariFiChange::SlowDown, VariFiSelection::Extend, false);
    // Upward zero crossings per second in a 0.1 s window starting at `u`.
    let rate = |u: f64| {
        let a = (u * out[0].len() as f64) as usize;
        let w = &out[0][a..a + 4800];
        w.windows(2).filter(|p| p[0] < 0.0 && p[1] >= 0.0).count() as f64 * 10.0
    };
    assert!((rate(0.0) - 1000.0).abs() < 20.0, "starts at the original pitch: {}", rate(0.0));
    let half = 1000.0 * 0.75;
    assert!((rate(0.5) - half).abs() < 30.0, "three quarters speed halfway: {}", rate(0.5));
    assert!(rate(0.9) < 250.0, "low near the stop: {}", rate(0.9));
}

#[test]
fn vari_fi_handles_empty_and_uneven_channels() {
    assert!(offline::vari_fi(&[], VariFiChange::SlowDown, VariFiSelection::Extend, true).is_empty());
    let out = offline::vari_fi(&[vec![], vec![]], VariFiChange::SpeedUp, VariFiSelection::FitTo, true);
    assert!(out.iter().all(Vec::is_empty));
    // A shorter channel is read as silence past its end, and every channel gets the same length.
    let out = offline::vari_fi(&[vec![0.5; 300], vec![0.5; 30]], VariFiChange::SlowDown, VariFiSelection::Extend, false);
    assert_eq!((out[0].len(), out[1].len()), (450, 450));
    assert!(out.iter().flatten().all(|v| v.is_finite()));
}
