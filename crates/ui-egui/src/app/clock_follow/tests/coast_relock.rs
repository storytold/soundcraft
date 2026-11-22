use super::*;
use soundcraft_midi::{Note, Sequence};
use soundcraft_model::{ChannelFormat, Clip, Insert, Session, SourceAudio, SourceId, TrackKind};
use soundcraft_playback::midi::MidiInputs;
use soundcraft_time::Range;
use std::sync::Arc;

// Render the same eighth-note pattern across the whole transition. Count physical
// attacks in PCM, including any beyond the last expected note.
fn render_hats(grid: TempoMap, sr: SampleRate, count: usize, end: i64) -> Vec<i64> {
    let mut song = Session { tempo: grid, sample_rate: sr, ..Default::default() };
    let source = SourceId(200);
    song.pool.insert(source, Arc::new(SourceAudio::new(craft_audio::AudioBuffer { sample_rate: 48000, channels: vec![vec![0.8; 96]] })));
    let id = song.add_track(TrackKind::Instrument, ChannelFormat::Stereo, None);
    let clip = song.new_clip_id();
    let tr = song.track_mut(id).unwrap();
    let mut inst = Insert::new("sampler");
    inst.sample = Some(source);
    inst.params.insert("root_note".into(), 60.);
    inst.params.insert("attack".into(), 0.);
    tr.instrument = Some(inst);
    let mut seq = Sequence::default();
    for i in 0..count {
        seq.notes.push(Note { start: i as i64 * 480, length: 80, pitch: 60, channel: 0, velocity: 110, release_velocity: 64 });
    }
    tr.playlist_mut().unwrap().clips.push(Clip::midi(clip, "hats", 0, end + 48000, seq));
    let pcm = soundcraft_mix::render_range(&song, Range::new(0, end), 512);
    let mut sounding = false;
    let mut attacks = Vec::new();
    for (i, sample) in pcm[0].iter().enumerate() {
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
        let mut grid = TempoMap::new(120., 4, 4).unwrap();
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
            update_grid(&mut grid, Some((0, origin)), 0, snapshot, sr, sr.samples(arrival_time + 0.030));
        }
        let end = stop.unwrap_or_else(|| expected.last().unwrap() + 48000);
        let hits = render_hats(grid, sr, expected.len(), end);
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
