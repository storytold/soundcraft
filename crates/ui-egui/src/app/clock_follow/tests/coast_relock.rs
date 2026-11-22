use super::sampler_song::sampler_song;
use super::*;

// Count attacks in the PCM captured while the MIDI input was arriving.
fn attacks(pcm: &[f32]) -> Vec<i64> {
    let mut sounding = false;
    let mut attacks = Vec::new();
    for (i, sample) in pcm.iter().enumerate() {
        let on = sample.abs() > 0.001;
        if on && !sounding {
            attacks.push(i as i64);
        }
        sounding = on;
    }
    attacks
}

#[test]
fn returning_clock_without_continue_keeps_the_deck_change_in_phase() {
    const RETURN_PULSE: usize = 398;
    const PULSES_PER_HAT: usize = 12;
    let sr = SampleRate::HZ_48000;
    for coast in [false, true] {
        let inputs = MidiInputs::with_port_names(vec![], vec!["DJ".into()]);
        let feed = inputs.feed();
        let port = feed.ports().assign("DJ");
        feed.set_clock_follow(Some(port), coast, 0, false);
        let origin = 20_000;
        feed.set_clock(origin);
        feed.receive(port, &[0xfa]);
        let mut song = sampler_song(TempoMap::new(120., 4, 4).unwrap(), sr, 65, 48_000 * 20);
        let mut mixer = soundcraft_mix::MixEngine::new(48_000., 128);
        let mut block = vec![vec![0.; 128]; 2];
        let mut pcm = Vec::new();
        let mut render_to = |end: i64, song: &soundcraft_model::Session| {
            while (pcm.len() as i64) < end {
                let pos = pcm.len() as i64;
                let frames = (end - pos).min(128) as usize;
                mixer.render_with_clock_limit(song, pos, frames, &mut block, feed.clock_render_tick());
                pcm.extend_from_slice(&block[0][..frames]);
            }
        };
        let mut expected = Vec::new();
        let mut stop = None;
        let mut actions = Vec::new();
        let mut clock_us = origin as f64;
        let mut snapshot = Snapshot::default();
        // At 8 s the 120 BPM deck drops out. Withhold 14 pulses and shift
        // the 128 BPM train to return at 8.300 s without Start or Continue.
        for pulse in 0..780 {
            if pulse > 0 {
                clock_us += 2_500_000. / if pulse <= 384 { 120. } else { 128. };
            }
            if pulse == RETURN_PULSE {
                clock_us += 26_562.5;
            }
            let song_time = (clock_us - origin as f64) / 1e6;
            if pulse % PULSES_PER_HAT == 0 {
                expected.push(sr.samples(song_time));
            }
            let stamp = clock_us.round() as i64;
            let arrival_time = (stamp - origin) as f64 / 1e6;
            if stop.is_none() {
                render_to(sr.samples(arrival_time), &song);
            }
            feed.set_clock(stamp);
            if !(384..RETURN_PULSE).contains(&pulse) {
                feed.receive(port, &[0xf8]);
            }
            snapshot = feed.clock_follow_status(&mut actions);
            for action in actions.drain(..) {
                if pulse >= RETURN_PULSE {
                    panic!("unexpected transport command after F8 resumed: {action:?}");
                }
                if action == Action::Stop {
                    assert!(stop.replace(sr.samples(arrival_time)).is_none());
                }
            }
            if pulse + 1 == RETURN_PULSE {
                assert_eq!(snapshot.state, if coast { State::Coasting } else { State::Stopped });
            }
            update_grid(&mut song.tempo, Some((0, origin)), 0, snapshot, sr, sr.samples(arrival_time + 0.030));
        }
        let end = stop.unwrap_or_else(|| expected.last().unwrap() + 48000);
        render_to(end, &song);
        let hits = attacks(&pcm);
        let expected_played = expected.partition_point(|at| *at < end);
        assert_eq!(hits.len(), expected_played);
        let errors: Vec<_> = hits.iter().zip(&expected).map(|(hit, master)| hit - master).collect();
        // After a missing pulse train, each hat must still pair with its own reference
        // rather than a neighbouring eighth note. Check the final phase more tightly.
        let last = *errors.last().unwrap();
        eprintln!(
            "coast={coast} state={:?} bpm={:?} hits={}/{} last_samples={last} max_abs_samples={} stop={stop:?}",
            snapshot.state,
            snapshot.bpm,
            hits.len(),
            expected_played,
            errors.iter().map(|e| e.abs()).max().unwrap()
        );
        if coast {
            let resumed = &errors[RETURN_PULSE.div_ceil(PULSES_PER_HAT)..];
            let settled_peak = resumed[1..].iter().map(|e| e.abs()).max().unwrap();
            eprintln!("post-return first={} samples; later peak={} samples", resumed[0], settled_peak);
            assert!(resumed[0].abs() <= 480, "first resumed hat exceeds 10 ms");
            assert!(settled_peak <= 192, "a subsequent hat exceeds 4 ms");
            assert!(stop.is_none());
            assert_eq!(snapshot.state, State::Locked);
            assert!((snapshot.bpm.unwrap() - 128.).abs() < 0.01);
            assert!(errors.iter().all(|e| e.abs() < 4800));
            assert!(last.abs() <= 192, "last hat exceeds 4 ms at 48 kHz");
        } else {
            assert!(stop.is_some());
            assert_eq!(hits.len(), 33, "only the hats before the loss timeout should sound");
            assert_eq!(snapshot.state, State::Stopped);
        }
    }
}
