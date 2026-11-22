use super::dj_render::{Ending, render_dj};
use super::*;

fn jitter(stamps: &[i64], amplitude: i64) -> Vec<i64> {
    let mut seed = 341_u64;
    stamps
        .iter()
        .enumerate()
        .map(|(index, &at)| {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
            at + if index == 0 { 0 } else { ((seed >> 32) % (2 * amplitude + 1) as u64) as i64 - amplitude }
        })
        .collect()
}

#[test]
fn brake_steady_slow_sources_lock_from_start_without_losing_notes() {
    let mut failures = Vec::new();
    for bpm in [16., 10., 5.] {
        let ideal: Vec<_> = (0..49).map(|p| (p as f64 * 2_500_000.0 / bpm).round() as i64).collect();
        for noise in [0, 1000] {
            let wire = jitter(&ideal, noise);
            let r = render_dj(bpm, &ideal, &wire, Ending::Release);
            let last = r.snapshots.last().unwrap();
            eprintln!(
                "steady {bpm} BPM jitter {noise}: eighth sample errors {:?}; transitions {:?}; losses {}, damage {}, extras {}, open tail {}",
                r.attacks, r.transitions, last.dropouts, r.damaged, r.extra, r.tail_open
            );
            if last.state != State::Locked
                || last.dropouts != 0
                || last.bpm.is_none_or(|v| (v - bpm).abs() > 0.01)
                || r.attacks.iter().any(|a| a.len() != 1)
                || r.damaged != 0
                || r.extra != 0
                || r.tail_open
            {
                failures.push((bpm, noise));
            }
        }
    }
    assert!(failures.is_empty(), "slow source(s) failed: {failures:?}");
}

#[test]
fn brake_shapes_keep_the_wire_phase_and_obey_each_ending() {
    const CUT: usize = 192;
    let mut failures = Vec::new();
    for length in [24, 48] {
        for exponential in [false, true] {
            for noise in [0, 1000] {
                for ending in [Ending::Stop, Ending::Loss { coast: true }, Ending::Loss { coast: false }, Ending::Release] {
                    let rise = matches!(ending, Ending::Release);
                    let base = CUT + length + 24; // Sustain five BPM for a full beat.
                    let pulses = base + if rise { 24 + 96 } else { 0 };
                    let mut time = 0.0_f64;
                    let ideal: Vec<_> = (0..=pulses)
                        .map(|p| {
                            let t = time.round() as i64;
                            let bpm = if p < CUT {
                                128.
                            } else if p < CUT + length {
                                let phase = (p - CUT + 1) as f64 / length as f64;
                                if exponential { 128. * (5.0_f64 / 128.).powf(phase) } else { 128. - 123. * phase }
                            } else if p < base {
                                5.
                            } else {
                                5. + 123. * ((p - base + 1) as f64 / 24.).min(1.)
                            };
                            time += 2_500_000. / bpm;
                            t
                        })
                        .collect();
                    let wire = jitter(&ideal, noise);
                    let r = render_dj(128., &ideal, &wire, ending);
                    let snap = r.snapshots.last().unwrap();
                    let mut ok = r.snapshots.iter().all(|s| s.state == State::Locked && s.dropouts == 0)
                        && r.damaged == 0
                        && r.extra == 0
                        && !r.tail_open
                        && r.attacks.iter().all(|v| v.len() == 1);
                    let min_wire_error = (CUT / 12 + 1..=base / 12)
                        .flat_map(|e| {
                            let delay = (wire[e * 12] - ideal[e * 12]) as f64 * 0.048;
                            r.attacks[e].iter().map(move |offset| *offset as f64 - delay)
                        })
                        .fold(0.0_f64, f64::min);
                    ok &= min_wire_error >= -48. && r.snapshots[base].bpm.is_some_and(|b| (5. - b).abs() < 0.1);
                    if rise {
                        ok &= snap.bpm.is_some_and(|v| (v - 128.).abs() < 0.1)
                            && r.attacks[(base + 48) / 12..].iter().flatten().all(|e| e.abs() <= 192);
                    } else {
                        ok &= r.final_state == if matches!(ending, Ending::Loss { coast: true }) { State::Coasting } else { State::Stopped };
                    }
                    let transitions: Vec<_> = r
                        .transitions
                        .iter()
                        .map(|&(p, t, state)| {
                            let prev = wire.get(p.saturating_sub(1)).copied().unwrap_or(0);
                            (p, t, state, t - prev)
                        })
                        .collect();
                    eprintln!(
                        "brake {length} exponential={exponential} jitter={noise} {ending:?}: eighth errors {:?}; state transitions (pulse, time_us, state, gap_us) {transitions:?}; earliest vs wire {min_wire_error:.1} samples, damage {}, extras {}, open tail {}, at 5 BPM {:?}",
                        r.attacks, r.damaged, r.extra, r.tail_open, r.snapshots[base].bpm
                    );
                    if !ok {
                        failures.push((length, exponential, noise, ending));
                    }
                }
            }
        }
    }
    assert!(failures.is_empty(), "brake failures: {failures:?}");
}

#[test]
fn brake_cable_loss_and_slow_relock_use_clock_and_active_sensing_separately() {
    let inputs = MidiInputs::with_port_names(vec![], vec!["slow master".into()]);
    let feed = inputs.feed();
    let port = feed.ports().assign("slow master");
    feed.set_clock_follow(Some(port), true, 0, false);
    let send = |time, status| {
        feed.set_clock(time);
        feed.receive(port, &[status]);
    };
    let status = |time| {
        feed.set_clock(time);
        feed.clock_follow_status(&mut Vec::new())
    };
    send(0, 0xfa);
    for p in 0..=24 {
        send(p * 20_000, 0xf8);
    }
    let last = 24 * 20_000;
    assert_eq!(status(last + 150_000).state, State::Locked);
    assert_eq!(status(last + 150_001).state, State::Coasting);
    let at = last + 300_000;
    send(at, 0xf8);
    assert_eq!(status(at + 249_999).state, State::Coasting);
    send(at + 250_000, 0xf8);
    let relocked = status(at + 250_000);
    assert_eq!(relocked.state, State::Locked, "the second 10 BPM pulse relocks");
    assert!((relocked.bpm.unwrap() - 10.).abs() < 0.01);

    send(at + 250_100, 0xfe);
    send(at + 450_100, 0xfe);
    assert_eq!(status(at + 749_999).state, State::Locked);
    assert_eq!(status(at + 750_101).state, State::Coasting, "a source that enabled Active Sensing stopped sending MIDI for 300 ms");
}

#[test]
fn active_sensing_counts_channel_messages_from_its_own_port_as_presence() {
    let inputs = MidiInputs::with_port_names(vec![], vec!["clock".into(), "other".into()]);
    let feed = inputs.feed();
    let master = feed.ports().assign("clock");
    let other = feed.ports().assign("other");
    feed.set_clock_follow(Some(master), true, 0, false);
    feed.set_clock(0);
    feed.receive(master, &[0xfa, 0xf8]);
    for p in 1..=24 {
        feed.set_clock(p * 140_000);
        feed.receive(master, &[0xf8]);
    }
    let last = 24 * 140_000;
    feed.set_clock(last + 5_000);
    feed.receive(master, &[0xfe]);
    feed.set_clock(last + 30_000);
    feed.receive(master, &[0x90, 60, 100]);
    feed.set_clock(last + 300_000);
    feed.receive(other, &[0xfe]);
    feed.set_clock(last + 330_000);
    assert_eq!(feed.clock_follow_status(&mut Vec::new()).state, State::Locked);
    feed.set_clock(last + 330_001);
    assert_eq!(feed.clock_follow_status(&mut Vec::new()).state, State::Coasting);
}
