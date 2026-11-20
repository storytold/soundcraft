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
    let stats = Arc::new(Stats::default());
    let metrics = stats.clone();
    let handle = std::thread::spawn(move || {
        drive(view, Arc::new(Session::default()), rx, status, metrics, move |_| {
            let attempt = count.fetch_add(1, Relaxed);
            Ok((Sink { fail: attempt == 0, events: evt.clone() }, None))
        })
    });
    tx.send(Update::Beats(vec![("DIN".into(), 0)], true)).unwrap();
    let deadline = Instant::now() + Duration::from_millis(700);
    while trouble.lock().unwrap().is_none() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(trouble.lock().unwrap().as_deref(), Some("DIN: device disappeared"));
    assert!(!stats.connected().contains("DIN"));
    let mut bytes = Vec::new();
    while !bytes.contains(&vec![CLOCK]) {
        bytes.push(got.recv_timeout(Duration::from_secs(4)).unwrap());
    }
    assert!(stats.connected().contains("DIN"));
    tx.send(Update::Encoder(Some(("DIN".into(), 0)), true, Box::new(Quarter))).unwrap();
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

#[test]
fn diagnostics_use_fixed_lateness_buckets_and_ignore_callback_subchunks() {
    let mut out = Output::default();
    let mut publisher = Publisher::new(out.view.clone(), out.metrics.clone());
    let now = Instant::now();
    let ten = Duration::from_millis(10);
    publisher.observe(DeviceTiming::new(now, None, 0), 0.010);
    publisher.observe(DeviceTiming::new(now, Some(ten), 8192), 0.010);
    publisher.observe(DeviceTiming::new(now + ten * 3, Some(ten), 0), 0.010);
    for delay in [0, 501, 999, 6_000, 25_000] {
        out.stats.record(now, now + Duration::from_micros(delay));
    }
    out.beat = vec![("DIN".into(), -240)];
    out.compensated = true;
    out.additional = Some(("DIN".into(), -240, false));
    let status = out.status();
    assert_eq!(status.lateness_buckets, [1, 2, 0, 0, 1, 0, 1]);
    assert_eq!((status.sent, status.lateness_p50_us, status.lateness_p95_us, status.lateness_max_us), (5, 1000, 25000, 25000));
    assert_eq!(
        (status.reported_lead_us, status.callback_gap_max_us, status.callback_gaps_over_two_buffers, status.lead_clamps),
        (10_000, 30_000, 1, 1)
    );
    let port = status.ports.get("DIN").unwrap();
    assert_eq!((port.beat_clock_total_us, port.mtc_total_us), (Some(5000), Some(-5000)));
}

#[test]
fn paused_timeline_sleeps_between_locates_and_keeps_a_future_start() {
    let now = Instant::now();
    let mut timeline = crate::sync_clock::Timeline::default();
    let mut b = block(now, 0., 256., Duration::ZERO);
    b.playing = false;
    timeline.push(b);
    timeline.push(Presentation { heard_at: now + Duration::from_millis(3), ..b });
    assert!(timeline.read(now, true, 0.).1.is_none());
    let next = now + Duration::from_millis(6);
    timeline.push(Presentation { pass: 2, playing: true, heard_at: next, ..b });
    assert_eq!(timeline.read(now, true, 0.).1, Some(next));
}

#[test]
fn receiver_recovers_grid_through_jitter_quantised_wakes_and_worker_stalls() {
    // At 120 BPM the ideal period is 20.833 ms. The virtual receiver knows only
    // MIDI bytes, the grid rate, and the timestamps recorded at the output port.
    for quantum_us in [1_000u64, 15_625] {
        for stall_ms in [0u64, 50, 200, 1000] {
            let base = Instant::now();
            let lead = Duration::from_millis(30);
            let view = View::default();
            let stats = Stats::default();
            let session = Session::default();
            let mut port = active_port(0, true);
            let mut pending = Vec::new();
            let mut sample = 0i64;
            let mut seed = 37u32;
            let mut publish_us = 0i64;
            let mut wake = 0u64;
            let mut pulse = 0i64;
            let mut relaunches = 0;
            let mut clocks = 0;
            let mut last_stamp = None;
            let mut gap = Duration::ZERO;
            for us in (0..=3_030_000).step_by(125) {
                let now = base + Duration::from_micros(us);
                while publish_us <= us as i64 {
                    view.lock().unwrap().push(block(base + Duration::from_secs_f64(sample as f64 / 48_000.), sample as f64, 256., lead));
                    sample += 256;
                    seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
                    publish_us = sample * 1_000_000 / 48_000 + i64::from(seed % 6001) - 3000;
                }
                if us < wake || (1_000_000..1_000_000 + stall_ms * 1000).contains(&us) {
                    continue;
                }
                wake = us + quantum_us;
                port.poll("virtual", now, &session, &view, &stats, &mut pending);
                for (stamp, m) in port.link.as_mut().unwrap().messages.drain(..) {
                    match m.as_slice() {
                        [START] => pulse = 0,
                        [STOP] => relaunches += 1,
                        [0xf2, lo, hi] => pulse = 6 * (i64::from(*lo) + 128 * i64::from(*hi)),
                        [CONTINUE] => {}
                        [CLOCK] => {
                            if let Some(last) = last_stamp {
                                gap = gap.max(now - last);
                            }
                            last_stamp = Some(now);
                            if pulse % 6 == 0 {
                                let ideal = base + lead + Duration::from_secs_f64(pulse as f64 / 48.);
                                // A first F8 bundled with Continue can arrive late by one worker tick.
                                let diff = if stamp >= ideal { stamp - ideal } else { ideal - stamp };
                                assert!(diff <= Duration::from_micros(quantum_us + 125), "pulse {pulse}, error {diff:?}");
                            }
                            clocks += 1;
                            pulse += 1;
                        }
                        unexpected => panic!("unexpected receiver message {unexpected:?}"),
                    }
                }
            }
            assert_eq!(relaunches, usize::from(stall_ms >= 200));
            if stall_ms < 200 {
                assert!(i64::abs_diff(clocks, 145) <= 1);
            }
            assert!(gap <= Duration::from_millis(stall_ms + 42 + if stall_ms >= 200 { 125 } else { 0 }));
        }
    }
}

#[test]
fn saved_external_offset_is_bounded_at_each_session_rate() {
    let base = Instant::now();
    let mut out = Output::default();
    out.beat.push(("DIN".into(), 24_000));
    for (hz, effective, delay_ms) in [(96_000, 24_000, 250), (48_000, 12_000, 250), (32_000, 8_000, 250)] {
        let s = Session::new("s", soundcraft_time::SampleRate::new(hz).unwrap());
        out.session = Some(Arc::new(s.clone()));
        let snapshot = out.status();
        let timing = &snapshot.ports["DIN"];
        assert_eq!((timing.requested_offset_samples, timing.offset_samples), (24_000, effective));
        assert_eq!(timing.beat_clock_total_us, Some(delay_ms * 1000));
        let mut port = active_port(24_000, false);
        let view = View::default();
        view.lock().unwrap().push(block(base, 0., 48000., Duration::ZERO));
        let due = base + Duration::from_millis(delay_ms as u64);
        for at in [due - Duration::from_micros(1), due] {
            port.poll("DIN", at, &s, &view, &Stats::default(), &mut Vec::new());
        }
        assert_eq!(port.link.unwrap().messages, [(due, vec![START]), (due, vec![CLOCK])]);
    }
}
