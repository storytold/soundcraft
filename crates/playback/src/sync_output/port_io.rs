use super::{Destination, Stats, SyncPort};
use std::collections::VecDeque;
use std::sync::{Arc, mpsc};
use std::time::Instant;

enum Bytes {
    Short([u8; 16], usize),
    Long(Vec<u8>),
}
impl Bytes {
    fn new(bytes: &[u8]) -> Self {
        let mut short = [0; 16];
        if let Some(out) = short.get_mut(..bytes.len()) {
            out.copy_from_slice(bytes);
            Self::Short(short, bytes.len())
        } else {
            Self::Long(bytes.to_vec())
        }
    }
    fn as_slice(&self) -> &[u8] {
        match self {
            Self::Short(bytes, len) => &bytes[..*len],
            Self::Long(bytes) => bytes,
        }
    }
}

struct Packet {
    at: Instant,
    bytes: Bytes,
    retry: bool,
    clock_pass: Option<u64>,
}
struct Completion {
    packets: Vec<Packet>,
    failure: Option<(usize, String)>,
}

/// One outstanding batch per destination. The second channel slot is reserved
/// for the final cleanup on shutdown, even if the OS has not consumed the first
/// batch. Dropping the scheduler never joins a thread blocked inside the driver.
pub(super) struct PortIo {
    tx: mpsc::SyncSender<Vec<Packet>>,
    completions: mpsc::Receiver<Completion>,
    staged: Vec<Packet>,
    pending: usize,
    rejected: Option<Completion>,
}

impl PortIo {
    pub(super) fn new(mut connection: impl SyncPort + 'static, stats: Arc<Stats>) -> Result<Self, String> {
        let (tx, rx) = mpsc::sync_channel::<Vec<Packet>>(2);
        let (done, completions) = mpsc::channel();
        std::thread::Builder::new()
            .name("MIDI port delivery".into())
            .spawn(move || {
                super::raise_delivery_priority();
                let mut next_pulse: Option<i64> = None;
                let mut rolling = false;
                for packets in rx {
                    let failure = packets.iter().enumerate().find_map(|(index, p)| {
                        let msg = p.bytes.as_slice();
                        // Decode this destination's musical position in wire order, including
                        // a nonzero SPP before Continue. A reconnection starts with no position.
                        match msg {
                            [0xfa] => {
                                next_pulse = Some(0);
                                rolling = true;
                            }
                            [0xfb] => rolling = true,
                            [0xfc] => rolling = false,
                            [0xf2, lo, hi] => next_pulse = Some(6 * (i64::from(*lo) | (i64::from(*hi) << 7))),
                            [0xf8] if rolling => {
                                if let (Some(pass), Some(pulse)) = (p.clock_pass, next_pulse)
                                    && pass != 0
                                    && stats.stopped_pass.load(std::sync::atomic::Ordering::Acquire) == pass
                                {
                                    let (stopped, last) = *stats.stopped_boundary.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                                    if stopped == pass && pulse > last {
                                        return None;
                                    }
                                }
                                next_pulse = next_pulse.map(|n| n.saturating_add(1));
                            }
                            _ => {}
                        }
                        match connection.send_at(p.at, msg) {
                            Ok(handed) => {
                                stats.delivered(p.at, handed, msg, p.retry);
                                None
                            }
                            Err(e) => Some((index, e)),
                        }
                    });
                    // Normal errors disconnect the scheduler's handle. During shutdown
                    // keep consuming its queued Stop and any unacknowledged cleanup.
                    let _ = done.send(Completion { packets, failure });
                }
            })
            .map_err(|e| e.to_string())?;
        Ok(Self { tx, completions, staged: Vec::new(), pending: 0, rejected: None })
    }
}
impl Destination for PortIo {
    fn submit(&mut self, at: Instant, bytes: &[u8], retry: bool, _stats: &Stats) -> Result<(), String> {
        self.staged.push(Packet { at, bytes: Bytes::new(bytes), retry, clock_pass: None });
        Ok(())
    }
    fn submit_beat(&mut self, at: Instant, pass: u64, _stats: &Stats) -> Result<(), String> {
        self.staged.push(Packet { at, bytes: Bytes::new(&[0xf8]), retry: false, clock_pass: Some(pass) });
        Ok(())
    }
    fn flush(&mut self) {
        if self.staged.is_empty() {
            return;
        }
        self.pending += 1;
        if let Err(err) = self.tx.try_send(std::mem::take(&mut self.staged)) {
            let (packets, reason) = match err {
                mpsc::TrySendError::Full(batch) => (batch, "MIDI delivery queue is full"),
                mpsc::TrySendError::Disconnected(batch) => (batch, "MIDI delivery thread stopped"),
            };
            self.rejected = Some(Completion { packets, failure: Some((0, reason.into())) });
        }
    }
    fn busy(&self) -> bool {
        self.pending != 0
    }
    fn ready(&mut self, cleanup: &mut VecDeque<Vec<u8>>) -> Result<bool, String> {
        if self.pending == 0 {
            return Ok(true);
        }
        let mut ack = match self.rejected.take() {
            Some(failure) => failure,
            None => match self.completions.try_recv() {
                Ok(ack) => ack,
                Err(mpsc::TryRecvError::Empty) => return Ok(false),
                Err(mpsc::TryRecvError::Disconnected) => return Err("MIDI delivery thread stopped".into()),
            },
        };
        self.pending -= 1;
        if let Some((first, error)) = ack.failure {
            for packet in ack.packets.get(first..).unwrap_or_default().iter().rev().filter(|p| p.retry) {
                cleanup.push_front(packet.bytes.as_slice().to_vec());
            }
            return Err(error);
        }
        ack.packets.clear();
        self.staged = ack.packets;
        Ok(self.pending == 0)
    }
}
impl Drop for PortIo {
    fn drop(&mut self) {
        self.flush();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_failure_index_preserves_the_error_and_existing_cleanup() {
        let (tx, _rx) = mpsc::sync_channel(2);
        let (_done, completions) = mpsc::channel();
        let failure = Completion { packets: Vec::new(), failure: Some((1, "delivery failed".into())) };
        let mut port = PortIo { tx, completions, staged: Vec::new(), pending: 1, rejected: Some(failure) };
        let mut cleanup = VecDeque::from([vec![0xfc]]);
        assert_eq!(port.ready(&mut cleanup), Err("delivery failed".into()));
        assert_eq!(cleanup, VecDeque::from([vec![0xfc]]));
    }
}
#[cfg(test)]
mod stop_boundary_tests {
    use super::super::{Output, SyncPort};
    use super::*;
    use crate::midi::MidiFeed;
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    struct Wire(Arc<Mutex<Vec<Vec<u8>>>>);
    impl SyncPort for Wire {
        fn send_at(&mut self, _: Instant, bytes: &[u8]) -> Result<Instant, String> {
            self.0.lock().unwrap().push(bytes.to_vec());
            Ok(Instant::now())
        }
    }

    fn check(start: i64, coast: bool, outgoing_pass: u64, final_clocks: usize, expect: usize) {
        let mut output = Output::default();
        let feed = Arc::new(MidiFeed::default());
        output.attach_mtc_feed(&feed);
        feed.set_clock_follow(Some(0), true, 0, false);
        let spp = [0xf2, (start / 6) as u8, 0];
        let launch = if start == 0 { 0xfa } else { 0xfb };
        feed.set_clock(0);
        feed.receive(0, &spp);
        feed.set_clock(0);
        feed.receive(0, &[launch]);
        feed.set_clock(0);
        feed.receive(0, &[0xf8]);
        feed.clock_audio_pass.store(42, std::sync::atomic::Ordering::Release);
        feed.set_clock(20_833);
        feed.receive(0, &[0xf8]);
        feed.set_clock(if coast { 900_000 } else { 22_000 });
        feed.receive(0, &[0xfc]);

        let sent = Arc::new(Mutex::new(Vec::new()));
        let mut port = PortIo::new(Wire(sent.clone()), output.stats.clone()).unwrap();
        let now = Instant::now();
        port.submit(now, &spp, false, &output.stats).unwrap();
        port.submit(now, &[launch], false, &output.stats).unwrap();
        for _ in 0..final_clocks {
            port.submit_beat(now, outgoing_pass, &output.stats).unwrap();
        }
        port.submit(now, &[0xfc], true, &output.stats).unwrap();
        port.flush();
        let mut cleanup = VecDeque::new();
        let end = Instant::now() + Duration::from_secs(2);
        while !port.ready(&mut cleanup).unwrap() {
            assert!(Instant::now() < end);
            std::thread::yield_now();
        }
        let sent = sent.lock().unwrap();
        assert_eq!(sent.iter().filter(|msg| msg.as_slice() == [0xf8]).count(), expect);
        assert_eq!(sent.last().unwrap().as_slice(), [0xfc]);
    }
    #[test]
    fn explicit_external_stop_keeps_the_owed_clock_and_rejects_only_surplus() {
        check(0, false, 42, 3, 2);
    }
    #[test]
    fn nonzero_continue_stop_uses_absolute_pulses_and_only_its_audio_pass() {
        check(96, false, 42, 3, 2);
        check(96, false, 43, 3, 3);
    }
    #[test]
    fn stop_after_loss_does_not_bound_coasting_by_the_old_wire_count() {
        check(0, true, 42, 3, 3);
    }

    #[test]
    fn stop_between_two_native_sends_preserves_an_owed_tick_in_the_outstanding_batch() {
        use crate::{Cmd, Shared, callback::AudioState};
        use std::sync::mpsc::{Receiver, SyncSender, channel, sync_channel};

        struct PausedPort {
            first_clock: Option<(SyncSender<()>, Receiver<()>)>,
            sent: std::sync::mpsc::Sender<u8>,
        }
        impl SyncPort for PausedPort {
            fn send_at(&mut self, _: Instant, bytes: &[u8]) -> Result<Instant, String> {
                let at = Instant::now();
                self.sent.send(bytes[0]).unwrap();
                if bytes == [0xf8]
                    && let Some((entered, resume)) = self.first_clock.take()
                {
                    entered.send(()).unwrap();
                    resume.recv_timeout(Duration::from_secs(2)).unwrap();
                }
                Ok(at)
            }
        }

        let mut output = Output::default();
        let feed = Arc::new(MidiFeed::default());
        output.attach_mtc_feed(&feed);
        feed.set_clock_follow(Some(0), false, 0, false);
        feed.set_clock(1);
        feed.receive(0, &[0xfa, 0xf8]);
        let (tx, rx) = channel();
        let shared = Arc::new(Shared { position: 0.into(), playing: false.into(), speed: 1000.into(), meters: Mutex::default() });
        let mut audio = AudioState::new(rx, shared, Arc::default(), 48_000., channel().0);
        tx.send(Cmd::Midi(feed.clone())).unwrap();
        tx.send(Cmd::Clock(Some(crate::sync_clock::Publisher::new(output.view.clone(), output.metrics.clone())))).unwrap();
        tx.send(Cmd::PlayClock { origin: 0, render_from: 0, arrival: None }).unwrap();
        audio.fill(&mut [0.; 256], 2);
        let (pass, playing) = output.view.lock().unwrap().transport_state().unwrap();
        assert!(playing);
        feed.set_clock(20_834);
        feed.receive(0, &[0xf8]);

        let (notify, entered) = sync_channel(1);
        let (resume, wait) = channel();
        let (sent, received) = channel();
        let mut port = PortIo::new(PausedPort { first_clock: Some((notify, wait)), sent }, output.stats.clone()).unwrap();
        let now = Instant::now();
        port.submit(now, &[0xfa], false, &output.stats).unwrap();
        for _ in 0..3 {
            port.submit_beat(now, pass, &output.stats).unwrap();
        }
        port.submit(now, &[0xfc], true, &output.stats).unwrap();
        port.flush();
        entered.recv_timeout(Duration::from_secs(2)).unwrap();
        feed.set_clock(22_000);
        feed.receive(0, &[0xfc]);
        resume.send(()).unwrap();
        let mut bytes = vec![];
        while bytes.last() != Some(&0xfc) {
            bytes.push(received.recv_timeout(Duration::from_secs(2)).unwrap());
        }
        assert_eq!(bytes, [0xfa, 0xf8, 0xf8, 0xfc]);
    }
}
