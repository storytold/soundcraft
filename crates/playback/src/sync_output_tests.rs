use super::*;
use crate::sync_clock::{DeviceTiming, Presentation};
use craft_midi_sync::sync::{CLOCK, CONTINUE, START, STOP};
use std::sync::atomic::AtomicUsize;

#[derive(Default)]
struct FakePort {
    messages: Vec<(Instant, Vec<u8>)>,
}
impl SyncPort for FakePort {
    fn send_at(&mut self, at: Instant, bytes: &[u8]) -> Result<Instant, String> {
        self.messages.push((at, bytes.to_vec()));
        Ok(at)
    }
}
fn block(start: Instant, sample: f64, frames: f64, lead: Duration) -> Presentation {
    Presentation {
        pass: 1,
        origin: 0,
        playing: true,
        sample,
        limit: sample + frames,
        heard_at: start + lead,
        samples_per_second: 48_000.,
        looped: None,
        end: None,
        lead,
    }
}
fn active_port(offset: i64, compensate: bool) -> Port<FakePort> {
    Port {
        link: Some(FakePort::default()),
        retry: Instant::now(),
        routes: [Some(Route { offset, compensate, encoder: Box::<Cursor>::default() }), None],
        failure: None,
    }
}

#[test]
fn destination_stamp_includes_device_lead_and_signed_port_offset_once() {
    let base = Instant::now();
    let lead = Duration::from_millis(30);
    let view = View::default();
    view.lock().unwrap().push(block(base, 0., 4096., lead));
    let session = Session::default();
    for (offset, compensate, expected) in [(0, true, 30), (0, false, 0), (-240, true, 25), (240, true, 35)] {
        let mut port = active_port(offset, compensate);
        let due = base + Duration::from_millis(expected);
        for now in [due - Duration::from_micros(1), due] {
            port.poll("virtual", now, &session, &view, &Stats::default(), &mut Vec::new());
        }
        assert_eq!(port.link.unwrap().messages, [(due, vec![START]), (due, vec![CLOCK])]);
    }
}
#[test]
fn paired_and_periodic_buffers_preserve_grid_without_relocations() {
    let base = Instant::now();
    for bpm in [60., 120., 180.] {
        for frames in [64usize, 256, 1024] {
            for paired in [false, true] {
                let lead = Duration::from_millis(30);
                let view = View::default();
                let mut session = Session::default();
                session.tempo.set_tempo(0, bpm).unwrap();
                let mut port = active_port(0, true);
                let stats = Stats::default();
                let mut pending = Vec::new();
                let mut frame = 0;
                let mut stamps = Vec::new();
                for ms in 0..=2030 {
                    let now = base + Duration::from_millis(ms);
                    while frame as f64 / 48_000. <= ms as f64 / 1000. {
                        for _ in 0..if paired { 2 } else { 1 } {
                            view.lock().unwrap().push(block(
                                base + Duration::from_secs_f64(frame as f64 / 48_000.),
                                frame as f64,
                                frames as f64,
                                lead,
                            ));
                            frame += frames;
                        }
                    }
                    port.poll("virtual", now, &session, &view, &stats, &mut pending);
                    for (due, msg) in port.link.as_mut().unwrap().messages.drain(..) {
                        match msg.as_slice() {
                            [CLOCK] => stamps.push(now),
                            [START] => assert_eq!(due, base + lead),
                            other => panic!("unwanted transport {other:?} at {bpm}"),
                        }
                    }
                }
                let ideal = 60. / (24. * bpm);
                assert!(stamps.len().abs_diff((2. / ideal) as usize + 1) <= 1);
                assert!(stamps.windows(2).all(|p| (p[1] - p[0]).as_secs_f64() < 2. * ideal));
            }
        }
    }
}
#[test]
fn fractional_limits_never_overshoot_the_next_buffer_and_a_stall_is_capped() {
    let now = Instant::now();
    for (ratio, size) in [(44_100. / 48_000., 64), (48_000. / 44_100., 1024), (0.5, 65), (1.025, 256)] {
        let dur = Duration::from_secs_f64(size as f64 / 48_000.);
        let mut previous = 0;
        for i in 0..100 {
            let sample = i as f64 * size as f64 * ratio;
            let b = Presentation { samples_per_second: 48_000. * ratio, ..block(now + dur * i, sample, size as f64 * ratio, Duration::ZERO) };
            assert!(b.sample_index_at(b.heard_at) >= previous);
            previous = b.sample_index_at(now + Duration::from_secs(10));
        }
    }
}
#[test]
fn reported_lead_is_bounded_and_shared_chunk_offsets_are_monotonic() {
    let now = Instant::now();
    let lead = Duration::from_millis(30);
    assert!(DeviceTiming::new(now, None, 0).clamped);
    assert_eq!(DeviceTiming::new(now, Some(Duration::from_secs(2)), 0).lead, Duration::ZERO);
    let a = DeviceTiming::new(now, Some(lead), 0);
    let b = DeviceTiming::new(now, Some(lead), 8192);
    assert_eq!(a.heard_instant(48_000., 8192), b.heard_instant(48_000., 0));
}
#[test]
fn a_single_client_port_is_shared_and_send_errors_trigger_reconnect() {
    struct Quarter;
    impl SyncEncoder for Quarter {
        fn reset(&mut self) {}
        fn update(&mut self, _b: Presentation, now: Instant, _s: &Session, send: &mut dyn FnMut(Instant, &[u8])) -> Option<Instant> {
            send(now, &[0xf1, 0]);
            None
        }
    }
    struct Sink {
        fail: bool,
        events: Sender<Vec<u8>>,
    }
    impl SyncPort for Sink {
        fn send_at(&mut self, _at: Instant, b: &[u8]) -> Result<Instant, String> {
            if self.fail {
                return Err("device disappeared".into());
            }
            self.events.send(b.to_vec()).unwrap();
            Ok(Instant::now())
        }
    }
    let now = Instant::now();
    let view = View::default();
    view.lock().unwrap().push(block(now, 0., 0., Duration::ZERO));
    let (tx, rx) = channel();
    let (evt, got) = channel();
    let attempts = Arc::new(AtomicUsize::new(0));
    let count = attempts.clone();
    let trouble = Arc::new(Mutex::new(None));
    let status = trouble.clone();
    let handle = std::thread::spawn(move || {
        drive(view, Arc::new(Session::default()), rx, status, Arc::default(), move |_| {
            let attempt = count.fetch_add(1, Relaxed);
            Ok((Sink { fail: attempt == 0, events: evt.clone() }, None))
        })
    });
    tx.send(Update::Beats(vec![("DIN".into(), 0)], true)).unwrap();
    tx.send(Update::Encoder(Some(("DIN".into(), 0)), true, Box::new(Quarter))).unwrap();
    let deadline = Instant::now() + Duration::from_millis(700);
    while trouble.lock().unwrap().is_none() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(trouble.lock().unwrap().as_deref(), Some("DIN: device disappeared"));
    let mut bytes = Vec::new();
    while !bytes.contains(&vec![0xf1, 0]) {
        bytes.push(got.recv_timeout(Duration::from_secs(4)).unwrap());
    }
    assert!(bytes.contains(&vec![START]) && bytes.contains(&vec![CLOCK]));
    assert_eq!(attempts.load(Relaxed), 2, "one failed connection and one shared replacement");
    drop(tx);
    handle.join().unwrap();
}

#[test]
fn late_worker_catches_up_with_spaced_clocks_or_resumes_once_at_the_next_spp() {
    let base = Instant::now();
    let view = View::default();
    view.lock().unwrap().push(block(base, 0., 48_000., Duration::ZERO));
    let mut port = active_port(0, true);
    let stats = Stats::default();
    let mut emitted = Vec::new();
    for ms in [0, 43, 43, 44, 45, 500, 501] {
        port.poll("fake", base + Duration::from_millis(ms), &Session::default(), &view, &stats, &mut Vec::new());
        for (_, msg) in port.link.as_mut().unwrap().messages.drain(..) {
            emitted.push((ms, msg));
        }
    }
    assert!(emitted.contains(&(43, vec![CLOCK])) && emitted.contains(&(44, vec![CLOCK])));
    assert_eq!(emitted.iter().filter(|(_, m)| m == &[STOP]).count(), 1);
    assert!(emitted.contains(&(500, vec![CONTINUE])));
    assert_eq!(stats.catch_up_sends.load(Relaxed), 1);
}
