use std::f32::consts::{FRAC_PI_4, PI, TAU};

use soundcraft_dsp::{Plugin, Transport, create, db_to_gain};

const SR: f32 = 48_000.0;
const BLOCK: usize = 512;

fn auto_pan(params: &[(&str, f32)]) -> Box<dyn Plugin> {
    let mut p = create("auto_pan").unwrap();
    p.prepare(SR, BLOCK, 2);
    for (id, v) in params {
        assert!(p.set_param(id, *v), "{id}");
    }
    p.reset();
    p
}

/// Runs `seconds` of a constant `level` on both channels in host-sized blocks.
fn run_level(p: &mut dyn Plugin, level: f32, seconds: f32) -> Vec<Vec<f32>> {
    let len = (seconds * SR) as usize;
    let mut out = vec![Vec::new(), Vec::new()];
    let mut pos = 0;
    while pos < len {
        let n = BLOCK.min(len - pos);
        let mut buf = vec![vec![level; n]; 2];
        p.process(&mut buf, n);
        for c in 0..2 {
            out[c].extend_from_slice(&buf[c]);
        }
        pos += n;
    }
    out
}

/// Pan position (-1 left ..= 1 right) of frame `i`, from the constant-power gains.
fn position(out: &[Vec<f32>], i: usize) -> f32 {
    4.0 / PI * out[1][i].atan2(out[0][i]) - 1.0
}

fn at(seconds: f32) -> usize {
    (seconds * SR) as usize
}

fn assert_near(got: f32, want: f32, tol: f32, what: &str) {
    assert!((got - want).abs() <= tol, "{what}: got {got}, want {want}");
}

#[test]
fn auto_pan_manual_places_the_signal_when_width_is_zero() {
    let mut p = auto_pan(&[("width", 0.0), ("manual", -100.0)]);
    let out = run_level(p.as_mut(), 0.5, 0.1);
    assert_near(out[0][at(0.05)], 0.5 * 2f32.sqrt(), 1e-4, "hard left, left channel");
    assert_near(out[1][at(0.05)], 0.0, 1e-4, "hard left, right channel");

    let mut p = auto_pan(&[("width", 0.0), ("manual", 50.0)]);
    let out = run_level(p.as_mut(), 0.5, 0.1);
    assert_near(position(&out, at(0.05)), 0.5, 1e-3, "manual 50 %");

    let mut p = auto_pan(&[("width", 0.0)]);
    let out = run_level(p.as_mut(), 0.5, 0.1);
    assert_near(out[0][at(0.05)], 0.5, 1e-4, "center keeps the level on the left");
    assert_near(out[1][at(0.05)], 0.5, 1e-4, "center keeps the level on the right");
}

#[test]
fn auto_pan_lfo_moves_from_left_to_right_and_back() {
    // 60 BPM with a Duration of 1 beat is one cycle per second.
    let mut p = auto_pan(&[("rate", 60.0), ("duration", 2.0)]);
    let out = run_level(p.as_mut(), 0.5, 1.0);
    for (t, want) in [(0.0, -1.0), (0.25, 0.0), (0.5, 1.0), (0.75, 0.0)] {
        assert_near(position(&out, at(t)), want, 0.05, &format!("left to right at {t} s"));
    }

    let mut p = auto_pan(&[("rate", 60.0), ("duration", 2.0), ("path", 1.0)]);
    let out = run_level(p.as_mut(), 0.5, 1.0);
    assert_near(position(&out, at(0.01)), 1.0, 0.05, "right to left starts on the right");
    assert_near(position(&out, at(0.5)), -1.0, 0.05, "right to left at 0.5 s");

    // Width 50 % sweeps half the field, and Manual moves that half to the right.
    let mut p = auto_pan(&[("rate", 60.0), ("duration", 2.0), ("width", 50.0), ("manual", 50.0)]);
    let out = run_level(p.as_mut(), 0.5, 1.0);
    assert_near(position(&out, at(0.0)), -0.25, 0.05, "width 50 % at the start");
    assert_near(position(&out, at(0.5)), 0.75, 0.05, "width 50 % at the far end");
}

#[test]
fn auto_pan_keeps_moving_at_the_slowest_rate() {
    // Rate 1 BPM over 4 bars of 4/4 is one cycle in 960 s; 600 s in, a saw is 62.5 % across.
    let mut p = auto_pan(&[("rate", 1.0), ("duration", 6.0), ("waveform", 3.0)]);
    let mut buf = vec![vec![0.5f32; BLOCK]; 2];
    for _ in 0..(600.0 * SR) as usize / BLOCK {
        buf.iter_mut().for_each(|c| c.fill(0.5));
        p.process(&mut buf, BLOCK);
    }
    assert_near(position(&buf, BLOCK - 1), 0.25, 0.01, "600 s into a 960 s cycle");
}

#[test]
fn auto_pan_waveforms_take_their_shapes() {
    let shape = |waveform: f32, t: f32| {
        let mut p = auto_pan(&[("rate", 60.0), ("duration", 2.0), ("waveform", waveform)]);
        let out = run_level(p.as_mut(), 0.5, 1.0);
        position(&out, at(t))
    };
    assert_near(shape(1.0, 0.25), 0.0, 0.05, "triangle at a quarter cycle");
    assert_near(shape(1.0, 0.125), -0.5, 0.05, "triangle at an eighth of a cycle");
    assert_near(shape(2.0, 0.2), -1.0, 0.02, "square in its first half");
    assert_near(shape(2.0, 0.7), 1.0, 0.02, "square in its second half");
    assert_near(shape(3.0, 0.25), -0.5, 0.05, "saw up");
    assert_near(shape(4.0, 0.25), 0.5, 0.05, "saw down");
    assert_near(shape(5.0, 1.0 / 6.0), 0.0, 0.05, "half sine reaches the center at a sixth of a cycle");
    assert_near(shape(6.0, 0.1), -1.0, 0.02, "4 step triangle, first step");
    assert_near(shape(6.0, 0.2), -1.0 / 3.0, 0.02, "4 step triangle, second step");
    assert_near(shape(6.0, 0.45), 1.0, 0.02, "4 step triangle, top step");
}

#[test]
fn auto_pan_rate_follows_the_session_tempo_and_duration() {
    // Linked to a 120 BPM session with a Duration of 1 beat: two cycles per second.
    let mut p = auto_pan(&[("link_tempo", 1.0), ("rate", 30.0), ("duration", 2.0)]);
    p.set_transport(&Transport { tempo: 120.0, numerator: 4, denominator: 4, ..Transport::default() });
    let out = run_level(p.as_mut(), 0.5, 0.5);
    assert_near(position(&out, at(0.25)), 1.0, 0.05, "linked to 120 BPM, 1 beat");

    // Not linked: Rate 120 BPM and the default Duration of 1 bar (4/4) is one cycle in 2 s.
    let mut p = auto_pan(&[]);
    p.set_transport(&Transport { tempo: 60.0, numerator: 4, denominator: 4, ..Transport::default() });
    let out = run_level(p.as_mut(), 0.5, 1.5);
    assert_near(position(&out, at(1.0)), 1.0, 0.05, "Rate 120 BPM, 1 bar");

    // A 3/4 bar at 120 BPM lasts 1.5 s.
    let mut p = auto_pan(&[("link_tempo", 1.0)]);
    p.set_transport(&Transport { tempo: 120.0, numerator: 3, denominator: 4, ..Transport::default() });
    let out = run_level(p.as_mut(), 0.5, 1.0);
    assert_near(position(&out, at(0.75)), 1.0, 0.05, "1 bar of 3/4");
}

#[test]
fn auto_pan_beat_clock_follows_the_transport_position() {
    let mut p = auto_pan(&[("trigger", 2.0), ("link_tempo", 1.0), ("duration", 2.0)]);
    // Half a beat into the song, the LFO is half way through its cycle.
    p.set_transport(&Transport { playing: true, tempo: 120.0, numerator: 4, denominator: 4, quarters: 7.5, ..Transport::default() });
    let out = run_level(p.as_mut(), 0.5, 0.01);
    assert_near(position(&out, 0), 1.0, 0.05, "half a beat in");

    let mut p = auto_pan(&[("trigger", 2.0), ("link_tempo", 1.0), ("duration", 2.0)]);
    p.set_transport(&Transport { playing: true, tempo: 120.0, numerator: 4, denominator: 4, quarters: 3.0, ..Transport::default() });
    let out = run_level(p.as_mut(), 0.5, 0.2);
    assert_near(position(&out, 0), -1.0, 0.05, "on the beat");
    assert_near(position(&out, at(0.125)), 0.0, 0.05, "a quarter of a beat later");

    // Not linked, Rate 60 BPM in a 120 BPM session: the LFO restarts on every beat (0.5 s) and
    // runs at 1 Hz in between, so it reaches the right and then starts again from the left.
    let mut p = auto_pan(&[("trigger", 2.0), ("rate", 60.0), ("duration", 2.0)]);
    p.set_transport(&Transport { playing: true, tempo: 120.0, numerator: 4, denominator: 4, quarters: 3.0, ..Transport::default() });
    let out = run_level(p.as_mut(), 0.5, 1.0);
    assert_near(position(&out, at(0.25)), 0.0, 0.05, "a quarter cycle at 1 Hz");
    assert_near(position(&out, at(0.49)), 1.0, 0.05, "on the right at the end of the beat");
    assert_near(position(&out, at(0.51)), -1.0, 0.05, "restarted on the next beat");
    assert_near(position(&out, at(0.75)), 0.0, 0.05, "a quarter cycle after the restart");

    // 2 Beats in 3/4, with the host updating the transport every block: the cycle runs on across
    // the bar line at quarter 3 instead of restarting there.
    let mut p = auto_pan(&[("trigger", 2.0), ("link_tempo", 1.0), ("waveform", 3.0), ("duration", 3.0)]);
    let mut buf = vec![vec![0.5f32; 100]; 2];
    let mut positions = Vec::new();
    for block in 0..480 {
        let quarters = 2.0 + block as f64 * 100.0 * 2.0 / f64::from(SR);
        let bar = (quarters / 3.0).floor();
        p.set_transport(&Transport {
            playing: true,
            tempo: 120.0,
            numerator: 3,
            denominator: 4,
            quarters,
            bar_start: bar * 3.0,
            bar: bar as i64,
            ..Transport::default()
        });
        buf.iter_mut().for_each(|c| c.fill(0.5));
        p.process(&mut buf, 100);
        positions.push((quarters, position(&buf, 0)));
    }
    for (q, want) in [(2.5, -0.5), (3.0, 0.0), (3.5, 0.5)] {
        let got = positions.iter().find(|(quarters, _)| (quarters - q).abs() < 1e-9).map(|(_, pos)| *pos).unwrap();
        assert_near(got, want, 0.02, &format!("2 beats in 3/4 at quarter {q}"));
    }

    // Bar-based cycles start on the bar line after a change from 4/4 to 3/4 at quarter 4.
    let three_four = |quarters: f64| Transport {
        playing: true,
        tempo: 120.0,
        numerator: 3,
        denominator: 4,
        quarters,
        bar_start: 4.0,
        bar: 1,
        ..Transport::default()
    };
    let mut p = auto_pan(&[("trigger", 2.0), ("link_tempo", 1.0), ("waveform", 3.0)]);
    p.set_transport(&three_four(4.0));
    let out = run_level(p.as_mut(), 0.5, 0.01);
    assert_near(position(&out, 0), -1.0, 0.02, "1 bar starts on the 3/4 bar line");
    let mut p = auto_pan(&[("trigger", 2.0), ("link_tempo", 1.0), ("waveform", 3.0), ("duration", 5.0)]);
    p.set_transport(&three_four(5.5));
    let out = run_level(p.as_mut(), 0.5, 0.01);
    assert_near(position(&out, 0), 0.5, 0.02, "half way through the second bar of 2 bars");
}

#[test]
fn auto_pan_single_runs_one_cycle_and_holds() {
    let mut p = auto_pan(&[("trigger", 1.0), ("rate", 60.0), ("duration", 2.0), ("waveform", 3.0)]);
    let out = run_level(p.as_mut(), 0.5, 2.0);
    assert_near(position(&out, at(0.5)), 0.0, 0.05, "half way through the one cycle");
    assert_near(position(&out, at(1.5)), 1.0, 0.02, "holds at the end of the path");
    assert_near(position(&out, at(1.99)), 1.0, 0.02, "still holding");

    // Starting the transport runs the cycle again.
    p.set_transport(&Transport { playing: true, tempo: 120.0, numerator: 4, denominator: 4, ..Transport::default() });
    let out = run_level(p.as_mut(), 0.5, 0.5);
    assert_near(position(&out, at(0.25)), -0.5, 0.05, "restarted when playback starts");
}

#[test]
fn auto_pan_envelope_source_pans_by_level() {
    let settled = |level: f32| {
        let mut p = auto_pan(&[("source", 1.0), ("threshold", -30.0)]);
        let out = run_level(p.as_mut(), level, 0.5);
        position(&out, at(0.45))
    };
    assert_near(settled(db_to_gain(-40.0)), -1.0, 0.02, "below the threshold");
    assert_near(settled(db_to_gain(-15.0)), 0.0, 0.05, "half way to full scale");
    assert_near(settled(1.0), 1.0, 0.02, "full scale");
}

#[test]
fn auto_pan_envelope_and_manual_triggers_start_a_cycle() {
    let mut p = auto_pan(&[("trigger", 3.0), ("rate", 60.0), ("duration", 2.0), ("waveform", 3.0), ("threshold", -30.0)]);
    let quiet = run_level(p.as_mut(), db_to_gain(-40.0), 0.5);
    assert_near(position(&quiet, at(0.4)), -1.0, 0.02, "waits at the start below the threshold");
    let loud = run_level(p.as_mut(), 1.0, 1.5);
    assert_near(position(&loud, at(0.25)), -0.5, 0.05, "a quarter cycle after the level crosses the threshold");
    assert_near(position(&loud, at(1.4)), 1.0, 0.02, "holds after one cycle while the level stays up");

    let mut p = auto_pan(&[("trigger", 4.0), ("rate", 60.0), ("duration", 2.0), ("waveform", 3.0)]);
    let before = run_level(p.as_mut(), 0.5, 0.5);
    assert_near(position(&before, at(0.4)), -1.0, 0.02, "waits for the manual trigger");
    assert!(p.set_param("retrigger", 1.0));
    let after = run_level(p.as_mut(), 0.5, 0.5);
    assert_near(position(&after, at(0.25)), -0.5, 0.05, "a quarter cycle after switching the trigger on");
    let held = run_level(p.as_mut(), 0.5, 1.0);
    assert!(p.set_param("retrigger", 0.0));
    let again = run_level(p.as_mut(), 0.5, 0.5);
    assert_near(position(&held, at(0.9)), 1.0, 0.02, "holds after one cycle");
    assert_near(position(&again, at(0.25)), -0.5, 0.05, "switching the trigger off starts another cycle");
}

#[test]
fn auto_pan_envelope_trigger_waits_for_the_release() {
    let mut p = auto_pan(&[
        ("trigger", 3.0),
        ("rate", 60.0),
        ("duration", 2.0),
        ("waveform", 3.0),
        ("threshold", -30.0),
        ("attack", 0.1),
        ("release", 100.0),
    ]);
    let quiet = db_to_gain(-40.0);
    let first = run_level(p.as_mut(), 1.0, 1.5);
    assert_near(position(&first, at(1.4)), 1.0, 0.02, "the first cycle ends and holds");
    // The envelope falls below the threshold about 0.38 s into the quiet part, so a 0.42 s dip
    // leaves it below for less than the 100 ms Release.
    run_level(p.as_mut(), quiet, 0.42);
    let short = run_level(p.as_mut(), 1.0, 0.5);
    assert_near(position(&short, at(0.25)), 1.0, 0.02, "no trigger before the release is over");
    run_level(p.as_mut(), quiet, 1.0);
    let long = run_level(p.as_mut(), 1.0, 0.5);
    assert_near(position(&long, at(0.25)), -0.5, 0.05, "a trigger once the release is over");
}

#[test]
fn auto_pan_output_gain_and_mono_tracks() {
    let mut p = auto_pan(&[("width", 0.0), ("output", 6.0)]);
    let out = run_level(p.as_mut(), 0.5, 0.1);
    assert_near(out[0][at(0.05)], 0.5 * db_to_gain(6.0), 1e-4, "+6 dB at the center");

    // A mono insert has nothing to pan to; Output still applies.
    let mut p = create("auto_pan").unwrap();
    p.prepare(SR, BLOCK, 1);
    assert!(p.set_param("output", -6.0));
    p.reset();
    let mut buf = vec![vec![0.5f32; BLOCK]];
    p.process(&mut buf, BLOCK);
    assert_near(buf[0][BLOCK - 1], 0.5 * db_to_gain(-6.0), 1e-4, "mono output");
}

#[test]
fn auto_pan_balances_a_stereo_signal_without_summing_it() {
    // A sine on the left, the same sine inverted plus an offset on the right: the channels differ,
    // and a mono sum would cancel the sine.
    let len = at(0.2);
    let left: Vec<f32> = (0..len).map(|i| 0.5 * (TAU * 440.0 * i as f32 / SR).sin()).collect();
    let right: Vec<f32> = left.iter().map(|x| 0.25 - x).collect();
    let run = |p: &mut dyn Plugin| {
        let mut out = vec![Vec::new(), Vec::new()];
        for start in (0..len).step_by(BLOCK) {
            let end = (start + BLOCK).min(len);
            let mut buf = vec![left[start..end].to_vec(), right[start..end].to_vec()];
            p.process(&mut buf, end - start);
            out[0].extend_from_slice(&buf[0]);
            out[1].extend_from_slice(&buf[1]);
        }
        out
    };

    let mut p = auto_pan(&[("width", 0.0)]);
    let out = run(p.as_mut());
    for i in 0..len {
        assert_near(out[0][i], left[i], 1e-5, "center passes the left channel");
        assert_near(out[1][i], right[i], 1e-5, "center passes the right channel");
    }

    // Half left: each channel keeps its own content, the left louder and the right quieter.
    let mut p = auto_pan(&[("width", 0.0), ("manual", -50.0)]);
    let out = run(p.as_mut());
    let theta = 0.5 * FRAC_PI_4;
    let (gl, gr) = (2f32.sqrt() * theta.cos(), 2f32.sqrt() * theta.sin());
    for i in at(0.05)..len {
        assert_near(out[0][i], left[i] * gl, 1e-5, "half left, left channel");
        assert_near(out[1][i], right[i] * gr, 1e-5, "half left, right channel");
    }

    // Anti-phase channels still drive the envelope follower to the right.
    let mut p = auto_pan(&[("source", 1.0), ("threshold", -30.0)]);
    let mut buf = vec![vec![1.0f32; BLOCK], vec![-1.0f32; BLOCK]];
    for _ in 0..at(0.5) / BLOCK {
        buf[0].fill(1.0);
        buf[1].fill(-1.0);
        p.process(&mut buf, BLOCK);
    }
    assert_near(buf[0][BLOCK - 1], 0.0, 1e-3, "anti-phase at full scale pans right: left");
    assert_near(buf[1][BLOCK - 1], -(2f32.sqrt()), 1e-3, "anti-phase at full scale pans right: right");
}
