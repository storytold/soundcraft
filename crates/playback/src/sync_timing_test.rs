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
        std::thread::scope(|scope| {
            for _ in 0..workers {
                scope.spawn(|| {
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
        println!(
            "180 BPM frames={frames} paired={paired} load_threads={workers} seconds={seconds} clocks={} starts={starts} spp={spp} stops={stops} late_p95_ms={:.3} gap_error_p95_ms={:.3} catch_up={}",
            stamps.len(),
            late[late.len() * 95 / 100],
            errors[errors.len() * 95 / 100],
            stats.catch_up_sends.load(Relaxed)
        );
    }
}
