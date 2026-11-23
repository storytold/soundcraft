use super::*;

fn feed(reader: &mut Follower, byte: u8, at: i64) {
    reader.receive(&[byte], at);
}

#[test]
fn coalesced_delivery_expires_after_individually_timed_pulses_resume() {
    let mut follower = Follower::default();
    for stamp in [0, 200, 400, 60_000, 60_200, 60_400] {
        feed(&mut follower, 0xf8, stamp);
    }
    assert!(follower.snapshot(60_400).packed_delivery);
    for stamp in [120_000, 140_000, 160_000, 180_000, 200_000] {
        feed(&mut follower, 0xf8, stamp);
    }
    assert!(!follower.snapshot(200_000).packed_delivery);
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
fn only_a_stopped_transport_needs_continue_after_clock_loss() {
    for coast in [true, false] {
        let mut r = Follower { coast, ..Follower::default() };
        feed(&mut r, 0xf8, -40_000);
        feed(&mut r, 0xf8, -20_000);
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
        assert_eq!(r.state, if coast { State::Coasting } else { State::Stopped });
        feed(&mut r, 0xf8, 321_000);
        assert!(r.take().is_none());
        assert_eq!(r.state, if coast { State::Locked } else { State::Stopped });
        feed(&mut r, 0xfb, 322_000);
        feed(&mut r, 0xf8, 341_000);
        assert_eq!(matches!(r.take(), Some(Action::Play { .. })), !coast);
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
                if pulse >= 24 {
                    assert!(!r.steady.is_tempo_ramp(), "false ramp at {bpm} BPM/{jitter} us, pulse {pulse}");
                }
                if pulse >= 240 {
                    errors.push((r.bpm().unwrap() - bpm).abs());
                    phase_errors.push((r.phase_error_us as f64).abs());
                }
            }
            errors.sort_by(f64::total_cmp);
            phase_errors.sort_by(f64::total_cmp);
            let p95 = errors[errors.len() * 95 / 100];
            eprintln!("steady {bpm} {jitter} BPMp95={p95:.8} phasep95_us={:.4}", phase_errors[phase_errors.len() * 95 / 100]);
            if bpm == 120. && jitter == 2000. {
                assert!(errors.iter().all(|err| *err < 0.05), "steady-state BPM crossed the display tolerance");
            }
            assert!(p95 < if bpm == 120. && jitter <= 2000. { 0.05 } else { 0.25 }, "{bpm} BPM/{jitter} us: {p95}");
            assert!(phase_errors[phase_errors.len() * 95 / 100] <= jitter * 3.5 + 1000.);
        }
    }
}

#[test]
fn rapid_phase_recovery_requires_a_sustained_step_and_expires() {
    for interrupt in [None, Some(0xfc), Some(0xfa)] {
        let mut reader = Follower::default();
        let mut stamp = 0;
        feed(&mut reader, 0xfa, stamp);
        // Establish the long-window tempo reference before introducing packet jitter.
        for _ in 0..192 {
            stamp += 30_000;
            feed(&mut reader, 0xf8, stamp);
        }
        // A late packet followed by an early one preserves the source tempo.
        for dt in [45_000, 15_000, 30_000, 30_000] {
            stamp += dt;
            feed(&mut reader, 0xf8, stamp);
            assert!(!reader.snapshot(stamp).recovering_tempo_step);
        }
        for _ in 0..4 {
            stamp += 20_000;
            feed(&mut reader, 0xf8, stamp);
        }
        assert!(reader.snapshot(stamp).recovering_tempo_step);
        if let Some(byte) = interrupt {
            feed(&mut reader, byte, stamp);
        } else {
            for _ in 0..24 {
                stamp += 20_000;
                feed(&mut reader, 0xf8, stamp);
            }
        }
        assert!(!reader.snapshot(stamp).recovering_tempo_step);
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

#[test]
fn midi_clock_wakes_only_on_transport_actions() {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    let feed = crate::midi::MidiFeed::default();
    let counter = Arc::new(AtomicUsize::default());
    let wake = counter.clone();
    feed.set_mtc_wakeup(|| {
        Box::new(move || {
            wake.fetch_add(1, Ordering::Relaxed);
        })
    });
    #[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
    let worker = Arc::new(AtomicUsize::default());
    #[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
    {
        let worker = worker.clone();
        feed.set_sync_waker(Arc::new(move |_| {
            worker.fetch_add(1, Ordering::Relaxed);
        }));
    }
    feed.set_clock_follow(Some(0), true, 0, false);
    feed.receive(0, &[0xfa]);
    for pulse in 1..=200 {
        feed.set_clock(pulse * 20_833);
        feed.receive(0, &[0xf8]);
    }
    assert_eq!(counter.load(Ordering::Relaxed), 2);
    feed.receive(0, &[0xfc]);
    feed.receive(0, &[0xf2, 1]);
    assert_eq!(counter.load(Ordering::Relaxed), 3);
    feed.receive(0, &[0]);
    assert_eq!(counter.load(Ordering::Relaxed), 4);
    #[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
    // Beat Clock transport is applied by the host through Player. Raw input has no
    // audio command pending and must not start an output-worker timeout.
    assert_eq!(worker.load(Ordering::Relaxed), 0);
}

#[test]
fn new_transport_survives_expiry_of_an_older_idle_pulse() {
    for request in [0xfa, 0xfb] {
        let mut r = Follower::default();
        feed(&mut r, 0xf8, 0);
        feed(&mut r, request, 145_000);
        if request == 0xfa {
            assert_eq!(r.take(), Some(Action::Locate(0)));
        }
        r.expire(151_000);
        feed(&mut r, 0xf8, 155_000);
        assert_eq!(r.take(), Some(Action::Play { pulse: 0, at: 155_000 }));
        let mut cold = Follower::default();
        feed(&mut cold, request, 0);
        cold.take();
        feed(&mut cold, 0xf8, 500_000);
        assert_eq!(cold.take(), Some(Action::Play { pulse: 0, at: 500_000 }));
    }
}

#[test]
fn failed_prepared_send_leaves_the_start_for_the_host() {
    use std::sync::{Arc, Mutex, atomic::Ordering};
    let shared = Arc::new(crate::Shared { position: 16.into(), playing: false.into(), speed: 1000.into(), meters: Mutex::default() });
    let (sender, receiver) = crate::channel();
    drop(receiver);
    let tx = Arc::new(sender);
    let revision = Arc::new(Mutex::new(true));
    #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
    let output: Arc<Mutex<crate::sync_output::Output>> = Arc::default();
    let cue = PreparedResume {
        revision: Arc::downgrade(&revision),
        pulse: 0,
        origin: 1000,
        offset: 0,
        sr: soundcraft_model::Session::default().sample_rate,
        tx: Arc::downgrade(&tx),
        shared: Arc::downgrade(&shared),
        #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
        output: Arc::downgrade(&output),
        #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
        silent: true,
    };
    let mut r = Follower { cue: Some(cue), ..Default::default() };
    r.push(Action::Play { pulse: 0, at: 20 });
    r.dispatch_prepared(|| 21);
    assert_eq!(r.take(), Some(Action::Play { pulse: 0, at: 20 }));
    assert!(!shared.playing.load(Ordering::Acquire));
    assert_eq!(shared.position.load(Ordering::Relaxed), 16);
}

#[test]
fn edit_during_prepared_clock_dispatch_leaves_the_original_start_for_the_host() {
    use std::sync::{Arc, Mutex, atomic::Ordering, mpsc::sync_channel};
    let shared = Arc::new(crate::Shared { position: 16.into(), playing: false.into(), speed: 1000.into(), meters: Mutex::default() });
    let (sender, receiver) = crate::channel();
    let tx = Arc::new(sender);
    let revision = Arc::new(Mutex::new(true));
    #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
    let output: Arc<Mutex<crate::sync_output::Output>> = Arc::default();
    let cue = PreparedResume {
        revision: Arc::downgrade(&revision),
        pulse: 0,
        origin: 1000,
        offset: 0,
        sr: soundcraft_model::Session::default().sample_rate,
        tx: Arc::downgrade(&tx),
        shared: Arc::downgrade(&shared),
        #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
        output: Arc::downgrade(&output),
        #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
        silent: true,
    };
    let mut follow = Follower { cue: Some(cue), ..Default::default() };
    follow.push(Action::Play { pulse: 0, at: 20 });
    let (checking, suspend) = sync_channel(0);
    let (edited, resume) = sync_channel(0);
    std::thread::scope(|scope| {
        scope.spawn(move || {
            suspend.recv().unwrap();
            *revision.lock().unwrap() = false;
            drop(revision);
            edited.send(()).unwrap();
        });
        // On the input thread the destination is available and the cue was observed
        // before the edit. Resume after invalidation, while calculating the last timing offset.
        follow.dispatch_prepared(|| {
            checking.send(()).unwrap();
            resume.recv().unwrap();
            21
        });
    });
    assert!(receiver.try_recv().is_err(), "the renderer must not receive a stale start");
    assert_eq!(follow.take(), Some(Action::Play { pulse: 0, at: 20 }));
    assert_eq!(follow.take(), None);
    assert!(!shared.playing.load(Ordering::Acquire));
    assert_eq!(shared.position.load(Ordering::Relaxed), 16);
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
#[test]
fn prepared_device_clock_needs_a_report_and_accepts_both_retained_and_true_zero() {
    use std::sync::{Arc, Mutex, atomic::Ordering};
    use std::time::{Duration, Instant};
    let shared = Arc::new(crate::Shared { position: 16.into(), playing: false.into(), speed: 1000.into(), meters: Mutex::default() });
    let (sender, receiver) = crate::channel();
    let tx = Arc::new(sender);
    let revision = Arc::new(Mutex::new(true));
    let doc = Arc::new(soundcraft_model::Session::default());
    let output: Arc<Mutex<crate::sync_output::Output>> = Arc::default();
    output.lock().unwrap().configure(&doc, &tx, None);
    let Ok(crate::Cmd::Clock(Some(mut publisher))) = receiver.try_recv() else { panic!("clock publisher missing") };
    let cue = PreparedResume {
        revision: Arc::downgrade(&revision),
        pulse: 0,
        origin: 1000,
        offset: 0,
        sr: doc.sample_rate,
        tx: Arc::downgrade(&tx),
        shared: Arc::downgrade(&shared),
        output: Arc::downgrade(&output),
        silent: false,
    };
    let mut timing = crate::sync_clock::StreamLead::default();
    for (report, expected_lead) in [(None, None), (Some(75), Some(75)), (None, Some(75)), (Some(0), Some(0))] {
        publisher.observe(timing.observe(Instant::now(), report.map(Duration::from_millis)), 0.01);
        let start = expected_lead.map(|ms| 1000 + doc.sample_rate.samples(ms as f64 / 1000.));
        assert_eq!(cue.dispatch(0, || 0), start);
        if let Some(expected) = start {
            assert!(
                matches!(receiver.try_recv(), Ok(crate::Cmd::PlayClock { origin: 1000, render_from, arrival: Some((0, 0)) }) if render_from == expected)
            );
        } else {
            assert!(receiver.try_recv().is_err());
        }
        shared.playing.store(false, Ordering::Relaxed);
    }
}

#[test]
fn stable_device_prediction_waits_for_a_continuous_tempo_ramp_to_end() {
    let mut reader = Follower::default();
    let mut stamp = 0;
    feed(&mut reader, 0xfa, stamp);
    for _ in 0..192 {
        stamp += 20_000;
        feed(&mut reader, 0xf8, stamp);
    }
    assert!(reader.render_prediction().is_some());
    // Two microseconds per pulse is well inside short-term regularity tolerance,
    // yet the sustained slowdown is measurable across the long fit.
    for i in 1..=192 {
        stamp += 20_000 + 2 * i;
        feed(&mut reader, 0xf8, stamp);
    }
    let snapshot = reader.snapshot(stamp);
    assert!(snapshot.tempo_ramp && !snapshot.recovering_tempo_step);
    assert!(reader.short.regular_intervals(reader.steady.period_us().unwrap()));
    assert!(reader.render_prediction().is_none());
    for _ in 0..192 {
        stamp += 20_384;
        feed(&mut reader, 0xf8, stamp);
    }
    assert!(reader.render_prediction().is_some());
}

#[test]
fn classifies_the_eight_percent_boundary_against_the_pre_step_period() {
    for jitter in [0_u64, 1000] {
        for pct in [-8.1_f64, -8.0, -7.9, 7.9, 8.0, 8.1] {
            for trial in 0..10 {
                let cut = 384 + trial;
                let mut rng = 42 + trial as u64;
                let mut stamp = 0.0_f64;
                let mut follower = Follower::default();
                follower.receive(&[0xfa], 0);
                let mut confirmations = Vec::new();
                for pulse in 0..=cut + 192 {
                    rng = rng.wrapping_mul(6364136223846793005).wrapping_add(1);
                    let delay = if pulse == 0 { 0 } else { ((rng >> 32) % (2 * jitter + 1)) as i64 - jitter as i64 };
                    follower.receive(&[0xf8], stamp.round() as i64 + delay);
                    if follower.step_intervals > 0 {
                        confirmations.push(pulse);
                    }
                    stamp += 2_500_000. / if pulse < cut { 128. } else { 128. * (1. + pct / 100.) };
                }
                if pct.abs() < 8. {
                    assert!(confirmations.is_empty(), "{pct}% {jitter}us trial {trial}: {confirmations:?}");
                } else {
                    assert!(matches!(confirmations.as_slice(), [p] if *p > cut), "{pct}% {jitter}us trial {trial}: {confirmations:?}");
                }
            }
        }
    }
}

#[test]
fn a_single_twenty_percent_interval_step_does_not_become_a_continuous_brake() {
    let mut f = Follower::default();
    let mut seed = 42_u64;
    let mut ideal = 0.0_f64;
    f.receive(&[0xfa], 0);
    for pulse in 0..590 {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
        let noise = if pulse == 0 { 0 } else { ((seed >> 32) % 2001) as i64 - 1000 };
        let arrival = ideal.round() as i64 + noise;
        f.receive(&[0xf8], arrival);
        assert!(!f.snapshot(arrival).braking, "an abrupt interval step entered brake tracking at pulse {pulse}");
        ideal += 2_500_000. / if pulse < 396 { 120. } else { 100. };
    }
}
