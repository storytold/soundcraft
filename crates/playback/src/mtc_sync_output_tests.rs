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
    let mut s = Session::default();
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

/// Real OS scheduling observations for the two encoders sharing a mock connection.
/// No wall-clock upper bound is a correctness gate.
#[test]
#[ignore]
fn measure_native_worker_quarter_frame_spacing() {
    for fps in [24, 30] {
        let mut sess = Session::default();
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
