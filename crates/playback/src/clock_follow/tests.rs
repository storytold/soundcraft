use super::*;

fn feed(reader: &mut Follower, byte: u8, at: i64) {
    reader.receive(&[byte], at);
}

#[test]
fn start_continue_spp_stop_and_realtime_inside_sysex() {
    let mut r = Follower::default();
    r.receive(&[0xf2, 11, 1], 0);
    assert_eq!(r.take(), Some(Action::Locate(139 * 6)));
    feed(&mut r, 0xfa, 10);
    assert_eq!(r.take(), Some(Action::Locate(0)));
    assert!(r.take().is_none());
    r.receive(&[0xf0, 1, 0xf8, 2, 0xf7], 1_000);
    assert_eq!(r.take(), Some(Action::Play { pulse: 0, at: 1_000 }));
    r.receive(&[0xf2, 127, 127], 2_000);
    assert!(r.take().is_none());
    feed(&mut r, 0xfc, 80_000);
    assert_eq!(r.take(), Some(Action::Stop));
    r.receive(&[0xf2, 3, 0xf8, 1], 81_000);
    assert_eq!(r.take(), Some(Action::Locate(131 * 6)));
    feed(&mut r, 0xfb, 82_000);
    feed(&mut r, 0xf8, 100_000);
    assert_eq!(r.take(), Some(Action::Play { pulse: 131 * 6, at: 100_000 }));
}

#[test]
fn a_fresh_start_after_idle_clock_timeout_arms_the_next_pulse() {
    let mut r = Follower::default();
    feed(&mut r, 0xf8, 1);
    feed(&mut r, 0xfa, 200_001);
    assert_eq!(r.take(), Some(Action::Locate(0)));
    feed(&mut r, 0xf8, 200_005);
    assert_eq!(r.take(), Some(Action::Play { pulse: 0, at: 200_005 }));
}

#[test]
fn coast_or_stop_once_on_loss_and_resume_only_after_transport() {
    for coast in [true, false] {
        let mut r = Follower { coast, ..Follower::default() };
        feed(&mut r, 0xfa, 0);
        feed(&mut r, 0xf8, 1);
        while r.take().is_some() {}
        r.expire(149_999);
        assert_eq!(r.state, State::Locked);
        r.expire(150_002);
        r.expire(300_500);
        assert_eq!(r.dropouts, 1);
        assert_eq!(r.state, if coast { State::Coasting } else { State::Stopped });
        assert_eq!(r.take(), if coast { None } else { Some(Action::Stop) });
        feed(&mut r, 0xf8, 301_000);
        assert!(r.take().is_none());
        feed(&mut r, 0xfb, 302_000);
        feed(&mut r, 0xf8, 321_000);
        assert!(matches!(r.take(), Some(Action::Play { .. })));
    }
}

fn gaussian(seed: &mut u64) -> f64 {
    let mut unit = || {
        *seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
        ((*seed >> 11) as f64 + 1.) / ((1u64 << 53) as f64 + 1.)
    };
    (-2. * unit().ln()).sqrt() * (std::f64::consts::TAU * unit()).cos()
}

#[test]
fn locks_and_predicts_pulses_across_tempos_jitter_and_usb_quantization() {
    for bpm in [90., 120., 174.] {
        for jitter in [500., 2000., 4000.] {
            let mut r = Follower::default();
            let mut seed = 100;
            let mut errors = Vec::new();
            let mut phase_errors = Vec::new();
            let interval = 60_000_000. / (24. * bpm);
            for pulse in 0..700 {
                let ideal = 100_000. + pulse as f64 * interval;
                let us = ((ideal + gaussian(&mut seed) * jitter) / 1000.).round() as i64 * 1000;
                r.receive(&[0xf8], us);
                if pulse == 23 {
                    assert!((r.bpm().unwrap() - bpm).abs() < 2., "failed to acquire within a beat");
                }
                if pulse >= 240 {
                    errors.push((r.bpm().unwrap() - bpm).abs());
                    phase_errors.push((r.phase_error_us as f64).abs());
                }
            }
            errors.sort_by(f64::total_cmp);
            phase_errors.sort_by(f64::total_cmp);
            let p95 = errors[errors.len() * 95 / 100];
            if bpm == 120. && jitter == 2000. {
                assert!(errors.iter().all(|err| *err < 0.05), "steady-state BPM crossed the display tolerance");
            }
            assert!(p95 < if bpm == 120. && jitter <= 2000. { 0.05 } else { 0.25 }, "{bpm} BPM/{jitter} us: {p95}");
            assert!(phase_errors[phase_errors.len() * 95 / 100] <= jitter * 3.5 + 1000.);
        }
    }
}

#[test]
fn responds_to_steps_within_a_beat_and_follows_a_tempo_ramp() {
    let mut r = Follower::default();
    let mut us: f64 = 0.;
    for _ in 0..220 {
        us += 60_000_000. / (120. * 24.);
        feed(&mut r, 0xf8, us.round() as i64);
    }
    for _ in 0..24 {
        us += 60_000_000. / (150. * 24.);
        feed(&mut r, 0xf8, us.round() as i64);
    }
    assert!((r.bpm().unwrap() - 150.).abs() < 0.1, "{:?}", r.bpm());
    for pulse in 1..=240 {
        let bpm = 150. - f64::from(pulse) * 0.15;
        us += 60_000_000. / (bpm * 24.);
        feed(&mut r, 0xf8, us.round() as i64);
        if pulse > 24 {
            assert!((r.bpm().unwrap() - bpm).abs() < 3.5, "{pulse} {bpm} {:?}", r.bpm());
        }
    }
}

#[test]
fn malformed_input_never_starts_without_a_real_start_or_continue() {
    let mut r = Follower::default();
    let mut seed = 11u64;
    for i in 0..50_000 {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
        let byte = (seed >> 40) as u8;
        if byte != 0xfa && byte != 0xfb {
            r.receive(&[byte], i);
        }
        assert!(!matches!(r.take(), Some(Action::Play { .. })));
    }
}
