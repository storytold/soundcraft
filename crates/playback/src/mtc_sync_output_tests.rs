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
fn a_burst_of_external_locates_queues_one_transport_refresh() {
    let output = Output::default();
    let wake = output.transport_waker();
    let (tx, rx) = channel();
    *output.wake_tx.lock().unwrap() = Some(tx);
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
    set_route(&mut map, 0, "device".to_owned(), Route { offset: -240, compensate: true, encoder: Box::<Cursor>::default() });
    set_route(&mut map, 1, "device".to_owned(), Route { offset: -240, compensate: true, encoder: Box::<crate::mtc::Encoder>::default() });
    assert_eq!(map.len(), 1);
    let messages = Arc::default();
    let p = map.get_mut("device").unwrap();
    p.link = Some(Capture(Arc::clone(&messages)));
    let v = View::default();
    v.lock().unwrap().push(Presentation {
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
    set_route(&mut routes, 1, "cable".into(), Route { offset: 0, compensate: true, encoder: Box::<crate::mtc::Encoder>::default() });
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
        let sample = f64::from(i * 240);
        let clock = now + Duration::from_millis(i as u64 * 5);
        master.lock().unwrap().push(Presentation {
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
        input.set_clock(i64::from(i) * 5000);
        audio.device_timing = Some(DeviceTiming::new(clock, Some(lead), 0));
        audio.fill(&mut [0.; 480], 2);
        if i >= 300 {
            let audible = |v: &View| v.lock().unwrap().read(clock, true, 0.).0.unwrap().sample_heard_at(clock);
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
