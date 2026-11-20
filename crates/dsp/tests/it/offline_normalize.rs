use soundcraft_dsp::offline::{normalize, peak};

#[test]
fn normalize_subnormal_peak_stays_finite() {
    for per_channel in [false, true] {
        let mut c = vec![vec![1e-44f32, -1e-44]];
        normalize(&mut c, 0.0, per_channel);
        assert!(c[0].iter().all(|s| s.is_finite()), "{c:?}");
        assert!((peak(&c) - 1.0).abs() < 1e-3, "{c:?}");
    }
}

#[test]
fn normalize_silent_and_normal_channels() {
    let mut c = vec![vec![0.0f32; 4], vec![0.25, -0.5]];
    normalize(&mut c, 0.0, true);
    assert_eq!(c[0], vec![0.0; 4]);
    assert!((peak(&c[1..]) - 1.0).abs() < 1e-5);
}
