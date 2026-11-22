use super::sampler_song::sampler_song;
use super::*;

fn edges(pcm: &[f32]) -> Vec<(i64, i64)> {
    let mut attack = 0;
    let mut previous = false;
    let mut hits = Vec::new();
    for (i, &v) in pcm.iter().enumerate() {
        let sounding = v.abs() > 0.001;
        if sounding && !previous {
            attack = i as i64;
        } else if !sounding && previous {
            hits.push((attack, i as i64));
        }
        previous = sounding;
    }
    hits
}

#[test]
fn tempo_steps_keep_every_rendered_eighth_and_its_waveform() {
    let sr = SampleRate::HZ_48000;
    let mut passed = true;
    for (before, after) in [(130., 80.), (120., 100.), (180., 60.), (100., 120.), (80., 130.), (60., 180.)] {
        for transition in [384_usize, 396] {
            let inputs = MidiInputs::with_port_names(vec![], vec!["drums".into()]);
            let feed = inputs.feed();
            let device = feed.ports().assign("drums");
            feed.set_clock_follow(Some(device), true, 0, false);
            feed.set_clock(0);
            feed.receive(device, &[0xfa]);
            let count = (transition + 192) / 12 + 1;
            let mut song = sampler_song(TempoMap::new(before, 4, 4).unwrap(), sr, count, 48_000 * 60);
            let mut mixer = soundcraft_mix::MixEngine::new(48_000., 128);
            let mut block = vec![vec![0.; 128]; 2];
            let mut pcm = Vec::new();
            let mut events = Vec::new();
            let mut expected = Vec::new();
            let mut us = 0.0_f64;
            // Feed only the current F8. Render to the next arrival before publishing its grid.
            // The 30 ms guard represents audio that a grid edit can no longer rewrite.
            for pulse in 0..=transition + 192 {
                let stamp = us.round() as i64;
                feed.set_clock(stamp);
                feed.receive(device, &[0xf8]);
                let snap = feed.clock_follow_status(&mut events);
                update_grid(&mut song.tempo, Some((0, 0)), 0, snap, sr, sr.samples(us / 1e6 + 0.030));
                if pulse % 12 == 0 {
                    expected.push(sr.samples(us / 1e6));
                }
                us += 2_500_000. / if pulse < transition { before } else { after };
                let end = if pulse == transition + 192 { expected.last().unwrap() + 48_000 } else { sr.samples(us / 1e6) };
                while (pcm.len() as i64) < end {
                    let pos = pcm.len() as i64;
                    let frames = (end - pos).min(128) as usize;
                    mixer.render_with_clock_limit(&song, pos, frames, &mut block, feed.clock_render_tick());
                    pcm.extend_from_slice(&block[0][..frames]);
                }
            }
            let hits = edges(&pcm);
            assert_eq!(hits.len(), expected.len(), "onset count {before}->{after} at {transition}");
            assert_eq!(hits[transition / 12].0, expected[transition / 12], "the boundary already played at the old tempo");
            let reference = &pcm[hits[0].0 as usize..hits[0].1 as usize];
            assert_eq!(reference.len(), 96);
            for &(on, off) in &hits {
                assert_eq!(&pcm[on as usize..off as usize], reference);
            }
            assert!(pcm[hits.last().unwrap().1 as usize..].iter().all(|x| x.abs() < 0.001));
            let signed: Vec<_> = hits.iter().zip(&expected).skip(transition / 12).map(|(hit, at)| (hit.0 - at) as f64 / 48.).collect();
            eprintln!("STEP {before}->{after} pulse={transition} hits={} ms={signed:.3?}", hits.len());
            passed &= signed[0].abs() <= 4. && signed[1].abs() <= 10. && signed[2..].iter().all(|e| e.abs() <= 4.);
        }
    }
    assert!(passed, "rendered eighth-note phase after a tempo step");
}

#[test]
fn early_device_render_waits_for_the_wire_and_keeps_the_located_attack() {
    let inputs = MidiInputs::with_port_names(vec![], vec!["drums".into()]);
    let feed = inputs.feed();
    let port = feed.ports().assign("drums");
    feed.set_clock_follow(Some(port), true, 0, false);
    feed.set_clock(0);
    feed.receive(port, &[0xfa]);
    feed.receive(port, &[0xf8]);
    let song = sampler_song(TempoMap::new(120., 4, 4).unwrap(), SampleRate::HZ_48000, 2, 48000);
    let mut mix = soundcraft_mix::MixEngine::new(48000., 128);
    let mut pcm = Vec::new();
    let mut block = vec![vec![0.; 128]; 2];
    // Start rendering 30 ms ahead, with only the first wire pulse in hand.
    let mut pos = 1440;
    mix.start_midi_at(0);
    let mut render = |end: i64, pcm: &mut Vec<f32>| {
        while pos < end {
            let n = (end - pos).min(128) as usize;
            mix.render_with_clock_limit(&song, pos, n, &mut block, feed.clock_render_tick());
            pcm.extend_from_slice(&block[0][..n]);
            pos += n as i64;
        }
    };
    for pulse in 1..=10 {
        render(1440 + pulse * 1000, &mut pcm);
        feed.set_clock(pulse * 1_000_000 / 48);
        feed.receive(port, &[0xf8]);
    }
    render(14000, &mut pcm);
    assert_eq!(edges(&pcm), vec![(0, 96)], "the second hat must wait for pulse 11");
    feed.set_clock(280_000);
    feed.receive(port, &[0xf8]);
    render(14400, &mut pcm);
    let hits = edges(&pcm);
    assert_eq!(hits.len(), 2);
    assert!(hits[1].0 >= 14000 - 1440);
    assert_eq!(hits[1].1 - hits[1].0, 96);
    assert_eq!(&pcm[..96], &pcm[hits[1].0 as usize..hits[1].1 as usize]);
}
