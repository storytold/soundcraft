use std::f32::consts::TAU;

use soundcraft_dsp::meter::{LoudnessMeter, PeakMeter, RmsMeter};
use soundcraft_dsp::offline::{self, FadeShape, Waveform};
use soundcraft_dsp::spectrum::{bin_hz, magnitude_db};
use soundcraft_dsp::{create, db_to_gain, gain_to_db, pan};

fn sine(freq: f32, amp: f32, sr: f32, len: usize) -> Vec<f32> {
    (0..len).map(|i| amp * (TAU * freq * i as f32 / sr).sin()).collect()
}

fn peak(x: &[f32]) -> f32 {
    x.iter().fold(0.0f32, |m, v| m.max(v.abs()))
}

/// Dominant frequency by FFT with parabolic interpolation.
fn dominant_hz(x: &[f32], sr: f32) -> f32 {
    let n = 32_768.min(x.len().next_power_of_two() / 2);
    let spec = magnitude_db(&x[x.len() / 2 - n / 2..], n);
    let k = spec.iter().enumerate().skip(1).max_by(|a, b| a.1.total_cmp(b.1)).unwrap().0;
    let (a, b, c) = (spec[k - 1], spec[k], spec[k + 1]);
    let d = 0.5 * (a - c) / (a - 2.0 * b + c);
    bin_hz(k, n, sr) + d * sr / n as f32
}

#[test]
fn db_helpers() {
    assert_eq!(db_to_gain(-200.0), 0.0);
    assert_eq!(db_to_gain(f32::NAN), 0.0);
    assert!((db_to_gain(-6.0206) - 0.5).abs() < 1e-4);
    assert_eq!(gain_to_db(0.0), -144.0);
    assert_eq!(gain_to_db(f32::NAN), -144.0);
    assert!((gain_to_db(0.5) + 6.0206).abs() < 1e-3);
    assert!((gain_to_db(-1.0)).abs() < 1e-6);
}

#[test]
fn pan_law_hard_right() {
    let (l, r) = pan::gains(1.0, pan::PanLaw::Minus3);
    assert!(l.abs() < 1e-6 && (r - 1.0).abs() < 1e-6);
    let (l, r) = pan::gains(f32::NAN, pan::PanLaw::Minus6);
    assert!((l - r).abs() < 1e-6);
}

#[test]
fn loudness_997hz_minus20_stereo_reads_minus20() {
    let sr = 48_000.0;
    let x = sine(997.0, db_to_gain(-20.0), sr, 48_000 * 10);
    let mut m = LoudnessMeter::new(sr, 2);
    m.process(&[x.clone(), x], 48_000 * 10);
    assert!((m.integrated_lufs() + 20.0).abs() < 0.1, "integrated {}", m.integrated_lufs());
    assert!((m.momentary_lufs() + 20.0).abs() < 0.1, "momentary {}", m.momentary_lufs());
    assert!((m.short_term_lufs() + 20.0).abs() < 0.1, "short {}", m.short_term_lufs());
    assert!((m.true_peak_db() + 20.0).abs() < 0.2, "tp {}", m.true_peak_db());
}

#[test]
fn loudness_full_scale_mono_and_44k() {
    for sr in [44_100.0f32, 48_000.0, 96_000.0] {
        let len = (sr * 5.0) as usize;
        let x = sine(997.0, 1.0, sr, len);
        let mut m = LoudnessMeter::new(sr, 1);
        m.process(&[x], len);
        assert!((m.integrated_lufs() + 3.01).abs() < 0.1, "{sr}: {}", m.integrated_lufs());
    }
}

#[test]
fn loudness_stays_bounded_at_low_sample_rates() {
    // The K-weighting shelf corner (~1.7 kHz) sits at or above Nyquist below ~3.4 kHz; the
    // filter must stay stable there instead of reading hundreds of LUFS (issue #240).
    for sr in [1000.0f32, 1200.0, 1600.0, 2000.0, 3000.0, 3400.0, 8000.0, 48_000.0] {
        let len = sr as usize;
        let mut m = LoudnessMeter::new(sr, 1);
        assert_eq!(m.sample_rate(), sr);
        m.process(&[sine(100.0, 0.5, sr, len)], len);
        for level in [m.integrated_lufs(), m.momentary_lufs(), m.short_term_lufs()] {
            assert!(level.is_finite() && (-30.0..=0.0).contains(&level), "{sr}: {level}");
        }
    }
}

#[test]
fn loudness_gating_ignores_silence() {
    let sr = 48_000.0;
    let mut x = sine(997.0, db_to_gain(-20.0), sr, 48_000 * 5);
    x.extend(std::iter::repeat_n(0.0, 48_000 * 20));
    let mut m = LoudnessMeter::new(sr, 2);
    m.process(&[x.clone(), x], 48_000 * 25);
    assert!((m.integrated_lufs() + 20.0).abs() < 0.2, "{}", m.integrated_lufs());
    assert!(m.momentary_lufs() < -100.0);
    m.reset();
    assert_eq!(m.integrated_lufs(), -144.0);
}

#[test]
fn true_peak_catches_intersample_overs() {
    // A sine at fs/4 phased 45° has samples at ±0.707·A but true peak A.
    let sr = 48_000.0;
    let x: Vec<f32> = (0..48_000).map(|i| (TAU * 0.25 * (i % 4) as f32 + TAU / 8.0).sin()).collect();
    assert!(peak(&x) < 0.71);
    let mut m = LoudnessMeter::new(sr, 1);
    m.process(&[x], 48_000);
    assert!(m.true_peak_db() > -0.5, "{}", m.true_peak_db());
}

#[test]
fn peak_meter_ballistics() {
    let mut m = PeakMeter::new(48_000.0);
    m.process(&[0.5; 480]);
    assert!((m.level_db() + 6.02).abs() < 0.01);
    m.process(&vec![0.0; 48_000]);
    assert!((m.level_db() - (-6.02 - 20.0)).abs() < 0.1, "{}", m.level_db());
    assert!((m.peak_hold_db() + 6.02).abs() < 0.01);
    assert!(!m.clipped());
    m.process(&[1.0]);
    assert!(m.clipped());
    m.reset_clip();
    assert!(!m.clipped());
    let mut r = RmsMeter::new(48_000.0);
    r.process(&sine(1000.0, 1.0, 48_000.0, 48_000));
    assert!((r.rms_db() + 3.01).abs() < 0.2, "{}", r.rms_db());
}

#[test]
fn resample_preserves_frequency_and_length() {
    let x = sine(1000.0, 0.5, 48_000.0, 48_000);
    let out = offline::resample(std::slice::from_ref(&x), 48_000, 44_100);
    assert_eq!(out[0].len(), 44_100);
    let f = dominant_hz(&out[0], 44_100.0);
    assert!((f - 1000.0).abs() < 1.0, "{f}");
    assert!((peak(&out[0][1000..43_000]) - 0.5).abs() < 0.01);
    let up = offline::resample(&out, 44_100, 96_000);
    assert_eq!(up[0].len(), 96_000);
    assert!((dominant_hz(&up[0], 96_000.0) - 1000.0).abs() < 1.0);
    // Content above the new Nyquist is removed.
    let hi = sine(23_000.0, 0.5, 48_000.0, 48_000);
    let down = offline::resample(&[hi], 48_000, 22_050);
    assert!(peak(&down[0][1000..20_000]) < 0.01, "alias {}", peak(&down[0][1000..20_000]));
    assert_eq!(offline::resample(&[vec![1.0; 10]], 0, 48_000)[0].len(), 10);
}

#[test]
fn time_stretch_length_and_pitch() {
    let sr = 48_000.0;
    let x = sine(440.0, 0.5, sr, 48_000);
    for ratio in [0.5, 0.8, 1.25, 2.0] {
        let out = offline::time_stretch(&[x.clone(), x.clone()], ratio, sr);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].len(), (48_000.0 * ratio).round() as usize);
        let f = dominant_hz(&out[0], sr);
        assert!((f - 440.0).abs() < 3.0, "ratio {ratio}: {f}");
    }
    assert!(offline::time_stretch(&[vec![]], 2.0, sr)[0].is_empty());
    let _ = offline::time_stretch(std::slice::from_ref(&x), f64::NAN, f32::NAN);
}

#[test]
fn pitch_shift_preserves_length_and_shifts() {
    let sr = 48_000.0;
    let x = sine(440.0, 0.5, sr, 48_000);
    let out = offline::pitch_shift(std::slice::from_ref(&x), 12.0, sr);
    assert_eq!(out[0].len(), x.len());
    let f = dominant_hz(&out[0], sr);
    assert!((f - 880.0).abs() < 6.0, "{f}");
    let down = offline::pitch_shift(std::slice::from_ref(&x), -7.0, sr);
    assert_eq!(down[0].len(), x.len());
    let f = dominant_hz(&down[0], sr);
    let want = 440.0 * 2f32.powf(-7.0 / 12.0);
    assert!((f - want).abs() < 4.0, "{f} vs {want}");
}

#[test]
fn pitch_shift_reaches_the_ends_of_its_range() {
    let sr = 48_000.0;
    let x = sine(440.0, 0.5, sr, 48_000);
    for st in [40.0f32, 48.0, -40.0, -48.0] {
        let out = offline::pitch_shift(std::slice::from_ref(&x), st, sr);
        assert_eq!(out[0].len(), x.len());
        let f = dominant_hz(&out[0], sr);
        let want = 440.0 * 2f32.powf(st / 12.0);
        assert!((f / want - 1.0).abs() < 0.002, "{st} semitones: {f} vs {want}");
    }
}

#[test]
fn normalize_hits_target_peak() {
    let mut ch = vec![sine(100.0, 0.2, 48_000.0, 4800), sine(100.0, 0.05, 48_000.0, 4800)];
    offline::normalize(&mut ch, -1.0, false);
    assert!((gain_to_db(peak(&ch[0])) + 1.0).abs() < 0.01);
    assert!((gain_to_db(peak(&ch[1])) + 13.04).abs() < 0.05);
    offline::normalize(&mut ch, -3.0, true);
    assert!((gain_to_db(peak(&ch[1])) + 3.0).abs() < 0.01);
    let mut silent = vec![vec![0.0; 10]];
    offline::normalize(&mut silent, 0.0, true);
    assert!(silent[0].iter().all(|&v| v == 0.0));
}

#[test]
fn simple_offline_ops() {
    let mut ch = vec![vec![1.0, 2.0, 3.0]];
    offline::reverse(&mut ch);
    assert_eq!(ch[0], vec![3.0, 2.0, 1.0]);
    offline::invert(&mut ch);
    assert_eq!(ch[0], vec![-3.0, -2.0, -1.0]);
    offline::remove_dc(&mut ch);
    assert!(ch[0].iter().sum::<f32>().abs() < 1e-5);
    offline::gain(&mut ch, f32::NAN);
    offline::fade(&mut ch, 2, 2, FadeShape::EqualPower);
    assert_eq!(ch[0][0], 0.0);
}

#[test]
fn non_silent_ranges_finds_bursts() {
    let sr = 48_000.0;
    let mut x = vec![0.0f32; 48_000];
    for (i, v) in sine(500.0, 0.5, sr, 4800).into_iter().enumerate() {
        x[10_000 + i] = v;
        x[30_000 + i] = v;
    }
    let r = offline::non_silent_ranges(std::slice::from_ref(&x), -40.0, 480, 100, 200);
    assert_eq!(r.len(), 2, "{r:?}");
    assert!((r[0].0 as i64 - 9_900).abs() <= 5 && (r[0].1 as i64 - 15_000).abs() <= 5, "{r:?}");
    assert!((r[1].0 as i64 - 29_900).abs() <= 5, "{r:?}");
    // A huge minimum gap merges them.
    let r = offline::non_silent_ranges(&[x], -40.0, 30_000, 0, 0);
    assert_eq!(r.len(), 1);
    assert!(offline::non_silent_ranges(&[vec![0.0; 100]], -40.0, 10, 5, 5).is_empty());
}

#[test]
fn silence_scan_in_blocks_finds_what_one_pass_finds() {
    let mut x = vec![0.0f32; 48_000];
    for (i, v) in sine(500.0, 0.5, 48_000.0, 4800).into_iter().enumerate() {
        x[10_000 + i] = v;
        x[30_000 + i] = v;
    }
    let whole: Vec<(u64, u64)> =
        offline::non_silent_ranges(std::slice::from_ref(&x), -40.0, 480, 100, 200).into_iter().map(|(a, b)| (a as u64, b as u64)).collect();
    let mut blocks = offline::SilenceScan::new(-40.0, 480);
    x.chunks(1000).for_each(|b| blocks.push(&[b.to_vec()]));
    assert_eq!(blocks.finish(100, 200), whole);
    // Skipping the silence between the bursts counts its length like scanning it.
    let mut skipped = offline::SilenceScan::new(-40.0, 480);
    skipped.push(&[x[..20_000].to_vec()]);
    skipped.skip(10_000);
    skipped.push(&[x[30_000..].to_vec()]);
    assert_eq!(skipped.finish(100, 200), whole);
}

#[test]
fn transients_detected_near_onsets() {
    let sr = 48_000.0;
    let mut x = vec![0.0f32; 48_000 * 2];
    let onsets = [9_600usize, 33_600, 60_000, 81_000];
    for &o in &onsets {
        for i in 0..15000 {
            x[o + i] += 0.8 * (-(i as f32) / 1500.0).exp() * (TAU * 180.0 * i as f32 / sr).sin();
        }
    }
    let t = offline::detect_transients(&[x], sr, 0.5);
    assert_eq!(t.len(), onsets.len(), "{t:?}");
    for (a, b) in t.iter().zip(onsets) {
        assert!((*a as i64 - b as i64).abs() < 480, "{a} vs {b}");
    }
    assert!(offline::detect_transients(&[vec![0.0; 48_000]], sr, 1.0).is_empty());
}

#[test]
fn signal_generator_offline() {
    for w in Waveform::ALL {
        let out = offline::signal_generator(w, 1000.0, -6.0, 48_000.0, 4800, 2);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].len(), 4800);
        assert!(peak(&out[0]) <= db_to_gain(-6.0) * 1.1 + 1e-3, "{w:?} {}", peak(&out[0]));
        assert!(peak(&out[0]) > 0.05, "{w:?}");
    }
}

#[test]
fn apply_plugin_compensates_latency() {
    let sr = 48_000.0;
    let mut p = create("pitch_shifter").unwrap();
    let mut x = vec![vec![0.0f32; 10_000]];
    x[0][2000] = 1.0;
    offline::apply_plugin(&mut x, p.as_mut(), sr);
    let pos = x[0].iter().enumerate().max_by(|a, b| a.1.abs().total_cmp(&b.1.abs())).unwrap().0;
    assert_eq!(pos, 2000);

    let mut r = create("room_reverb").unwrap();
    let out = offline::apply_plugin_with_tail(&[vec![1.0; 100]], r.as_mut(), sr);
    assert!(out[0].len() > 100 + 48_000);
}

#[test]
fn spectrum_edge_cases() {
    assert!(magnitude_db(&[], 1).is_empty());
    let m = magnitude_db(&[f32::NAN; 64], 64);
    assert!(m.iter().all(|v| v.is_finite()));
}
