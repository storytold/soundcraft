use super::sampler_song::sampler_song;
use super::*;

#[derive(Debug, Clone, Copy)]
enum Delivery {
    Jitter(i64),
    Burst(usize),
    VariableBurst,
}
fn delivery_times(ideal: &[f64], mode: Delivery) -> Vec<i64> {
    let mut seed = 42u64;
    let mut times = Vec::with_capacity(ideal.len());
    let mut group = 0;
    while times.len() < ideal.len() {
        let p = times.len();
        match mode {
            Delivery::Jitter(amplitude) => {
                seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
                let delta = ((seed >> 32) % (2 * amplitude + 1) as u64) as i64 - amplitude;
                times.push(ideal[p].round() as i64 + if p == 0 { 0 } else { delta });
            }
            _ => {
                let n = if let Delivery::Burst(n) = mode { n } else { 3 + group % 3 };
                if p == 0 {
                    // A single F8 starts transport at the same sample on both clocks.
                    times.push(ideal[0].round() as i64);
                    continue;
                }
                // Every coalesced group reaches the driver after its last source pulse.
                let width = n.min(ideal.len() - p);
                let last = ideal[p + width - 1].round() as i64;
                for j in 0..width {
                    times.push(last + j as i64 * 200);
                }
                group += 1;
            }
        }
    }
    times
}
struct Observation {
    attacks: Vec<Vec<i64>>,
    // Physical lateness of the event's own F8, independent of SoundCraft.
    wire: Vec<i64>,
    malformed: usize,
    unmatched: usize,
    steps: Vec<(usize, u8)>,
    dropouts: u64,
}
fn render_train(from: f64, to: f64, transition: usize, n: usize, mode: Delivery) -> Observation {
    let sr = SampleRate::HZ_48000;
    let inputs = MidiInputs::with_port_names(vec![], vec!["source".into()]);
    let feed = inputs.feed();
    let port = feed.ports().assign("source");
    feed.set_clock_follow(Some(port), true, 0, false);
    feed.set_clock(0);
    feed.receive(port, &[0xfa]);
    let mut now = 0.;
    let ideal: Vec<f64> = (0..n)
        .map(|p| {
            let t = now;
            now += 2_500_000. / if p < transition { from } else { to };
            t
        })
        .collect();
    let times = delivery_times(&ideal, mode);
    let expected: Vec<_> = ideal.iter().step_by(12).map(|at| sr.samples(at / 1e6)).collect();
    let end = sr.samples(now / 1e6) + 48_000;
    let mut song = sampler_song(TempoMap::new(from, 4, 4).unwrap(), sr, expected.len(), end);
    let mut mixer = soundcraft_mix::MixEngine::new(48_000., 512);
    let mut block = vec![vec![0.; 512]; 2];
    let mut pos = 0;
    let mut steps = Vec::new();
    let mut actions = Vec::new();
    let mut dropouts = 0;
    let mut hits: Vec<(i64, Vec<f32>)> = Vec::new();
    let mut sounding = false;
    for p in 0..=n {
        let until = times.get(p).map_or(end, |at| sr.samples(*at as f64 / 1e6));
        while pos < until {
            let frames = (until - pos).min(512) as usize;
            mixer.render_clock_block(&song, pos, frames, &mut block, feed.clock_render_tick(), feed.clock_delivery());
            for (i, &v) in block[0][..frames].iter().enumerate() {
                let high = v.abs() > 0.001;
                if high {
                    if !sounding {
                        hits.push((pos + i as i64, Vec::with_capacity(96)));
                    }
                    hits.last_mut().unwrap().1.push(v);
                }
                sounding = high;
            }
            pos += frames as i64;
        }
        if p < n {
            feed.set_clock(times[p]);
            feed.receive(port, &[0xf8]);
            let snap = feed.clock_follow_status(&mut actions);
            actions.clear();
            if snap.step_intervals != 0 {
                steps.push((p, snap.step_intervals));
            }
            dropouts = snap.dropouts;
            update_grid(&mut song.tempo, Some((0, 0)), 0, snap, sr, pos + 1440);
        }
    }
    let reference = hits.iter().find(|(_, voice)| voice.len() == 96).unwrap();
    let malformed = hits.iter().filter(|(_, voice)| voice != &reference.1).count() + usize::from(sounding);
    let mut attacks = vec![Vec::new(); expected.len()];
    let mut unmatched = 0;
    for (hit, _) in hits {
        let right = expected.partition_point(|v| *v < hit);
        let near = match right {
            0 => 0,
            r if r == expected.len() => r - 1,
            r if hit - expected[r - 1] < expected[r] - hit => r - 1,
            r => r,
        };
        let late = hit - expected[near];
        if near + 1 == expected.len() && late > (expected[near] - expected[near - 1]) / 2 {
            unmatched += 1;
        } else {
            attacks[near].push(late);
        }
    }
    let wire = times.iter().step_by(12).zip(&expected).map(|(time, ideal)| (sr.samples(*time as f64 / 1e6) - ideal).max(0)).collect();
    Observation { attacks, wire, unmatched, malformed, steps, dropouts }
}
fn record(label: &str, r: &Observation, change: usize) {
    let pre = r.steps.iter().filter(|(p, _)| *p <= change).count();
    let onset = r.steps.iter().find(|(p, _)| *p > change).map(|(p, n)| (p - change, n));
    let missing = r.attacks.iter().filter(|v| v.is_empty()).count();
    let extra = r.attacks.iter().map(|v| v.len().saturating_sub(1)).sum::<usize>() + r.unmatched;
    eprintln!(
        "CASE {label} missing={missing} extra={extra} damaged={} dropouts={} early_steps={pre} confirmations={} first={onset:?}",
        r.malformed,
        r.dropouts,
        r.steps.len()
    );
    if std::env::var_os("SOUNDCRAFT_PACKET_TRACE").is_some() {
        eprintln!("HITS {label} {:?}", r.attacks);
        eprintln!("STEPS {label} {:?}", r.steps);
    }
}
#[test]
fn tempo_steps_with_timestamp_noise_render_complete_eighth_notes() {
    let mut valid = true;
    for (from, to) in [(130., 80.), (120., 100.), (180., 60.), (100., 120.), (80., 130.), (60., 180.)] {
        for transition in [384, 396] {
            for mode in [Delivery::Jitter(1000), Delivery::Jitter(2000), Delivery::VariableBurst] {
                let r = render_train(from, to, transition, transition + 193, mode);
                record(&format!("{from}-{to}-{transition}-{mode:?}"), &r, transition);
                if let Delivery::Jitter(_) = mode {
                    // Uniform timestamp noise still gives one observation per wire pulse.
                    let start = transition / 12;
                    valid &= r.attacks[start + 1].iter().all(|x| x.abs() <= 480) // 10 ms
                        && r.attacks[start + 2..].iter().flatten().all(|x| x.abs() <= 288); // 6 ms
                }
                valid &= r.malformed == 0
                    && r.unmatched == 0
                    && r.attacks.iter().all(|v| v.len() == 1)
                    && r.dropouts == 0
                    && matches!(r.steps.as_slice(), [(p, _)] if *p > transition && *p <= transition + 16);
            }
        }
    }
    assert!(valid);
}
#[test]
fn trailing_packed_slowdowns_preserve_every_attack_at_its_wire_time() {
    // 48 kHz. The source has eight beats after the cut, including attacks falling
    // at different positions within each USB batch.
    let verbose = std::env::var_os("SOUNDCRAFT_PACKET_TRACE").is_some();
    let mut valid = true;
    for (from, to) in [(180., 60.), (130., 80.)] {
        for change in [384, 396] {
            for mode in [Delivery::Burst(3), Delivery::Burst(4), Delivery::Burst(5), Delivery::VariableBurst] {
                let r = render_train(from, to, change, change + 193, mode);
                let label = format!("{from}-{to}-{change}-{mode:?}");
                record(&label, &r, change);
                valid &= r.dropouts == 0 && r.unmatched == 0 && r.malformed == 0 && r.attacks.iter().all(|note| note.len() == 1);
                let first_offbeat = (change / 12 + 1) | 1;
                let mut max_wire = 0;
                let mut max_residual = 0;
                let mut worst_early = 0;
                for (k, (notes, &wire)) in r.attacks.iter().zip(&r.wire).enumerate().skip(change / 12 + 1) {
                    let [measured] = notes.as_slice() else { continue };
                    let residual = measured - wire;
                    if verbose {
                        eprintln!("PACKED_NOTE {label} eighth={k} wire_samples={wire} measured_samples={measured} residual_samples={residual}");
                    }
                    max_wire = max_wire.max(wire);
                    max_residual = max_residual.max(residual.abs());
                    worst_early = worst_early.min(*measured);
                    valid &= *measured >= -48 && residual.abs() <= if k == first_offbeat { 480 } else { 288 };
                }
                eprintln!("PACKED {label} max_wire_samples={max_wire} max_residual_samples={max_residual} earliest_samples={worst_early}");
            }
        }
    }
    assert!(valid, "a packed slowdown lost or damaged a voice, or exceeded the attack or wire-relative tolerance");
}

#[test]
#[ignore = "ten simulated minutes for each burst width"]
fn ten_minutes_of_coalesced_delivery_do_not_confirm_a_tempo_step() {
    let mut valid = true;
    for (name, mode) in [("3", Delivery::Burst(3)), ("4", Delivery::Burst(4)), ("5", Delivery::Burst(5)), ("3to5", Delivery::VariableBurst)] {
        let r = render_train(120., 120., 28_800, 28_800, mode);
        record(&format!("A4-{name}"), &r, 28_800);
        valid &= r.steps.is_empty()
            && r.dropouts == 0
            && r.malformed == 0
            && r.unmatched == 0
            && r.attacks.iter().zip(&r.wire).all(|(v, wire)| matches!(v.as_slice(), [x] if (x-wire).abs() <= 1));
    }
    assert!(valid);
}
