use super::*;
use crate::mtc_preroll::silent_pcm_prefix;
use soundcraft_model::{ChannelFormat, Clip, SourceAudio, SourceId, TrackKind};

fn guide() -> Session {
    let mut s = session(FrameRate::Fps30);
    let t = s.add_track(TrackKind::Audio, ChannelFormat::Stereo, None);
    let mut right = vec![0.0; 50000];
    right[12002] = f32::MIN_POSITIVE;
    s.pool.insert(SourceId(1), Arc::new(SourceAudio::new(craft_audio::AudioBuffer { sample_rate: 48000, channels: vec![vec![0.; 50000], right] })));
    let id = s.new_clip_id();
    s.track_mut(t).unwrap().playlist_mut().unwrap().clips.push(Clip::audio(id, "guide", SourceId(1), 2, 0, 40000));
    s
}

#[test]
fn a_silent_guide_can_join_at_presentation_but_every_channel_protects_its_attack() {
    let s = guide();
    assert!(silent_pcm_prefix(&s, 0, 12000));
    assert!(!silent_pcm_prefix(&s, 0, 12001));
    assert!(!silent_pcm_prefix(&s, 12000, 12001));
    assert!(!silent_pcm_prefix(&s, 13000, 35000)); // exceeds the stereo inspection budget
}

#[test]
fn live_inputs_instruments_and_click_retain_the_located_start() {
    let mut s = guide();
    s.edit.click = true;
    assert!(!silent_pcm_prefix(&s, 0, 5000));
    s.edit.click = false;
    s.tracks[0].mixer.input_monitor = true;
    assert!(!silent_pcm_prefix(&s, 0, 5000));
    s.tracks[0].mixer.input_monitor = false;
    s.add_track(TrackKind::Instrument, ChannelFormat::Stereo, None);
    assert!(!silent_pcm_prefix(&s, 0, 5000));
}

#[test]
fn a_dry_lead_in_starts_the_same_pcm_with_a_locate_or_a_rolling_join() {
    use crate::midi::MidiFeed;
    let rate = FrameRate::Fps30;
    let render = |located: bool| {
        let mut s = session(rate);
        let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, None);
        let mut wave = vec![0.; 100000];
        for k in [12000, 36000, 60000, 84000] {
            wave[k..k + 8].fill(0.5);
        }
        let len = wave.len() as i64;
        s.pool.insert(SourceId(1), Arc::new(SourceAudio::new(craft_audio::AudioBuffer { sample_rate: 48000, channels: vec![wave] })));
        let id = s.new_clip_id();
        s.track_mut(t).unwrap().playlist_mut().unwrap().clips.push(Clip::audio(id, "guide", SourceId(1), 0, 0, len));
        // The hardware acceptance routes USB input to an armed, muted meter strip.
        let meter = s.add_track(TrackKind::Audio, ChannelFormat::Mono, None);
        let mixer = &mut s.track_mut(meter).unwrap().mixer;
        mixer.mute = true;
        mixer.record_arm = true;
        let (mut audio, tx, _) = callback(s, 48000.);
        let feed = Arc::new(MidiFeed::default());
        feed.set_mtc_source(Some(0));
        tx.send(Cmd::Midi(feed.clone())).unwrap();
        feed.set_clock(0);
        if located {
            feed.receive(0, &locate(0, rate));
        }
        let start = Instant::now();
        let mut piece = 0;
        let mut last = false;
        let mut hits = Vec::new();
        for sample in (0..110000).step_by(256) {
            let now = sample as i64 * 1000000 / 48000;
            while 100000 + piece * 1000000 / 120 <= now {
                feed.set_clock(100000 + piece * 1000000 / 120);
                feed.receive(0, &quarters(2 + 2 * (piece / 8), rate)[piece as usize % 8]);
                piece += 1;
            }
            feed.set_clock(now);
            audio.device_buffer_frames = 256;
            audio.device_timing = Some(DeviceTiming::new(start + Duration::from_micros(now as u64), Some(Duration::from_micros(15166)), 0));
            let mut pcm = [0.; 512];
            audio.fill(&mut pcm, 2);
            for (offset, pair) in pcm.as_chunks::<2>().0.iter().enumerate() {
                let high = pair[0].abs() > 0.03;
                if high && !last {
                    hits.push(sample + offset);
                }
                last = high;
            }
        }
        hits
    };
    let reference = render(false);
    assert_eq!(reference.len(), 4);
    assert!(reference.windows(2).all(|w| w[1] - w[0] == 24000));
    assert_eq!(render(true), reference);
}

#[test]
#[ignore = "opt-in timing profile; wall-clock quantiles depend on host load"]
fn bounded_prefix_cost_when_a_dry_stereo_guide_fills_the_scan_budget() {
    use std::hint::black_box;
    use std::time::Instant;
    let s = guide();
    for (label, from, to) in [("2x16384 zeros", 13000, 29384), ("2x16385 exceeds cap", 13000, 29385), ("nonzero second channel", 0, 12001)] {
        let mut elapsed = Vec::with_capacity(10000);
        for _ in 0..10000 {
            let t = Instant::now();
            black_box(silent_pcm_prefix(black_box(&s), from, to));
            elapsed.push(t.elapsed().as_nanos());
        }
        elapsed.sort_unstable();
        println!(
            "prefix {label}: {} calls; p50 {} ns p95 {} ns p99 {} ns max {} ns",
            elapsed.len(),
            elapsed[4999],
            elapsed[9499],
            elapsed[9899],
            elapsed[9999]
        );
    }
}
