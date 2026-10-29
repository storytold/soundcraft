use super::*;
use crate::SampleBuffer;
use std::f32::consts::TAU;

const SR: f32 = 48_000.0;

fn sampler(sample: SampleBuffer) -> Sampler {
    let mut s = Sampler::new();
    s.prepare(SR, 512, 2);
    assert!(s.set_sample(Some(Arc::new(sample))));
    s
}

fn mono(sample_rate: f32, data: Vec<f32>) -> SampleBuffer {
    SampleBuffer { sample_rate, channels: vec![data] }
}

fn sine(freq: f32, sr: f32, len: usize) -> Vec<f32> {
    (0..len).map(|i| 0.5 * (TAU * freq * i as f32 / sr).sin()).collect()
}

/// Renders `len` frames in 512-frame blocks; returns both channels.
fn run(s: &mut Sampler, len: usize) -> [Vec<f32>; 2] {
    let mut out = [Vec::new(), Vec::new()];
    let mut buf = vec![vec![1.0f32; 512]; 2];
    let mut done = 0;
    while done < len {
        let n = 512.min(len - done);
        s.process(&mut buf, n);
        out[0].extend_from_slice(&buf[0][..n]);
        out[1].extend_from_slice(&buf[1][..n]);
        done += n;
    }
    out
}

fn peak(x: &[f32]) -> f32 {
    x.iter().fold(0.0f32, |m, v| m.max(v.abs()))
}

/// Frequency from upward zero crossings.
fn freq(x: &[f32], sr: f32) -> f32 {
    let ups: Vec<usize> = x.windows(2).enumerate().filter(|(_, w)| w[0] < 0.0 && w[1] >= 0.0).map(|(i, _)| i).collect();
    let (Some(a), Some(b)) = (ups.first(), ups.last()) else { return 0.0 };
    (ups.len() - 1) as f32 * sr / (b - a) as f32
}

#[test]
fn silent_without_a_sample_and_replaces_the_buffer() {
    let mut s = Sampler::new();
    s.prepare(SR, 512, 2);
    s.note_on(0, 60, 127);
    let out = run(&mut s, 4800);
    assert_eq!(peak(&out[0]), 0.0);
    assert_eq!(peak(&out[1]), 0.0);
}

#[test]
fn root_note_plays_the_sample_verbatim() {
    let data = sine(440.0, SR, 9600);
    let mut s = sampler(mono(SR, data.clone()));
    s.set_param("velocity", 0.0);
    s.note_on(0, 60, 100);
    let out = run(&mut s, 9600);
    // Past the 0.1-ms-ish attack the output is the sample itself, on both sides.
    for i in 100..9600 {
        assert!((out[0][i] - data[i]).abs() < 1e-6, "frame {i}: {} vs {}", out[0][i], data[i]);
    }
    assert_eq!(out[0], out[1], "a mono sample feeds both channels");
}

#[test]
fn keys_transpose_from_the_root_note() {
    let mut s = sampler(mono(SR, sine(400.0, SR, 96_000)));
    s.note_on(0, 72, 127);
    let up = run(&mut s, 24_000);
    assert!((freq(&up[0][480..], SR) - 800.0).abs() < 2.0, "an octave up: {}", freq(&up[0][480..], SR));
    s.reset();
    s.note_on(0, 53, 127);
    let down = run(&mut s, 24_000);
    let want = 400.0 * 2f32.powf(-7.0 / 12.0);
    assert!((freq(&down[0][480..], SR) - want).abs() < 2.0);
    // Root note 48 makes key 60 play an octave up; transpose and fine tune add on top.
    s.reset();
    s.set_param("root_note", 48.0);
    s.set_param("transpose", -12.0);
    s.set_param("fine_tune", 100.0);
    s.note_on(0, 60, 127);
    let out = run(&mut s, 24_000);
    let want = 400.0 * 2f32.powf(1.0 / 12.0);
    assert!((freq(&out[0][480..], SR) - want).abs() < 2.0, "{}", freq(&out[0][480..], SR));
}

#[test]
fn key_tracking_off_plays_every_key_at_the_sample_pitch() {
    let mut s = sampler(mono(SR, sine(400.0, SR, 48_000)));
    s.set_param("key_track", 0.0);
    s.note_on(0, 90, 127);
    let out = run(&mut s, 24_000);
    assert!((freq(&out[0][480..], SR) - 400.0).abs() < 2.0);
}

#[test]
fn converts_the_sample_rate() {
    // A 300 Hz tone recorded at 22.05 kHz still plays at 300 Hz on a 48 kHz host.
    let mut s = sampler(mono(22_050.0, sine(300.0, 22_050.0, 22_050)));
    s.note_on(0, 60, 127);
    let out = run(&mut s, 60_000);
    assert!((freq(&out[0][480..], SR) - 300.0).abs() < 1.0, "{}", freq(&out[0][480..], SR));
    // And it lasts as long as the recording: one second.
    assert!(peak(&out[0][46_000..47_500]) > 0.1);
    assert_eq!(peak(&out[0][48_200..]), 0.0, "the sample ended");
    // A 96 kHz recording is read faster, not cut short.
    let mut s = sampler(mono(96_000.0, sine(1000.0, 96_000.0, 96_000)));
    s.note_on(0, 60, 127);
    let out = run(&mut s, 48_000);
    assert!((freq(&out[0][480..], SR) - 1000.0).abs() < 2.0);
}

#[test]
fn key_and_velocity_ranges_gate_notes() {
    let mut s = sampler(mono(SR, vec![0.5; 4800]));
    s.set_param("key_low", 48.0);
    s.set_param("key_high", 60.0);
    s.set_param("vel_low", 40.0);
    s.set_param("vel_high", 100.0);
    for (note, vel, sounds) in [(47, 80, false), (61, 80, false), (48, 80, true), (60, 80, true), (55, 39, false), (55, 101, false), (55, 40, true)] {
        s.reset();
        s.note_on(0, note, vel);
        let out = run(&mut s, 960);
        assert_eq!(peak(&out[0]) > 0.0, sounds, "note {note} velocity {vel}");
    }
    // Reversed ranges are read in order rather than muting everything.
    s.set_param("key_low", 60.0);
    s.set_param("key_high", 48.0);
    s.reset();
    s.note_on(0, 50, 80);
    assert!(peak(&run(&mut s, 960)[0]) > 0.0);
}

#[test]
fn velocity_sensitivity_scales_level() {
    let level = |sens: f32, vel: u8| {
        let mut s = sampler(mono(SR, vec![0.5; 4800]));
        s.set_param("velocity", sens);
        s.note_on(0, 60, vel);
        peak(&run(&mut s, 2400)[0][1000..])
    };
    assert!((level(100.0, 127) - 0.5).abs() < 1e-4);
    assert!((level(100.0, 64) - 0.5 * 64.0 / 127.0).abs() < 1e-4);
    assert!((level(0.0, 1) - 0.5).abs() < 1e-4, "no sensitivity: every velocity is full level");
    assert!(level(70.0, 30) < level(70.0, 127));
}

#[test]
fn adsr_shapes_the_note() {
    let mut s = sampler(mono(SR, vec![0.5; 96_000]));
    s.set_param("velocity", 0.0);
    s.set_param("attack", 100.0);
    s.set_param("decay", 100.0);
    s.set_param("sustain", 50.0);
    s.set_param("release", 100.0);
    s.note_on(0, 60, 127);
    let out = run(&mut s, 24_000);
    // Linear attack: halfway up at 50 ms, full level at 100 ms, then decay to the sustain level.
    assert!((out[0][2400] - 0.25).abs() < 0.01, "attack midpoint {}", out[0][2400]);
    assert!(peak(&out[0][4700..4900]) > 0.49);
    assert!((out[0][20_000] - 0.25).abs() < 0.005, "sustain {}", out[0][20_000]);
    s.note_off(0, 60);
    let out = run(&mut s, 24_000);
    assert!(out[0][100] > 0.2, "release starts from the sustain level");
    assert!(peak(&out[0][5000..]) < 1e-3, "released after 100 ms");
    assert_eq!(peak(&out[0][12_000..]), 0.0, "the voice ended");
}

#[test]
fn gate_mode_stops_on_note_off_and_one_shot_plays_through() {
    let mut s = sampler(mono(SR, vec![0.5; 24_000]));
    s.set_param("release", 1.0);
    s.note_on(0, 60, 127);
    s.note_off(1000, 60);
    let out = run(&mut s, 24_000);
    assert!(peak(&out[0][..900]) > 0.1);
    assert_eq!(peak(&out[0][2000..]), 0.0, "gate: released at the note-off");
    s.set_param("play_mode", 1.0);
    s.reset();
    s.note_on(0, 60, 127);
    s.note_off(1000, 60);
    let out = run(&mut s, 30_000);
    assert!(peak(&out[0][20_000..23_900]) > 0.1, "one shot: ignores the note-off");
    assert_eq!(peak(&out[0][24_100..]), 0.0, "and stops at the end of the sample");
    assert!(s.tail_samples() >= 24_000, "a one-shot's tail covers the sample");
    // Stopping the transport still silences one-shots.
    s.reset();
    s.note_on(0, 60, 127);
    run(&mut s, 512);
    s.all_notes_off();
    assert_eq!(peak(&run(&mut s, 4800)[0][2400..]), 0.0);
}

#[test]
fn retriggering_a_held_key_releases_the_old_voice() {
    let mut s = sampler(mono(SR, vec![0.25; 48_000]));
    s.set_param("velocity", 0.0);
    s.set_param("release", 1.0);
    s.note_on(0, 60, 127);
    s.note_on(256, 60, 127);
    let out = run(&mut s, 4800);
    assert!(peak(&out[0][2400..]) < 0.26, "voices did not stack: {}", peak(&out[0][2400..]));
    // One-shots overlap.
    s.set_param("play_mode", 1.0);
    s.reset();
    s.note_on(0, 60, 127);
    s.note_on(256, 60, 127);
    let out = run(&mut s, 4800);
    assert!((peak(&out[0][2400..]) - 0.5).abs() < 1e-3);
}

#[test]
fn forward_loop_sustains_past_the_end() {
    let data = sine(480.0, SR, 4800);
    let mut s = sampler(mono(SR, data.clone()));
    s.set_param("velocity", 0.0);
    s.set_param("loop_mode", 1.0);
    s.set_param("loop_start", 50.0);
    s.note_on(0, 60, 127);
    let out = run(&mut s, 48_000);
    assert!(peak(&out[0][40_000..]) > 0.4, "still sounding long after the 0.1 s sample");
    // After the first pass it repeats the loop region (2400 frames, whole cycles of 480 Hz).
    for i in 0..2400 {
        assert!((out[0][4800 + i] - data[2400 + i]).abs() < 1e-4, "frame {i}");
    }
    assert!((freq(&out[0][4800..], SR) - 480.0).abs() < 1.0);
    // Without the loop the note ends with the sample.
    s.set_param("loop_mode", 0.0);
    s.reset();
    s.note_on(0, 60, 127);
    assert_eq!(peak(&run(&mut s, 9600)[0][4900..]), 0.0);
}

#[test]
fn ping_pong_loop_reverses_direction() {
    // A rising ramp, looped ping-pong over the whole sample, rises then falls.
    let data: Vec<f32> = (0..1000).map(|i| i as f32 / 1000.0).collect();
    let mut s = sampler(mono(SR, data));
    s.set_param("velocity", 0.0);
    s.set_param("loop_mode", 2.0);
    s.note_on(0, 60, 127);
    let out = run(&mut s, 3000);
    assert!(out[0][900] > out[0][500], "rising");
    assert!(out[0][1500] < out[0][1100], "falling after the loop end");
    assert!(out[0][2500] > out[0][2100], "rising again after the loop start");
    assert!(out[0].iter().all(|x| x.is_finite() && *x <= 1.0));
}

/// Where a ping-pong voice is after `n` steps, by walking a triangle wave over the loop.
fn triangle(start: f64, forward: bool, step: f64, n: u32, ls: f64, le: f64) -> (f64, bool) {
    let width = le - ls;
    // Unfold the walk onto one line, then fold it back: phase 0..width rising, width..2 width falling.
    let signed = if forward { start - ls } else { (le - start) + width };
    let t = (signed + f64::from(n) * step).rem_euclid(2.0 * width);
    if t < width { (ls + t, true) } else { (le - (t - width), false) }
}

#[test]
fn a_ping_pong_step_wider_than_the_loop_keeps_the_phase() {
    let (ls, le) = (100.0, 110.0);
    // Steps below, equal to and above the loop's width, with fractions, both ways round.
    for step in [0.37, 3.0, 9.99, 10.0, 10.5, 23.7, 25.0, 31.3, 99.9] {
        for forward in [true, false] {
            let mut v = Voice::new();
            v.pos = if forward { 102.5 } else { 107.25 };
            v.forward = forward;
            v.step = step;
            let start = v.pos;
            for n in 1..=40u32 {
                advance(&mut v, 1000.0, Some((ls, le)), LoopMode::PingPong);
                let (want, want_forward) = triangle(start, forward, step, n, ls, le);
                assert!(v.pos >= ls - 1e-9 && v.pos <= le + 1e-9, "step {step} n {n}: {} left the loop", v.pos);
                // At an exact end the direction is where the position is heading next.
                let at_end = (want - ls).abs() < 1e-6 || (want - le).abs() < 1e-6;
                assert!((v.pos - want).abs() < 1e-6, "step {step} {forward} n {n}: at {} want {want}", v.pos);
                assert!(at_end || v.forward == want_forward, "step {step} {forward} n {n}: direction");
            }
        }
    }
}

#[test]
fn a_ping_pong_step_below_the_loop_width_moves_as_before() {
    let mut v = Voice::new();
    v.pos = 108.0;
    v.step = 5.0;
    advance(&mut v, 1000.0, Some((100.0, 110.0)), LoopMode::PingPong);
    assert_eq!((v.pos, v.forward), (107.0, false), "one reflection off the end");
    advance(&mut v, 1000.0, Some((100.0, 110.0)), LoopMode::PingPong);
    assert_eq!((v.pos, v.forward), (102.0, false));
    advance(&mut v, 1000.0, Some((100.0, 110.0)), LoopMode::PingPong);
    assert_eq!((v.pos, v.forward), (103.0, true), "and one off the start");
}

#[test]
fn a_loop_too_short_to_loop_plays_once() {
    let mut s = sampler(mono(SR, vec![0.5; 1000]));
    s.set_param("loop_mode", 1.0);
    s.set_param("loop_start", 40.0);
    s.set_param("loop_end", 40.0);
    s.note_on(0, 60, 127);
    assert_eq!(peak(&run(&mut s, 4800)[0][1100..]), 0.0);
}

#[test]
fn sample_start_skips_into_the_sample() {
    let data: Vec<f32> = (0..1000).map(|i| i as f32 / 1000.0).collect();
    let mut s = sampler(mono(SR, data));
    s.set_param("velocity", 0.0);
    s.set_param("attack", 0.1);
    s.set_param("start", 50.0);
    s.note_on(0, 60, 127);
    let out = run(&mut s, 2000);
    assert!((out[0][10] - 0.509).abs() < 0.002, "{}", out[0][10]);
    assert_eq!(peak(&out[0][520..]), 0.0, "half the sample left");
}

#[test]
fn stereo_samples_keep_their_sides() {
    let sample = SampleBuffer { sample_rate: SR, channels: vec![vec![0.5; 4800], vec![-0.25; 4800]] };
    let mut s = sampler(sample);
    s.set_param("velocity", 0.0);
    s.note_on(0, 60, 127);
    let out = run(&mut s, 2400);
    assert!((out[0][1000] - 0.5).abs() < 1e-6);
    assert!((out[1][1000] + 0.25).abs() < 1e-6);
}

#[test]
fn note_offsets_are_sample_accurate() {
    let mut s = sampler(mono(SR, vec![0.5; 4800]));
    s.note_on(300, 60, 127);
    let out = run(&mut s, 512);
    assert!(out[0][..300].iter().all(|&v| v == 0.0));
    assert!(out[0][301..].iter().any(|&v| v != 0.0));
}

#[test]
fn many_notes_steal_voices_and_stay_bounded() {
    let mut s = sampler(mono(SR, sine(220.0, SR, 48_000)));
    s.set_param("level", 12.0);
    for n in 0..100u8 {
        s.note_on(usize::from(n), n, 127);
    }
    let out = run(&mut s, 9600);
    assert!(out[0].iter().all(|v| v.is_finite()));
    assert!(s.voices.iter().filter(|v| v.active()).count() <= VOICES);
}

#[test]
fn a_new_sample_stops_the_old_voices() {
    let mut s = sampler(mono(SR, vec![0.5; 48_000]));
    s.note_on(0, 60, 127);
    run(&mut s, 512);
    assert!(s.set_sample(Some(Arc::new(mono(SR, vec![0.1; 10])))));
    assert_eq!(peak(&run(&mut s, 512)[0]), 0.0);
    assert!(s.set_sample(None));
    s.note_on(0, 60, 127);
    assert_eq!(peak(&run(&mut s, 512)[0]), 0.0);
}

#[test]
fn hostile_samples_and_params_never_panic() {
    let samples = [
        SampleBuffer { sample_rate: f32::NAN, channels: vec![vec![0.5; 100]] },
        SampleBuffer { sample_rate: 0.0, channels: vec![vec![0.5; 100]] },
        SampleBuffer { sample_rate: f32::INFINITY, channels: vec![vec![0.5; 100]] },
        SampleBuffer { sample_rate: 1.0e-30, channels: vec![vec![0.5; 100]] },
        SampleBuffer { sample_rate: SR, channels: vec![] },
        SampleBuffer { sample_rate: SR, channels: vec![vec![]] },
        SampleBuffer { sample_rate: SR, channels: vec![vec![0.5; 100], vec![0.5; 3]] },
        SampleBuffer { sample_rate: SR, channels: vec![vec![0.5; 1]] },
        SampleBuffer { sample_rate: SR, channels: vec![vec![f32::NAN; 100]; 9] },
    ];
    let params: Vec<(&str, f32)> = SAMPLER_PARAMS.iter().flat_map(|p| [(p.id, f32::NAN), (p.id, f32::INFINITY), (p.id, -1.0e30)]).collect();
    for sample in samples {
        let mut s = Sampler::new();
        s.prepare(f32::NAN, 0, 99);
        s.set_sample(Some(Arc::new(sample)));
        for (id, v) in &params {
            assert!(s.set_param(id, *v));
            for n in [0u8, 60, 127, 255] {
                s.note_on(0, n, 255);
                s.note_on(7, n, 1);
            }
            let mut io = vec![vec![0.0f32; 64]; 3];
            s.process(&mut io, 1000);
            assert!(io.iter().flatten().all(|v| v.is_finite()), "{id} = {v}");
            s.process(&mut [], 64);
            let _ = s.tail_samples();
        }
    }
    // Loops with every mode at the extremes of pitch.
    let mut s = sampler(mono(SR, sine(100.0, SR, 1000)));
    for mode in [0.0, 1.0, 2.0] {
        s.set_param("loop_mode", mode);
        for (t, root) in [(48.0, 0.0), (-48.0, 127.0)] {
            s.set_param("transpose", t);
            s.set_param("root_note", root);
            s.note_on(0, 127, 127);
            s.note_on(0, 0, 127);
            let out = run(&mut s, 4096);
            assert!(out[0].iter().all(|v| v.is_finite() && v.abs() < 4.0), "mode {mode} transpose {t}");
        }
    }
}
