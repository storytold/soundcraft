use super::sampler_song::sampler_song;
use super::*;

#[test]
fn continuous_pitch_fader_round_trip_keeps_eighths_in_phase() {
    let sr = SampleRate::HZ_48000;
    let mut ok = true;
    for (bars, delta) in [(16, 0.3), (32, 124. * 0.08), (32, -124. * 0.08)] {
        let n = bars * 96;
        for jitter in [0_i64, 1000, 2000] {
            let inputs = MidiInputs::with_port_names(vec![], vec!["deck".into()]);
            let feed = inputs.feed();
            let port = feed.ports().assign("deck");
            feed.set_clock_follow(Some(port), true, 0, false);
            feed.set_clock(20_000);
            feed.receive(port, &[0xfa]);
            let count = n * 2 / 12 + 1;
            let mut song = sampler_song(TempoMap::new(124., 4, 4).unwrap(), sr, count, 48000 * 180);
            let mut mix = soundcraft_mix::MixEngine::new(48000., 128);
            let mut block = vec![vec![0.; 128]; 2];
            let mut pcm = Vec::<f32>::new();
            let mut render_to = |end: i64, song: &soundcraft_model::Session| {
                while (pcm.len() as i64) < end {
                    let start = pcm.len() as i64;
                    let frames = (end - start).min(128) as usize;
                    mix.render_with_clock_limit(song, start, frames, &mut block, feed.clock_render_tick());
                    pcm.extend_from_slice(&block[0][..frames]);
                }
            };
            let mut seed = 42_u64;
            let mut us = 0.0_f64;
            let mut actions = Vec::new();
            let mut expected = Vec::new();
            let mut steps = 0;
            let mut recovering = false;
            let mut last = None;
            let mut jump = 0.0_f64;
            let mut rate_lag = 0.0_f64;
            let mut fit_bias = 0.0_f64;
            let mut signed = Vec::new();
            let mut ramp_pulses = 0;
            for i in 0..=2 * n {
                seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
                let noise = if i == 0 { 0 } else { ((seed >> 32) % (2 * jitter + 1) as u64) as i64 - jitter };
                let at = us.round() as i64 + noise;
                // Render up to the next arrival with only the previous packet available.
                render_to(sr.samples(at as f64 / 1e6), &song);
                feed.set_clock(at + 20_000);
                feed.receive(port, &[0xf8]);
                let snap = feed.clock_follow_status(&mut actions);
                ramp_pulses += usize::from(snap.tempo_ramp);
                if i % 12 == 0 {
                    expected.push(sr.samples(us / 1e6));
                }
                let bpm = 124. + delta * (1. - (i as f64 / n as f64 - 1.).abs());
                if let Some(b) = snap.bpm {
                    // The tempo estimator acquires within one beat. Ignore the initial estimate.
                    if i >= 24 && snap.state == State::Locked {
                        if let Some(prev) = last {
                            jump = jump.max(f64::abs(b - prev));
                        }
                        rate_lag = rate_lag.max((b - bpm).abs());
                        fit_bias = fit_bias.max((snap.stamp_us as f64 - 20000. - us).abs() / 1000.);
                    }
                    last = Some(b);
                }
                steps += usize::from(snap.recovering_tempo_step && !recovering);
                recovering = snap.recovering_tempo_step;
                update_grid(&mut song.tempo, Some((0, 20000)), 0, snap, sr, sr.samples(at as f64 / 1e6 + 0.030));
                us += 2_500_000. / bpm;
            }
            let end = expected.last().unwrap() + 48000;
            render_to(end, &song);
            let mut edges = Vec::new();
            let mut active = None;
            for (i, &v) in pcm.iter().enumerate() {
                if v.abs() > 0.001 {
                    active.get_or_insert(i);
                } else if let Some(begin) = active.take() {
                    edges.push((begin, i));
                }
            }
            assert_eq!(edges.len(), expected.len());
            let reference = &pcm[edges[0].0..edges[0].1];
            assert_eq!(reference.len(), 96);
            for (&(on, off), &master) in edges.iter().zip(&expected) {
                assert_eq!(&pcm[on..off], reference);
                signed.push(on as i64 - master);
            }
            assert!(pcm[edges.last().unwrap().1..].iter().all(|x| *x == 0.));
            let peak = signed.iter().map(|v| v.abs()).max().unwrap() as f64 / 48.;
            eprintln!(
                "RAMP d={delta:.2} jitter={jitter} peak={peak:.4} last={} hits={count} false_steps={steps} adaptive={ramp_pulses} locked_jump={jump:.5} estimator_bpm_error={rate_lag:.5} fitted_stamp_error_ms={fit_bias:.5} samples={signed:?}",
                signed.last().unwrap()
            );
            ok &= peak <= 4. && steps == 0 && jump <= 0.1 && (delta.abs() > 1. || ramp_pulses == 0);
        }
    }
    assert!(ok);
}
