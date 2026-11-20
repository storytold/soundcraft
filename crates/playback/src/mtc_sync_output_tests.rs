use super::*;
use crate::sync_clock::Presentation;
use std::sync::atomic::AtomicUsize;
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
    assert_eq!(output.additional, None, "no session or worker accepted this route");

    let (tx, rx) = channel();
    let worker = std::thread::spawn(|| {});
    output.worker = Some((WorkerSender { tx, worker: worker.thread().clone() }, worker));
    assert!(output.encoder(port.clone(), true, Box::<crate::mtc::Encoder>::default()));
    assert!(matches!(rx.try_recv(), Ok(Update::Encoder(name, true, _)) if name == port));
    assert_eq!(output.additional, Some(("model".to_owned(), 240, true)));
}

#[test]
fn a_burst_of_external_locates_queues_one_transport_refresh() {
    let output = Output::default();
    let wake = output.transport_waker();
    let (tx, rx) = channel();
    *output.wake_tx.lock().unwrap() = Some(WorkerSender { tx, worker: std::thread::current() });
    for _ in 0..100 {
        wake();
    }
    let Ok(Update::TransportWake(pending)) = rx.try_recv() else {
        panic!("expected a transport refresh for the audio callback's new pass");
    };
    assert!(rx.try_recv().is_err());
    pending.store(false, Relaxed);
    wake();
    assert!(matches!(rx.try_recv(), Ok(Update::TransportWake(_))));
}

#[test]
fn both_encoders_use_one_connection_and_the_same_offset() {
    let now = Instant::now();
    let lead = Duration::from_millis(30);
    let due = now + Duration::from_millis(25);
    let mut map = BTreeMap::new();
    set_route(&mut map, 0, "device".to_owned(), Route { map: None, offset: -240, compensate: true, encoder: Box::<Cursor>::default() });
    set_route(&mut map, 1, "device".to_owned(), Route { map: None, offset: -240, compensate: true, encoder: Box::<crate::mtc::Encoder>::default() });
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
        lead,
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
        set_route(&mut routes, 1, "MTC".into(), Route { map: None, offset, compensate: true, encoder: Box::<crate::mtc::Encoder>::default() });
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
                    lead,
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
        output.additional = Some(("MTC".into(), offset, true));
        output.metrics.lead_us.store(20000, Relaxed);
        output.metrics.system_delay_samples.store(delay as u64, Relaxed);
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
    set_route(&mut routes, 1, "MTC".into(), Route { map: None, offset: 0, compensate: false, encoder: Box::<crate::mtc::Encoder>::default() });
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
                lead: Duration::ZERO,
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
    set_route(&mut routes, 1, "cable".into(), Route { map: None, offset: 0, compensate: true, encoder: Box::<crate::mtc::Encoder>::default() });
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
            lead,
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
/// Real OS scheduling observations for the two encoders sharing a mock connection.
/// No wall-clock upper bound is a correctness gate.
#[test]
#[ignore]
fn measure_native_worker_quarter_frame_spacing() {
    for fps in [24, 30] {
        let mut sess = Session::new("quarter-frame spacing", soundcraft_time::SampleRate::HZ_48000);
        sess.timecode_start = 0;
        sess.frame_rate = if fps == 24 { soundcraft_time::FrameRate::Fps24 } else { soundcraft_time::FrameRate::Fps30 };
        let samples = Arc::new(Mutex::new(Vec::new()));
        let opens = Arc::new(AtomicUsize::new(0));
        let v = View::default();
        let (tx, rx) = channel();
        let start = Instant::now() + Duration::from_millis(80);
        v.lock().unwrap().push(Presentation {
            generation: 0,
            sample_rate: soundcraft_time::SampleRate::HZ_48000,
            system_delay_samples: 0,
            pass: 1,
            origin: 0,
            playing: true,
            sample: 0.,
            limit: 240_000.,
            heard_at: start,
            samples_per_second: 48_000.,
            looped: None,
            end: None,
            lead: Duration::from_millis(30),
        });
        let captured = samples.clone();
        let times = opens.clone();
        let worker = std::thread::spawn(move || {
            drive(v, Arc::new(sess), rx, Arc::default(), Arc::default(), move |_| {
                times.fetch_add(1, Relaxed);
                Ok((Capture(captured.clone()), None))
            })
        });
        tx.send(Update::Beats(vec![("capture".into(), 0)], true)).unwrap();
        tx.send(Update::Encoder(Some(("capture".into(), 0)), true, Box::<crate::mtc::Encoder>::default())).unwrap();
        std::thread::sleep(start.saturating_duration_since(Instant::now()) + Duration::from_millis(5100));
        drop(tx);
        worker.join().unwrap();
        assert_eq!(opens.load(Relaxed), 1);
        let rows = samples.lock().unwrap();
        let q: Vec<_> = rows.iter().filter(|r| r.2[0] == 0xf1).collect();
        let ideal = 250. / f64::from(fps);
        let mut gaps: Vec<_> = q.windows(2).map(|w| (w[1].1 - w[0].1).as_secs_f64() * 1000.).collect();
        let pairs = gaps.iter().filter(|v| **v < 1.).count();
        for dt in &mut gaps {
            *dt = (*dt - ideal).abs();
        }
        let mut late: Vec<_> = q.iter().map(|r| r.1.saturating_duration_since(r.0).as_secs_f64() * 1000.).collect();
        gaps.sort_by(f64::total_cmp);
        late.sort_by(f64::total_cmp);
        assert!(!gaps.is_empty());
        println!(
            "fps={fps} qf={} pairs_lt_1ms={pairs} interval_error_p95_ms={:.3} lateness_p95_ms={:.3}",
            q.len(),
            gaps[gaps.len() * 95 / 100],
            late[late.len() * 95 / 100]
        );
    }
}
