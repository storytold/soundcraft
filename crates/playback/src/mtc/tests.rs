use super::*;
use crate::{AudioState, MeterSnapshot};
use soundcraft_time::{Range, SampleRate};
use std::sync::atomic::{AtomicI64, AtomicU32};
use std::sync::mpsc::{Sender, channel};

fn session(rate: FrameRate) -> Session {
    let mut session = Session::new("MTC", SampleRate::new(48_000).unwrap());
    session.frame_rate = rate;
    session
}

#[test]
fn rate_bits_midnight_and_drop_frame_minute_boundaries() {
    for (rate, bits, end) in [
        (FrameRate::Fps24, 0x00, 2_073_600),
        (FrameRate::Fps25, 0x20, 2_160_000),
        (FrameRate::Fps29_97Drop, 0x40, 2_589_408),
        (FrameRate::Fps30, 0x60, 2_592_000),
    ] {
        assert_eq!(locate(end, rate).bytes(), &[0xf0, 0x7f, 0x7f, 1, 1, bits, 0, 0, 0, 0xf7]);
        assert_eq!(locate(-1, rate), locate(end - 1, rate));
        assert_eq!(quarters(0, rate)[7], [0xf1, 0x70 | (bits >> 4)]);
    }
    assert_eq!(&locate(1800, FrameRate::Fps29_97Drop).bytes()[5..9], &[0x40, 1, 0, 2]);
    assert_eq!(&locate(17_982, FrameRate::Fps29_97Drop).bytes()[5..9], &[0x40, 10, 0, 0]);
    assert_eq!(quarters(1800, FrameRate::Fps29_97Drop)[0], [0xf1, 2]);
}

fn callback(s: Session, device_rate: f64) -> (AudioState, Sender<Cmd>, Receiver<Event>) {
    let shared = Arc::new(Shared {
        position: AtomicI64::new(0),
        playing: AtomicBool::new(false),
        speed: AtomicU32::new(1000),
        meters: Mutex::new(MeterSnapshot::default()),
        output_position: Mutex::default(),
    });
    let (commands, rx) = channel();
    let state = AudioState::new(rx, shared, Arc::new(s), device_rate, channel().0);
    let (output, packets) = sync_channel(256);
    commands.send(Cmd::Mtc(Some(Generator::new(output)))).unwrap();
    (state, commands, packets)
}

#[test]
fn callback_generates_spaced_groups_at_session_time_and_stops_at_range_end() {
    for rate in [FrameRate::Fps24, FrameRate::Fps25, FrameRate::Fps29_97Drop, FrameRate::Fps30] {
        let mut s = session(rate);
        let frame = if rate.is_drop() { 1800 } else { rate.nominal() * 60 };
        s.timecode_start = rate.samples_at_frame(frame, s.sample_rate);
        let (mut state, commands, packets) = callback(s, 44_100.0);
        commands.send(Cmd::Play { from: 0, end: Some(7200), looped: None }).unwrap();
        state.fill(&mut vec![0.; 44_100 / 5 * 2], 2);
        let events: Vec<_> = packets.try_iter().collect();
        assert_eq!(events.first().unwrap().1, locate(frame, rate));
        assert!(matches!(events.last().unwrap().1, Packet::Locate(_)));
        let quarters_out: Vec<_> = events.iter().filter(|(_, p)| matches!(p, Packet::Quarter(_))).collect();
        assert!(quarters_out.len() >= 8);
        for (index, (at, p)) in quarters_out[..8].iter().enumerate() {
            let (n, d) = rate.rational();
            let elapsed = at.duration_since(quarters_out[0].0).as_secs_f64();
            assert!((elapsed - index as f64 * d as f64 / (4 * n) as f64).abs() < 0.0001);
            assert_eq!(*p, Packet::Quarter(quarters(frame, rate)[index]));
        }
        state.fill(&mut [0.; 512], 2);
        assert!(packets.try_recv().is_err());
    }
}

#[test]
fn stopped_locate_play_wrap_seek_and_disable_use_the_callback_command_path() {
    let rate = FrameRate::Fps25;
    let (mut state, commands, packets) = callback(session(rate), 48_000.);
    commands.send(Cmd::Locate(48_000)).unwrap();
    state.fill(&mut [0.; 256], 2);
    assert_eq!(packets.try_iter().map(|(_, p)| p).collect::<Vec<_>>(), [locate(25, rate)]);
    commands.send(Cmd::Play { from: 0, end: None, looped: Some(Range::new(0, 3840)) }).unwrap();
    state.fill(&mut vec![0.; 4000 * 2], 2);
    let events: Vec<_> = packets.try_iter().map(|(_, p)| p).collect();
    assert_eq!(events.iter().filter(|p| **p == locate(0, rate)).count(), 2);
    assert_eq!(events[9], locate(0, rate));
    assert_eq!(events[10], Packet::Quarter([0xf1, 0]));
    commands.send(Cmd::Locate(1920)).unwrap();
    state.fill(&mut [0.; 256], 2);
    assert_eq!(packets.try_recv().unwrap().1, locate(1, rate));
    commands.send(Cmd::Stop).unwrap();
    state.fill(&mut [0.; 256], 2);
    assert!(matches!(packets.try_recv().unwrap().1, Packet::Locate(_)));
    commands.send(Cmd::Mtc(None)).unwrap();
    commands.send(Cmd::Play { from: 0, end: None, looped: None }).unwrap();
    state.fill(&mut [0.; 256], 2);
    assert!(packets.try_recv().is_err());
}

#[test]
fn unsupported_session_rate_sends_no_mislabeled_packets() {
    let (mut state, commands, packets) = callback(session(FrameRate::Fps23_976), 48_000.);
    commands.send(Cmd::Play { from: 0, end: None, looped: None }).unwrap();
    state.fill(&mut [0.; 1024], 2);
    assert!(packets.try_recv().is_err());
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn worker_delivers_each_packet_after_its_deadline_and_releases_the_port() {
    let (sent, received) = channel();
    let output = MtcOutput::with_sink(move |bytes| sent.send((Instant::now(), bytes.to_vec())).map_err(|e| e.to_string())).unwrap();
    let now = Instant::now();
    let later = now + Duration::from_millis(25);
    output.tx.send((now, locate(25, FrameRate::Fps25))).unwrap();
    output.tx.send((later, Packet::Quarter([0xf1, 0x10]))).unwrap();
    let timeout = Duration::from_secs(2);
    assert_eq!(received.recv_timeout(timeout).unwrap().1, locate(25, FrameRate::Fps25).bytes());
    let (at, bytes) = received.recv_timeout(timeout).unwrap();
    assert!(at >= later);
    assert_eq!(bytes, [0xf1, 0x10]);
    output.tx.send((Instant::now() + Duration::from_secs(60), Packet::Quarter([0xf1, 0x20]))).unwrap();
    drop(output);
    assert!(matches!(received.try_recv(), Err(std::sync::mpsc::TryRecvError::Disconnected)));
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn delayed_port_resumes_at_a_complete_quarter_frame_group() {
    let (sent, received) = channel();
    let output = MtcOutput::with_sink(move |bytes| sent.send(bytes.to_vec()).map_err(|e| e.to_string())).unwrap();
    let now = Instant::now();
    output.tx.send((now - Duration::from_secs(1), Packet::Quarter([0xf1, 0x10]))).unwrap();
    output.tx.send((now, Packet::Quarter([0xf1, 0x20]))).unwrap();
    output.tx.send((now, Packet::Quarter([0xf1, 0x01]))).unwrap();
    assert_eq!(received.recv_timeout(Duration::from_secs(2)).unwrap(), [0xf1, 0x01]);
}
