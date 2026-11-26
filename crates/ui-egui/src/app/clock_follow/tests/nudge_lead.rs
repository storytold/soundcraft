//! A DJ holds a small pitch-fader nudge for two bars while the device plays a buffer
//! behind its output lead. Every eighth must still sound near the source's beat.

use super::sampler_song::sampler_song;
use super::*;
use soundcraft_mix::MixEngine;

const RATE: i64 = 48_000;
const CHUNK: usize = 256;
const CUT: usize = 384;
const NUDGE: usize = 192;
const PULSES: usize = CUT + NUDGE + 96;

#[derive(Clone, Copy, Debug)]
struct Case {
    pct: f64,
    jitter_us: i64,
    buffer: usize,
    lead_ms: i64,
}

/// Ideal source times (µs) and jittered arrivals of every F8.
fn wire(case: Case) -> (Vec<f64>, Vec<i64>) {
    let base = 2_500_000. / 128.;
    let mut ideal = Vec::with_capacity(PULSES);
    let mut at = 0.;
    for p in 0..PULSES {
        ideal.push(at);
        at += if (CUT..CUT + NUDGE).contains(&p) { base / (1. + case.pct / 100.) } else { base };
    }
    let mut seed = 42_u64;
    let mut last = 0;
    let arrivals = ideal
        .iter()
        .map(|t| {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
            let j = ((seed >> 32) % (2 * case.jitter_us as u64 + 1)) as i64 - case.jitter_us;
            last = (t.round() as i64 + j).max(last);
            last
        })
        .collect();
    (ideal, arrivals)
}

fn onsets(pcm: &[f32]) -> Vec<usize> {
    let mut starts = Vec::new();
    let mut end = None;
    for (i, sample) in pcm.iter().enumerate() {
        if sample.abs() > 1e-10 {
            if end.is_none_or(|e| i - e > 12) {
                starts.push(i);
            }
            end = Some(i);
        }
    }
    starts
}

/// Signed error (samples) of each eighth heard after the nudge starts.
fn nudge_errors(case: Case) -> Vec<(usize, i64)> {
    let (ideal, arrivals) = wire(case);
    let sample = |us: f64| (us * RATE as f64 / 1e6).round() as i64;
    let inputs = MidiInputs::with_port_names(vec![], vec!["wire".into()]);
    let feed = inputs.feed();
    let port = feed.ports().assign("wire");
    feed.set_clock_follow(Some(port), true, 0, false);
    feed.set_clock(0);
    feed.receive(port, &[0xfa]);
    let rate = SampleRate::HZ_48000;
    let end = sample(ideal[PULSES - 1]) + RATE;
    let mut song = sampler_song(TempoMap::new(128., 4, 4).unwrap(), rate, PULSES / 12, end);
    let mut mix = MixEngine::new(RATE as f32, CHUNK);
    let mut audio = vec![vec![0.; CHUNK]; 2];
    let mut pcm = Vec::new();
    let mut actions = Vec::new();
    let lead = case.lead_ms * 48;
    let micros = |samples: i64| (samples * 1_000_000 + RATE / 2) / RATE;
    let first_callback = sample(ideal[CUT - 48]);
    let mut callback = 0;
    let mut pulse = 0;
    while callback + lead < sample(ideal[PULSES - 1]) {
        while pulse < PULSES && arrivals[pulse] <= micros(callback) {
            feed.set_clock(arrivals[pulse]);
            feed.receive(port, &[0xf8]);
            let snap = feed.clock_follow_status(&mut actions);
            actions.clear();
            let guard = rate.samples((2.5 / snap.bpm.unwrap_or(128.)).max(case.buffer as f64 / RATE as f64));
            update_grid(&mut song.tempo, Some((0, 0)), 0, snap, rate, callback + lead + guard);
            pulse += 1;
        }
        feed.set_clock(micros(callback));
        let pos = callback + lead;
        for offset in (0..case.buffer).step_by(CHUNK) {
            let n = (case.buffer - offset).min(CHUNK);
            let at = pos + offset as i64;
            let hard = feed.clock_render_tick();
            let limit = feed.clock_render_tick_for_output(micros(callback), micros(at + n as i64));
            if let Some(tick) = limit.filter(|_| limit > hard) {
                mix.render_with_clock_prediction(&song, at, n, &mut audio, tick);
            } else {
                mix.render_with_clock_limit(&song, at, n, &mut audio, limit);
            }
            if callback >= first_callback {
                pcm.extend_from_slice(&audio[0][..n]);
            }
        }
        callback += case.buffer as i64;
    }
    // pcm[0] is session sample (and heard time) first_callback + lead.
    let heard: Vec<i64> = onsets(&pcm).into_iter().map(|i| first_callback + lead + i as i64).collect();
    ((CUT - 36) / 12..PULSES / 12 - 1)
        .filter_map(|note| {
            let target = sample(ideal[note * 12]);
            heard.iter().map(|&h| h - target).filter(|e| e.abs() < 2_400).min_by_key(|e| e.abs()).map(|e| (note * 12, e))
        })
        .collect()
}

#[test]
fn small_sustained_nudges_keep_eighths_on_the_beat_behind_device_lead() {
    let mut failures = Vec::new();
    for pct in [0.5, -0.5, 1.0] {
        for jitter_us in [0, 500] {
            let case = Case { pct, jitter_us, buffer: 256, lead_ms: 7 };
            let errors = nudge_errors(case);
            // Eighths before the nudge give the rig's fixed onset offset.
            let before: Vec<_> = errors.iter().filter(|(p, _)| *p <= CUT).map(|(_, e)| *e).collect();
            assert!(before.len() >= 3, "{case:?}: no reference eighths");
            let baseline = before.iter().sum::<i64>() / before.len() as i64;
            let after: Vec<_> = errors.iter().filter(|(p, _)| *p > CUT).map(|(p, e)| (*p - CUT, *e - baseline)).collect();
            assert!(after.len() >= (PULSES - CUT) / 12 - 3, "{case:?}: {} eighths heard", after.len());
            // The first eighth after each change (the nudge and its release) is held at the
            // wire, as a slowdown may follow. It may be late but never early.
            let first_after_change = |p: usize| p == 12 || p == NUDGE + 12;
            let late: Vec<_> = after.iter().filter(|&&(p, e)| e < -192 || (!first_after_change(p) && e > 192)).copied().collect();
            eprintln!("{case:?}: relative to the steady eighths (pulse after cut, samples) {after:?}");
            if !late.is_empty() {
                failures.push((case.pct, case.jitter_us, late.len()));
            }
        }
    }
    assert!(failures.is_empty(), "eighths stayed off the beat after the first eighth of a held nudge: {failures:?}");
}
