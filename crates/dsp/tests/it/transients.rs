use soundcraft_dsp::offline::detect_transients;
use std::f32::consts::TAU;

const SR: f32 = 48_000.0;

fn at(secs: f32) -> usize {
    (secs * SR) as usize
}

fn mix(len: f32, parts: &[(f32, Vec<f32>)]) -> Vec<f32> {
    let mut x = vec![0.0; at(len)];
    for (start, sig) in parts {
        let fade = at(0.03).min(sig.len());
        for (i, v) in sig.iter().enumerate() {
            let left = sig.len() - i;
            let g = if left < fade { 0.5 - 0.5 * (std::f32::consts::PI * left as f32 / fade as f32).cos() } else { 1.0 };
            if let Some(o) = x.get_mut(at(*start) + i) {
                *o += v * g;
            }
        }
    }
    x
}

fn noise(seed: &mut u32) -> f32 {
    *seed ^= *seed << 13;
    *seed ^= *seed >> 17;
    *seed ^= *seed << 5;
    *seed as f32 / u32::MAX as f32 * 2.0 - 1.0
}

fn hit(level: f32, decay: f32, seed: &mut u32) -> Vec<f32> {
    let mut prev = 0.0;
    (0..at(decay * 6.0))
        .map(|i| {
            let w = noise(seed);
            let hp = w - prev;
            prev = w;
            level * hp * (-(i as f32) / SR / decay).exp()
        })
        .collect()
}

fn note(f: f32, level: f32, secs: f32) -> Vec<f32> {
    (0..at(secs))
        .map(|i| {
            let t = i as f32 / SR;
            let s: f32 = (1..=8).map(|k| (TAU * f * k as f32 * t).sin() * (-t * (0.6 + 0.4 * k as f32)).exp() / k as f32).sum();
            level * s * (t / 0.002).min(1.0)
        })
        .collect()
}

fn drums(len: f32) -> (Vec<f32>, Vec<usize>) {
    let mut seed = 7;
    let mut parts = Vec::new();
    let mut onsets = Vec::new();
    let mut t = 0.3;
    while t < len - 0.5 {
        parts.push((t, hit(0.9, 0.04, &mut seed)));
        onsets.push(at(t));
        for k in 1..4 {
            parts.push((t + 0.14 * k as f32, hit(0.09, 0.03, &mut seed)));
            onsets.push(at(t + 0.14 * k as f32));
        }
        t += 0.56;
    }
    (mix(len, &parts), onsets)
}

fn near(found: &[usize], onsets: &[usize], tolerance: usize) -> bool {
    found.len() == onsets.len() && found.iter().zip(onsets).all(|(f, o)| f.abs_diff(*o) <= tolerance)
}

#[test]
fn notes_starting_over_a_ringing_note_are_placed_on_their_attacks() {
    let starts = [0.3, 0.72, 1.14, 1.56, 1.98, 2.4];
    let freqs = [196.0, 261.6, 196.0, 329.6, 246.9, 392.0];
    let parts: Vec<(f32, Vec<f32>)> =
        starts.iter().zip(freqs).enumerate().map(|(k, (s, f))| (*s, note(f, 0.2 + 0.1 * (k % 3) as f32, 1.1))).collect();
    let found = detect_transients(&[mix(4.0, &parts)], SR, 0.5);
    let onsets: Vec<usize> = starts.iter().map(|s| at(*s)).collect();
    assert!(near(&found, &onsets, at(0.003)), "{found:?} vs {onsets:?}");
}

#[test]
fn quieter_hits_between_loud_ones_count() {
    let (x, onsets) = drums(6.0);
    let found = detect_transients(&[x], SR, 0.5);
    assert!(near(&found, &onsets, at(0.001)), "{found:?} vs {onsets:?}");
}

#[test]
fn a_held_chord_with_vibrato_and_chorus_has_no_transients() {
    let mut ph = [0.0f32; 6];
    let x: Vec<f32> = (0..at(6.0))
        .map(|i| {
            let t = i as f32 / SR;
            let mut s = 0.0;
            for (j, f) in [220.0f32, 277.2, 329.6].iter().enumerate() {
                for (d, sign) in [(0, 1.0f32), (1, -1.0)] {
                    let p = &mut ph[j * 2 + d];
                    *p += TAU * f * (1.0 + sign * 0.004 * (TAU * (0.3 + 0.07 * j as f32) * t).sin()) / SR;
                    s += (1..=6).map(|k| (*p * k as f32).sin() / k as f32).sum::<f32>();
                }
            }
            0.04 * s * (t / 0.4).min(1.0) * (1.0 + 0.3 * (TAU * 4.0 * t).sin())
        })
        .collect();
    let found: Vec<usize> = detect_transients(&[x], SR, 0.5).into_iter().filter(|p| *p > at(0.5)).collect();
    assert!(found.is_empty(), "{found:?}");
}

#[test]
fn transients_do_not_depend_on_where_the_analysed_audio_starts() {
    let (mut x, _) = drums(8.0);
    let from = at(3.3);
    x.iter_mut().skip(from - at(0.4)).for_each(|v| *v *= 0.05);
    let whole = detect_transients(&[x.clone()], SR, 0.5);
    let part: Vec<usize> = detect_transients(&[x.get(from..).unwrap_or_default().to_vec()], SR, 0.5).into_iter().map(|p| p + from).collect();
    let settled = |v: &[usize]| v.iter().copied().filter(|p| *p > from + at(0.25)).collect::<Vec<_>>();
    assert!(!settled(&whole).is_empty());
    assert_eq!(settled(&whole), settled(&part), "{whole:?} vs {part:?}");
}

#[test]
fn a_fast_release_is_not_a_transient() {
    let starts: Vec<f32> = (0..8).map(|k| 0.3 + k as f32 * 0.5).collect();
    let note: Vec<f32> = (0..at(0.4))
        .map(|i| {
            let t = i as f32 / SR;
            let level = if t < 0.15 { (t / 0.01).min(1.0) } else { 0.001f32.powf((t - 0.15) / 0.04) };
            let saw: f32 = (1..=20).map(|k| (TAU * 659.0 * k as f32 * t).sin() / k as f32).sum();
            0.2 * level * saw
        })
        .collect();
    let parts: Vec<(f32, Vec<f32>)> = starts.iter().map(|s| (*s, note.clone())).collect();
    let found = detect_transients(&[mix(4.5, &parts)], SR, 0.5);
    let onsets: Vec<usize> = starts.iter().map(|s| at(*s)).collect();
    assert!(near(&found, &onsets, at(0.002)), "{found:?} vs {onsets:?}");
}

#[test]
fn channels_that_cancel_when_mixed_still_show_their_transients() {
    let (left, onsets) = drums(4.0);
    let right: Vec<f32> = left.iter().map(|v| -v).collect();
    let found = detect_transients(&[left, right], SR, 0.5);
    assert!(near(&found, &onsets, at(0.001)), "{found:?} vs {onsets:?}");
}
