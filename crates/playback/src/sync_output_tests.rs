use super::*;
use crate::sync_clock::{DeviceTiming, Presentation};
use craft_midi_sync::sync::{CLOCK, CONTINUE, START, STOP};
use std::sync::atomic::AtomicUsize;

/// On Windows, set SOUNDCRAFT_LOOPBACK_IN and SOUNDCRAFT_LOOPBACK_OUT to the
/// exact names of a dedicated virtual cable. The Unix paths create their own port.
#[test]
#[ignore = "requires an OS MIDI loopback driver"]
fn beat_clock_reaches_a_virtual_midi_receiver() {
    use midir::{Ignore, MidiInput};
    let mut input = MidiInput::new("SoundCraft test receiver").unwrap();
    input.ignore(Ignore::None);
    let (events, observed) = channel();
    let record = move |_: u64, msg: &[u8], _: &mut ()| {
        let _ = events.send(msg.to_vec());
    };
    #[cfg(target_os = "windows")]
    let (_reader, name) = {
        let name = std::env::var("SOUNDCRAFT_LOOPBACK_IN").expect("specify a dedicated virtual MIDI input");
        let mut matching = input.ports().into_iter().filter(|p| input.port_name(p).is_ok_and(|n| n == name));
        let p = matching.next().expect("specified input not found");
        assert!(matching.next().is_none());
        (input.connect(&p, "receive clock", record, ()).unwrap(), std::env::var("SOUNDCRAFT_LOOPBACK_OUT").expect("specify its virtual MIDI output"))
    };
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    let (_reader, name) = {
        use midir::os::unix::VirtualInput;
        let unique = format!("SoundCraft G11 probe {}", std::process::id());
        let reader = input.create_virtual(&unique, record, ()).unwrap();
        let list = crate::midi_sync_outputs().unwrap();
        let name = list.into_iter().find(|n| n.contains(&unique)).expect("OS did not publish the virtual destination");
        (reader, name)
    };
    let ready = Instant::now() + Duration::from_millis(40);
    let view = View::default();
    view.lock().unwrap().push(Presentation {
        generation: 0,
        sample_rate: soundcraft_time::SampleRate::default(),
        system_delay_samples: 0,
        pass: 1,
        origin: 0,
        playing: true,
        sample: 0.,
        limit: 4000.,
        samples_per_second: 48_000.,
        heard_at: ready,
        looped: None,
        end: None,
        lead: Some(Duration::ZERO),
    });
    let (tx, rx) = channel();
    let registry = ports::Registry::default();
    let worker =
        std::thread::spawn(move || drive(view, Arc::new(Session::default()), rx, Arc::default(), Arc::default(), |name| registry.connect(name)));
    tx.send(Update::Beats(vec![(name, 0)], true)).unwrap();
    assert_eq!(observed.recv_timeout(Duration::from_secs(2)).unwrap(), [START]);
    assert_eq!(observed.recv_timeout(Duration::from_secs(2)).unwrap(), [CLOCK]);
    drop(tx);
    worker.join().unwrap();
}

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
        generation: 0,
        sample_rate: soundcraft_time::SampleRate::default(),
        system_delay_samples: 0,
        pass: 1,
        origin: 0,
        playing: true,
        sample,
        limit: sample + frames,
        heard_at: start + lead,
        samples_per_second: 48_000.,
        looped: None,
        end: None,
        lead: Some(lead),
    }
}
fn active_port(offset: i64, compensate: bool) -> Port<FakePort> {
    Port {
        link: Some(FakePort::default()),
        retry: Instant::now(),
        routes: [Some(Route { map: None, offset, compensate, encoder: Box::<Cursor>::default() }), None],
        failure: None,
        future: Vec::new(),
        stop_pending: false,
        timing_epochs: [0; 2],
        waiting_for_timing: false,
    }
}

#[test]
fn output_stream_lead_survives_bad_callbacks_and_updates_on_recovery() {
    let mut stream = crate::sync_clock::StreamLead::default();
    let base = Instant::now();
    for (report, expected) in [(Some(15), 15), (None, 15), (Some(1_001), 15), (Some(9), 9), (Some(0), 0), (None, 0)] {
        let t = stream.observe(base, report.map(Duration::from_millis));
        assert_eq!(t.heard_instant(48_000., 48), base + Duration::from_millis(expected + 1));
    }
}

#[test]
fn timing_warning_follows_the_compensated_destinations() {
    let mut out = Output::default();
    let mut publisher = Publisher::new(out.view.clone(), out.metrics.clone());
    publisher.observe(crate::sync_clock::StreamLead::default().observe(Instant::now(), None), 0.01);
    let unknown = Some(crate::OutputTimingIssue::Unknown);
    out.compensated = true;
    assert_eq!(out.timing_issue(), None, "no destinations require output timing");
    out.beat.push(("DIN".into(), 0));
    assert_eq!(out.timing_issue(), unknown);
    out.compensated = false;
    assert_eq!(out.timing_issue(), None, "uncompensated Clock uses no device lead");
    out.additional = Some(("encoder".into(), 0, false));
    assert_eq!(out.timing_issue(), None);
    out.additional.as_mut().unwrap().2 = true;
    assert_eq!(out.timing_issue(), unknown);
    out.additional = None;
    assert_eq!(out.timing_issue(), None);
}

#[test]
fn output_stream_lead_is_unknown_for_a_new_output_until_the_first_valid_report() {
    let mut out = Output::default();
    out.compensated = true;
    out.beat.push(("DIN".into(), 0));
    let mut publisher = Publisher::new(out.view.clone(), out.metrics.clone());
    let mut old = crate::sync_clock::StreamLead::default();
    let base = Instant::now();
    assert_eq!(out.status().lead_source, crate::OutputLeadSource::Unobserved);
    assert_eq!(out.timing_issue(), None);
    publisher.observe(old.observe(base, Some(Duration::from_millis(15))), 0.01);
    publisher.observe(old.observe(base, Some(Duration::from_secs(2))), 0.01);
    assert_eq!(out.timing_issue(), Some(crate::OutputTimingIssue::Retained { lead_us: 15_000 }));
    assert_eq!(out.status().ports["DIN"].beat_clock_total_us, Some(15_000));
    let mut reopened = crate::sync_clock::StreamLead::default();
    publisher.observe(reopened.observe(base, None), 0.01);
    let status = serde_json::to_value(out.status()).unwrap();
    assert_eq!(status["reported_lead_us"], serde_json::Value::Null);
    assert_eq!(status["ports"]["DIN"]["beat_clock_total_us"], serde_json::Value::Null);
    assert_eq!(status["ports"]["DIN"]["lead_source"], "unknown");
    assert_eq!(status["ports"]["DIN"]["beat_clock_enabled"], true);
    assert_eq!(status["ports"]["DIN"]["mtc_enabled"], false);
    assert_eq!(out.timing_issue(), Some(crate::OutputTimingIssue::Unknown));
    let zero = reopened.observe(base, Some(Duration::ZERO));
    publisher.observe(zero, 0.01);
    assert_eq!(out.timing_issue(), None);
    assert_eq!(out.status().ports["DIN"].beat_clock_total_us, Some(0));
    assert_eq!(out.status().lead_source, crate::OutputLeadSource::Reported);
    publisher.observe(reopened.observe(base, None), 0.01);
    assert_eq!(out.timing_issue(), Some(crate::OutputTimingIssue::Retained { lead_us: 0 }));
    assert_eq!(out.reported_timing_count(), 2);
    assert_eq!(out.status().ports["DIN"].lead_source, crate::OutputLeadSource::Retained);
}

#[test]
fn output_stream_lead_wakes_a_parked_worker_after_the_first_valid_report() {
    struct Wire(Sender<Vec<u8>>);
    impl SyncPort for Wire {
        fn send_at(&mut self, at: Instant, bytes: &[u8]) -> Result<Instant, String> {
            self.0.send(bytes.to_vec()).unwrap();
            Ok(at)
        }
    }
    let view = View::default();
    view.lock().unwrap().push(Presentation { lead: None, ..block(Instant::now(), 0., 48_000., Duration::ZERO) });
    let worker_view = view.clone();
    let (tx, rx) = channel();
    let (wire, packets) = channel();
    let (opened, ready) = channel();
    let worker = std::thread::spawn(move || {
        drive(worker_view, Arc::new(Session::default()), rx, Default::default(), Arc::default(), |_| {
            opened.send(()).unwrap();
            Ok((Wire(wire.clone()), None))
        })
    });
    tx.send(Update::Beats(vec![("DIN".into(), 0)], true)).unwrap();
    ready.recv_timeout(Duration::from_secs(3)).unwrap();
    // Longer than the normal control-change wake window; recovery must not
    // need another transport command or an idle polling loop.
    assert!(matches!(packets.recv_timeout(Duration::from_millis(400)), Err(std::sync::mpsc::RecvTimeoutError::Timeout)));
    view.lock().unwrap().push(block(Instant::now(), 0., 48_000., Duration::from_millis(15)));
    assert_eq!(packets.recv_timeout(Duration::from_secs(2)).unwrap(), [START]);
    assert_eq!(packets.recv_timeout(Duration::from_secs(2)).unwrap(), [CLOCK]);
    drop(tx);
    worker.thread().unpark();
    worker.join().unwrap();
}

#[test]
fn output_stream_lead_gates_only_compensated_midi_until_a_timestamp_is_available() {
    let base = Instant::now();
    let view = View::default();
    let session = Session::default();
    let stats = Stats::default();
    let mut compensated = active_port(0, true);
    let mut direct = active_port(0, false);
    let mut stream = crate::sync_clock::StreamLead::default();
    let publish = |timing: DeviceTiming, sample: f64| {
        view.lock().unwrap().push(Presentation {
            lead: timing.lead(),
            heard_at: timing.heard_instant(48_000., 0),
            ..block(timing.callback, sample, 48_000., Duration::ZERO)
        });
    };
    publish(stream.observe(base, None), 0.);
    compensated.poll("comp", base, &session, &view, &stats, &mut vec![]);
    direct.poll("direct", base, &session, &view, &stats, &mut vec![]);
    assert!(compensated.link.as_ref().unwrap().messages.is_empty());
    assert_eq!(direct.link.as_ref().unwrap().messages, [(base, vec![START]), (base, vec![CLOCK])]);
    let next = base + Duration::from_millis(10);
    let lead = Duration::from_millis(15);
    publish(stream.observe(next, Some(lead)), 0.);
    let due = next + lead;
    assert_eq!(compensated.poll("comp", next, &session, &view, &stats, &mut vec![]), Some(due));
    assert!(compensated.link.as_ref().unwrap().messages.is_empty());
    compensated.poll("comp", due, &session, &view, &stats, &mut vec![]);
    assert_eq!(compensated.link.as_ref().unwrap().messages, [(due, vec![START]), (due, vec![CLOCK])]);
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
    assert_eq!(DeviceTiming::new(now, Some(Duration::from_secs(2)), 0).lead(), None);
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
        (Some(10_000), 30_000, 1, 1)
    );
    let port = status.ports.get("DIN").unwrap();
    assert_eq!((port.beat_clock_total_us, port.mtc_total_us), (Some(5000), Some(0)));
}

#[test]
fn paused_timeline_sleeps_between_locates_and_keeps_a_future_start() {
    let now = Instant::now();
    let mut timeline = crate::sync_clock::Timeline::default();
    let mut b = block(now, 0., 256., Duration::ZERO);
    b.playing = false;
    timeline.push(b);
    timeline.push(Presentation { heard_at: now + Duration::from_millis(3), ..b });
    assert!(timeline.read(now, true, 0).1.is_none());
    let next = now + Duration::from_millis(6);
    timeline.push(Presentation { pass: 2, playing: true, heard_at: next, ..b });
    assert_eq!(timeline.read(now, true, 0).1, Some(next));
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
            let mut recovery_pulses = 0;
            let recovery = 1_000_000 + stall_ms * 1000;
            for us in (0..=3_030_000).step_by(125) {
                let now = base + Duration::from_micros(us);
                while publish_us <= us as i64 {
                    view.lock().unwrap().push(block(base + Duration::from_secs_f64(sample as f64 / 48_000.), sample as f64, 256., lead));
                    sample += 256;
                    seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
                    publish_us = sample * 1_000_000 / 48_000 + i64::from(seed % 6001) - 3000;
                }
                // A queued control wake can run between timer ticks immediately after a stall.
                let extra_wake = stall_ms > 0 && (recovery..recovery + 4_000).contains(&us);
                if (us < wake && !extra_wake) || (1_000_000..recovery).contains(&us) {
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
                                if extra_wake {
                                    assert!(now - last >= Duration::from_millis(1), "burst at {us} us");
                                    recovery_pulses += 1;
                                }
                            }
                            last_stamp = Some(now);
                            let ideal = base + lead + Duration::from_secs_f64(pulse as f64 / 48.);
                            if us >= recovery + 50_000 {
                                assert!(
                                    now.saturating_duration_since(ideal).max(ideal.saturating_duration_since(now))
                                        <= Duration::from_micros(quantum_us + 125),
                                    "actual pulse {pulse} at {us} us"
                                );
                            }
                            if pulse % 6 == 0 {
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
            assert!(recovery_pulses < crate::sync_clock::RELAUNCH_PULSES);
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
        view.lock().unwrap().push(Presentation {
            sample_rate: s.sample_rate,
            samples_per_second: s.sample_rate.as_f64(),
            ..block(base, 0., 48000., Duration::ZERO)
        });
        let due = base + Duration::from_millis(delay_ms as u64);
        for at in [due - Duration::from_micros(1), due] {
            port.poll("DIN", at, &s, &view, &Stats::default(), &mut Vec::new());
        }
        assert_eq!(port.link.unwrap().messages, [(due, vec![START]), (due, vec![CLOCK])]);
    }
}

#[test]
fn input_wakes_reread_an_idle_timeline_without_periodic_polling() {
    for has_port in [false, true] {
        let stats = Arc::new(Stats::default());
        let observed = stats.clone();
        let (tx, rx) = channel();
        let (opened, ready) = channel();
        let worker = std::thread::spawn(move || {
            drive(View::default(), Arc::new(Session::default()), rx, Default::default(), observed, move |_| {
                opened.send(()).unwrap();
                Ok((FakePort::default(), None))
            });
        });
        let tx = WorkerSender { tx, worker: worker.thread().clone() };
        if has_port {
            tx.send(Update::Beats(vec![("DIN".into(), 0)], true)).unwrap();
            ready.recv_timeout(Duration::from_secs(5)).unwrap();
            // Let the initial configuration's transport grace expire.
            std::thread::sleep(Duration::from_millis(200));
        }
        let before = stats.iterations.load(Relaxed);
        if !has_port {
            tx.send(Update::Wake(None)).unwrap();
        }
        let pending = Arc::new(AtomicBool::new(false));
        for _ in 0..50 {
            tx.send(Update::InputWake(pending.clone())).unwrap();
            std::thread::sleep(Duration::from_millis(20));
        }
        drop(tx);
        worker.thread().unpark();
        worker.join().unwrap();
        let iterations = stats.iterations.load(Relaxed) - before;
        assert!((50..=60).contains(&iterations), "{iterations} iterations for 50 input wakes, configured port: {has_port}");
    }
}

#[test]
fn idle_worker_starts_the_device_when_play_publishes_after_a_long_buffer() {
    struct Sink(Sender<Vec<u8>>);
    impl SyncPort for Sink {
        fn send_at(&mut self, _: Instant, bytes: &[u8]) -> Result<Instant, String> {
            self.0.send(bytes.to_vec()).map_err(|e| e.to_string())?;
            Ok(Instant::now())
        }
    }
    for delay in [180, 600, 2500] {
        let view = View::default();
        let publisher = Publisher::new(view.clone(), Arc::default());
        let stopped = Presentation { pass: 0, playing: false, ..block(Instant::now(), 0., 0., Duration::ZERO) };
        publisher.publish(stopped);
        let stats = Arc::new(Stats::default());
        let metrics = stats.clone();
        let (tx, rx) = channel();
        let (events, received) = channel();
        let (open, ready) = channel();
        let worker = std::thread::spawn(move || {
            drive(view, Arc::new(Session::default()), rx, Default::default(), stats, |_| {
                open.send(()).unwrap();
                Ok((Sink(events.clone()), None))
            })
        });
        tx.send(Update::Beats(vec![("DIN".into(), 0)], true)).unwrap();
        ready.recv_timeout(Duration::from_secs(3)).unwrap();
        std::thread::sleep(Duration::from_millis(300));
        tx.send(Update::Wake(Some((stopped.pass, stopped.playing)))).unwrap();
        std::thread::sleep(Duration::from_millis(delay));
        if delay > 2000 {
            assert_eq!(metrics.pending_transition_timeouts.load(Relaxed), 1);
            let before = metrics.iterations.load(Relaxed);
            // The audio device keeps publishing stopped frames. They must not cause polling.
            for _ in 0..50 {
                publisher.publish(Presentation { heard_at: Instant::now(), ..stopped });
                std::thread::sleep(Duration::from_millis(2));
            }
            assert_eq!(metrics.iterations.load(Relaxed), before);
        }
        let started = Instant::now();
        // The audio callback may skip a contended publication. This fixture exercises
        // the single transition the worker sees once a playing block actually arrives.
        let deadline = started + Duration::from_secs(1);
        loop {
            if let Ok(mut timeline) = publisher.view.try_lock() {
                let playing = block(Instant::now(), 0., 480., Duration::ZERO);
                timeline.push(playing);
                assert_eq!(timeline.transport_state(), Some((playing.pass, true)));
                break;
            }
            assert!(Instant::now() < deadline, "fixture could not publish the playing block");
            std::thread::yield_now();
        }
        assert_eq!(received.recv_timeout(Duration::from_secs(1)).ok(), Some(vec![START]), "Play delayed by {delay} ms");
        assert_eq!(received.recv_timeout(Duration::from_secs(1)).ok(), Some(vec![CLOCK]));
        eprintln!("Play delayed by {delay} ms: Start + F8 in {:?}", started.elapsed());
        drop(tx);
        worker.join().unwrap();
    }
}

#[test]
fn stopping_an_already_stopped_presentation_does_not_expect_another_transition() {
    let mut output = Output::default();
    let (tx, requests) = channel();
    let worker = std::thread::spawn(|| {});
    let sender = WorkerSender { tx, worker: worker.thread().clone() };
    output.worker.replace((sender, worker));
    output.view.lock().unwrap().push(Presentation { playing: false, ..block(Instant::now(), 0., 0., Duration::ZERO) });
    let (audio, callback) = channel();
    output.send_transport(&audio, Cmd::Stop);
    assert!(matches!(callback.try_recv(), Ok(Cmd::Stop)));
    assert!(matches!(requests.try_recv(), Err(std::sync::mpsc::TryRecvError::Empty)));
}

#[test]
fn a_non_grid_start_published_before_the_worker_reads_wake_does_not_time_out() {
    let mut output = Output::default();
    let view = output.view.clone();
    let stats = output.stats.clone();
    let initial = Presentation { pass: 0, playing: false, ..block(Instant::now(), 0., 0., Duration::ZERO) };
    output.view.lock().unwrap().push(initial);
    let (tx, rx) = channel();
    let (opened, ready) = channel();
    let (release, paused) = channel();
    let worker = std::thread::spawn(move || {
        drive(view, Arc::new(Session::default()), rx, Default::default(), stats, move |_| {
            opened.send(()).unwrap();
            paused.recv().unwrap();
            Ok((FakePort::default(), None))
        });
    });
    let sender = WorkerSender { tx, worker: worker.thread().clone() };
    output.worker.replace((sender, worker));
    output.beats(vec![("DIN".into(), 0)], false);
    ready.recv_timeout(Duration::from_secs(3)).unwrap();
    let (audio, callback) = channel();
    output.send_transport(&audio, Cmd::Play { from: 47_125, end: None, looped: None });
    assert!(matches!(callback.try_recv(), Ok(Cmd::Play { from: 47_125, .. })));
    output.view.lock().unwrap().push(Presentation { origin: 47_125, ..block(Instant::now(), 47_125., 4_800., Duration::ZERO) });
    release.send(()).unwrap();
    std::thread::sleep(Duration::from_millis(2400));
    assert!(output.stats.sent.load(Relaxed) >= 3, "expected SPP, Continue and Clock");
    assert_eq!(output.stats.pending_transition_timeouts.load(Relaxed), 0);
}

#[test]
fn parked_worker_handles_control_and_shutdown_without_a_new_audio_block() {
    let mut output = Output::default();
    let (tx, rx) = channel();
    let (opened, ready) = channel();
    let view = output.view.clone();
    let stats = output.stats.clone();
    let worker = std::thread::spawn(move || {
        drive(view, Arc::new(Session::default()), rx, Default::default(), stats, move |_| {
            opened.send(()).unwrap();
            Ok((FakePort::default(), None))
        });
    });
    let sender = WorkerSender { tx, worker: worker.thread().clone() };
    output.wake_tx.lock().unwrap().replace(sender.clone());
    output.worker.replace((sender, worker));
    output.beats(vec![("DIN".into(), 0)], true);
    ready.recv_timeout(Duration::from_secs(3)).unwrap();
    let (audio, _callback) = channel();
    output.send_transport(&audio, Cmd::Play { from: 0, end: None, looped: None });
    let limit = Instant::now() + Duration::from_secs(4);
    while output.stats.pending_transition_timeouts.load(Relaxed) == 0 {
        assert!(Instant::now() < limit, "worker never entered its post-timeout wait");
        std::thread::sleep(Duration::from_millis(5));
    }
    let wake = output.waker();
    wake();
    let limit = Instant::now() + Duration::from_millis(100);
    while output.pending_wake.load(Relaxed) {
        assert!(Instant::now() < limit, "input control failed to wake the parked worker");
        std::thread::yield_now();
    }
    let start = Instant::now();
    drop(output);
    assert!(start.elapsed() < Duration::from_millis(100));
}

#[test]
fn repeating_beat_settings_starts_a_worker_that_failed_to_start() {
    let mut output = Output::default();
    let ports = vec![("DIN".into(), 0)];
    output.beats(ports.clone(), true);
    assert!(output.worker.is_none());
    output.configure(&Arc::new(Session::default()), &channel().0);
    output.beats(ports, true);
    assert!(output.worker.is_some());
}

#[test]
fn negative_offset_is_limited_to_the_output_lead_and_reported() {
    let base = Instant::now();
    let session = Session::default();
    for (offset, compensate, system_delay_samples, expected_us) in [(-4800, true, 0, 1000), (-480, false, 0, 0), (-4800, true, 2400, 1000)] {
        let lead = Duration::from_millis(20);
        let view = View::default();
        let mut port = active_port(offset, compensate);
        let stats = Stats::default();
        let mut seen = Vec::new();
        for us in (0..=500_000).step_by(250) {
            let now = base + Duration::from_micros(us);
            if us % 10_000 == 0 {
                view.lock().unwrap().push(Presentation { system_delay_samples, ..block(now, us as f64 * 0.048, 480., lead) });
            }
            port.poll("DIN", now, &session, &view, &stats, &mut Vec::new());
            for (_, msg) in port.link.as_mut().unwrap().messages.drain(..) {
                if msg == [CLOCK] {
                    seen.push(us);
                }
            }
        }
        let mut out = Output::default();
        out.beat.push(("DIN".into(), offset));
        out.compensated = compensate;
        Publisher::new(out.view.clone(), out.metrics.clone()).observe(DeviceTiming::new(base, Some(lead), 0), 0.01);
        out.view.lock().unwrap().push(Presentation { system_delay_samples, ..block(base, 0., 480., lead) });
        let status = out.status();
        let info = &status.ports["DIN"];
        assert!(info.advance_limited);
        assert_eq!(info.requested_offset_samples, offset);
        assert_eq!(info.device_lead_us, Some(20_000));
        assert_eq!(info.system_delay_samples, system_delay_samples as u64);
        assert_eq!(info.effective_total_us, Some(expected_us as i64));
        assert!(seen.len() >= 24);
        for (i, &actual) in seen.iter().enumerate() {
            let ideal = expected_us + (i as f64 * 1e6 / 48.).round() as u64;
            assert!(actual.abs_diff(ideal) <= 1500, "pulse {i} at {actual} us, expected {ideal}");
        }
        for pair in seen.windows(2) {
            assert!((pair[1] - pair[0]).abs_diff(20833) <= 1500);
        }
    }
}

#[test]
fn a_wrap_at_the_buffer_limit_waits_for_the_next_snapshot() {
    let base = Instant::now();
    let session = Session::default();
    for roll in [false, true] {
        let view = View::default();
        let r = soundcraft_time::Range::new(0, 4800);
        let b = Presentation { looped: Some(r), ..block(base, 4320., 480., Duration::ZERO) };
        view.lock().unwrap().push(b);
        let mut port = active_port(0, true);
        let stats = Stats::default();
        port.poll("DIN", base, &session, &view, &stats, &mut Vec::new());
        port.link.as_mut().unwrap().messages.clear();
        let boundary = base + Duration::from_millis(10);
        port.poll("DIN", boundary, &session, &view, &stats, &mut Vec::new());
        assert!(port.link.as_ref().unwrap().messages.is_empty(), "speculative loop transport");
        view.lock().unwrap().push(Presentation { pass: 2, playing: roll, ..block(boundary, 0., 480., Duration::ZERO) });
        port.poll("DIN", boundary, &session, &view, &stats, &mut Vec::new());
        let got: Vec<_> = port.link.unwrap().messages.into_iter().map(|(_, msg)| msg[0]).collect();
        assert_eq!(got, if roll { vec![STOP, START, CLOCK] } else { vec![STOP] });
    }
}

struct Messages(Vec<(Duration, Vec<u8>)>);
impl SyncEncoder for Messages {
    fn reset(&mut self) {}
    fn update(&mut self, _: Presentation, now: Instant, _: &Session, send: &mut dyn FnMut(Instant, &[u8])) -> Option<Instant> {
        for (delay, msg) in self.0.drain(..) {
            send(now + delay, &msg);
        }
        None
    }
}

#[test]
fn review_superseded_pass_discards_locates_both_before_and_at_their_deadline() {
    for change_ms in [4, 10] {
        let t = Instant::now();
        let delay = Duration::from_millis(10);
        let view = View::default();
        let s = Session::default();
        let stats = Stats::default();
        let mut port = active_port(0, false);
        port.routes[0].as_mut().unwrap().encoder = Box::new(Messages(vec![(delay, vec![0xf2, 7, 0])]));
        view.lock().unwrap().push(block(t, 0., 480., Duration::ZERO));
        port.poll("DIN", t, &s, &view, &stats, &mut vec![]);
        let changed = t + Duration::from_millis(change_ms);
        view.lock().unwrap().push(Presentation { pass: 2, ..block(changed, 4800., 480., Duration::ZERO) });
        port.poll("DIN", changed, &s, &view, &stats, &mut vec![]);
        port.poll("DIN", t + delay, &s, &view, &stats, &mut vec![]);
        assert!(port.link.unwrap().messages.is_empty(), "obsolete SPP at {change_ms} ms");
    }
}

#[test]
fn review_failed_transport_stop_is_retried_before_new_pass_start() {
    struct FailsOnce(bool, Vec<Vec<u8>>);
    impl SyncPort for FailsOnce {
        fn send_at(&mut self, at: Instant, msg: &[u8]) -> Result<Instant, String> {
            if msg == [STOP] && std::mem::take(&mut self.0) {
                return Err("port unavailable".into());
            }
            self.1.push(msg.to_vec());
            Ok(at)
        }
    }
    let now = Instant::now();
    let old = active_port(0, false);
    let mut port = Port {
        link: Some(FailsOnce(true, vec![])),
        retry: now,
        routes: old.routes,
        future: vec![],
        stop_pending: false,
        timing_epochs: [0; 2],
        waiting_for_timing: false,
        failure: None,
    };
    let view = View::default();
    let s = Session::default();
    let stats = Stats::default();
    for (ms, play) in [(0, false), (1, true), (2, false), (1002, true)] {
        let t = now + Duration::from_millis(ms);
        view.lock().unwrap().push(Presentation { pass: ms, playing: play, ..block(t, 0., 480., Duration::ZERO) });
        if ms == 1002 {
            assert!(port.link.is_none());
            port.link = Some(FailsOnce(false, vec![]));
            port.routes[0].as_mut().unwrap().encoder.reset();
        }
        port.poll("DIN", t, &s, &view, &stats, &mut vec![]);
    }
    assert_eq!(port.link.unwrap().1, [vec![STOP], vec![START], vec![CLOCK]]);
}

#[test]
fn review_delay_uses_rate_of_the_audible_callback_after_an_edit() {
    let now = Instant::now();
    let out = &mut Output::default();
    out.beat.push(("DIN".into(), 0));
    out.compensated = true;
    let mut publisher = Publisher::new(out.view.clone(), out.metrics.clone());
    publisher.observe(DeviceTiming::new(now, Some(Duration::ZERO), 0), 0.01);
    publisher.publish(Presentation {
        sample_rate: soundcraft_time::SampleRate::HZ_48000,
        system_delay_samples: 4800,
        ..block(now, 0., 480., Duration::ZERO)
    });
    out.session = Some(Arc::new(Session::new("new", soundcraft_time::SampleRate::new(96_000).unwrap())));
    assert_eq!(out.status().ports["DIN"].effective_total_us, Some(100_000));
}

#[test]
fn review_unavailable_generation_never_encodes_another_sessions_tempo() {
    let now = Instant::now();
    let mut p = active_port(0, false);
    let view = View::default();
    let s = Arc::new(Session::default());
    for generation in 1..=crate::sync_clock::MAX_REVISIONS as u64 + 1 {
        view.lock().unwrap().register(SessionRevision { generation, doc: s.clone(), remap: true }, true);
    }
    view.lock().unwrap().push(Presentation { generation: 1, ..block(now, 0., 480., Duration::ZERO) });
    let stats = Stats::default();
    p.poll("DIN", now, &s, &view, &stats, &mut vec![]);
    assert!(p.link.as_ref().unwrap().messages.is_empty());
    let next = now + Duration::from_millis(10);
    view.lock().unwrap().push(Presentation { generation: 9, pass: 2, ..block(next, 0., 480., Duration::ZERO) });
    p.poll("DIN", next, &s, &view, &stats, &mut vec![]);
    assert_eq!(p.link.unwrap().messages.iter().map(|(_, m)| m[0]).collect::<Vec<_>>(), [START, CLOCK]);
}

#[test]
fn future_messages_wait_in_the_port_queue_without_blocking_other_ports() {
    let now = Instant::now();
    let later = Duration::from_secs(2);
    let view = View::default();
    view.lock().unwrap().push(block(now, 0., 480., Duration::ZERO));
    let s = Session::default();
    let stats = Stats::default();
    let mut port = active_port(0, false);
    port.routes[0].as_mut().unwrap().encoder = Box::new(Messages(vec![(later, vec![0xa1]), (later, vec![0xa2])]));
    let mut clock = active_port(0, false);
    let mut buf = Vec::new();
    assert_eq!(port.poll("future", now, &s, &view, &stats, &mut buf), Some(now + later));
    assert!(port.link.as_ref().unwrap().messages.is_empty());
    clock.poll("clock", now, &s, &view, &stats, &mut buf);
    assert_eq!(clock.link.unwrap().messages.len(), 2);
    port.poll("future", now + later, &s, &view, &stats, &mut buf);
    assert_eq!(port.link.unwrap().messages, [(now + later, vec![0xa1]), (now + later, vec![0xa2])]);
}

#[test]
fn extended_sync_messages_are_delivered_and_oversized_messages_are_reported() {
    let now = Instant::now();
    let view = View::default();
    view.lock().unwrap().push(block(now, 0., 480., Duration::ZERO));
    let mut port = active_port(0, false);
    let packet = |len| {
        let mut m = vec![0; len];
        m[0] = 0xf0;
        m[len - 1] = 0xf7;
        m
    };
    port.routes[0].as_mut().unwrap().encoder = Box::new(Messages(vec![(Duration::ZERO, packet(13)), (Duration::ZERO, packet(17))]));
    let stats = Stats::default();
    port.poll("DIN", now, &Session::default(), &view, &stats, &mut Vec::new());
    assert_eq!(stats.oversized_dropped.load(Relaxed), 1);
    assert_eq!(port.link.unwrap().messages, [(now, packet(13))]);
    assert_eq!(port.failure.as_deref(), Some("DIN: MIDI message longer than 16 bytes dropped"));
}

#[test]
fn a_failed_stop_on_route_removal_reports_and_reconnects() {
    struct FailStop {
        first: bool,
        tx: Sender<(bool, Vec<u8>)>,
    }
    impl SyncPort for FailStop {
        fn send_at(&mut self, _: Instant, msg: &[u8]) -> Result<Instant, String> {
            let _ = self.tx.send((self.first, msg.to_vec()));
            if self.first && msg == [STOP] {
                return Err("stop rejected".into());
            }
            Ok(Instant::now())
        }
    }
    let view = View::default();
    view.lock().unwrap().push(block(Instant::now(), 0., 480., Duration::ZERO));
    let (tx, rx) = channel();
    let (sent, wire) = channel();
    let snapshot = Output::default();
    let stats = snapshot.stats.clone();
    let metrics = stats.clone();
    let trouble = snapshot.trouble.clone();
    let error = trouble.clone();
    let worker = std::thread::spawn(move || {
        let mut first = true;
        drive(view, Arc::new(Session::default()), rx, error, metrics, move |_| {
            Ok((FailStop { first: std::mem::take(&mut first), tx: sent.clone() }, None))
        })
    });
    tx.send(Update::Beats(vec![("DIN".into(), 0)], true)).unwrap();
    assert_eq!(wire.recv_timeout(Duration::from_secs(1)).unwrap().1, [START]);
    tx.send(Update::Encoder(Some(("DIN".into(), 0)), true, Box::new(Messages(vec![])))).unwrap();
    tx.send(Update::Beats(vec![], true)).unwrap();
    while wire.recv_timeout(Duration::from_secs(1)).unwrap().1 != [STOP] {}
    let until = Instant::now() + Duration::from_millis(200);
    while trouble.lock().unwrap().is_none() && Instant::now() < until {
        std::thread::yield_now();
    }
    assert_eq!(trouble.lock().unwrap().as_deref(), Some("DIN: stop rejected"));
    assert_eq!(snapshot.status().error.as_deref(), Some("DIN: stop rejected"));
    assert!(!stats.connected().contains("DIN"));
    assert_eq!(wire.recv_timeout(Duration::from_secs(3)).unwrap(), (false, vec![STOP]));
    drop(tx);
    worker.join().unwrap();
}

#[test]
fn compensation_includes_the_mixer_system_delay_when_delay_compensation_is_on() {
    let base = Instant::now();
    let session = Session::default();
    for (system_delay_samples, compensate, delay_ms) in [(0, true, 20), (4800, true, 120), (4800, false, 0)] {
        let view = View::default();
        view.lock().unwrap().push(Presentation { system_delay_samples, ..block(base, 0., 480., Duration::from_millis(20)) });
        let mut port = active_port(0, compensate);
        let stats = Stats::default();
        let due = base + Duration::from_millis(delay_ms);
        for ms in 0..=120 {
            port.poll("DIN", base + Duration::from_millis(ms), &session, &view, &stats, &mut Vec::new());
        }
        assert_eq!(port.link.unwrap().messages, [(due, vec![START]), (due, vec![CLOCK])]);
    }
}

#[test]
fn tempo_change_relocates_when_the_new_map_becomes_audible() {
    let base = Instant::now();
    let lead = Duration::from_millis(50);
    let initial = Arc::new(Session::default());
    let mut slow = (*initial).clone();
    slow.tempo.set_tempo(0, 60.).unwrap();
    let slow = Arc::new(slow);
    let view = View::default();
    view.lock().unwrap().register(SessionRevision { generation: 1, doc: initial.clone(), remap: true }, true);
    let mut port = active_port(0, true);
    let stats = Stats::default();
    let mut changes = Vec::new();
    let mut before = Vec::new();
    for ms in 0..=750 {
        let now = base + Duration::from_millis(ms);
        if ms == 500 {
            view.lock().unwrap().register(SessionRevision { generation: 2, doc: slow.clone(), remap: true }, true);
        }
        if ms % 10 == 0 {
            view.lock().unwrap().push(Presentation { generation: if ms >= 510 { 2 } else { 1 }, ..block(now, ms as f64 * 48., 480., lead) });
        }
        port.poll("DIN", now, if ms >= 500 { &slow } else { &initial }, &view, &stats, &mut Vec::new());
        for (_, msg) in port.link.as_mut().unwrap().messages.drain(..) {
            if msg == [STOP] {
                changes.push(ms);
            }
            if (500..560).contains(&ms) && msg == [CLOCK] {
                before.push(ms);
            }
        }
    }
    assert_eq!(before, [509, 530, 550]);
    assert_eq!(changes, [560]);
}

#[test]
fn unavailable_session_generations_are_ineligible_and_report_backlog_drops() {
    use crate::sync_clock::MAX_REVISIONS;
    let (cmd, _rx) = channel();
    let mut out = Output::default();
    out.stats.connected().insert("DIN".into());
    let s = Arc::new(Session::default());
    for generation in 1..=MAX_REVISIONS + 1 {
        assert_eq!(out.configure(&s, &cmd), generation as u64);
    }
    let latest = out.generation;
    let mut timeline = out.view.lock().unwrap();
    assert!(timeline.map(1).is_none());
    assert_eq!(timeline.map(2).unwrap().generation, 2);
    timeline.retire_before(latest);
    assert!(timeline.map(2).is_none());
    drop(timeline);
    assert_eq!(out.status().session_backlog_drops, 1);
}

#[test]
fn connected_worker_accepts_control_and_shuts_down_while_a_future_message_is_pending() {
    struct Future(Option<Sender<()>>);
    impl SyncEncoder for Future {
        fn reset(&mut self) {}
        fn update(&mut self, _: Presentation, now: Instant, _: &Session, send: &mut dyn FnMut(Instant, &[u8])) -> Option<Instant> {
            if let Some(started) = self.0.take() {
                send(now + Duration::from_secs(2), &[0xf8]);
                started.send(()).unwrap();
            }
            None
        }
    }
    let view = View::default();
    view.lock().unwrap().push(block(Instant::now(), 0., 480., Duration::ZERO));
    let (tx, rx) = channel();
    let (queued, ready) = channel();
    let (opened, devices) = channel();
    let worker = std::thread::spawn(move || {
        drive(view, Arc::new(Session::default()), rx, Default::default(), Arc::new(Stats::default()), |name| {
            let _ = opened.send(name.to_owned());
            Ok((FakePort::default(), None))
        });
    });
    tx.send(Update::Encoder(Some(("future".into(), 0)), false, Box::new(Future(Some(queued))))).unwrap();
    assert_eq!(devices.recv_timeout(Duration::from_secs(3)).unwrap(), "future");
    ready.recv_timeout(Duration::from_secs(3)).unwrap();
    std::thread::sleep(Duration::from_millis(225));
    let elapsed = Instant::now();
    tx.send(Update::Beats(vec![("controller".into(), 0)], false)).unwrap();
    let bound = Duration::from_millis(100);
    assert_eq!(devices.recv_timeout(bound).unwrap(), "controller");
    let control = elapsed.elapsed();
    std::thread::sleep(Duration::from_millis(225));
    let mut output = Output::default();
    let control_tx = WorkerSender { tx, worker: worker.thread().clone() };
    output.worker.replace((control_tx, worker));
    let elapsed = Instant::now();
    drop(output);
    let shutdown = elapsed.elapsed();
    eprintln!("control={control:?} shutdown={shutdown:?}");
    assert!(shutdown < bound, "Output::drop took {shutdown:?}");
}

#[test]
fn review_a_stall_beyond_the_repayment_beat_relocates_to_the_current_position() {
    let t = Instant::now();
    let view = View::default();
    let s = Session::default();
    let mut p = active_port(0, true);
    let stats = Stats::default();
    for ms in (0..=240).chain([1200]) {
        let now = t + Duration::from_millis(ms);
        if ms % 10 == 0 {
            view.lock().unwrap().push(Presentation {
                lead: (!(100..200).contains(&ms)).then_some(Duration::ZERO),
                ..block(now, ms as f64 * 48., 480., Duration::ZERO)
            });
        }
        p.link.as_mut().unwrap().messages.clear();
        p.poll("DIN", now, &s, &view, &stats, &mut vec![]);
    }
    assert_eq!(p.link.unwrap().messages.iter().map(|(_, m)| m[0]).collect::<Vec<_>>(), [STOP, 0xf2, CONTINUE]);
}

#[test]
fn recovered_clock_repays_missing_ticks_evenly_within_one_beat() {
    let base = Instant::now();
    let capture = |gap: std::ops::Range<u64>| {
        let view = View::default();
        let song = Session::new("clock recovery", soundcraft_time::SampleRate::HZ_48000);
        let mut port = active_port(0, true);
        let stats = Stats::default();
        let mut pending = Vec::new();
        let mut ticks = Vec::new();
        let mut transport = Vec::new();
        for ms in 0..=1000 {
            let now = base + Duration::from_millis(ms);
            if ms % 10 == 0 {
                let lead = (!gap.contains(&ms)).then_some(Duration::ZERO);
                view.lock().unwrap().push(Presentation { lead, ..block(now, ms as f64 * 48., 480., Duration::ZERO) });
            }
            port.poll("desk", now, &song, &view, &stats, &mut pending);
            for (due, msg) in port.link.as_mut().unwrap().messages.drain(..) {
                if msg == [CLOCK] {
                    ticks.push((ms, due.duration_since(base).as_secs_f64() * 1000.));
                } else {
                    transport.push((ms, msg));
                }
            }
        }
        (ticks, transport)
    };
    let (reference, reference_transport) = capture(0..0);
    for gap in [100..200, 50..250] {
        let (ticks, transport) = capture(gap.clone());
        let beat_end = gap.end + 500;
        let recovered: Vec<_> = ticks.iter().copied().filter(|(ms, _)| (gap.end..beat_end).contains(ms)).collect();
        let intervals: Vec<_> = recovered.windows(2).map(|p| p[1].0 - p[0].0).collect();
        eprintln!(
            "gap={gap:?} first={:?} intervals_ms={intervals:?} count={}/{} recovery_ticks={}",
            recovered.first(),
            ticks.len(),
            reference.len(),
            recovered.len()
        );
        assert_eq!(transport, reference_transport);
        assert!(ticks.iter().all(|(ms, _)| !gap.contains(ms)));
        assert_eq!(ticks.len(), reference.len());
        assert_eq!(ticks.iter().filter(|(ms, _)| *ms < beat_end).count(), reference.iter().filter(|(ms, _)| *ms < beat_end).count());
        let first_live = reference.iter().find(|(ms, _)| *ms >= gap.end).unwrap();
        assert_eq!(recovered[0], *first_live);
        assert!(recovered.iter().all(|(ms, due)| (0.0..=1.001).contains(&(*ms as f64 - due))));
        assert!(intervals.iter().min().unwrap() > &1);
        assert!(intervals.iter().max().unwrap() - intervals.iter().min().unwrap() <= 1);
        let after = |rows: &[(u64, f64)]| rows.iter().copied().filter(|(ms, _)| *ms >= beat_end).collect::<Vec<_>>();
        assert_eq!(after(&ticks), after(&reference));
    }
}

#[test]
fn a_missed_timing_gap_invalidates_only_the_compensated_route_queue() {
    let base = Instant::now();
    let view = View::default();
    let song = Session::default();
    let stats = Stats::default();
    let mut port = active_port(0, true);
    let delay = Duration::from_millis(20);
    for (index, compensate) in [true, false].into_iter().enumerate() {
        port.routes[index] =
            Some(Route { map: None, offset: 0, compensate, encoder: Box::new(Messages(vec![(delay, vec![if compensate { 0xf1 } else { 0xf8 }])])) });
    }
    view.lock().unwrap().push(block(base, 0., 480., Duration::ZERO));
    port.poll("desk", base, &song, &view, &stats, &mut vec![]);
    // Both callback publications land between two worker polls.
    let now = base + delay;
    let mut timeline = view.lock().unwrap();
    timeline.push(Presentation { lead: None, ..block(base + delay / 2, 480., 480., Duration::ZERO) });
    timeline.push(block(now, 960., 480., Duration::ZERO));
    // A positively offset destination must wait for the recovered block to become audible.
    let (audible, wake) = timeline.read(now, true, 480);
    assert!(audible.is_none());
    assert_eq!(wake, Some(now + Duration::from_millis(10)));
    drop(timeline);
    port.poll("desk", now, &song, &view, &stats, &mut vec![]);
    assert_eq!(port.link.unwrap().messages, [(now, vec![0xf8])]);
}

#[test]
fn a_stop_cancels_clock_repayment_and_a_new_pass_starts_normally() {
    let base = Instant::now();
    let view = View::default();
    let song = Session::new("stop during repayment", soundcraft_time::SampleRate::HZ_48000);
    let stats = Stats::default();
    let mut port = active_port(0, true);
    let mut pending = Vec::new();
    let mut messages = Vec::new();
    for ms in 0..=410 {
        let now = base + Duration::from_millis(ms);
        if ms % 10 == 0 {
            let mut b = block(now, ms as f64 * 48., 480., Duration::ZERO);
            b.lead = (!(100..200).contains(&ms)).then_some(Duration::ZERO);
            if ms >= 300 {
                b.playing = ms >= 400;
                b.pass = if b.playing { 3 } else { 2 };
                b.sample = if b.playing { (ms - 400) as f64 * 48. } else { 14_400. };
                b.limit = b.sample + 480.;
            }
            view.lock().unwrap().push(b);
        }
        port.poll("desk", now, &song, &view, &stats, &mut pending);
        for (_, msg) in port.link.as_mut().unwrap().messages.drain(..) {
            if ms >= 300 {
                messages.push((ms, msg));
            }
        }
    }
    assert_eq!(messages, [(300, vec![STOP]), (400, vec![START]), (400, vec![CLOCK])]);
}

#[test]
fn concurrent_delivery_stats_keep_every_event_and_the_largest_delay() {
    let stats = Stats::default();
    let at = Instant::now();
    std::thread::scope(|threads| {
        for writer in 0..4 {
            let stats = &stats;
            threads.spawn(move || {
                for i in 0..1000 {
                    stats.record(at, at + Duration::from_micros((999 - i) * 4 + writer));
                }
            });
        }
    });
    assert_eq!(stats.sent.load(Relaxed), 4000);
    assert_eq!(stats.max.load(Relaxed), 3999);
    assert_eq!(stats.buckets.iter().map(|value| value.load(Relaxed)).sum::<u64>(), 4000);
}
