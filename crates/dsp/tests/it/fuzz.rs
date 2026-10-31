use proptest::prelude::*;
use soundcraft_dsp::{create, offline, plugins};

proptest! {
    #![proptest_config(ProptestConfig::with_cases(48))]

    /// Any parameter values and any finite input: no panic, finite output.
    #[test]
    fn plugins_survive_random_params(
        which in 0usize..64,
        values in proptest::collection::vec(any::<f32>(), 32),
        input in proptest::collection::vec(-4.0f32..4.0, 1..700),
        block in 1usize..300,
    ) {
        let info = plugins()[which % plugins().len()];
        let mut p = create(info.id).unwrap();
        p.prepare(44_100.0, block, 2);
        for (param, v) in info.params.iter().zip(values.iter().cycle()) {
            p.set_param(param.id, *v);
        }
        p.note_on(0, 60, 100);
        let mut io = [input.clone(), input.clone()];
        let len = input.len();
        let mut pos = 0;
        while pos < len {
            let n = block.min(len - pos);
            let mut buf = vec![io[0][pos..pos + n].to_vec(), io[1][pos..pos + n].to_vec()];
            p.process(&mut buf, n);
            io[0][pos..pos + n].copy_from_slice(&buf[0]);
            io[1][pos..pos + n].copy_from_slice(&buf[1]);
            pos += n;
        }
        prop_assert!(io.iter().flatten().all(|v| v.is_finite()), "{} produced non-finite output", info.id);
    }

    /// Offline helpers accept any arguments without panicking.
    #[test]
    fn offline_survives_random_args(
        data in proptest::collection::vec(any::<f32>(), 0..400),
        a in any::<f32>(),
        n in 0usize..1000,
        r in any::<f64>(),
    ) {
        let ch = vec![data.clone(), data];
        let _ = offline::non_silent_ranges(&ch, a, n, n, n);
        let _ = offline::detect_transients(&ch, a, a);
        let _ = offline::time_stretch(&ch, r, a);
        let _ = offline::pitch_shift(&ch, a, a);
        let change = if n % 2 == 0 { offline::SpeedRampChange::SpeedUp } else { offline::SpeedRampChange::SlowDown };
        let selection = if n % 3 == 0 { offline::SpeedRampSelection::Extend } else { offline::SpeedRampSelection::FitTo };
        let _ = offline::speed_ramp(&ch, change, selection, n % 5 == 0);
        let _ = offline::resample(&ch, n as u32, (n as u32).wrapping_mul(3) % 200_000);
        let mut m = ch.clone();
        offline::normalize(&mut m, a, n % 2 == 0);
        offline::fade(&mut m, n, n / 2, offline::FadeShape::SCurve);
        offline::gain(&mut m, a);
        let _ = soundcraft_dsp::spectrum::magnitude_db(&ch[0], n);
    }
}
