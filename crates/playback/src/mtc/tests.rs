use super::*;
use crate::{AudioState, MeterSnapshot};
use soundcraft_time::{Range, SampleRate};
use std::sync::atomic::{AtomicI64, AtomicU32};
use std::sync::mpsc::{Sender, channel};

fn session(rate: FrameRate) -> Session {
    let mut session = Session::new("MTC", SampleRate::new(48_000).unwrap());
    session.frame_rate = rate;
    session
}

#[test]
fn rate_bits_midnight_and_drop_frame_minute_boundaries() {
    for (rate, bits, end) in [
        (FrameRate::Fps24, 0x00, 2_073_600),
        (FrameRate::Fps25, 0x20, 2_160_000),
        (FrameRate::Fps29_97Drop, 0x40, 2_589_408),
        (FrameRate::Fps30, 0x60, 2_592_000),
    ] {
        assert_eq!(locate(end, rate).bytes(), &[0xf0, 0x7f, 0x7f, 1, 1, bits, 0, 0, 0, 0xf7]);
        assert_eq!(locate(-1, rate), locate(end - 1, rate));
        assert_eq!(quarters(0, rate)[7], [0xf1, 0x70 | (bits >> 4)]);
    }
    assert_eq!(&locate(1800, FrameRate::Fps29_97Drop).bytes()[5..9], &[0x40, 1, 0, 2]);
    assert_eq!(&locate(17_982, FrameRate::Fps29_97Drop).bytes()[5..9], &[0x40, 10, 0, 0]);
    assert_eq!(quarters(1800, FrameRate::Fps29_97Drop)[0], [0xf1, 2]);
}

fn callback(s: Session, device_rate: f64) -> (AudioState, Sender<Cmd>, Arc<Queue>) {
    let shared = Arc::new(Shared {
        position: AtomicI64::new(0),
        playing: AtomicBool::new(false),
        speed: AtomicU32::new(1000),
        meters: Mutex::new(MeterSnapshot::default()),
        output_position: Mutex::default(),
    });
    let (commands, rx) = channel();
    let state = AudioState::new(rx, shared, Arc::new(s), device_rate, channel().0);
    let packets = Queue::new();
    let output = packets.clone();
    commands.send(Cmd::Mtc(Some(Generator::new(output)))).unwrap();
    (state, commands, packets)
}

#[test]
fn callback_generates_spaced_groups_at_session_time_and_stops_at_range_end() {
    for rate in [FrameRate::Fps24, FrameRate::Fps25, FrameRate::Fps29_97Drop, FrameRate::Fps30] {
        let mut s = session(rate);
        let frame = if rate.is_drop() { 1800 } else { rate.nominal() * 60 };
        s.timecode_start = rate.samples_at_frame(frame, s.sample_rate);
        let (mut state, commands, packets) = callback(s, 44_100.0);
        commands.send(Cmd::Play { from: 0, end: Some(7200), looped: None }).unwrap();
        state.fill(&mut vec![0.; 44_100 / 5 * 2], 2);
        let events: Vec<_> = std::iter::from_fn(|| packets.pop()).collect();
        assert_eq!(events.first().unwrap().1, locate(frame, rate));
        assert!(matches!(events.last().unwrap().1, Packet::Locate(_)));
        let quarters_out: Vec<_> = events.iter().filter(|(_, p)| matches!(p, Packet::Quarter(_))).collect();
        assert!(quarters_out.len() >= 8);
        for (index, (at, p)) in quarters_out[..8].iter().enumerate() {
            let (n, d) = rate.rational();
            let elapsed = at.duration_since(quarters_out[0].0).as_secs_f64();
            assert!((elapsed - index as f64 * d as f64 / (4 * n) as f64).abs() < 0.0001);
            assert_eq!(*p, Packet::Quarter(quarters(frame, rate)[index]));
        }
        state.fill(&mut [0.; 512], 2);
        assert!(packets.pop().is_none());
    }
}

#[test]
fn stopped_locate_play_wrap_seek_and_disable_use_the_callback_command_path() {
    let rate = FrameRate::Fps25;
    let (mut state, commands, packets) = callback(session(rate), 48_000.);
    commands.send(Cmd::Locate(48_000)).unwrap();
    state.fill(&mut [0.; 256], 2);
    assert_eq!(std::iter::from_fn(|| packets.pop()).map(|(_, p)| p).collect::<Vec<_>>(), [locate(25, rate)]);
    commands.send(Cmd::Play { from: 0, end: None, looped: Some(Range::new(0, 3840)) }).unwrap();
    state.fill(&mut vec![0.; 4000 * 2], 2);
    let events: Vec<_> = std::iter::from_fn(|| packets.pop()).map(|(_, p)| p).collect();
    assert_eq!(events.iter().filter(|p| **p == locate(0, rate)).count(), 2);
    assert_eq!(events[9], locate(0, rate));
    assert_eq!(events[10], Packet::Quarter([0xf1, 0]));
    commands.send(Cmd::Locate(1920)).unwrap();
    state.fill(&mut [0.; 256], 2);
    assert_eq!(packets.pop().unwrap().1, locate(1, rate));
    commands.send(Cmd::Stop).unwrap();
    state.fill(&mut [0.; 256], 2);
    assert!(matches!(packets.pop().unwrap().1, Packet::Locate(_)));
    commands.send(Cmd::Mtc(None)).unwrap();
    commands.send(Cmd::Play { from: 0, end: None, looped: None }).unwrap();
    state.fill(&mut [0.; 256], 2);
    assert!(packets.pop().is_none());
}

#[test]
fn unsupported_session_rate_sends_no_mislabeled_packets() {
    let (mut state, commands, packets) = callback(session(FrameRate::Fps60), 48_000.);
    commands.send(Cmd::Play { from: 0, end: None, looped: None }).unwrap();
    state.fill(&mut [0.; 1024], 2);
    assert!(packets.pop().is_none());
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn worker_delivers_each_packet_after_its_deadline_and_releases_the_port() {
    let (sent, received) = channel();
    let output = MtcOutput::with_sink(move |bytes| sent.send((Instant::now(), bytes.to_vec())).map_err(|e| e.to_string())).unwrap();
    let now = Instant::now();
    let later = now + Duration::from_millis(25);
    assert!(output.tx.push((now, locate(25, FrameRate::Fps25))));
    assert!(output.tx.push((later, Packet::Quarter([0xf1, 0x10]))));
    let timeout = Duration::from_secs(2);
    assert_eq!(received.recv_timeout(timeout).unwrap().1, locate(25, FrameRate::Fps25).bytes());
    let (at, bytes) = received.recv_timeout(timeout).unwrap();
    assert!(at >= later);
    assert_eq!(bytes, [0xf1, 0x10]);
    assert!(output.tx.push((Instant::now() + Duration::from_secs(60), Packet::Quarter([0xf1, 0x20]))));
    drop(output);
    assert!(matches!(received.try_recv(), Err(std::sync::mpsc::TryRecvError::Disconnected)));
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn delayed_port_resumes_at_a_complete_quarter_frame_group() {
    let (sent, received) = channel();
    let output = MtcOutput::with_sink(move |bytes| sent.send(bytes.to_vec()).map_err(|e| e.to_string())).unwrap();
    let now = Instant::now();
    assert!(output.tx.push((now - Duration::from_secs(1), Packet::Quarter([0xf1, 0x10]))));
    assert!(output.tx.push((now, Packet::Quarter([0xf1, 0x20]))));
    assert!(output.tx.push((now, Packet::Quarter([0xf1, 0x01]))));
    assert_eq!(received.recv_timeout(Duration::from_secs(2)).unwrap(), [0xf1, 0x01]);
}

#[test]
fn device_lead_and_chunk_offset_date_quarters_at_audio_presentation() {
    let (mut state, commands, queue) = callback(session(FrameRate::Fps30), 48_000.);
    let now = Instant::now();
    let lead = Duration::from_millis(30);
    commands.send(Cmd::Play { from: 0, end: None, looped: None }).unwrap();
    let mut events = Vec::new();
    for offset in [0, 8192] {
        state.device_timing = Some((now, lead, offset));
        state.fill(&mut vec![0.; 8192 * 2], 2);
        events.extend(std::iter::from_fn(|| queue.pop()).filter(|(_, p)| matches!(p, Packet::Quarter(_))));
    }
    assert_eq!(events.len(), 41);
    for (i, (stamp, _)) in events.iter().enumerate() {
        let expected = now + lead + Duration::from_secs_f64(i as f64 / 120.);
        assert!((*stamp).max(expected).duration_since((*stamp).min(expected)) < Duration::from_micros(1));
    }
}

#[test]
fn generator_and_reader_agree_on_time_at_the_seventh_piece() {
    use crate::mtc_input::{ChaseEvent, Reader};
    for rate in [FrameRate::Fps24, FrameRate::Fps25, FrameRate::Fps29_97Drop, FrameRate::Fps30] {
        let s = session(rate);
        let q = Queue::new();
        let now = Instant::now();
        Generator::new(q.clone()).span(&s, 0., 16_000, 1., 48_000., now);
        let mut reader = Reader::default();
        let mut groups = 0;
        while let Some((stamp, packet)) = q.pop() {
            if !matches!(packet, Packet::Quarter(_)) {
                continue;
            }
            let elapsed = stamp.duration_since(now);
            let us = elapsed.as_micros() as i64;
            reader.read(packet.bytes(), us);
            if let Some(ChaseEvent::Roll(p, _)) = reader.take(us) {
                assert!(p.samples(s.sample_rate).abs_diff(s.sample_rate.samples(elapsed.as_secs_f64())) <= 1);
                groups += 1;
            }
        }
        assert!(groups >= 2);
    }
}

#[test]
fn locked_device_tracks_and_stops_during_a_ui_stall() {
    use crate::midi::MidiFeed;
    use soundcraft_model::{ChannelFormat, Clip, SourceAudio, SourceId, TrackKind};
    let mut s = session(FrameRate::Fps30);
    let track = s.add_track(TrackKind::Audio, ChannelFormat::Mono, None);
    let mut wave = vec![0.; 288_000];
    for (onset, hz) in [(225_600, 261.63), (240_000, 440.), (264_000, 739.99)] {
        for (n, v) in wave[onset..onset + 1440].iter_mut().enumerate() {
            *v = (std::f32::consts::TAU * hz * n as f32 / 48_000.).sin() * 0.5;
        }
    }
    s.pool.insert(SourceId(1), Arc::new(SourceAudio::new(craft_audio::AudioBuffer { sample_rate: 48_000, channels: vec![wave] })));
    let id = s.new_clip_id();
    s.track_mut(track).unwrap().playlist_mut().unwrap().clips.push(Clip::audio(id, "timing", SourceId(1), 0, 0, 288_000));
    let (mut audio, commands, _) = callback(s, 48_000.);
    let shared = audio.shared.clone();
    let feed = Arc::new(MidiFeed::default());
    feed.set_mtc_source(Some(0));
    commands.send(Cmd::Midi(feed.clone())).unwrap();
    commands.send(Cmd::Mtc(None)).unwrap();
    let now = Instant::now();
    let lead = Duration::from_millis(25);
    // Two groups establish lock. The first audio block starts exactly at the locate.
    feed.set_clock(0);
    feed.receive(0, locate(150, FrameRate::Fps30).bytes());
    let mut piece = 0;
    let mut buffer = [0.; 480]; // 5 ms of stereo
    let mut started = None;
    let mut attacks = Vec::new();
    let mut last_sound = -999_999;
    let mut phase = None;
    for block in 0..=2400 {
        let us = block * 5000;
        while piece * 1_000_000 / 120 <= us {
            let time = piece * 1_000_000 / 120;
            feed.set_clock(time);
            let group = quarters(152 + 2 * (piece / 8), FrameRate::Fps30);
            feed.receive(0, &group[(piece % 8) as usize]);
            piece += 1;
        }
        feed.set_clock(us);
        if started.is_none() && piece >= 16 {
            started = Some(us);
        }
        let clock = now + Duration::from_micros(us as u64);
        audio.device_timing = Some((clock, lead, 0));
        audio.fill(&mut buffer, 2);
        if let Some(frame) = buffer.as_chunks::<2>().0.iter().position(|pair| pair[0].abs() > 0.03) {
            let onset = us + frame as i64 * 1_000_000 / 48_000;
            if onset - last_sound > 150_000 {
                attacks.push(onset);
            }
            last_sound = onset;
        }
        audio.publish_output_position(clock, lead, 240);
        if started.is_some() {
            let presented = shared.presented_position_at(clock);
            if us == 800_000 {
                assert_eq!(feed.mtc_diagnostics().hard_relocates, 0, "sub-second drift must not seek");
            }
            if us == 11_935_000 {
                phase = Some(presented - ((152. * 1600.) as i64 + us * 48_000 / 1_000_000));
            }
        }
    }
    assert!(phase.unwrap().unsigned_abs() <= 480, "17 s phase: {phase:?} samples");
    assert_eq!(attacks.len(), 2, "unexpected rendered attacks: {attacks:?}");
    assert!((attacks[0] - started.unwrap()).abs() < 50_000);
    let interval = attacks[1] - attacks[0];
    assert!((interval - 500_000).abs() < 15_000, "interval {interval} us");
    assert_eq!(feed.mtc_diagnostics().hard_relocates, 1, "the protected anchor caused exactly one relaunch after sustained drift");
    assert_eq!(shared.speed.load(Ordering::Relaxed), 1000);
    feed.set_clock(12_301_000);
    audio.fill(&mut buffer, 2);
    assert!(!shared.playing.load(Ordering::Relaxed));
    assert_eq!(shared.speed.load(Ordering::Relaxed), 1000);
    println!("MTC callback-only phase={} samples, first-to-second interval={} us", phase.unwrap(), interval);
}

#[test]
fn recording_request_is_ignored_while_waiting_for_external_timecode() {
    let (mut state, cmd, _) = callback(session(FrameRate::Fps30), 48_000.);
    let feed = Arc::new(crate::midi::MidiFeed::default());
    feed.set_mtc_source(Some(0));
    cmd.send(Cmd::Midi(feed.clone())).unwrap();
    cmd.send(Cmd::Recording(true, 0)).unwrap();
    state.fill(&mut [0.; 480], 2);
    assert!(!state.is_recording());
}

#[test]
fn bad_device_rate_cannot_panic_during_position_publication() {
    let (mut audio, cmd, _) = callback(session(FrameRate::Fps30), 48_000.);
    audio.shared.playing.store(true, Ordering::Relaxed);
    cmd.send(Cmd::PlayMtc(0)).unwrap();
    audio.fill(&mut [0.; 128], 2);
    for rate in [0., f64::NAN] {
        audio.device_rate = rate;
        audio.publish_output_position(Instant::now(), Duration::ZERO, 128);
    }
}

#[test]
fn saturated_output_queue_restarts_at_a_complete_group() {
    let q = Queue::new();
    let mut generator = Generator::new(q.clone());
    let session = session(FrameRate::Fps30);
    let start = Instant::now();
    generator.span(&session, 0., 96_000, 1., 48_000., start);
    assert!(generator.next.is_none());
    let mut count = 0;
    let mut previous = start;
    while let Some((when, _)) = q.pop() {
        assert!(when >= previous);
        previous = when;
        count += 1;
    }
    assert!(count > 8);
    generator.span(&session, 96_000., 8192, 1., 48_000., start + Duration::from_secs(2));
    assert_eq!(q.pop().unwrap().1, locate(60, FrameRate::Fps30));
    for packet in quarters(60, FrameRate::Fps30) {
        assert_eq!(q.pop().unwrap().1, Packet::Quarter(packet));
    }
}

#[test]
fn pulled_down_outputs_use_fractional_cadence_and_nominal_labels() {
    for rate in [FrameRate::Fps23_976, FrameRate::Fps29_97] {
        let s = session(rate);
        let q = Queue::new();
        let now = Instant::now();
        let one_hour = 3600 * rate.nominal();
        let start = rate.samples_at_frame(one_hour, s.sample_rate);
        Generator::new(q.clone()).span(&s, start as f64, 20_000, 1., 48_000., now);
        let (_, packet) = q.pop().expect("pull-down generator must send a locate");
        let hour = if rate == FrameRate::Fps23_976 { 1 } else { 0x61 };
        assert_eq!(packet.bytes(), &[0xf0, 0x7f, 0x7f, 1, 1, hour, 0, 0, 0, 0xf7]);
        let (first, _) = q.pop().unwrap();
        let (second, _) = q.pop().unwrap();
        assert!((second.duration_since(first).as_secs_f64() - 1.001 / (4. * rate.nominal() as f64)).abs() < 1e-8);
    }
}

#[test]
fn external_lock_starts_and_runs_at_unity_without_repaints() {
    let (mut state, cmd, _) = callback(session(FrameRate::Fps30), 48_000.);
    let feed = Arc::new(crate::midi::MidiFeed::default());
    feed.set_mtc_source(Some(0));
    cmd.send(Cmd::Midi(feed.clone())).unwrap();
    let now = Instant::now();
    for piece in 0..16 {
        let us = piece * 1_000_000 / 120;
        feed.set_clock(us);
        feed.receive(0, &quarters(2 * (piece / 8), FrameRate::Fps30)[(piece % 8) as usize]);
    }
    state.device_timing = Some((now, Duration::from_millis(30), 0));
    state.fill(&mut [0.; 480], 2);
    assert!(state.shared.playing.load(Ordering::Relaxed), "device callback starts on lock");
    state.publish_output_position(now, Duration::from_millis(30), 240);
    assert!(state.shared.presented_position_at(now).abs_diff(6000) <= 1, "piece 15 at 125 ms");
    assert_eq!(state.shared.speed.load(Ordering::Relaxed), 1000);
}
