use std::collections::HashSet;
use std::f32::consts::TAU;

use soundcraft_dsp::{Plugin, Unit, create, db_to_gain, gain_to_db, is_generator, plugin_info, plugins};

const SR: f32 = 48_000.0;
const BLOCK: usize = 512;

fn run(p: &mut dyn Plugin, input: &[Vec<f32>]) -> Vec<Vec<f32>> {
    let len = input[0].len();
    let ch = input.len();
    let mut out = vec![Vec::with_capacity(len); ch];
    let mut pos = 0;
    let mut buf = vec![vec![0.0f32; BLOCK]; ch];
    while pos < len {
        let n = BLOCK.min(len - pos);
        for c in 0..ch {
            buf[c][..n].copy_from_slice(&input[c][pos..pos + n]);
        }
        p.process(&mut buf, n);
        for c in 0..ch {
            out[c].extend_from_slice(&buf[c][..n]);
        }
        pos += n;
    }
    out
}

fn sine(freq: f32, amp: f32, len: usize) -> Vec<f32> {
    (0..len).map(|i| amp * (TAU * freq * i as f32 / SR).sin()).collect()
}

fn peak(x: &[f32]) -> f32 {
    x.iter().fold(0.0f32, |m, v| m.max(v.abs()))
}

fn rms(x: &[f32]) -> f32 {
    (x.iter().map(|v| v * v).sum::<f32>() / x.len().max(1) as f32).sqrt()
}

fn prepared(id: &str) -> Box<dyn Plugin> {
    let mut p = create(id).unwrap();
    p.prepare(SR, BLOCK, 2);
    p
}

fn is_source(id: &str) -> bool {
    plugin_info(id).unwrap().is_instrument || is_generator(id)
}

#[test]
fn registry_ids_unique_and_creatable() {
    let mut ids = HashSet::new();
    for info in plugins() {
        assert!(ids.insert(info.id), "duplicate id {}", info.id);
        assert!(info.short_name.chars().count() <= 8, "{} short name too long", info.id);
        let p = create(info.id).unwrap_or_else(|| panic!("cannot create {}", info.id));
        assert_eq!(p.info().id, info.id);
        assert_eq!(plugin_info(info.id).unwrap().id, info.id);
    }
    assert!(create("nope").is_none());
    assert!(plugins().len() >= 25);
}

#[test]
fn params_well_formed() {
    for info in plugins() {
        let mut ids = HashSet::new();
        for p in info.params {
            assert!(ids.insert(p.id), "{}: duplicate param {}", info.id, p.id);
            assert!(p.min <= p.default && p.default <= p.max, "{}.{} default out of range", info.id, p.id);
            assert!(p.min.is_finite() && p.max.is_finite());
            if p.unit == Unit::Choice {
                assert_eq!(p.choices.len() as f32, p.max + 1.0, "{}.{}", info.id, p.id);
            }
            let n = p.to_normalized(p.default);
            assert!((p.from_normalized(n) - p.default).abs() <= 1e-3 * (1.0 + p.default.abs()), "{}.{}", info.id, p.id);
        }
        let mut plug = create(info.id).unwrap();
        for p in info.params {
            assert_eq!(plug.param(p.id), Some(p.default), "{}.{}", info.id, p.id);
            assert!(plug.set_param(p.id, p.max + 1000.0));
            assert_eq!(plug.param(p.id), Some(p.max));
        }
        assert!(!plug.set_param("definitely_not_a_param", 1.0));
        assert_eq!(plug.param("definitely_not_a_param"), None);
    }
}

#[test]
fn silence_in_silence_out() {
    for info in plugins() {
        if is_source(info.id) {
            continue;
        }
        let mut p = prepared(info.id);
        let out = run(p.as_mut(), &vec![vec![0.0; 48_000]; 2]);
        for c in &out {
            assert!(peak(c) < 1e-6, "{} made noise from silence: {}", info.id, peak(c));
        }
    }
}

#[test]
fn impulse_response_finite_and_bounded() {
    for info in plugins() {
        let mut p = prepared(info.id);
        let mut x = vec![vec![0.0; 96_000]; 2];
        x[0][100] = 1.0;
        x[1][100] = 1.0;
        p.note_on(0, 36, 100);
        let out = run(p.as_mut(), &x);
        for c in &out {
            assert!(c.iter().all(|v| v.is_finite()), "{} produced non-finite output", info.id);
            assert!(peak(c) < 8.0, "{} impulse peak {}", info.id, peak(c));
        }
    }
}

#[test]
fn hostile_params_never_break_processing() {
    let hostile = [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, -1e30, 1e30, 0.0, -0.0];
    for info in plugins() {
        for &v in &hostile {
            let mut p = prepared(info.id);
            for param in info.params {
                assert!(p.set_param(param.id, v));
                let got = p.param(param.id).unwrap();
                assert!(got.is_finite() && got >= param.min && got <= param.max, "{}.{} = {got}", info.id, param.id);
            }
            p.note_on(3, 60, 127);
            let x = vec![sine(440.0, 0.5, 9600), sine(660.0, 0.5, 9600)];
            let out = run(p.as_mut(), &x);
            assert!(out.iter().flatten().all(|s| s.is_finite()), "{} non-finite with param value {v}", info.id);
            let _ = p.latency();
            let _ = p.tail_samples();
            assert!(p.gain_reduction_db().is_finite());
        }
    }
}

#[test]
fn odd_host_calls_do_not_panic() {
    for info in plugins() {
        let mut p = create(info.id).unwrap();
        // Unprepared-ish, weird rates, block bigger than prepared, mismatched channels.
        for (sr, block, ch) in [(0.0, 0, 0), (f32::NAN, 16, 1), (-44_100.0, 64, 2), (1e9, 64, 8), (44_100.0, 32, 2)] {
            p.prepare(sr, block, ch);
            let mut io = vec![vec![0.25f32; 300]; 3];
            p.process(&mut io, 4096);
            p.process(&mut io, 0);
            p.process(&mut [], 128);
            let mut short = vec![vec![0.1f32; 10], vec![0.1f32; 3]];
            p.process(&mut short, 100);
            p.note_on(10_000, 200, 255);
            p.note_off(0, 200);
            p.all_notes_off();
            p.process(&mut io, 300);
            p.reset();
        }
    }
}

#[test]
fn param_changes_are_smooth() {
    // Jumping gain by 24 dB mid-stream must not produce a step discontinuity.
    let mut p = prepared("gain");
    let x = vec![vec![0.5f32; 4096]; 1];
    let _ = run(p.as_mut(), &x);
    p.set_param("gain", 24.0);
    let out = run(p.as_mut(), &x);
    let max_step = out[0].windows(2).map(|w| (w[1] - w[0]).abs()).fold(0.0f32, f32::max);
    assert!(max_step < 0.05, "zipper step {max_step}");
    let out = run(p.as_mut(), &x);
    assert!((out[0][4095] - 0.5 * db_to_gain(24.0)).abs() < 0.01);
}

#[test]
fn eq_magnitude_at_center_matches_gain() {
    let sr = SR;
    let resp = soundcraft_dsp::eq7_response(&[("mid_freq", 1000.0), ("mid_gain", 9.0), ("mid_q", 2.0)], &[1000.0, 30.0, 15_000.0], sr);
    assert!((resp[0] - 9.0).abs() < 0.05, "{resp:?}");
    assert!(resp[1].abs() < 0.2 && resp[2].abs() < 0.2, "{resp:?}");
    let r1 = soundcraft_dsp::eq1_response(&[("freq", 2500.0), ("gain", -12.0)], &[2500.0], sr);
    assert!((r1[0] + 12.0).abs() < 0.05);

    // And the real-time EQ agrees with the curve.
    let mut p = prepared("eq_7band");
    p.set_param("mid_freq", 1000.0);
    p.set_param("mid_gain", 9.0);
    p.set_param("mid_q", 2.0);
    p.reset();
    let out = run(p.as_mut(), &[sine(1000.0, 0.1, 48_000)]);
    let g = gain_to_db(rms(&out[0][24_000..]) / rms(&sine(1000.0, 0.1, 48_000)[24_000..]));
    assert!((g - 9.0).abs() < 0.2, "measured {g}");

    // HPF attenuates lows.
    let r = soundcraft_dsp::eq7_response(&[("hpf_on", 1.0), ("hpf_freq", 200.0), ("hpf_slope", 3.0)], &[50.0, 200.0, 2000.0], sr);
    assert!(r[0] < -40.0 && (r[1] + 3.0).abs() < 0.2 && r[2].abs() < 0.1, "{r:?}");
}

#[test]
fn eq_switches_and_choices_apply_to_a_running_eq() {
    // (plugin, settings, the one switch or choice changed while running, test tone)
    let cases: [(&str, &[(&str, f32)], (&str, f32), f32); 7] = [
        ("eq_7band", &[("hpf_freq", 2000.0), ("hpf_slope", 3.0)], ("hpf_on", 1.0), 1000.0),
        ("eq_7band", &[("hpf_freq", 2000.0), ("hpf_slope", 3.0), ("hpf_on", 1.0)], ("hpf_on", 0.0), 1000.0),
        ("eq_7band", &[("hpf_freq", 2000.0), ("hpf_slope", 0.0), ("hpf_on", 1.0)], ("hpf_slope", 3.0), 1000.0),
        ("eq_7band", &[("lpf_freq", 2000.0), ("lpf_slope", 3.0)], ("lpf_on", 1.0), 4000.0),
        ("eq_7band", &[("lpf_freq", 2000.0), ("lpf_slope", 0.0), ("lpf_on", 1.0)], ("lpf_slope", 3.0), 4000.0),
        ("eq_1band", &[("freq", 1000.0)], ("type", 4.0), 10_000.0),
        ("channel_strip", &[("hpf_freq", 2000.0)], ("hpf_on", 1.0), 1000.0),
    ];
    let mut stale = Vec::new();
    for (id, setup, (key, value), tone) in cases {
        let x = [sine(tone, 0.5, 24_000)];
        let level = |p: &mut dyn Plugin| gain_to_db(rms(&run(p, &x)[0][12_000..]));
        let build = |settings: &[(&str, f32)]| {
            let mut p = create(id).unwrap();
            p.prepare(SR, BLOCK, 1);
            for &(k, v) in settings {
                assert!(p.set_param(k, v), "{id} {k}");
            }
            p.reset();
            p
        };
        let mut live = build(setup);
        let before = level(live.as_mut());
        assert!(live.set_param(key, value));
        let after = level(live.as_mut());
        let want = level(build(&[setup, &[(key, value)]].concat()).as_mut());
        assert!((before - want).abs() > 3.0, "{id} {key}={value}: the tone must tell the settings apart");
        if (after - want).abs() >= 0.1 {
            stale.push(format!("{id} {key}={value}: live {after:.2} dB, fresh {want:.2} dB, before {before:.2} dB"));
        }
    }
    assert!(stale.is_empty(), "{stale:#?}");
}

#[test]
fn compressor_steady_state_gain_reduction() {
    let mut p = prepared("compressor");
    p.set_param("threshold", -20.0);
    p.set_param("ratio", 4.0);
    p.set_param("knee", 0.0);
    p.set_param("attack", 1.0);
    p.set_param("release", 50.0);
    // Constant level at -10 dBFS: 10 dB over → 7.5 dB of reduction.
    let level = db_to_gain(-10.0);
    let out = run(p.as_mut(), &vec![vec![level; 48_000]; 2]);
    let got = gain_to_db(out[0][47_999]) - -10.0;
    assert!((got + 7.5).abs() < 0.1, "gain change {got}");
    assert!((p.gain_reduction_db() - 7.5).abs() < 0.1);

    // A sine settles near the same reduction (peak detection).
    p.reset();
    let x = sine(1000.0, level, 48_000);
    let out = run(p.as_mut(), &[x.clone(), x]);
    let got = gain_to_db(peak(&out[0][40_000..])) + 10.0;
    assert!((got + 7.5).abs() < 1.0, "sine gain change {got}");

    // Limiter mode with lookahead reports latency.
    p.set_param("limiter", 1.0);
    p.set_param("lookahead", 5.0);
    assert_eq!(p.latency(), 240);
}

#[test]
fn gate_closes_below_threshold() {
    let mut p = prepared("expander_gate");
    p.set_param("threshold", -40.0);
    p.set_param("range", 80.0);
    p.set_param("hold", 10.0);
    p.set_param("release", 20.0);
    let quiet = sine(300.0, db_to_gain(-60.0), 48_000);
    let out = run(p.as_mut(), std::slice::from_ref(&quiet));
    let atten = gain_to_db(peak(&out[0][24_000..]) / peak(&quiet[24_000..]));
    assert!(atten < -70.0, "gate attenuation only {atten}");
    let loud = sine(300.0, db_to_gain(-10.0), 48_000);
    let out = run(p.as_mut(), std::slice::from_ref(&loud));
    let g = gain_to_db(peak(&out[0][24_000..]) / peak(&loud[24_000..]));
    assert!(g.abs() < 0.5, "open gate changed level {g}");
}

#[test]
fn maximizer_respects_ceiling_and_reports_latency() {
    let mut p = prepared("maximizer");
    p.set_param("threshold", -12.0);
    p.set_param("ceiling", -1.0);
    let x = sine(220.0, 0.9, 48_000);
    let out = run(p.as_mut(), &[x.clone(), x]);
    assert!(peak(&out[0]) <= db_to_gain(-1.0) + 1e-5, "{}", peak(&out[0]));
    assert!(p.latency() > 0 && p.latency() < 200);
    assert!(p.gain_reduction_db() > 5.0);
}

#[test]
fn de_esser_reduces_sibilance_only() {
    let mut p = prepared("de_esser");
    p.set_param("threshold", -40.0);
    p.set_param("range", 20.0);
    let hi = sine(8000.0, 0.5, 48_000);
    let out = run(p.as_mut(), std::slice::from_ref(&hi));
    assert!(gain_to_db(rms(&out[0][24_000..]) / rms(&hi[24_000..])) < -6.0);
    p.reset();
    let lo = sine(200.0, 0.005, 48_000);
    let out = run(p.as_mut(), std::slice::from_ref(&lo));
    assert!(gain_to_db(rms(&out[0][24_000..]) / rms(&lo[24_000..])).abs() < 0.5);
}

#[test]
fn latency_reporting_aligns_pitch_shifter() {
    let mut p = prepared("pitch_shifter");
    let lat = p.latency();
    assert!(lat > 0);
    let mut x = vec![0.0f32; 8000];
    x[1000] = 1.0;
    let out = run(p.as_mut(), &[x]);
    let pos = out[0].iter().enumerate().max_by(|a, b| a.1.abs().total_cmp(&b.1.abs())).unwrap().0;
    assert_eq!(pos, 1000 + lat);
}

#[test]
fn pitch_shifter_moves_pitch() {
    let mut p = prepared("pitch_shifter");
    p.set_param("semitones", 12.0);
    p.reset();
    let out = run(p.as_mut(), &[sine(440.0, 0.5, 48_000)]);
    let spec = soundcraft_dsp::spectrum::magnitude_db(&out[0][16_384..], 16_384);
    let bin = spec.iter().enumerate().max_by(|a, b| a.1.total_cmp(b.1)).unwrap().0;
    let hz = soundcraft_dsp::spectrum::bin_hz(bin, 16_384, SR);
    assert!((hz - 880.0).abs() < 15.0, "peak at {hz}");
}

#[test]
fn reverb_has_tail_and_decays() {
    for id in ["room_reverb", "plate_reverb"] {
        let mut p = prepared(id);
        p.set_param("mix", 100.0);
        p.set_param("decay", 1.0);
        let mut x = vec![vec![0.0; 48_000 * 4]; 2];
        x[0][0] = 1.0;
        x[1][0] = 1.0;
        let out = run(p.as_mut(), &x);
        let early = rms(&out[0][2400..24_000]);
        let late = rms(&out[0][48_000 * 3..]);
        assert!(early > 1e-3, "{id} no reverb");
        assert!(late < early * 1e-2, "{id} not decaying {early} {late}");
        assert!(p.tail_samples() > 48_000);
        // Stereo decorrelation.
        assert!(out[0][2400..4800] != out[1][2400..4800]);
    }
}

#[test]
fn delay_repeats_at_time() {
    let mut p = prepared("mod_delay");
    p.set_param("time", 100.0);
    p.set_param("mix", 100.0);
    p.set_param("feedback", 0.0);
    p.set_param("lpf", 20_000.0);
    p.reset();
    let mut x = vec![0.0f32; 20_000];
    x[0] = 1.0;
    let out = run(p.as_mut(), &[x]);
    let pos = out[0].iter().enumerate().max_by(|a, b| a.1.abs().total_cmp(&b.1.abs())).unwrap().0;
    assert!((pos as i64 - 4800).abs() <= 2, "echo at {pos}");
}

#[test]
fn modulation_tail_covers_feedback_echoes() {
    for (id, delay, feedback) in [("chorus", 30.0, 90.0), ("flanger", 10.0, 95.0), ("flanger", 10.0, -95.0)] {
        let mut p = prepared(id);
        for (param, v) in [("delay", delay), ("feedback", feedback), ("depth", 0.0), ("mix", 100.0)] {
            assert!(p.set_param(param, v));
        }
        p.reset();
        let mut x = vec![0.0f32; 48_000 * 5];
        x[0] = 0.5;
        let out = run(p.as_mut(), &[x]);
        let floor = peak(&out[0]) * 1e-3;
        let last = out[0].iter().rposition(|v| v.abs() > floor).unwrap();
        assert!(last < p.tail_samples(), "{id} feedback {feedback}: echo at {last}, tail {}", p.tail_samples());
    }
}

#[test]
fn trim_and_invert() {
    let mut p = prepared("invert");
    let out = run(p.as_mut(), &[vec![0.5; 16]]);
    assert!(out[0].iter().all(|&v| v == -0.5));
    let mut t = prepared("trim");
    t.set_param("gain", -96.0);
    t.reset();
    let out = run(t.as_mut(), &[vec![0.5; 16]]);
    assert!(out[0].iter().all(|&v| v == 0.0));
    t.set_param("gain", 0.0);
    t.set_param("invert", 1.0);
    t.reset();
    let out = run(t.as_mut(), &[vec![0.5; 16]]);
    assert!(out[0].iter().all(|&v| (v + 0.5).abs() < 1e-6));
}

#[test]
fn time_shift_delays() {
    let mut p = prepared("time_shift");
    p.set_param("delay_samples", 123.0);
    p.reset();
    let mut x = vec![0.0f32; 1000];
    x[10] = 1.0;
    let out = run(p.as_mut(), &[x]);
    assert_eq!(out[0][133], 1.0);
}

#[test]
fn dc_removal_removes_dc() {
    let mut p = prepared("dc_offset_removal");
    let out = run(p.as_mut(), &[vec![0.3; 96_000]]);
    assert!(out[0][95_999].abs() < 1e-3);
}

#[test]
fn signal_generator_level() {
    let mut p = prepared("signal_generator");
    p.set_param("level", -6.0);
    p.reset();
    let out = run(p.as_mut(), &[vec![0.0; 48_000]]);
    assert!((gain_to_db(peak(&out[0])) + 6.0).abs() < 0.05);
}

#[test]
fn synth_note_on_sounds_and_note_off_decays() {
    let mut p = prepared("subtractive_synth");
    p.note_on(0, 60, 100);
    let out = run(p.as_mut(), &vec![vec![0.0; 24_000]; 2]);
    assert!(rms(&out[0][4800..]) > 0.01, "synth silent");
    assert_eq!(out[0], out[1]);
    p.note_off(0, 60);
    let out = run(p.as_mut(), &vec![vec![0.0; 96_000]; 2]);
    assert!(peak(&out[0][72_000..]) < 1e-6, "synth did not decay");
    // Polyphony: a chord is louder than one note and stays finite.
    for n in [60, 64, 67, 71, 74, 77, 81, 84, 60, 62, 65, 69, 72, 76, 79, 83, 86, 88] {
        p.note_on(0, n, 127);
    }
    let out = run(p.as_mut(), &vec![vec![0.0; 9600]; 2]);
    assert!(out[0].iter().all(|v| v.is_finite()) && peak(&out[0]) > 0.05);
    p.all_notes_off();
    let out = run(p.as_mut(), &vec![vec![0.0; 96_000]; 2]);
    assert!(peak(&out[0][72_000..]) < 1e-6);
}

#[test]
fn synth_follows_the_sustain_pedal_and_pitch_bend() {
    let mut p = prepared("subtractive_synth");
    p.midi(0, [0xB0, 64, 127]);
    p.note_on(0, 60, 100);
    run(p.as_mut(), &vec![vec![0.0; 4800]; 2]);
    p.note_off(0, 60);
    let out = run(p.as_mut(), &vec![vec![0.0; 96_000]; 2]);
    assert!(rms(&out[0][72_000..]) > 0.01, "the pedal holds the note");
    p.midi(0, [0xB0, 64, 0]);
    let out = run(p.as_mut(), &vec![vec![0.0; 96_000]; 2]);
    assert!(peak(&out[0][72_000..]) < 1e-6, "lifting the pedal releases it");

    let rises = |x: &[f32]| x.windows(2).filter(|w| w[0] < 0.0 && w[1] >= 0.0).count() as f64;
    let mut p = prepared("subtractive_synth");
    p.set_param("osc1_wave", 2.0);
    p.set_param("osc_mix", 0.0);
    p.note_on(0, 69, 100);
    let plain = run(p.as_mut(), &[vec![0.0; 48_000]]);
    p.midi(0, [0xE0, 0x7F, 0x7F]);
    let bent = run(p.as_mut(), &[vec![0.0; 48_000]]);
    let ratio = rises(&bent[0][4800..]) / rises(&plain[0][4800..]);
    assert!((ratio - 2f64.powf(2.0 / 12.0)).abs() < 0.01, "{ratio}");
    p.midi(0, [0xB0, 121, 0]);
    let reset = run(p.as_mut(), &[vec![0.0; 48_000]]);
    assert!((rises(&reset[0][4800..]) / rises(&plain[0][4800..]) - 1.0).abs() < 0.01);
}

#[test]
fn synth_event_offsets_are_sample_accurate() {
    let mut p = prepared("subtractive_synth");
    p.set_param("amp_attack", 0.5);
    p.note_on(300, 69, 127);
    let out = run(p.as_mut(), &[vec![0.0; 512]]);
    assert!(out[0][..300].iter().all(|&v| v == 0.0));
    assert!(out[0][300..].iter().any(|&v| v != 0.0));
}

#[test]
fn drum_kit_notes_sound_and_end() {
    for note in [36, 38, 39, 42, 46, 45, 47, 50, 49, 51] {
        let mut p = prepared("drum_synth");
        p.note_on(0, note, 120);
        let out = run(p.as_mut(), &[vec![0.0; 48_000 * 7]]);
        assert!(peak(&out[0][..4800]) > 0.01, "drum note {note} silent");
        assert!(peak(&out[0][48_000 * 6..]) < 1e-6, "drum note {note} rings forever");
    }
    let mut p = prepared("drum_synth");
    p.note_on(0, 99, 120);
    let out = run(p.as_mut(), &[vec![0.0; 4800]]);
    assert!(peak(&out[0]) == 0.0, "unmapped note should be silent");
}

#[test]
fn saturator_adds_harmonics_and_stays_bounded() {
    let mut p = prepared("saturator");
    p.set_param("drive", 30.0);
    let out = run(p.as_mut(), &[sine(1000.0, 1.0, 48_000)]);
    assert!(peak(&out[0]) <= 1.01);
    let spec = soundcraft_dsp::spectrum::magnitude_db(&out[0][8192..], 8192);
    let third = (3000.0 / (SR / 8192.0)) as usize;
    assert!(spec[third - 1..=third + 1].iter().cloned().fold(f32::MIN, f32::max) > -30.0);
}

/// Amplitude of the `freq` component of `x` (a whole number of cycles keeps it exact).
fn tone_level(x: &[f32], freq: f32) -> f32 {
    let (mut re, mut im) = (0.0f64, 0.0f64);
    for (i, v) in x.iter().enumerate() {
        let w = f64::from(TAU * freq / SR) * i as f64;
        re += f64::from(*v) * w.cos();
        im += f64::from(*v) * w.sin();
    }
    (2.0 * (re * re + im * im).sqrt() / x.len() as f64) as f32
}

/// A Subharmonic with only the wet path and wide filters, so a band's octave reaches the output.
fn subharmonic_wet() -> Box<dyn Plugin> {
    let mut p = prepared("subharmonic");
    p.set_param("mix", 100.0);
    p.set_param("hp_freq", 5.0);
    p.set_param("lp_freq", 1000.0);
    p
}

#[test]
fn subharmonic_adds_the_octave_below_each_range_band() {
    for (range, lower, upper) in [(0.0, 120.0, 180.0), (1.0, 80.0, 120.0), (2.0, 60.0, 90.0), (3.0, 40.0, 60.0)] {
        for (band, freq) in [("lower", lower), ("upper", upper)] {
            let mut p = subharmonic_wet();
            p.set_param("range", range);
            p.set_param("lower_gain", if band == "lower" { 0.0 } else { -120.0 });
            p.set_param("upper_gain", if band == "upper" { 0.0 } else { -120.0 });
            let out = run(p.as_mut(), &[sine(freq, 0.5, 96_000)]);
            // One second after the filters settle: a whole number of cycles of both tones.
            let tail = &out[0][48_000..];
            let sub = tone_level(tail, freq / 2.0);
            assert!(sub > 0.2, "range {range} {band} band: {sub} at {} Hz", freq / 2.0);
            assert!(sub > 4.0 * tone_level(tail, freq), "range {range} {band} band keeps its input tone");
        }
    }
}

#[test]
fn subharmonic_mix_keeps_one_path_at_full_level() {
    // A 1 kHz tone is far above every band, so the output is the dry path alone.
    for (mix, dry) in [(0.0, 1.0), (25.0, 1.0), (50.0, 1.0), (75.0, 0.5), (100.0, 0.0)] {
        let mut p = prepared("subharmonic");
        p.set_param("mix", mix);
        let out = run(p.as_mut(), &[sine(1000.0, 0.5, 48_000)]);
        let level = tone_level(&out[0][24_000..], 1000.0);
        assert!((level - 0.5 * dry).abs() < 0.01, "mix {mix}: dry level {level}");
    }
    // The wet path is at full level from 50 % up and at half level at 25 %.
    let wet = |mix: f32| {
        let mut p = prepared("subharmonic");
        p.set_param("mix", mix);
        p.set_param("wet_solo", 1.0);
        let out = run(p.as_mut(), &[sine(60.0, 0.5, 96_000)]);
        tone_level(&out[0][48_000..], 30.0)
    };
    let full = wet(50.0);
    assert!(full > 0.1, "wet level {full}");
    assert!((wet(100.0) - full).abs() < 0.01 * full);
    assert!((wet(25.0) - 0.5 * full).abs() < 0.01 * full);
}

#[test]
fn subharmonic_solo_plays_one_band_and_mutes_the_dry_signal() {
    let level = |solo: &str, gain: f32| {
        let mut p = prepared("subharmonic");
        p.set_param("lower_gain", gain);
        p.set_param(solo, 1.0);
        let x = sine(60.0, 0.5, 96_000).iter().zip(sine(1000.0, 0.5, 96_000)).map(|(a, b)| a + b).collect();
        let out = run(p.as_mut(), &[x]);
        let tail = &out[0][48_000..];
        (tone_level(tail, 30.0), tone_level(tail, 1000.0))
    };
    let (sub, dry) = level("lower_solo", 0.0);
    assert!(sub > 0.1 && dry < 0.01, "lower solo: sub {sub}, dry {dry}");
    // With the soloed band turned off nothing else plays, though the upper band is on.
    let (sub, dry) = level("lower_solo", -120.0);
    assert!(sub < 0.01 && dry < 0.01, "silent lower solo: sub {sub}, dry {dry}");
    let (sub, dry) = level("upper_solo", -120.0);
    assert!(sub > 0.05 && dry < 0.01, "upper solo: sub {sub}, dry {dry}");
}

#[test]
fn subharmonic_drive_keeps_every_character_bounded() {
    let driven = |character: usize, q: f32| {
        let mut p = subharmonic_wet();
        p.set_param("input", 18.0);
        p.set_param("lower_gain", 24.0);
        p.set_param("upper_gain", 24.0);
        p.set_param("drive", 18.0);
        p.set_param("character", character as f32);
        p.set_param("hp_q", q);
        p.set_param("lp_q", q);
        run(p.as_mut(), &[sine(60.0, 1.0, 48_000)])
    };
    for character in 0..6 {
        let out = driven(character, 0.707);
        let top = peak(&out[0][24_000..]);
        assert!(top > 0.5 && top < 1.5, "character {character}: peak {top}");
        // Q 5 filters after the saturator ring well above full scale, but never to infinity.
        let out = driven(character, 5.0);
        assert!(out[0].iter().all(|v| v.is_finite()), "character {character} at Q 5");
        assert!(peak(&out[0][24_000..]) > 0.5, "character {character} at Q 5 is silent");
    }
}

#[test]
fn lofi_quantizes() {
    let mut p = prepared("lofi");
    p.set_param("bits", 2.0);
    p.set_param("sample_rate", 48_000.0);
    let out = run(p.as_mut(), &[sine(100.0, 0.9, 4800)]);
    let mut levels: Vec<i32> = out[0].iter().map(|v| (v * 1000.0).round() as i32).collect();
    levels.sort();
    levels.dedup();
    assert!(levels.len() <= 5, "{levels:?}");
}

fn ring_mod(params: &[(&str, f32)]) -> Box<dyn Plugin> {
    let mut p = prepared("ring_mod");
    for &(id, v) in params {
        assert!(p.set_param(id, v), "ring_mod.{id}");
    }
    p.reset();
    p
}

/// Loudest bin within two bins of `hz` in an 8192-point spectrum.
fn band_db(spec: &[f32], hz: f32) -> f32 {
    let bin = (hz / (SR / 8192.0)).round() as usize;
    spec[bin - 2..=bin + 2].iter().cloned().fold(f32::MIN, f32::max)
}

fn zero_crossings(x: &[f32]) -> usize {
    x.windows(2).filter(|w| (w[0] < 0.0) != (w[1] < 0.0)).count()
}

#[test]
fn ring_mod_makes_sum_and_difference_tones() {
    let mut p = ring_mod(&[("effect", 0.0), ("frequency", 300.0), ("mod_depth", 0.0), ("mix", 100.0)]);
    let out = run(p.as_mut(), &[sine(1000.0, 0.5, 48_000)]);
    let spec = soundcraft_dsp::spectrum::magnitude_db(&out[0][8192..], 8192);
    let (low, high, input) = (band_db(&spec, 700.0), band_db(&spec, 1300.0), band_db(&spec, 1000.0));
    // The two tones fall at different places between bins, so their bin peaks differ a little.
    assert!((low - high).abs() < 2.0, "700 Hz {low} dB, 1300 Hz {high} dB");
    assert!(low - input > 40.0, "the 1000 Hz input is left at {input} dB against {low} dB");
}

#[test]
fn ring_mod_frequency_mod_adds_sidebands() {
    let x = sine(1000.0, 0.5, 48_000);
    let dry = soundcraft_dsp::spectrum::magnitude_db(&x[8192..], 8192);
    assert!(band_db(&dry, 1000.0) - band_db(&dry, 1200.0) > 40.0);
    let mut p = ring_mod(&[("effect", 1.0), ("frequency", 200.0), ("mix", 100.0)]);
    let out = run(p.as_mut(), &[x]);
    let spec = soundcraft_dsp::spectrum::magnitude_db(&out[0][8192..], 8192);
    for hz in [800.0, 1200.0] {
        assert!(band_db(&spec, 1000.0) - band_db(&spec, hz) < 20.0, "no sideband at {hz} Hz");
    }
}

#[test]
fn ring_mod_resonators_ring_at_the_frequency_with_their_sign() {
    // 480 Hz at 48 kHz is a 100-sample period.
    for (effect, sign) in [(2.0, 1.0), (3.0, -1.0)] {
        let mut p = ring_mod(&[("effect", effect), ("frequency", 480.0), ("resonance", 90.0), ("mix", 100.0)]);
        let mut x = vec![0.0f32; 9600];
        x[0] = 1.0;
        let out = run(p.as_mut(), &[x]);
        let y = &out[0];
        assert!((y[0] - 0.1).abs() < 1e-4, "effect {effect}: direct {}", y[0]);
        assert!((y[100] - sign * 0.09).abs() < 1e-4, "effect {effect}: first echo {}", y[100]);
        assert!((y[200] - 0.081).abs() < 1e-4, "effect {effect}: second echo {}", y[200]);
        assert!(y[50].abs() < 1e-6 && y[150].abs() < 1e-6, "effect {effect}: sound between the echoes");
        let last = y.iter().rposition(|v| v.abs() > 0.1e-3).unwrap();
        assert!(last < p.tail_samples(), "effect {effect}: ringing at {last}, tail {}", p.tail_samples());
    }
}

#[test]
fn ring_mod_modulation_sources_move_the_frequency() {
    // A constant input through Ring gives the carrier itself.
    let dc = vec![1.0f32; 48_000];

    // The envelope of a full-scale input lifts 100 Hz by one octave at 50 % depth.
    let mut p = ring_mod(&[("effect", 0.0), ("frequency", 100.0), ("mod_source", 1.0), ("mod_depth", 50.0), ("mix", 100.0)]);
    let out = run(p.as_mut(), std::slice::from_ref(&dc));
    let n = zero_crossings(&out[0][24_000..]);
    assert!((195..=205).contains(&n), "envelope: {n} crossings in 0.5 s, 200 expected for 200 Hz");

    // A 1 Hz LFO raises the frequency for its first half cycle and lowers it for the second.
    let mut p = ring_mod(&[("effect", 0.0), ("frequency", 100.0), ("mod_source", 0.0), ("mod_rate", 1.0), ("mod_depth", 100.0), ("mix", 100.0)]);
    let out = run(p.as_mut(), std::slice::from_ref(&dc));
    let (up, down) = (zero_crossings(&out[0][..12_000]), zero_crossings(&out[0][24_000..36_000]));
    assert!(up > 3 * down, "LFO: {up} crossings rising, {down} falling");

    // Sample and hold at 4 Hz keeps one frequency for each 12,000-sample step.
    let mut p = ring_mod(&[("effect", 0.0), ("frequency", 100.0), ("mod_source", 2.0), ("mod_rate", 4.0), ("mod_depth", 100.0), ("mix", 100.0)]);
    let out = run(p.as_mut(), std::slice::from_ref(&dc));
    let mut steps = Vec::new();
    for s in out[0].chunks(12_000) {
        let (a, b) = (zero_crossings(&s[..6000]), zero_crossings(&s[6000..]));
        assert!(a.abs_diff(b) <= 2, "sample and hold changed within a step: {a} then {b}");
        steps.push(a);
    }
    steps.sort_unstable();
    steps.dedup_by(|a, b| a.abs_diff(*b) <= 2);
    assert!(steps.len() >= 3, "sample and hold steps: {steps:?}");
}
