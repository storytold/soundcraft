use super::*;
use crate::sync_clock::{DeviceTiming, Publisher, View};
use crate::{AudioState, Cmd, Shared};
use soundcraft_time::SampleRate;
use std::sync::{
    Arc,
    atomic::Ordering,
    mpsc::{Sender, channel},
};

fn session(rate: FrameRate) -> Session {
    let mut s = Session::new("sync", SampleRate::new(48_000).unwrap());
    s.frame_rate = rate;
    s.timecode_start = 0;
    s
}
fn callback(s: Session, device_rate: f64) -> (AudioState, Sender<Cmd>, View) {
    let shared = Arc::new(Shared { position: 0.into(), playing: false.into(), speed: 1000.into(), meters: Default::default() });
    let (commands, rx) = channel();
    let state = AudioState::new(rx, shared, Arc::new(s), device_rate, channel().0);
    let view = View::default();
    commands.send(Cmd::Clock(Some(Publisher::new(view.clone(), Default::default())))).unwrap();
    (state, commands, view)
}
fn block(at: Instant, position: i64, frames: i64, playing: bool, pass: u64) -> Presentation {
    Presentation {
        pass,
        origin: position,
        playing,
        sample: position as f64,
        heard_at: at,
        samples_per_second: 48_000.,
        limit: (position + frames) as f64,
        looped: None,
        end: None,
        lead: Duration::from_millis(30),
    }
}

#[test]
fn all_six_rates_align_eight_wire_pieces_and_session_offset_to_presentation() {
    for rate in [FrameRate::Fps23_976, FrameRate::Fps24, FrameRate::Fps25, FrameRate::Fps29_97, FrameRate::Fps29_97Drop, FrameRate::Fps30] {
        let mut s = session(rate);
        s.timecode_start = rate.samples_at_frame(3600 * rate.nominal(), s.sample_rate);
        let mut enc = Encoder::default();
        let callback = Instant::now();
        let heard = callback + Duration::from_millis(30);
        let b = block(heard, 0, 48_000, true, 0);
        let mut sent = vec![];
        let mut now = heard + Duration::from_micros(50);
        for _ in 0..24 {
            let next = enc.update(b, now, &s, &mut |at, m| sent.push((at, m.to_vec())));
            now = next.unwrap();
        }
        assert_eq!(sent[0].1, locate(3600 * rate.nominal(), rate));
        let quarter = rate.samples_per_frame(s.sample_rate) / 4.;
        let mut reader = crate::mtc_input::Reader::default();
        for (i, (stamp, msg)) in sent[1..].iter().enumerate() {
            let since = stamp.duration_since(heard);
            let expected = i as f64 * quarter / 48_000.;
            assert!((since.as_secs_f64() - expected).abs() < 1e-7, "{rate:?} {i} {since:?}");
            assert_eq!(*msg, quarters(3600 * rate.nominal() + 2 * (i as i64 / 8), rate)[i % 8]);
            let us = since.as_micros() as i64;
            reader.read(msg, us);
            if i >= 15 && i % 8 == 7 {
                let Some(crate::mtc_input::ChaseEvent::Roll(p, _)) = reader.take(us) else {
                    panic!("reader did not lock at piece {i}");
                };
                assert!(p.in_session_rate(rate).samples(s.sample_rate).abs_diff(s.timecode_start + s.sample_rate.samples(expected)) <= 1);
            }
        }
    }
}

#[test]
fn stopped_locates_drop_frame_labels_and_delayed_worker_resumes_a_whole_group() {
    let s = session(FrameRate::Fps29_97Drop);
    let now = Instant::now();
    let mut enc = Encoder::default();
    for (i, (frame, label)) in
        [(1799, [0x40, 0, 59, 29]), (1800, [0x40, 1, 0, 2]), (17_981, [0x40, 9, 59, 29]), (17_982, [0x40, 10, 0, 0])].into_iter().enumerate()
    {
        let b = block(now, s.frame_rate.samples_at_frame(frame, s.sample_rate), 0, false, i as u64);
        let mut messages = vec![];
        for _ in 0..2 {
            assert_eq!(enc.update(b, now, &s, &mut |_, m| messages.push(m.to_vec())), None);
        }
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0][5..9], label);
    }
    let b = block(now, 0, 48_000, true, 5);
    let mut messages = vec![];
    enc.update(b, now, &s, &mut |_, m| messages.push(m.to_vec()));
    let new = enc.update(b, now + Duration::from_millis(350), &s, &mut |_, m| messages.push(m.to_vec())).unwrap();
    enc.update(b, new, &s, &mut |_, m| messages.push(m.to_vec()));
    assert!(enc.stale > 0);
    assert_eq!(messages.last().unwrap()[1] >> 4, 0);
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
    let (mut audio, commands, view) = callback(s, 48_000.);
    let shared = audio.shared.clone();
    let feed = Arc::new(MidiFeed::default());
    feed.set_mtc_source(Some(0));
    commands.send(Cmd::Midi(feed.clone())).unwrap();
    let now = Instant::now();
    let lead = Duration::from_millis(25);
    // Two groups establish lock. The first audio block starts exactly at the locate.
    feed.set_clock(0);
    feed.receive(0, &locate(150, FrameRate::Fps30));
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
        audio.device_timing = Some(DeviceTiming::new(clock, Some(lead), 0));
        audio.fill(&mut buffer, 2);
        if let Some(frame) = buffer.as_chunks::<2>().0.iter().position(|pair| pair[0].abs() > 0.03) {
            let onset = us + frame as i64 * 1_000_000 / 48_000;
            if onset - last_sound > 150_000 {
                attacks.push(onset);
            }
            last_sound = onset;
        }
        if started.is_some() {
            let presented = view.lock().unwrap().read(clock, true, 0.).0.unwrap().sample_heard_at(clock).floor() as i64;
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
fn external_lock_starts_and_runs_at_unity_without_repaints() {
    let (mut state, cmd, view) = callback(session(FrameRate::Fps30), 48_000.);
    let feed = Arc::new(crate::midi::MidiFeed::default());
    feed.set_mtc_source(Some(0));
    cmd.send(Cmd::Midi(feed.clone())).unwrap();
    let now = Instant::now();
    for piece in 0..16 {
        let us = piece * 1_000_000 / 120;
        feed.set_clock(us);
        feed.receive(0, &quarters(2 * (piece / 8), FrameRate::Fps30)[(piece % 8) as usize]);
    }
    state.device_timing = Some(DeviceTiming::new(now, Some(Duration::from_millis(30)), 0));
    state.fill(&mut [0.; 480], 2);
    assert!(state.shared.playing.load(Ordering::Relaxed), "device callback starts on lock");

    assert!((view.lock().unwrap().read(now + Duration::from_millis(30), true, 0.).0.unwrap().sample - 7440.).abs() < 1., "piece 15 at 125 ms");
    assert_eq!(state.shared.speed.load(Ordering::Relaxed), 1000);
}
