//! Optional host timing harness. No MIDI driver, hardware, or wall-clock CI thresholds.
use super::*;
use crate::sync_clock::Presentation;
use craft_midi_sync::sync::{CLOCK, START, STOP};

#[test]
#[ignore = "host timing measurement; run alone with --nocapture --test-threads=1"]
fn beat_clock_host_timing() {
    for (frames, paired, load, seconds) in [(64, false, 0, 5), (256, false, 0, 5), (1024, true, 0, 5), (1024, true, 30, 30)] {
        let mut session = Session::default();
        session.tempo.set_tempo(0, 180.).unwrap();
        let rate = session.sample_rate.as_f64();
        let (samples, record) = channel();
        struct Sink(Sender<(Instant, Instant, Vec<u8>)>);
        impl SyncPort for Sink {
            fn send_at(&mut self, at: Instant, b: &[u8]) -> Result<Instant, String> {
                let now = Instant::now();
                self.0.send((at, now, b.to_vec())).unwrap();
                Ok(now)
            }
        }
        let view = View::default();
        let worker_view = view.clone();
        let (tx, rx) = channel();
        let stats = Arc::new(Stats::default());
        let worker_stats = stats.clone();
        let worker = std::thread::spawn(move || {
            drive(worker_view, Arc::new(session), rx, Arc::default(), worker_stats, move |_| Ok((Sink(samples.clone()), None)))
        });
        tx.send(Update::Beats(vec![("in-process".into(), 0)], true)).unwrap();
        let start = Instant::now() + Duration::from_millis(80);
        let lead = Duration::from_millis(30);
        let end = start + Duration::from_secs(seconds);
        let workers = load.min(std::thread::available_parallelism().map_or(1, usize::from).saturating_sub(2));
        // Emulate the device callback at elevated priority independently of
        // the output worker. Record host lateness of this synthetic publisher too.
        let producer_priority = thread_priority::ThreadPriorityValue::try_from(68u8)
            .ok()
            .is_some_and(|v| thread_priority::ThreadPriority::Crossplatform(v).set_for_current().is_ok());
        let mut publish_late = Vec::new();
        std::thread::scope(|scope| {
            for _ in 0..workers {
                scope.spawn(|| {
                    if let Ok(normal) = thread_priority::ThreadPriorityValue::try_from(50u8) {
                        let _ = thread_priority::ThreadPriority::Crossplatform(normal).set_for_current();
                    }
                    let mut seed = 1u64;
                    while Instant::now() < end {
                        for _ in 0..16384 {
                            seed = std::hint::black_box(seed.wrapping_mul(6364136223846793005).wrapping_add(1));
                        }
                    }
                });
            }
            let mut index = 0;
            while (index as f64) < rate * seconds as f64 {
                let due = start + Duration::from_secs_f64(index as f64 / rate);
                std::thread::sleep(due.saturating_duration_since(Instant::now()));
                publish_late.push(Instant::now().saturating_duration_since(due).as_secs_f64() * 1000.);
                for _ in 0..if paired { 2 } else { 1 } {
                    view.lock().unwrap().push(Presentation {
                        pass: 1,
                        origin: 0,
                        playing: true,
                        sample: index as f64,
                        limit: (index + frames) as f64,
                        samples_per_second: rate,
                        heard_at: start + lead + Duration::from_secs_f64(index as f64 / rate),
                        looped: None,
                        end: None,
                        lead,
                    });
                    if index == 0 {
                        tx.send(Update::Wake).unwrap();
                    }
                    index += frames;
                }
            }
        });
        std::thread::sleep((end + lead).saturating_duration_since(Instant::now()));
        drop(tx);
        worker.join().unwrap();
        let rows = record.try_iter().collect::<Vec<_>>();
        let stamps = rows.iter().filter(|(_, _, m)| m == &[CLOCK]).collect::<Vec<_>>();
        let mut late = stamps.iter().map(|(t, a, _)| a.saturating_duration_since(*t).as_secs_f64() * 1000.).collect::<Vec<_>>();
        let mut errors = stamps.windows(2).map(|p| ((p[1].1 - p[0].1).as_secs_f64() * 1000. - 60_000. / (180. * 24.)).abs()).collect::<Vec<_>>();
        late.sort_by(f64::total_cmp);
        errors.sort_by(f64::total_cmp);
        let spp = rows.iter().filter(|(_, _, b)| b[0] == 0xf2).count();
        let stops = rows.iter().filter(|(_, _, b)| b[0] == STOP).count();
        let starts = rows.iter().filter(|(_, _, b)| b[0] == START).count();
        assert!(!errors.is_empty());
        publish_late.sort_by(f64::total_cmp);
        println!(
            "worker_priority={} producer_raised={producer_priority} producer_p95_ms={:.3} producer_max_ms={:.3}",
            stats.priority.load(Relaxed),
            publish_late[publish_late.len() * 95 / 100],
            publish_late.last().unwrap()
        );
        println!(
            "180 BPM frames={frames} paired={paired} load_threads={workers} seconds={seconds} clocks={} starts={starts} spp={spp} stops={stops} late_p95_ms={:.3} gap_error_p95_ms={:.3} catch_up={}",
            stamps.len(),
            late[late.len() * 95 / 100],
            errors[errors.len() * 95 / 100],
            stats.catch_up_sends.load(Relaxed)
        );
    }
}

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
        let list = output_names().unwrap();
        let name = list.into_iter().find(|n| n.contains(&unique)).expect("OS did not publish the virtual destination");
        (reader, name)
    };
    let ready = Instant::now() + Duration::from_millis(40);
    let view = View::default();
    view.lock().unwrap().push(Presentation {
        pass: 1,
        origin: 0,
        playing: true,
        sample: 0.,
        limit: 4000.,
        samples_per_second: 48_000.,
        heard_at: ready,
        looped: None,
        end: None,
        lead: Duration::ZERO,
    });
    let (tx, rx) = channel();
    let worker = std::thread::spawn(move || drive(view, Arc::new(Session::default()), rx, Arc::default(), Arc::default(), open_unique));
    tx.send(Update::Beats(vec![(name, 0)], true)).unwrap();
    assert_eq!(observed.recv_timeout(Duration::from_secs(2)).unwrap(), [START]);
    assert_eq!(observed.recv_timeout(Duration::from_secs(2)).unwrap(), [CLOCK]);
    drop(tx);
    worker.join().unwrap();
}
