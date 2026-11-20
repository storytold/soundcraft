//! MIDI Time Code output, clocked by the samples delivered to the playback device.
//! The audio callback queues dated packets. Only the output worker waits or calls the MIDI driver.
use crate::{Cmd, Player, Shared};
use craft_midi_sync::sync::{Rate, full_frame, quarter_frames};
use soundcraft_model::Session;
use soundcraft_time::{FrameRate, Timecode};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
#[cfg(any(all(test, not(target_arch = "wasm32")), target_os = "linux", target_os = "macos", target_os = "windows"))]
use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

pub fn supported(rate: FrameRate) -> bool {
    matches!(rate, FrameRate::Fps23_976 | FrameRate::Fps24 | FrameRate::Fps25 | FrameRate::Fps29_97 | FrameRate::Fps29_97Drop | FrameRate::Fps30)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Packet {
    Locate([u8; 10]),
    Quarter([u8; 2]),
}

impl Packet {
    #[cfg(any(test, target_os = "linux", target_os = "macos", target_os = "windows"))]
    fn bytes(&self) -> &[u8] {
        match self {
            Self::Locate(bytes) => bytes,
            Self::Quarter(bytes) => bytes,
        }
    }
}

// craft-midi encodes integer-rate labels. For drop frame, craft-time supplies the skipped labels
// and the wire rate bits distinguish them from 30 non-drop. A MIDI day wraps at midnight.
fn label(frame: i64, rate: FrameRate) -> (f64, Rate) {
    let day = if rate.is_drop() { 2_589_408 } else { rate.nominal() * 86_400 };
    let time = Timecode::from_frames(frame.rem_euclid(day), rate);
    let seconds = (time.hours * 3600 + time.minutes * 60 + time.seconds) as f64 + time.frames as f64 / rate.nominal() as f64;
    (seconds, Rate::of(rate.nominal() as u8))
}

fn locate(frame: i64, rate: FrameRate) -> Packet {
    let (seconds, wire) = label(frame, rate);
    let mut message = full_frame(seconds, wire);
    if rate.is_drop() {
        message[5] = (message[5] & 0x1f) | 0x40;
    }
    Packet::Locate(message)
}

pub(crate) fn quarters(frame: i64, rate: FrameRate) -> [[u8; 2]; 8] {
    let (seconds, wire) = label(frame, rate);
    let mut messages = quarter_frames(seconds, wire);
    if rate.is_drop() {
        messages[7][1] = (messages[7][1] & !6) | 4;
    }
    messages
}

type Event = (Instant, Packet);

// The existing SPSC MIDI ring carries a signed nanosecond offset and four
// data bytes. The fixed SysEx header/trailer are reconstructed on the MIDI thread.
pub(crate) struct Queue {
    ring: crate::midi::Ring,
    epoch: Instant,
    retired: AtomicBool,
}
impl Queue {
    fn new() -> Arc<Self> {
        Arc::new(Self { ring: crate::midi::Ring::new(128), epoch: Instant::now(), retired: false.into() })
    }
    fn push(&self, (when, packet): Event) -> bool {
        let ns =
            if when >= self.epoch { when.duration_since(self.epoch).as_nanos() as i64 } else { -(self.epoch.duration_since(when).as_nanos() as i64) };
        let bytes = match packet {
            Packet::Locate([_, _, _, _, _, h, m, s, f, _]) => [h, m, s, f],
            Packet::Quarter([status, data]) => [status, data, 0, 0],
        };
        self.ring.push(ns, u32::from_be_bytes(bytes))
    }
    #[cfg(any(test, target_os = "linux", target_os = "macos", target_os = "windows"))]
    fn pop(&self) -> Option<Event> {
        let (ns, bits) = self.ring.pop()?;
        let [h, m, s, f] = bits.to_be_bytes();
        let packet = if h == 0xf1 { Packet::Quarter([h, m]) } else { Packet::Locate([0xf0, 0x7f, 0x7f, 1, 1, h, m, s, f, 0xf7]) };
        let dt = Duration::from_nanos(ns.unsigned_abs());
        Some((if ns < 0 { self.epoch.checked_sub(dt)? } else { self.epoch.checked_add(dt)? }, packet))
    }
}

pub(crate) struct Generator {
    tx: Arc<Queue>,
    next: Option<i64>,
    group: [[u8; 2]; 8],
}

impl Generator {
    fn new(tx: Arc<Queue>) -> Self {
        Self { tx, next: None, group: [[0; 2]; 8] }
    }

    pub(crate) fn reset(&mut self) {
        self.next = None;
    }

    /// The first sample, length and sampling ratio of a contiguous span of device frames.
    /// A stopped transport delivers a zero-length span for a locate, with no quarter frames.
    pub(crate) fn span(&mut self, session: &Session, start: f64, length: usize, step: f64, device_rate: f64, at: Instant) {
        let rate = session.frame_rate;
        if !supported(rate) {
            self.reset();
            return;
        }
        let per_quarter = rate.samples_per_frame(session.sample_rate) / 4.0;
        let mut next = match self.next {
            Some(next) => next,
            None => {
                let frame = rate.frames_at(start.floor() as i64, session.sample_rate);
                if !self.tx.push((at, locate(frame, rate))) {
                    return;
                }
                // Start a whole eight-piece group, aligned with an even frame on the timeline.
                ((start / (8.0 * per_quarter)).ceil() as i64).saturating_mul(8)
            }
        };
        let end = start + length as f64 * step;
        while (next as f64) * per_quarter < end && length > 0 {
            let piece = next.rem_euclid(8) as usize;
            if piece == 0 {
                self.group = quarters(next.div_euclid(4), rate);
            }
            let seconds = ((next as f64 * per_quarter - start) / (step * device_rate)).max(0.0);
            if !self.tx.push((at + Duration::from_secs_f64(seconds), Packet::Quarter(self.group[piece]))) {
                // A slow port lost part of the group. Relocate and begin a fresh group.
                self.reset();
                return;
            }
            next += 1;
        }
        self.next = Some(next);
    }
}

pub struct MtcOutput {
    route: Sender<Arc<Queue>>,
    tx: Arc<Queue>,
    closed: Arc<AtomicBool>,
    failure: Arc<Mutex<Option<String>>>,
    worker: Option<std::thread::JoinHandle<()>>,
    attached: Weak<Shared>,
}

impl MtcOutput {
    /// Attach or reattach after the application replaces its playback device.
    pub fn attach(&mut self, player: &Player) {
        let shared = Arc::downgrade(&player.shared);
        if !self.attached.ptr_eq(&shared) {
            if !self.attached.ptr_eq(&Weak::new()) {
                self.tx.retired.store(true, Ordering::Release);
                self.tx = Queue::new();
                let _ = self.route.send(self.tx.clone());
                if let Some(worker) = &self.worker {
                    worker.thread().unpark();
                }
            }
            player.install_mtc_queue(Some(self.tx.clone()));
            self.attached = shared;
        }
    }

    pub fn error(&self) -> Option<String> {
        self.failure.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone()
    }

    #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
    pub fn open(name: &str) -> Result<Self, String> {
        let midi = midir::MidiOutput::new("SoundCraft MTC").map_err(|e| e.to_string())?;
        let mut matching = midi.ports().into_iter().filter(|p| midi.port_name(p).is_ok_and(|n| n == name));
        let port = matching.next().ok_or_else(|| format!("MIDI output unavailable: {name}"))?;
        if matching.next().is_some() {
            return Err(format!("More than one MIDI output is named {name}"));
        }
        let mut connection = midi.connect(&port, "SoundCraft MTC").map_err(|e| e.to_string())?;
        Self::with_sink(move |bytes| connection.send(bytes).map_err(|e| e.to_string()))
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    pub fn open(_name: &str) -> Result<Self, String> {
        Err("MIDI output is unavailable on this platform".into())
    }

    #[cfg(any(all(test, not(target_arch = "wasm32")), target_os = "linux", target_os = "macos", target_os = "windows"))]
    fn with_sink(send: impl FnMut(&[u8]) -> Result<(), String> + Send + 'static) -> Result<Self, String> {
        let tx = Queue::new();
        let current = tx.clone();
        let (route, rx) = channel();
        let closed = Arc::new(AtomicBool::new(false));
        let failure = Arc::new(Mutex::new(None));
        let stop = Arc::clone(&closed);
        let failed = Arc::clone(&failure);
        let worker = std::thread::Builder::new()
            .name("MTC output".into())
            .spawn(move || {
                if let Err(e) = dispatch(current, rx, &stop, send) {
                    *failed.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(e);
                }
            })
            .map_err(|e| e.to_string())?;
        Ok(Self { route, tx, closed, failure, worker: Some(worker), attached: Weak::new() })
    }
}

#[cfg(any(all(test, not(target_arch = "wasm32")), target_os = "linux", target_os = "macos", target_os = "windows"))]
fn dispatch(
    mut queue: Arc<Queue>,
    rx: Receiver<Arc<Queue>>,
    closed: &AtomicBool,
    mut send: impl FnMut(&[u8]) -> Result<(), String>,
) -> Result<(), String> {
    let mut partial = false;
    let mut active = Instant::now();
    while !closed.load(Ordering::Acquire) {
        if let Some(next) = rx.try_iter().last() {
            queue = next;
            partial = false;
        }
        let Some((when, packet)) = queue.pop() else {
            let ms = if active.elapsed() < Duration::from_millis(100) { 1 } else { 10 };
            std::thread::park_timeout(Duration::from_millis(ms));
            continue;
        };
        active = Instant::now();
        while let Some(wait) = when.checked_duration_since(Instant::now()) {
            if closed.load(Ordering::Acquire) {
                return Ok(());
            }
            if queue.retired.load(Ordering::Acquire) {
                break;
            }
            std::thread::park_timeout(wait);
        }
        if closed.load(Ordering::Acquire) {
            break;
        }
        if queue.retired.load(Ordering::Acquire) {
            continue;
        }
        match packet {
            // After a stalled device or system sleep, bursting obsolete quarter frames would
            // falsely report fast playback. Resume on piece zero of a fresh two-frame group.
            Packet::Quarter(_) if when.elapsed() > Duration::from_millis(100) => {
                partial = true;
                continue;
            }
            Packet::Quarter([_, data]) if partial && data >> 4 != 0 => continue,
            _ => partial = false,
        }
        send(packet.bytes())?;
    }
    Ok(())
}

impl Drop for MtcOutput {
    fn drop(&mut self) {
        self.closed.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            worker.thread().unpark();
            let _ = worker.join();
        }
    }
}

impl Player {
    pub fn clear_mtc_output(&self) {
        self.install_mtc_queue(None);
    }
    fn install_mtc_queue(&self, queue: Option<Arc<Queue>>) {
        // Keep the allocation alive across detach. Reclaim replaced buffers on
        // this control thread after the callback has dropped its producer.
        let mut buffers = self.mtc_buffers.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        buffers.retain(|q| Arc::strong_count(q) > 1);
        buffers.extend(queue.iter().cloned());
        let _ = self.tx.send(Cmd::Mtc(queue.map(Generator::new)));
    }
}

/// Names as reported by the native MIDI output backend.
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
pub fn output_ports() -> Result<Vec<String>, String> {
    let midi = midir::MidiOutput::new("SoundCraft MTC").map_err(|e| e.to_string())?;
    Ok(midi.ports().iter().filter_map(|p| midi.port_name(p).ok()).collect())
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
pub fn output_ports() -> Result<Vec<String>, String> {
    Ok(Vec::new())
}

#[cfg(test)]
mod tests;
