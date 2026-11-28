use super::*;
use crate::sync_clock::{DeviceTiming, Publisher, View};
use crate::{AudioState, Cmd, Shared};
use std::time::Duration;

use std::sync::{
    Arc,
    atomic::Ordering,
    mpsc::{Sender, channel},
};

fn session(rate: FrameRate) -> Session {
    let mut s = Session::new("sync", soundcraft_time::SampleRate::HZ_48000);
    s.frame_rate = rate;
    s.timecode_start = 0;
    s
}
fn callback(s: Session, device_rate: f64) -> (AudioState, Sender<Cmd>, View) {
    let shared = Arc::new(Shared { position: 0.into(), playing: false.into(), speed: 1000.into(), meters: Default::default() });
    let (commands, rx) = channel();
    let state = AudioState::new(rx, shared, Arc::new(s), device_rate, channel().0);
    let view = View::default();
    assert!(commands.send(Cmd::Clock(Some(Publisher::new(view.clone(), Default::default())))).is_ok());
    (state, commands, view)
}
fn block(at: Instant, position: i64, frames: i64, playing: bool, pass: u64) -> Presentation {
    Presentation {
        generation: 0,
        sample_rate: soundcraft_time::SampleRate::HZ_48000,
        system_delay_samples: 0,
        pass,
        origin: position,
        playing,
        sample: position as f64,
        heard_at: at,
        samples_per_second: 48_000.,
        limit: (position + frames) as f64,
        looped: None,
        end: None,
        lead: Some(Duration::from_millis(30)),
    }
}

#[test]
fn generated_timecode_counts_audio_samples_when_device_and_host_clocks_differ() {
    let song = session(FrameRate::Fps30);
    let first = Instant::now();
    // In 600 host seconds these rates yield 28,798,848 / 28,800,000 / 28,801,152 samples.
    // At 30 fps each QF spans exactly 400 session samples. The upper limit is exclusive.
    for (actual_hz, expected) in [(47_998.08, 71_998), (48_000.0, 72_000), (48_001.92, 72_003)] {
        let mut encoder = Encoder::default();
        let mut b = block(first, 0, (600.0_f64 * actual_hz).round() as i64, true, 1);
        b.samples_per_second = actual_hz;
        let mut ordinal = 0_u32;
        let mut locates = 0;
        for ms in (0..=600_002).step_by(2) {
            encoder.update(b, first + Duration::from_millis(ms), &song, &mut |at, bytes| {
                if bytes[0] == 0xf0 {
                    locates += 1;
                } else {
                    assert_eq!(bytes[0], 0xf1);
                    assert_eq!(u32::from(bytes[1] >> 4), ordinal % 8);
                    let sample = (at.duration_since(first).as_secs_f64() * actual_hz).round() as u64;
                    assert_eq!(sample, u64::from(ordinal) * 400);
                    ordinal += 1;
                }
            });
        }
        assert_eq!((locates, ordinal), (1, expected));
    }
}

#[test]
fn slightly_late_first_presentation_never_bunches_quarter_frames() {
    use crate::sync_clock::SyncEncoder;
    let song = session(FrameRate::Fps30);
    let first = Instant::now();
    let mut b = block(first, 728, 48_000, true, 1);
    b.origin = 0;
    let mut mtc = Encoder::default();
    let mut qfs = Vec::new();
    for us in (0..=485_000).step_by(100) {
        let now = first + Duration::from_micros(us);
        mtc.update(b, now, &song, &mut |_, msg| {
            if msg.first() == Some(&0xf1) {
                qfs.push(us)
            }
        });
    }
    let min_qf = qfs.windows(2).map(|p| p[1] - p[0]).min().unwrap();
    assert!(min_qf >= 8_100, "minimum QF spacing {min_qf} us");
}

/// Runs a 25 fps generator (a quarter frame is 10 ms) whose worker first wakes 9 ms after
/// the origin, then on time except that the wake after the 11th quarter frame comes `late`.
/// Returns (handed off, stamp, message) for every message.
fn wake_late_once(late: Duration) -> (Encoder, Instant, Vec<(Instant, Instant, Vec<u8>)>) {
    use crate::sync_clock::SyncEncoder;
    let song = session(FrameRate::Fps25);
    let first = Instant::now();
    let b = block(first, 0, 48_000, true, 1);
    let mut mtc = Encoder::default();
    let mut sent = Vec::new();
    let mut now = first + Duration::from_millis(9);
    let mut delayed = false;
    while now < first + Duration::from_millis(500) {
        let Some(mut next) = mtc.update(b, now, &song, &mut |at, msg| sent.push((now, at, msg.to_vec()))) else { break };
        if !delayed && sent.iter().filter(|m| m.2[0] == 0xf1).count() == 11 {
            delayed = true;
            next += late;
        }
        now = next;
    }
    assert!(delayed);
    (mtc, first, sent)
}

#[test]
fn a_few_milliseconds_late_wake_keeps_the_group_and_catches_up_spaced() {
    // Piece 3 of the second group (grid 110 ms) is handed off at 125 ms.
    let (mut mtc, first, sent) = wake_late_once(Duration::from_millis(15));
    let qfs: Vec<_> = sent.iter().filter(|m| m.2[0] == 0xf1).collect();
    assert_eq!(sent.iter().filter(|m| m.2[0] == 0xf0).count(), 1, "{sent:?}");
    // No piece is skipped: grid stamps every 10 ms from the origin, in order.
    for (i, (_, at, msg)) in qfs.iter().enumerate() {
        assert_eq!(*at - first, Duration::from_millis(10 * i as u64), "{sent:?}");
        assert_eq!(usize::from(msg[1] >> 4), i % 8);
    }
    let handoffs: Vec<_> = qfs.windows(2).map(|p| p[1].0 - p[0].0).collect();
    assert!(handoffs.iter().all(|&gap| gap >= CATCH_UP_SPACING), "{handoffs:?}");
    assert!(handoffs.iter().any(|&gap| gap < Duration::from_millis(10)), "the late pieces caught up: {handoffs:?}");
    assert!(qfs.last().is_some_and(|m| m.0 == m.1), "back on the grid");
    assert_eq!(mtc.take_stale_dropped(), 0);
}

#[test]
fn a_wake_more_than_two_frames_late_sends_a_full_message_and_resumes_on_a_group() {
    // Piece 3 of the second group (grid 110 ms) would be handed off at 205 ms,
    // more than one group (80 ms) behind.
    let (mut mtc, first, sent) = wake_late_once(Duration::from_millis(95));
    let resync = sent.iter().skip(1).position(|m| m.2[0] == 0xf0).expect("a second Full Message") + 1;
    let (handed, _, full) = &sent[resync];
    // The Full Message carries the live frame (hh mm ss ff with the 25 fps rate bits).
    let frame = ((*handed - first).as_millis() / 40) as u8;
    assert_eq!(full[5..9], [0x20, 0, 0, frame]);
    // Quarter frames resume at the next group: piece 0 on an even frame boundary.
    let (_, at, msg) = &sent[resync + 1];
    let ms = (*at - first).as_millis();
    assert_eq!((msg[0], msg[1] >> 4, ms % 80), (0xf1, 0, 0), "{sent:?}");
    assert!(*at >= *handed && ms - (*handed - first).as_millis() <= 80);
    assert!(sent[resync + 1..].windows(2).all(|p| p[1].0 - p[0].0 >= Duration::from_millis(10) - Duration::from_micros(1)));
    assert!(mtc.take_stale_dropped() > 0);
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
            let presented = view.lock().unwrap().read(clock, true, 0).0.unwrap().sample_heard_at(clock).floor() as i64;
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
fn mtc_chase_waits_for_observed_lead_before_testing_presentation_drift() {
    let rate = FrameRate::Fps25;
    let (mut audio, tx, _) = callback(session(rate), 48_000.);
    let feed = Arc::new(crate::midi::MidiFeed::default());
    feed.set_mtc_source(Some(0));
    tx.send(Cmd::Midi(feed.clone())).unwrap();
    let now = Instant::now();
    // Model a backend with no usable timestamps, followed by an actual zero-lead report.
    // Each QF group advances 80 ms while our deliberately short callback renders only 5 ms.
    for group in 0..42 {
        for piece in 0..8 {
            let us = (group * 8 + piece) * 10_000;
            feed.set_clock(us);
            feed.receive(0, &quarters(group * 2, rate)[piece as usize]);
        }
        let measured = (group >= 22).then_some(Duration::ZERO);
        audio.device_timing = Some(DeviceTiming::new(now, measured, 0));
        audio.fill(&mut [0.; 480], 2);
        if group == 21 {
            assert!(audio.shared.playing.load(Ordering::Acquire), "input transport follows the MTC lock");
            assert_eq!(feed.mtc_diagnostics().hard_relocates, 0, "unknown audible phase cannot prove sustained drift");
        }
    }
    assert_eq!(feed.mtc_diagnostics().hard_relocates, 1, "measured zero is a valid phase reference");
}

#[test]
fn external_lock_starts_and_runs_at_unity_without_repaints() {
    for reject_next in [false, true] {
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
        let mut stream = crate::sync_clock::StreamLead::default();
        let first = stream.observe(now, Some(Duration::from_millis(30)));
        state.device_timing = Some(if reject_next { stream.observe(now, None) } else { first });
        state.fill(&mut [0.; 480], 2);
        assert!(state.shared.playing.load(Ordering::Relaxed), "device callback starts on lock");

        assert!((view.lock().unwrap().read(now + Duration::from_millis(30), true, 0).0.unwrap().sample - 7440.).abs() < 1., "piece 15 at 125 ms");
        assert_eq!(state.shared.speed.load(Ordering::Relaxed), 1000);
    }
}

#[test]
fn only_mtc_transitions_wake_the_host() {
    use std::sync::atomic::AtomicUsize;
    let feed = crate::midi::MidiFeed::default();
    let count = Arc::new(AtomicUsize::new(0));
    let called = count.clone();
    feed.set_mtc_wakeup(|| {
        Box::new(move || {
            called.fetch_add(1, Ordering::Relaxed);
        })
    });
    feed.set_mtc_source(Some(0));
    for n in 0..800 {
        feed.set_clock(n * 10_000);
        feed.receive(0, &quarters(n / 8 * 2, FrameRate::Fps25)[(n % 8) as usize]);
    }
    assert_eq!(count.load(Ordering::Relaxed), 1);
    feed.receive(0, &[0xfc]);
    feed.receive(0, &locate(1, FrameRate::Fps25));
    assert_eq!(count.load(Ordering::Relaxed), 3);
    feed.receive(0, &[0xf1, 0x7f]); // invalid unpaired final piece
    assert_eq!(count.load(Ordering::Relaxed), 3);
}

#[test]
fn external_timecode_before_session_start_preserves_the_cursor_and_recovers() {
    let rate = FrameRate::Fps25;
    let mut s = session(rate);
    s.timecode_start = s.sample_rate.samples(3600.);
    let (mut audio, tx, _) = callback(s, 48_000.);
    let feed = Arc::new(crate::midi::MidiFeed::default());
    feed.set_mtc_source(Some(0));
    tx.send(Cmd::Midi(feed.clone())).unwrap();
    // Player::locate publishes the cursor and queues the device command.
    audio.shared.position.store(48_000, Ordering::Relaxed);
    tx.send(Cmd::Locate(48_000)).unwrap();
    feed.set_clock(0);
    feed.receive(0, &locate(3590 * 25, rate));
    audio.fill(&mut [0.; 480], 2);
    assert_eq!(audio.shared.position.load(Ordering::Relaxed), 48_000, "before-start locate");
    for n in 0..16 {
        feed.set_clock(n * 10_000);
        feed.receive(0, &quarters(3590 * 25 + n / 8 * 2, rate)[(n % 8) as usize]);
    }
    audio.fill(&mut [0.; 480], 2);
    assert!(!audio.shared.playing.load(Ordering::Acquire));
    assert_eq!(audio.shared.position.load(Ordering::Relaxed), 48_000, "before-start roll");
    feed.receive(0, &locate(3600 * 25, rate));
    audio.fill(&mut [0.; 480], 2);
    assert_eq!(audio.shared.position.load(Ordering::Relaxed), 0);
    for n in 0..16 {
        feed.set_clock(200_000 + n * 10_000);
        feed.receive(0, &quarters(3600 * 25 + n / 8 * 2, rate)[(n % 8) as usize]);
    }
    audio.fill(&mut [0.; 480], 2);
    assert!(audio.shared.playing.load(Ordering::Acquire));
}

#[test]
fn going_offline_before_ui_observes_roll_stops_chase_and_permits_record() {
    let (mut state, tx, _) = callback(session(FrameRate::Fps25), 48_000.);
    let feed = Arc::new(crate::midi::MidiFeed::default());
    feed.set_mtc_source(Some(0));
    tx.send(Cmd::Midi(feed.clone())).unwrap();
    for n in 0..16 {
        feed.set_clock(n * 10_000);
        feed.receive(0, &quarters(n / 8 * 2, FrameRate::Fps25)[(n % 8) as usize]);
    }
    state.fill(&mut [0.; 480], 2);
    assert!(state.shared.playing.load(Ordering::Acquire));
    let actions = tx.clone();
    state.before_commands = Some(Box::new(move || {
        feed.set_mtc_source(None);
        assert!(actions.send(Cmd::EndChase(None)).is_ok());
    }));
    state.fill(&mut [0.; 480], 2);
    assert!(!state.shared.playing.load(Ordering::Acquire), "offline must halt an unobserved chase");
    state.before_commands = None;
    tx.send(Cmd::Recording(true, 1)).unwrap();
    state.fill(&mut [0.; 480], 2);
    assert!(state.is_recording());
    tx.send(Cmd::Play { from: 4800, end: None, looped: None }).unwrap();
    tx.send(Cmd::EndChase(None)).unwrap();
    state.fill(&mut [0.; 480], 2);
    assert!(state.shared.playing.load(Ordering::Acquire), "offline must preserve local playback");
}

#[test]
fn a_stale_ui_locate_cannot_rewind_a_newer_roll() {
    let (mut state, tx, _) = callback(session(FrameRate::Fps25), 48000.);
    let feed = Arc::new(crate::midi::MidiFeed::default());
    feed.set_mtc_source(Some(0));
    tx.send(Cmd::Midi(feed.clone())).unwrap();
    feed.set_clock(0);
    feed.receive(0, &locate(500, FrameRate::Fps25));
    state.fill(&mut [0.; 480], 2);
    let located = state.shared.position.load(Ordering::Acquire);
    for i in 0..16 {
        feed.set_clock(100_000 + i * 10000);
        feed.receive(0, &quarters(600 + i / 8 * 2, FrameRate::Fps25)[(i % 8) as usize]);
    }
    state.fill(&mut [0.; 480], 2);
    assert!(state.shared.playing.load(Ordering::Acquire));
    let before = state.shared.position.load(Ordering::Acquire);
    // The UI acts on the locate it observed before the roll began.
    tx.send(Cmd::Locate(located)).unwrap();
    state.fill(&mut [0.; 480], 2);
    assert!(state.shared.playing.load(Ordering::Acquire));
    assert_eq!(state.shared.position.load(Ordering::Acquire), before + 240);
}

#[test]
fn ui_stop_from_an_earlier_lock_cannot_restart_the_new_roll() {
    let (mut state, tx, _) = callback(session(FrameRate::Fps25), 48000.);
    let feed = Arc::new(crate::midi::MidiFeed::default());
    feed.set_mtc_source(Some(0));
    tx.send(Cmd::Midi(feed.clone())).unwrap();
    let roll = |base: i64| {
        for i in 0..16 {
            feed.set_clock(base + i * 10000);
            feed.receive(0, &quarters(i / 8 * 2, FrameRate::Fps25)[(i % 8) as usize]);
        }
    };
    roll(0);
    state.fill(&mut [0.; 480], 2);
    feed.receive(0, &[0xfc]);
    let (event, old_lock) = feed.take_mtc_with_lock().unwrap();
    assert_eq!(event, crate::mtc_input::ChaseEvent::Stop);
    state.fill(&mut [0.; 480], 2);
    roll(300000);
    state.fill(&mut [0.; 480], 2);
    assert!(state.shared.playing.load(Ordering::Acquire));
    let before = state.shared.position.load(Ordering::Acquire);
    let actions = tx.clone();
    state.before_commands = Some(Box::new(move || actions.send(Cmd::EndChase(Some(old_lock))).unwrap()));
    state.fill(&mut [0.; 480], 2);
    assert_eq!(state.shared.position.load(Ordering::Acquire), before + 240);
}

#[test]
fn a_before_session_observation_cannot_end_a_newer_roll() {
    let rate = FrameRate::Fps25;
    let mut s = session(rate);
    s.timecode_start = s.sample_rate.samples(3600.);
    let (mut state, tx, _) = callback(s, 48000.);
    let feed = Arc::new(crate::midi::MidiFeed::default());
    feed.set_mtc_source(Some(0));
    tx.send(Cmd::Midi(feed.clone())).unwrap();
    // The UI takes a locate before the session start, with the lock it belongs to.
    feed.set_clock(0);
    feed.receive(0, &locate(3590 * 25, rate));
    let (_, observed) = feed.take_mtc_with_lock().unwrap();
    state.fill(&mut [0.; 480], 2);
    assert!(!state.shared.playing.load(Ordering::Acquire));
    // Before the UI acts on it, a roll inside the session begins.
    feed.receive(0, &locate(3600 * 25, rate));
    for i in 0..16 {
        feed.set_clock(100_000 + i * 10000);
        feed.receive(0, &quarters(3600 * 25 + i / 8 * 2, rate)[(i % 8) as usize]);
    }
    state.fill(&mut [0.; 480], 2);
    assert!(state.shared.playing.load(Ordering::Acquire));
    let before = state.shared.position.load(Ordering::Acquire);
    // What the UI sends for the before-session observation leaves the newer roll playing.
    tx.send(Cmd::EndChase(Some(observed))).unwrap();
    state.fill(&mut [0.; 480], 2);
    assert!(state.shared.playing.load(Ordering::Acquire));
    assert_eq!(state.shared.position.load(Ordering::Acquire), before + 240);
}

#[test]
fn jam_sync_audio_free_runs_for_ten_seconds_and_seeks_only_on_the_second_new_group() {
    let (mut audio, tx, _) = callback(session(FrameRate::Fps25), 48000.);
    let feed = Arc::new(crate::midi::MidiFeed::default());
    feed.set_mtc_options(FrameRate::Fps25, 8, true, 0);
    feed.set_mtc_source(Some(0));
    tx.send(Cmd::Midi(feed.clone())).unwrap();
    audio.fill(&mut [0.; 480], 2);
    assert!(!audio.shared.playing.load(Ordering::Acquire));
    let mut first = 0;
    let mut before_jump = 0;
    for block in 0..=2094 {
        let piece = block / 2;
        feed.set_clock(block * 5000);
        if block % 2 == 0 && !(16..1016).contains(&piece) {
            let frame = piece / 8 * 2 + if piece >= 1032 { 25 } else { 0 };
            feed.receive(0, &quarters(frame, FrameRate::Fps25)[(piece % 8) as usize]);
        }
        audio.fill(&mut [0.; 480], 2);
        let cursor = audio.shared.position.load(Ordering::Acquire);
        match block {
            30 => {
                assert!(audio.shared.playing.load(Ordering::Acquire));
                first = cursor;
            }
            2030 => {
                assert_eq!(cursor - first, 480_000);
                assert_eq!(feed.mtc_diagnostics().freewheel_state, "jam sync running on internal clock");
            }
            2062 => {
                assert_eq!(feed.mtc_diagnostics().hard_relocates, 0);
                assert_eq!(feed.mtc_diagnostics().freewheel_state, "locked");
            }
            2078 => {
                assert_eq!(feed.mtc_diagnostics().hard_relocates, 0);
                before_jump = cursor;
            }
            2094 => {
                assert_eq!(feed.mtc_diagnostics().hard_relocates, 1);
                assert!(cursor - before_jump >= 48000);
            }
            _ => {}
        }
    }
}

#[test]
fn first_integer_sample_of_an_even_fractional_frame_emits_its_quarter_group() {
    let mut s = session(FrameRate::Fps29_97);
    s.timecode_start = s.frame_rate.samples_at_frame(2, s.sample_rate);
    let now = Instant::now();
    for at in [0, 1] {
        let mut enc = Encoder::default();
        let mut wire = vec![];
        let wake = enc.update(block(now, at, 48000, true, 1), now, &s, &mut |_, m| wire.push(m.to_vec()));
        assert_eq!(wire[0], locate(2, s.frame_rate));
        if at == 0 {
            assert_eq!(wire[1], quarters(2, s.frame_rate)[0]);
        } else {
            assert_eq!(wire.len(), 1);
            assert!(wake.unwrap() > now + Duration::from_millis(60));
        }
    }
}

#[cfg(test)]
mod anchored_start;
#[cfg(test)]
mod silent_preroll;
