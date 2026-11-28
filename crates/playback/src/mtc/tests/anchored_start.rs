use super::*;
use crate::midi::MidiFeed;
use soundcraft_model::{ChannelFormat, Clip, SourceAudio, SourceId, TrackKind};

fn replay(rate: FrameRate, locate_frame: i64, at: usize, score: usize, events: &[(i64, u8)], times: &[i64], buffer: usize) -> Vec<(i64, i64)> {
    let mut s = session(rate);
    let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, None);
    let mut wave = vec![0.; at + 96000];
    // Distinguish a skipped predecessor, the protected downbeat and a later marker.
    if at > 1000 {
        wave[at - 1000..at - 995].fill(0.5);
    }
    for k in [at, score, at + 24000] {
        wave[k..k + 5].fill(0.5);
    }
    let len = wave.len() as i64;
    s.pool.insert(SourceId(1), Arc::new(SourceAudio::new(craft_audio::AudioBuffer { sample_rate: 48000, channels: vec![wave] })));
    let id = s.new_clip_id();
    s.track_mut(t).unwrap().playlist_mut().unwrap().clips.push(Clip::audio(id, "markers", SourceId(1), 0, 0, len));
    let (mut audio, commands, _) = callback(s, 48000.);
    let feed = Arc::new(MidiFeed::default());
    feed.set_mtc_source(Some(0));
    commands.send(Cmd::Midi(feed.clone())).unwrap();
    feed.set_clock(0);
    feed.receive(0, &locate(locate_frame, rate));
    let base = Instant::now();
    let mut j = 0;
    let mut hits = Vec::new();
    let mut loud = false;
    for (index, &us) in times.iter().enumerate() {
        while j < events.len() && events[j].0 <= us {
            feed.set_clock(events[j].0);
            feed.receive(0, &[0xf1, events[j].1]);
            j += 1;
        }
        feed.set_clock(us);
        audio.device_buffer_frames = buffer;
        audio.device_timing = Some(DeviceTiming::new(base + Duration::from_micros(us as u64), Some(Duration::from_micros(15166)), 0));
        let mut pcm = vec![0.; buffer * 2];
        audio.fill(&mut pcm, 2);
        for (k, pair) in pcm.as_chunks::<2>().0.iter().enumerate() {
            let high = pair[0].abs() > 0.03;
            if high && !loud {
                hits.push((us + (k as i64 * 1000000 + 24000) / 48000, (index * buffer + k) as i64));
            }
            loud = high;
        }
    }
    assert_eq!(hits.len(), 3, "unexpected attacks at locate {at}: {hits:?}");
    assert_eq!(hits[2].1 - hits[0].1, 24000, "anchor and half-second interval");
    assert_eq!(hits[1].1 - hits[0].1, (score - at) as i64);
    hits
}

#[test]
fn anchored_start_preserves_attack_phase_across_device_buffers() {
    let rate = FrameRate::Fps30;
    let events: Vec<_> = (0..120usize)
        .map(|piece| {
            let stamp = 100_000 + piece as i64 * 1_000_000 / 120;
            (stamp, quarters(60 + 2 * (piece / 8) as i64, rate)[piece % 8][1])
        })
        .collect();
    let mut all = Vec::new();
    for buffer in [128, 256, 512, 2048] {
        let first: Vec<_> = (0..buffer)
            .step_by(buffer / 8)
            .map(|phase| {
                let times: Vec<_> = (phase..48_000).step_by(buffer).map(|sample| sample as i64 * 1_000_000 / 48_000).collect();
                replay(rate, 60, 96_000, 108_000, &events, &times, buffer)[0].0
            })
            .collect();
        let spread = first.iter().max().unwrap() - first.iter().min().unwrap();
        eprintln!("buffer={buffer} first={first:?} spread={spread} us");
        assert!(spread <= 21, "buffer {buffer}: first attacks {first:?}");
        // The stopped locate is presented one device buffer after lock (`follow_mtc`): less that
        // buffer, every size starts the attack at the same moment.
        all.extend(first.iter().map(|us| us - buffer as i64 * 1_000_000 / 48_000));
    }
    let spread = all.iter().max().unwrap() - all.iter().min().unwrap();
    assert!(spread <= 21, "first attacks less one device buffer, across sizes: {all:?}");
}
