//! Scorecard for external Clock follow: eighth notes on the sampler behind a simulated
//! device buffer and output lead, against the source's ideal beat. Run with
//! `cargo test -p soundcraft-ui-egui --lib follow_bench -- --ignored --nocapture`.

use super::sampler_song::sampler_song;
use super::*;
use soundcraft_mix::MixEngine;

const RATE: i64 = 48_000;
const CHUNK: usize = 256;

#[derive(Clone, Copy, Debug)]
enum Profile {
    /// Constant source tempo.
    Steady,
    /// Hold a tempo change of this many percent for eight beats from beat 16, then return.
    Nudge(f64),
    /// Step to this many percent from beat 16 and stay there.
    Step(f64),
    /// Glide linearly in BPM to this many percent over eight beats from beat 16, then hold.
    Ramp(f64),
}

#[derive(Clone, Copy, Debug)]
pub(super) struct Case {
    profile: Profile,
    bpm: f64,
    session_bpm: f64,
    jitter_us: i64,
    buffer: usize,
    lead_samples: i64,
}

const CHANGE: usize = 16 * 24;
const PULSES: usize = CHANGE + 16 * 24;

fn period_at(case: Case, pulse: usize) -> f64 {
    let base = 2_500_000. / case.bpm;
    match case.profile {
        Profile::Steady => base,
        Profile::Nudge(pct) if (CHANGE..CHANGE + 192).contains(&pulse) => base / (1. + pct / 100.),
        Profile::Nudge(_) => base,
        Profile::Step(pct) if pulse >= CHANGE => base / (1. + pct / 100.),
        Profile::Step(_) => base,
        Profile::Ramp(pct) => {
            let progress = (pulse.saturating_sub(CHANGE) as f64 / 192.).min(1.);
            base / (1. + pct / 100. * progress)
        }
    }
}

fn wire(case: Case) -> (Vec<f64>, Vec<i64>) {
    let mut ideal = Vec::with_capacity(PULSES);
    let mut at = 0.;
    for p in 0..PULSES {
        ideal.push(at);
        at += period_at(case, p);
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

/// Signed error in samples of every eighth (pulse index, error); None if it never sounded.
pub(super) fn eighth_errors(case: Case) -> Vec<(usize, Option<i64>)> {
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
    let mut song = sampler_song(TempoMap::new(case.session_bpm, 4, 4).unwrap(), rate, PULSES / 12, end);
    let mut mix = MixEngine::new(RATE as f32, CHUNK);
    let mut audio = vec![vec![0.; CHUNK]; 2];
    // Heard sample (session sample) of each rendered frame starts at the lead.
    let mut pcm = Vec::new();
    let mut actions = Vec::new();
    let lead = case.lead_samples;
    let micros = |samples: i64| (samples * 1_000_000 + RATE / 2) / RATE;
    let mut callback = 0;
    let mut pulse = 0;
    while callback + lead < end {
        while pulse < PULSES && arrivals[pulse] <= micros(callback) {
            feed.set_clock(arrivals[pulse]);
            feed.receive(port, &[0xf8]);
            let snap = feed.clock_follow_status(&mut actions);
            actions.clear();
            let guard = rate.samples((2.5 / snap.bpm.unwrap_or(case.bpm)).max(case.buffer as f64 / RATE as f64));
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
            pcm.extend_from_slice(&audio[0][..n]);
        }
        callback += case.buffer as i64;
    }
    let heard: Vec<i64> = onsets(&pcm).into_iter().map(|i| lead + i as i64).collect();
    (1..PULSES / 12 - 1)
        .map(|note| {
            let target = sample(ideal[note * 12]);
            (note * 12, heard.iter().map(|&h| h - target).filter(|e| e.abs() < 4_800).min_by_key(|e| e.abs()))
        })
        .collect()
}

pub(super) struct Score {
    pub mean_ms: f64,
    pub sd_ms: f64,
    pub worst_ms: f64,
    pub early: usize,
    pub missing: usize,
    pub late_after_change: usize,
}

/// Scores eighths from beat 4 on (after lock). Early: more than 2 ms before the beat.
/// Late after change: eighths in the two beats after the change more than 4 ms off.
pub(super) fn score(case: Case) -> Score {
    let errors = eighth_errors(case);
    let ms: Vec<f64> = errors.iter().filter(|(p, _)| *p >= 96).filter_map(|(_, e)| e.map(|e| e as f64 / 48.)).collect();
    let missing = errors.iter().filter(|(p, e)| *p >= 96 && e.is_none()).count();
    let n = ms.len().max(1) as f64;
    let mean = ms.iter().sum::<f64>() / n;
    let sd = (ms.iter().map(|e| (e - mean) * (e - mean)).sum::<f64>() / n).sqrt();
    let changed = !matches!(case.profile, Profile::Steady);
    let late_after_change = errors
        .iter()
        .filter(|(p, _)| changed && (CHANGE + 1..=CHANGE + 48).contains(p))
        .filter(|(_, e)| e.is_none_or(|e| (e as f64 / 48.).abs() > 4.))
        .count();
    Score {
        mean_ms: mean,
        sd_ms: sd,
        worst_ms: ms.iter().fold(0., |w: f64, e| w.max(e.abs())),
        early: ms.iter().filter(|e| **e < -2.).count(),
        missing,
        late_after_change,
    }
}

pub(super) fn cases() -> Vec<(&'static str, Case)> {
    let mut out = Vec::new();
    // Device lead measured on an RME UFX: 324 samples at 256 frames, 1092 at 1024.
    for (buffer, lead_samples) in [(256, 324), (1024, 1092)] {
        let base = Case { profile: Profile::Steady, bpm: 128., session_bpm: 128., jitter_us: 0, buffer, lead_samples };
        out.extend([
            ("steady clean", base),
            ("steady ±0.5 ms", Case { jitter_us: 500, ..base }),
            ("steady ±1 ms", Case { jitter_us: 1000, ..base }),
            ("session 120, source 135 ±1 ms", Case { bpm: 135., session_bpm: 120., jitter_us: 1000, ..base }),
            ("nudge +0.5% ±0.5 ms", Case { profile: Profile::Nudge(0.5), jitter_us: 500, ..base }),
            ("nudge -0.5% ±0.5 ms", Case { profile: Profile::Nudge(-0.5), jitter_us: 500, ..base }),
            ("nudge +1% ±0.5 ms", Case { profile: Profile::Nudge(1.), jitter_us: 500, ..base }),
            ("step +8% ±1 ms", Case { profile: Profile::Step(8.), jitter_us: 1000, ..base }),
            ("step -7.9% ±1 ms", Case { profile: Profile::Step(-7.9), jitter_us: 1000, ..base }),
            ("step -20% clean", Case { profile: Profile::Step(-20.), ..base }),
            ("step -20% ±1 ms", Case { profile: Profile::Step(-20.), jitter_us: 1000, ..base }),
            ("ramp 128->140 ±0.5 ms", Case { profile: Profile::Ramp(9.4), jitter_us: 500, ..base }),
            ("ramp 128->116 ±0.5 ms", Case { profile: Profile::Ramp(-9.4), jitter_us: 500, ..base }),
        ]);
    }
    out
}

#[test]
#[ignore = "scorecard; prints a table"]
fn follow_scorecard() {
    eprintln!("{:<32} {:>5} | {:>7} {:>6} {:>6} {:>5} {:>6} {:>7}", "case", "frm", "mean", "sd", "worst", "early", "chg", "missing");
    for (name, case) in cases() {
        let s = score(case);
        eprintln!(
            "{:<32} {:>5} | {:>+7.2} {:>6.2} {:>6.2} {:>5} {:>6} {:>7}",
            name, case.buffer, s.mean_ms, s.sd_ms, s.worst_ms, s.early, s.late_after_change, s.missing
        );
    }
}

#[test]
#[ignore = "trace one scorecard case named by SOUNDCRAFT_BENCH_CASE"]
fn follow_trace() {
    let want = std::env::var("SOUNDCRAFT_BENCH_CASE").unwrap_or_default();
    let frames: usize = std::env::var("SOUNDCRAFT_BENCH_FRAMES").ok().and_then(|v| v.parse().ok()).unwrap_or(1024);
    for (name, case) in cases().into_iter().filter(|(n, c)| *n == want && c.buffer == frames) {
        let row: Vec<String> = eighth_errors(case)
            .iter()
            .filter(|(p, _)| (CHANGE - 48..=CHANGE + 144).contains(p))
            .map(|(p, e)| format!("{}:{}", *p as i64 - CHANGE as i64, e.map_or("miss".into(), |e| format!("{:.1}", e as f64 / 48.))))
            .collect();
        eprintln!("{name} {}: {}", case.buffer, row.join(" "));
    }
}

/// A large device buffer with a jittery source: the score is projected from the filtered
/// pulse train, so notes stay on the beat instead of trailing the wire by about a buffer.
#[test]
fn steady_and_nudged_clocks_stay_on_the_beat_with_a_large_device_buffer() {
    let base = Case { profile: Profile::Steady, bpm: 128., session_bpm: 128., jitter_us: 1000, buffer: 1024, lead_samples: 1092 };
    for (name, case) in [
        ("steady ±1 ms", base),
        ("session 120, source 135 ±1 ms", Case { bpm: 135., session_bpm: 120., ..base }),
        ("nudge -0.5% ±0.5 ms", Case { profile: Profile::Nudge(-0.5), jitter_us: 500, ..base }),
    ] {
        let s = score(case);
        assert_eq!(s.missing, 0, "{name}");
        assert_eq!(s.early, 0, "{name}: an eighth came more than 2 ms early");
        assert!(s.mean_ms.abs() <= 3. && s.worst_ms <= 10., "{name}: mean {:+.2} ms, worst {:.2} ms", s.mean_ms, s.worst_ms);
    }
}
