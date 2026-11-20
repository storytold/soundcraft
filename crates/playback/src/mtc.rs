//! MIDI Time Code output, clocked by the samples delivered to the playback device.
//! The audio callback queues dated packets. Only the output worker waits or calls the MIDI driver.
use crate::{Cmd, Player, Shared};
use craft_midi_sync::sync::{Rate, full_frame, quarter_frames};
use soundcraft_model::Session;
use soundcraft_time::{FrameRate, Timecode};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::SyncSender;
#[cfg(any(test, target_os = "linux", target_os = "macos", target_os = "windows"))]
use std::sync::mpsc::{Receiver, sync_channel};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

pub fn supported(rate: FrameRate) -> bool {
    matches!(rate, FrameRate::Fps24 | FrameRate::Fps25 | FrameRate::Fps29_97Drop | FrameRate::Fps30)
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

fn quarters(frame: i64, rate: FrameRate) -> [[u8; 2]; 8] {
    let (seconds, wire) = label(frame, rate);
    let mut messages = quarter_frames(seconds, wire);
    if rate.is_drop() {
        messages[7][1] = (messages[7][1] & !6) | 4;
    }
    messages
}

type Event = (Instant, Packet);

pub(crate) struct Generator {
    tx: SyncSender<Event>,
    next: Option<i64>,
    group: [[u8; 2]; 8],
}

impl Generator {
    fn new(tx: SyncSender<Event>) -> Self {
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
                if self.tx.try_send((at, locate(frame, rate))).is_err() {
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
            if self.tx.try_send((at + Duration::from_secs_f64(seconds), Packet::Quarter(self.group[piece]))).is_err() {
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
    tx: SyncSender<Event>,
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
            player.set_mtc_output(Some(self));
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
        let (tx, rx) = sync_channel(128);
        let closed = Arc::new(AtomicBool::new(false));
        let failure = Arc::new(Mutex::new(None));
        let stop = Arc::clone(&closed);
        let failed = Arc::clone(&failure);
        let worker = std::thread::Builder::new()
            .name("MTC output".into())
            .spawn(move || {
                if let Err(e) = dispatch(rx, &stop, send) {
                    *failed.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(e);
                }
            })
            .map_err(|e| e.to_string())?;
        Ok(Self { tx, closed, failure, worker: Some(worker), attached: Weak::new() })
    }
}

#[cfg(any(all(test, not(target_arch = "wasm32")), target_os = "linux", target_os = "macos", target_os = "windows"))]
fn dispatch(rx: Receiver<Event>, closed: &AtomicBool, mut send: impl FnMut(&[u8]) -> Result<(), String>) -> Result<(), String> {
    let mut partial = false;
    while !closed.load(Ordering::Acquire) {
        let Ok((when, packet)) = rx.recv_timeout(Duration::from_millis(20)) else { continue };
        while let Some(wait) = when.checked_duration_since(Instant::now()) {
            if closed.load(Ordering::Acquire) {
                return Ok(());
            }
            std::thread::park_timeout(wait);
        }
        if closed.load(Ordering::Acquire) {
            break;
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
    pub fn set_mtc_output(&self, output: Option<&MtcOutput>) {
        let _ = self.tx.send(Cmd::Mtc(output.map(|p| Generator::new(p.tx.clone()))));
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
