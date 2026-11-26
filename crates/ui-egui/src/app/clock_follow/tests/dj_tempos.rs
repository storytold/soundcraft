use super::dj_render::{Ending, Rendered, render_dj};

const WARMUP: usize = 384;

fn whole_song(pulses: usize, mut interval_us: impl FnMut(usize, f64) -> f64) -> Vec<i64> {
    let mut at = 0.0_f64;
    (0..=pulses)
        .map(|p| {
            let stamp = at.round() as i64;
            at += interval_us(p, at);
            stamp
        })
        .collect()
}

fn jitter(ideal: &[i64], amplitude: i64, mut seed: u64) -> Vec<i64> {
    ideal
        .iter()
        .enumerate()
        .map(|(p, at)| {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
            if p == 0 { *at } else { *at + ((seed >> 32) % (2 * amplitude + 1) as u64) as i64 - amplitude }
        })
        .collect()
}

fn period(bpm: f64) -> f64 {
    2_500_000. / bpm
}

fn errors(name: &str, r: &Rendered) -> Vec<i64> {
    let missing = r.attacks.iter().filter(|slot| slot.is_empty()).count();
    let repeated: usize = r.attacks.iter().map(|slot| slot.len().saturating_sub(1)).sum();
    eprintln!(
        "{name}: {} scored eighths, {missing} missing, {repeated} repeated, {} unmatched, {} changed waveforms, open tail {}",
        r.attacks.len(),
        r.extra,
        r.damaged,
        r.tail_open
    );
    assert_eq!(r.extra, 0, "{name}: unmatched attack");
    assert!(!r.tail_open, "{name}: sound past the fixture release");
    assert_eq!(r.damaged, 0, "{name}: incomplete or changed waveform");
    let signed: Vec<_> = r
        .attacks
        .iter()
        .enumerate()
        .map(|(i, note)| {
            assert_eq!(note.len(), 1, "{name}: eighth {i}, offsets {note:?}");
            note[0]
        })
        .collect();
    eprintln!("{name} signed eighth errors (48 kHz samples) {signed:?}");
    signed
}

fn peak(samples: &[i64]) -> i64 {
    samples.iter().map(|v| v.abs()).max().unwrap_or(0)
}

fn steps(r: &Rendered) -> Vec<usize> {
    r.snapshots.iter().enumerate().filter(|(_, s)| s.step_intervals != 0).map(|(p, _)| p).collect()
}

#[test]
fn small_dj_nudges_keep_each_eighth_and_settle_in_two_beats() {
    for amplitude in [0, 1000] {
        for pct in [-5., -3.5, -2., 2., 3.5, 5.] {
            for duration in [500_000., 1_000_000., 2_000_000.] {
                let start = WARMUP as f64 * period(128.);
                let release = start + duration;
                let bpm = 128. * (1. + pct / 100.);
                let ideal = whole_song(WARMUP + 288, |p, at| {
                    if p < WARMUP || at >= release {
                        return period(128.);
                    }
                    // A nudge ends at the specified time, even between two wire pulses.
                    let left = release - at;
                    if left >= period(bpm) { period(bpm) } else { left + period(128.) * (1. - left / period(bpm)) }
                });
                let r = render_dj(128., &ideal, &jitter(&ideal, amplitude, 42), Ending::Release);
                let name = format!("nudge {pct}% {duration}us jitter {amplitude}us");
                let signed = errors(&name, &r);
                assert!(steps(&r).is_empty(), "{name} false steps {:?}", steps(&r));
                let deadline = ideal.partition_point(|&t| (t as f64) < release) + 48;
                let settle = r.snapshots[deadline].bpm.unwrap();
                eprintln!("{name}: readout after two beats {settle:.4} BPM");
                assert!((settle - 128.).abs() <= 0.1, "{name}: {settle} BPM");
                assert!(peak(&signed[WARMUP / 12..]) <= 192, "{name}: peak {} samples", peak(&signed[WARMUP / 12..]));
            }
        }
    }
}

#[test]
fn half_and_double_time_keep_the_first_offbeat_and_following_eighths() {
    for amplitude in [0, 1000] {
        for (from, to) in [(174., 87.), (87., 174.), (140., 70.), (70., 140.), (120., 60.), (60., 120.)] {
            for cut in [WARMUP, WARMUP + 12] {
                let ideal = whole_song(cut + 192, |p, _| period(if p < cut { from } else { to }));
                let r = render_dj(from, &ideal, &jitter(&ideal, amplitude, 42), Ending::Release);
                let name = format!("ratio {from}->{to} at {cut} jitter {amplitude}us");
                let signed = errors(&name, &r);
                assert!(r.snapshots.iter().all(|s| !s.braking), "{name}: an abrupt tempo ratio triggered the continuous brake response");
                let x = &signed[cut / 12..];
                assert!(x[0].abs() <= 192 && x[1].abs() <= 480 && peak(&x[2..]) <= 192, "{name}");
            }
        }
    }
}

#[test]
fn exact_step_threshold_uses_pretransition_period_in_both_directions() {
    for amplitude in [0, 1000] {
        for pct in [-8.1_f64, -8.0, -7.9, 7.9, 8.0, 8.1] {
            for trial in 0..10 {
                let cut = WARMUP + trial;
                let ideal = whole_song(cut + 192, |p, _| period(if p < cut { 128. } else { 128. * (1. + pct / 100.) }));
                let r = render_dj(128., &ideal, &jitter(&ideal, amplitude, 42 + trial as u64), Ending::Release);
                let name = format!("threshold {pct} trial {trial} jitter {amplitude}us");
                let signed = errors(&name, &r);
                let transitions = steps(&r);
                eprintln!("{name}: confirmed at {transitions:?}, cut {cut}");
                if pct.abs() < 8. {
                    assert!(transitions.is_empty(), "{name}: false steps {transitions:?}");
                    assert!(peak(&signed[cut.div_ceil(12)..]) <= 192, "{name}");
                } else {
                    assert!(matches!(transitions.as_slice(), [p] if *p > cut), "{name}: {transitions:?}");
                }
            }
        }
    }
}

#[test]
fn accelerating_build_detects_the_drop_on_a_linear_and_convex_ramp() {
    let length = 8 * 4 * 24;
    let drop = WARMUP + length;
    for convex in [false, true] {
        for amplitude in [0, 1000] {
            let ideal = whole_song(drop + 192, |p, _| {
                if !(WARMUP..drop).contains(&p) {
                    return period(128.);
                }
                let x = (p - WARMUP) as f64 / length as f64;
                let y = x + 1. / length as f64;
                // Integrate one wire interval against BPM as a function of score position.
                if convex {
                    let scale = (32_f64 / 128.).sqrt();
                    2_500_000. * length as f64 / (128_f64 * 32.).sqrt() * ((y * scale).atan() - (x * scale).atan())
                } else {
                    2_500_000. * length as f64 / 32. * ((128. + 32. * y) / (128. + 32. * x)).ln()
                }
            });
            let r = render_dj(128., &ideal, &jitter(&ideal, amplitude, 42), Ending::Release);
            let name = format!("build convex={convex} jitter={amplitude}");
            let s = errors(&name, &r);
            let det = steps(&r);
            assert!(matches!(det.as_slice(),[p] if *p>drop), "{name} step pulses {det:?}");
            assert!(peak(&s[WARMUP / 12..=drop / 12]) <= 192, "{name} ramp");
            assert!(s[drop / 12 + 1].abs() <= 480 && peak(&s[drop / 12 + 2..]) <= 192, "{name} drop");
        }
    }
}

#[test]
fn gradual_one_percent_steps_follow_the_grid_without_confirming_a_large_step() {
    let mut levels = vec![120_f64];
    while *levels.last().unwrap() < 132. {
        levels.push((levels.last().unwrap() * 1.01).min(132.));
    }
    while *levels.last().unwrap() > 120. {
        levels.push((levels.last().unwrap() * 0.99).max(120.));
    }
    let ideal = whole_song(WARMUP + 24 * levels.len() + 192, |p, _| {
        let i = if p < WARMUP { 0 } else { ((p - WARMUP) / 24 + 1).min(levels.len() - 1) };
        period(levels[i])
    });
    for amplitude in [0, 1000] {
        let r = render_dj(120., &ideal, &jitter(&ideal, amplitude, 42), Ending::Release);
        let s = errors("one-percent tempo staircase", &r);
        assert!(steps(&r).is_empty());
        assert!(peak(&s[WARMUP / 12..]) <= 192, "jitter {amplitude}: {} samples", peak(&s[WARMUP / 12..]));
    }
}

#[test]
fn sinusoidal_tempo_wobble_keeps_each_eighth_without_false_steps() {
    for depth in [0.005, 0.01] {
        for frequency in [0.5, 2.] {
            let omega = std::f64::consts::TAU * frequency;
            let origin = WARMUP as f64 * period(128.);
            let ideal = whole_song(WARMUP + 64 * 4 * 24, |p, at| {
                if p < WARMUP {
                    return period(128.);
                }
                let t = (at - origin) / 1e6;
                // Invert the exact integral of the sinusoid over this pulse.
                let (mut lo, mut hi) = (0., 0.04);
                for _ in 0..40 {
                    let mid = (lo + hi) * 0.5;
                    let phase = 128. * (mid + depth * ((omega * t).cos() - (omega * (t + mid)).cos()) / omega);
                    if phase < 2.5 {
                        lo = mid;
                    } else {
                        hi = mid;
                    }
                }
                (lo + hi) * 0.5e6
            });
            for amplitude in [0, 1000] {
                let r = render_dj(128., &ideal, &jitter(&ideal, amplitude, 42), Ending::Release);
                let name = format!("wobble {depth} {frequency}Hz jitter {amplitude}");
                let s = errors(&name, &r);
                let toggles = r.snapshots.windows(2).filter(|p| p[0].tempo_ramp != p[1].tempo_ramp).count();
                eprintln!("{name}: ramp toggles {toggles}");
                assert!(steps(&r).is_empty(), "{name}");
                assert!(peak(&s[WARMUP / 12..]) <= 192, "{name}");
            }
        }
    }
}
#[test]
#[ignore = "20 five-to-seven-minute tracks at 48 kHz; run serially"]
fn twenty_full_length_dj_tracks_preserve_the_recorded_eighths_through_transitions() {
    let bpms = [118., 124., 130., 136., 142., 148., 142., 136., 130., 124., 118., 124., 130., 136., 142., 148., 142., 136., 130., 124.];
    let mut ideal = Vec::new();
    let mut exclude = Vec::new();
    let mut burst = 0..0;
    let mut time = 0_f64;
    for (track, &tempo) in bpms.iter().enumerate() {
        let seconds = 303. + ((track * 37) % 101) as f64;
        let bars = (seconds * tempo / 240.).ceil() as usize;
        let length = bars * 96;
        let start = ideal.len();
        let blend = length - 16 * 96;
        let next = bpms.get(track + 1).copied().unwrap_or(tempo);
        let special = 32 * 96;
        for p in 0..length {
            let bpm = if track == 7 && (special..special + 48).contains(&p) {
                let k = p - special;
                if k < 24 { tempo * (5_f64 / tempo).powf((k + 1) as f64 / 24.) } else { 5. * (tempo / 5.).powf((k - 23) as f64 / 24.) }
            } else if track == 12 && (special..special + 16 * 96).contains(&p) {
                tempo / 2.
            } else if p >= blend {
                tempo + (next - tempo) * (p - blend) as f64 / (length - blend) as f64
            } else {
                tempo
            };
            ideal.push(time.round() as i64);
            if ideal.len() % 12 == 1 {
                let pickup = track == 0 && p < 48;
                let mixing = track + 1 < bpms.len() && p >= blend;
                let braking = track == 7 && (special..special + 96).contains(&p);
                let half_step = track == 12 && ((special..special + 48).contains(&p) || (special + 16 * 96..special + 16 * 96 + 48).contains(&p));
                exclude.push(pickup || mixing || braking || half_step);
            }
            time += period(bpm);
        }
        if track == 5 {
            burst = start + special..start + special + 16 * 96;
        }
    }
    let mut arrived = jitter(&ideal, 1000, 71);
    let mut head = burst.start;
    let mut width = 3;
    while head < burst.end {
        let end = (head + width).min(burst.end);
        let last = arrived[end - 1];
        for (i, t) in arrived[head..end].iter_mut().enumerate() {
            *t = last + i as i64 * 200;
        }
        width = if width == 5 { 3 } else { width + 1 };
        head = end;
    }
    let r = render_dj(bpms[0], &ideal, &arrived, Ending::Release);
    let signed = errors("20 tracks; 16-bar blends; seeded jitter; brake, half-time and causal bursts", &r);
    assert_eq!(signed.len(), exclude.len());
    // Arrival batching is a declared delivery transition. Keep its wire-relative
    // errors and the inclusive percentile visible beside the steady-state result.
    let mut all_stable = Vec::new();
    let mut individual = Vec::new();
    let mut batched = 0;
    let mut max_late = 0;
    let mut earliest = 0;
    let mut earliest_vs_wire = 0;
    let mut within_wire = true;
    for (i, (&error, &transition)) in signed.iter().zip(&exclude).enumerate() {
        if transition {
            continue;
        }
        all_stable.push(error.abs());
        let pulse = i * 12;
        if burst.contains(&pulse) {
            let wire = ((arrived[pulse] - ideal[pulse]).max(0) as f64 * 0.048).round() as i64;
            let residual = error - wire;
            max_late = max_late.max(residual);
            earliest = earliest.min(error);
            earliest_vs_wire = earliest_vs_wire.min(residual);
            within_wire &= error >= -48 && residual <= if batched == 0 { 480 } else { 288 };
            eprintln!("DJ8_NOTE eighth={i} wire_samples={wire} measured_samples={error} residual_samples={residual}");
            batched += 1;
        } else {
            individual.push(error.abs());
        }
    }
    let percentile = |a: &mut Vec<i64>| {
        a.sort_unstable();
        a[(a.len() * 999).div_ceil(1000) - 1]
    };
    let p999 = percentile(&mut individual);
    eprintln!(
        "20 tracks, {} eighths, {} outside transitions, p99.9 {p999} samples; including burst {} samples; {batched} batched eighths, max signed late beyond wire {max_late} samples, earliest vs grid {earliest} samples, earliest vs wire {earliest_vs_wire} samples; max {} grid events",
        signed.len(),
        individual.len(),
        percentile(&mut all_stable),
        r.tempo_events
    );
    assert!(within_wire, "the transition to grouped F8 crossed the wire-relative lateness or grid earliness limit");
    assert!(p999 <= 192);
    assert!(r.tempo_events <= 512, "unreferenced grid events during a live mix");
}
