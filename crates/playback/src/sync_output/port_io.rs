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
                for packets in rx {
                    let failure = packets.iter().enumerate().find_map(|(index, p)| {
                        let msg = p.bytes.as_slice();
                        match connection.send_at(p.at, msg) {
                            Ok(handed) => {
                                stats.delivered(p.at, handed, msg, p.retry);
                                None
                            }
                            Err(e) => Some((index, e)),
                        }
                    });
                    let failed = failure.is_some();
                    let _ = done.send(Completion { packets, failure });
                    if failed {
                        break;
                    }
                }
            })
            .map_err(|e| e.to_string())?;
        Ok(Self { tx, completions, staged: Vec::new(), pending: 0, rejected: None })
    }
}
impl Destination for PortIo {
    fn submit(&mut self, at: Instant, bytes: &[u8], retry: bool, _stats: &Stats) -> Result<(), String> {
        self.staged.push(Packet { at, bytes: Bytes::new(bytes), retry });
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
            for packet in ack.packets[first..].iter().rev().filter(|p| p.retry) {
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
