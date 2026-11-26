use super::*;
use crate::sync_clock::Presentation;
use std::sync::mpsc::channel;
type Handoff = (Instant, Instant, Vec<u8>);
struct Capture(Arc<Mutex<Vec<Handoff>>>);
impl SyncPort for Capture {
    fn send_at(&mut self, at: Instant, bytes: &[u8]) -> Result<Instant, String> {
        let actual = Instant::now();
        self.0.lock().unwrap().push((at, actual, bytes.to_vec()));
        Ok(actual)
    }
}

#[test]
fn an_mtc_route_is_cached_only_after_the_worker_accepts_it() {
    let mut output = Output::default();
    let port = Some(("model".to_owned(), 240));
    assert!(!output.encoder(port.clone(), true, Box::<crate::mtc::Encoder>::default()));
    assert!(output.routes[RouteKind::Mtc.index()].ports.is_empty(), "no session or worker accepted this route");

    let (tx, rx) = channel();
    let worker = std::thread::spawn(|| {});
    output.worker = Some((WorkerSender { tx, worker: worker.thread().clone() }, worker));
    assert!(output.encoder(port.clone(), true, Box::<crate::mtc::Encoder>::default()));
    assert!(matches!(rx.try_recv(), Ok(Update::Encoder(name, true, _)) if name == port));
    assert!(output.routes[RouteKind::Mtc.index()] == Routes { ports: vec![("model".to_owned(), 240)], compensate: true });
}

#[test]
fn a_burst_of_external_locates_queues_one_transport_refresh() {
    let output = Output::default();
    let wake = output.transport_waker();
    let (tx, rx) = channel();
    *output.wake_tx.lock().unwrap() = Some(WorkerSender { tx, worker: std::thread::current() });
    use crate::mtc_input::{ChaseEvent, Reader};
    let mut reader = Reader::default();
    reader.receive(&[0xf0, 0x7f, 0x7f, 1, 1, 0, 0, 1, 0, 0xf7], 0, 0);
    let locate = reader.pending.unwrap();
    let ChaseEvent::Locate(position) = locate else {
        panic!("expected full frame");
    };
    let stopped = super::tests::block(Instant::now(), 0., 0., Duration::ZERO);
    output.view.lock().unwrap().push(Presentation { pass: 5, playing: false, ..stopped });
    wake(ChaseEvent::Stop);
    assert!(rx.try_recv().is_err());
    for _ in 0..100 {
        wake(locate);
    }
    output.view.lock().unwrap().push(Presentation { pass: 6, playing: true, ..stopped });
    let Ok(Update::TransportWake(pending, Some((5, false)))) = rx.try_recv() else {
        panic!("expected the transport state before the callback consumed the locate");
    };
    assert!(rx.try_recv().is_err());
    pending.store(false, Relaxed);
    wake(ChaseEvent::Roll(position, 0));
    assert!(rx.try_recv().is_err());
    wake(ChaseEvent::Stop);
    assert!(matches!(rx.try_recv(), Ok(Update::TransportWake(_, Some((6, true))))));
}

#[test]
fn both_encoders_use_one_connection_and_the_same_offset() {
    let now = Instant::now();
    let lead = Duration::from_millis(30);
    let due = now + Duration::from_millis(25);
    let mut map = BTreeMap::new();
    set_route(
        &mut map,
        RouteKind::BeatClock.index(),
        "device".to_owned(),
        Route { map: None, offset: -240, compensate: true, encoder: Box::<crate::beat_clock::Cursor>::default() },
    );
    set_route(
        &mut map,
        RouteKind::Mtc.index(),
        "device".to_owned(),
        Route { map: None, offset: -240, compensate: true, encoder: Box::<crate::mtc::Encoder>::default() },
    );
    assert_eq!(map.len(), 1);
    let messages = Arc::default();
    let p = map.get_mut("device").unwrap();
    p.link = Some(Capture(Arc::clone(&messages)));
    let v = View::default();
    v.lock().unwrap().push(Presentation {
        generation: 0,
        sample_rate: soundcraft_time::SampleRate::HZ_48000,
        system_delay_samples: 0,
        pass: 1,
        origin: 0,
        playing: true,
        sample: 0.,
        limit: 4800.,
        heard_at: now + lead,
        samples_per_second: 48_000.,
        looped: None,
        end: None,
        lead: Some(lead),
    });
    let mut s = Session::new("coexisting encoders", soundcraft_time::SampleRate::HZ_48000);
    s.frame_rate = soundcraft_time::FrameRate::Fps30;
    s.timecode_start = 0;
    for i in 0..100 {
        p.poll("device", due + Duration::from_millis(i), &s, &v, &Stats::default(), &mut Vec::new());
    }
    let data = messages.lock().unwrap();
    assert!(data.windows(2).all(|r| r[0].0 <= r[1].0));
    assert!(data.iter().any(|(t, _, b)| *t == due && b == &[0xf8]));
    assert!(data.iter().any(|(t, _, b)| *t == due && b == &[0xf1, 0]));
    assert_eq!(data.iter().filter(|(_, _, m)| m[0] == 0xf1).count(), 12);
}

#[test]
fn generator_waits_for_observed_presentation_only_when_compensation_is_enabled() {
    let now = Instant::now();
    let mut song = Session::new("MTC output", soundcraft_time::SampleRate::HZ_48000);
    song.frame_rate = soundcraft_time::FrameRate::Fps25;
    song.timecode_start = 0;
    for compensate in [true, false] {
        let mut routes = BTreeMap::new();
        set_route(
            &mut routes,
            RouteKind::Mtc.index(),
            "MTC".into(),
            Route { map: None, offset: 0, compensate, encoder: Box::<crate::mtc::Encoder>::default() },
        );
        let port = routes.get_mut("MTC").unwrap();
        let capture = Arc::default();
        port.link = Some(Capture(Arc::clone(&capture)));
        let view = View::default();
        let block = super::tests::block(now, 0., 4800., Duration::ZERO);
        view.lock().unwrap().push(Presentation { lead: None, ..block });
        port.poll("MTC", now, &song, &view, &Stats::default(), &mut Vec::new());
        assert_eq!(capture.lock().unwrap().is_empty(), compensate);

        // A real zero report allows immediate delivery, even on the compensated route.
        let next = now + Duration::from_millis(10);
        view.lock().unwrap().push(Presentation { heard_at: next, sample: 480., ..block });
        port.poll("MTC", next, &song, &view, &Stats::default(), &mut Vec::new());
        assert!(capture.lock().unwrap().iter().any(|(_, _, msg)| msg[0] == 0xf1));
    }
}

#[test]
fn mtc_quarters_use_the_mixer_delay_and_report_limited_negative_advances() {
    use soundcraft_time::{FrameRate, SampleRate};
    let base = Instant::now();
    let lead = Duration::from_millis(20);
    let mut session = Session::new("generator delay", SampleRate::HZ_48000);
    session.frame_rate = FrameRate::Fps25;
    session.timecode_start = 0;
    for (offset, delay, expected_us, limited) in
        [(-4800, 0, 1000, true), (-4800, 2400, 1000, true), (0, 2400, 70000, false), (480, 2400, 80000, false)]
    {
        let cable = View::default();
        let mut routes = BTreeMap::new();
        set_route(
            &mut routes,
            RouteKind::Mtc.index(),
            "MTC".into(),
            Route { map: None, offset, compensate: true, encoder: Box::<crate::mtc::Encoder>::default() },
        );
        let p = routes.get_mut("MTC").unwrap();
        let packets = Arc::new(Mutex::new(Vec::new()));
        p.link = Some(Capture(packets.clone()));
        let stats = Stats::default();
        for ms in 0..=500 {
            let now = base + Duration::from_millis(ms);
            if ms % 10 == 0 {
                cable.lock().unwrap().push(Presentation {
                    generation: 0,
                    sample_rate: session.sample_rate,
                    system_delay_samples: delay,
                    pass: 1,
                    origin: 0,
                    playing: true,
                    sample: ms as f64 * 48.,
                    limit: ms as f64 * 48. + 480.,
                    heard_at: now + lead,
                    lead: Some(lead),
                    samples_per_second: 48000.,
                    looped: None,
                    end: None,
                });
            }
            p.poll("MTC", now, &session, &cable, &stats, &mut Vec::new());
        }
        let onsets: Vec<_> =
            packets.lock().unwrap().iter().filter(|(_, _, b)| b[0] == 0xf1).map(|(at, _, _)| at.duration_since(base).as_micros() as i64).collect();
        assert!(onsets.len() > 40, "{onsets:?}");
        for (index, onset) in onsets.into_iter().enumerate() {
            assert!(onset.abs_diff(expected_us + index as i64 * 10000) <= 1, "offset {offset}, delay {delay}, quarter {index}, onset {onset}");
        }
        let mut output = Output::default();
        output.routes[RouteKind::Mtc.index()] = Routes { ports: vec![("MTC".into(), offset)], compensate: true };
        crate::sync_clock::Publisher::new(output.view.clone(), output.metrics.clone())
            .observe(crate::sync_clock::DeviceTiming::new(base, Some(lead), 0), 0.005);
        output.view = cable.clone();
        let status = output.status();
        assert_eq!(status.ports["MTC"].mtc_total_us, Some(expected_us));
        assert_eq!(status.ports["MTC"].advance_limited, limited);
    }
}

#[test]
fn mtc_rate_and_start_change_at_the_first_rendered_block_of_the_new_generation() {
    use soundcraft_time::{FrameRate, SampleRate};
    let base = Instant::now();
    let mut first = Session::new("old", SampleRate::HZ_48000);
    first.timecode_start = 0;
    first.frame_rate = FrameRate::Fps30;
    let first = Arc::new(first);
    let mut edited = (*first).clone();
    edited.timecode_start = 48000 * 3600;
    edited.frame_rate = FrameRate::Fps25;
    let edited = Arc::new(edited);
    let view = View::default();
    view.lock().unwrap().register(SessionRevision { generation: 1, doc: first, remap: true }, true);
    let mut routes = BTreeMap::new();
    set_route(
        &mut routes,
        RouteKind::Mtc.index(),
        "MTC".into(),
        Route { map: None, offset: 0, compensate: false, encoder: Box::<crate::mtc::Encoder>::default() },
    );
    let cable = routes.get_mut("MTC").unwrap();
    let packets = Arc::new(Mutex::new(Vec::new()));
    cable.link = Some(Capture(packets.clone()));
    let stats = Stats::default();
    for ms in 0..220u64 {
        let now = base + Duration::from_millis(ms);
        if ms == 50 {
            view.lock().unwrap().register(SessionRevision { generation: 2, doc: edited.clone(), remap: true }, true);
        }
        if ms % 10 == 0 {
            view.lock().unwrap().push(Presentation {
                generation: if ms >= 150 { 2 } else { 1 },
                sample_rate: SampleRate::HZ_48000,
                system_delay_samples: 0,
                pass: 1,
                origin: 0,
                playing: true,
                sample: ms as f64 * 48.,
                limit: ms as f64 * 48. + 480.,
                heard_at: now,
                lead: Some(Duration::ZERO),
                samples_per_second: 48000.,
                looped: None,
                end: None,
            });
        }
        cable.poll("MTC", now, &edited, &view, &stats, &mut Vec::new());
    }
    let locates: Vec<_> =
        packets.lock().unwrap().iter().filter(|(_, _, b)| b[0] == 0xf0).map(|(at, _, b)| (at.duration_since(base).as_millis(), b[5])).collect();
    assert_eq!(locates, vec![(0, 0x60), (150, 0x21)]);
}

/// Route the real encoder through the worker's SyncPort boundary into a second
/// transport. Each device queues 30 ms of audio after its callback. Compare the
/// sound heard at the same instant rather than either device's render cursor.
#[test]
fn generator_and_slave_agree_on_heard_time_after_acquisition() {
    use crate::midi::MidiFeed;
    use crate::sync_clock::{DeviceTiming, Publisher};
    use crate::{AudioState, Cmd, Shared};
    use soundcraft_time::{FrameRate, SampleRate};

    struct Cable {
        input: Arc<MidiFeed>,
        epoch: Instant,
    }
    impl SyncPort for Cable {
        fn send_at(&mut self, at: Instant, bytes: &[u8]) -> Result<Instant, String> {
            self.input.set_clock(at.duration_since(self.epoch).as_micros() as i64);
            self.input.receive(0, bytes);
            Ok(at)
        }
    }

    let mut sess = Session::new("MTC loopback", SampleRate::HZ_48000);
    sess.frame_rate = FrameRate::Fps25;
    sess.timecode_start = 0;
    let session = Arc::new(sess);
    let input = Arc::new(MidiFeed::default());
    input.set_mtc_options(session.frame_rate, 8, false, 0);
    input.set_mtc_source(Some(0));

    let now = Instant::now();
    let lead = Duration::from_millis(30);
    let mut routes = BTreeMap::new();
    set_route(
        &mut routes,
        RouteKind::Mtc.index(),
        "cable".into(),
        Route { map: None, offset: 0, compensate: true, encoder: Box::<crate::mtc::Encoder>::default() },
    );
    let port = routes.get_mut("cable").unwrap();
    port.link = Some(Cable { input: input.clone(), epoch: now });
    let master = View::default();
    let slave = View::default();
    let shared = Arc::new(Shared { position: 0.into(), playing: false.into(), speed: 1000.into(), meters: Default::default() });
    let (tx, rx) = channel();
    let mut audio = AudioState::new(rx, shared, session.clone(), 48_000., channel().0);
    tx.send(Cmd::Midi(input.clone())).unwrap();
    tx.send(Cmd::Clock(Some(Publisher::new(slave.clone(), Default::default())))).unwrap();
    let stats = Stats::default();
    let mut pending = Vec::new();
    let mut worst = 0.;
    for i in 0..=600 {
        let sample = (i * 240) as f64;
        let clock = now + Duration::from_millis(i as u64 * 5);
        master.lock().unwrap().push(Presentation {
            generation: 0,
            sample_rate: soundcraft_time::SampleRate::HZ_48000,
            system_delay_samples: 0,
            pass: 1,
            origin: 0,
            playing: true,
            sample,
            limit: sample + 240.,
            heard_at: clock + lead,
            samples_per_second: 48_000.,
            looped: None,
            end: None,
            lead: Some(lead),
        });
        port.poll("cable", clock, &session, &master, &stats, &mut pending);
        input.set_clock(i as i64 * 5000);
        audio.device_timing = Some(DeviceTiming::new(clock, Some(lead), 0));
        audio.fill(&mut [0.; 480], 2);
        if i >= 300 {
            let audible = |v: &View| v.lock().unwrap().read(clock, true, 0).0.unwrap().sample_heard_at(clock);
            worst = f64::max(worst, (audible(&master) - audible(&slave)).abs());
        }
    }
    assert_eq!(input.mtc_diagnostics().locks, 1);
    assert_eq!(input.mtc_diagnostics().hard_relocates, 1);
    assert!(worst <= 48., "master/slave heard position varies by {worst} samples at 48 kHz");
}
#[test]
fn recovered_mtc_locates_live_and_starts_a_complete_quarter_frame_group() {
    for (loss_ms, recovery_ms) in [(20, 50), (100, 200)] {
        let base = Instant::now();
        let mut song = Session::new("MTC recovery", soundcraft_time::SampleRate::HZ_48000);
        song.frame_rate = soundcraft_time::FrameRate::Fps25;
        song.timecode_start = 0;
        let mut routes = BTreeMap::new();
        set_route(
            &mut routes,
            RouteKind::Mtc.index(),
            "desk".into(),
            Route { map: None, offset: 0, compensate: true, encoder: Box::<crate::mtc::Encoder>::default() },
        );
        let capture = Arc::default();
        let port = routes.get_mut("desk").unwrap();
        port.link = Some(Capture(Arc::clone(&capture)));
        let view = View::default();
        let stats = Stats::default();
        let mut recovered = Vec::new();
        let mut pending = Vec::new();
        for ms in 0..=recovery_ms + 120 {
            let now = base + Duration::from_millis(ms);
            if ms % 10 == 0 {
                let lead = (!(loss_ms..recovery_ms).contains(&ms)).then_some(Duration::ZERO);
                view.lock().unwrap().push(Presentation { lead, ..super::tests::block(now, ms as f64 * 48., 480., Duration::ZERO) });
            }
            port.poll("desk", now, &song, &view, &stats, &mut pending);
            for (due, _, bytes) in capture.lock().unwrap().drain(..) {
                assert!(!(loss_ms..recovery_ms).contains(&ms));
                if ms >= recovery_ms {
                    assert_eq!(due, now);
                    recovered.push((ms, bytes));
                }
            }
        }
        eprintln!(
            "gap={loss_ms}..{recovery_ms} full_frame_ms={} first_qf_ms={} qf_sequence={:?}",
            recovered[0].0,
            recovered[1].0,
            recovered[1..9].iter().map(|(ms, _)| ms).collect::<Vec<_>>()
        );
        let locate_frame = (recovery_ms / 40) as u8;
        assert_eq!(recovered[0], (recovery_ms, vec![0xf0, 0x7f, 0x7f, 1, 1, 0x20, 0, 0, locate_frame, 0xf7]));
        let group_ms = recovery_ms.div_ceil(80) * 80;
        assert_eq!(stats.stale_dropped.load(Relaxed), (group_ms - loss_ms) / 10);
        for (piece, (ms, bytes)) in recovered[1..9].iter().enumerate() {
            assert_eq!(*ms, group_ms + 10 * piece as u64);
            assert_eq!(bytes.as_slice(), crate::mtc::quarters((group_ms / 40) as i64, song.frame_rate)[piece]);
        }
    }
}
