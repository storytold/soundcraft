//! Hardware MIDI input devices and lock-free event queue.
//!
//! A [`MidiManager`] discovers physical MIDI input ports via `midir` (CoreMIDI, ALSA, WinMM)
//! and streams parsed MIDI events into a preallocated, lock-free, allocation-free [`MidiInputRing`].
//! The audio callback drains this ring without blocking or allocating.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, AtomicU32, AtomicU64, AtomicUsize, Ordering};

#[cfg(not(target_arch = "wasm32"))]
use midir::{Ignore, MidiInput, MidiInputConnection};

/// Default capacity for the lock-free MIDI input ring (power of two).
pub const DEFAULT_RING_CAPACITY: usize = 2048;

use soundcraft_midi::{CtrlEvent, CtrlKind, Note, Sequence};
pub use soundcraft_midi::{LiveMidiEvent, MidiTake, RecordedMidiEvent, parse_midi_message};
use soundcraft_model::{Session, TrackId};

/// Cache-line padding wrapper (64 bytes) to eliminate false sharing between CPU cores.
#[repr(align(64))]
pub struct CachePadded<T>(pub T);

impl<T> std::ops::Deref for CachePadded<T> {
    type Target = T;
    #[inline]
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<T> std::ops::DerefMut for CachePadded<T> {
    #[inline]
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

static MONOTONIC_ANCHOR: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();

/// Monotonic microsecond timestamp using safe zero-allocation standard library clock.
#[inline]
pub fn monotonic_now_us() -> u64 {
    let start = MONOTONIC_ANCHOR.get_or_init(std::time::Instant::now);
    start.elapsed().as_micros() as u64
}

#[repr(align(64))]
struct Slot {
    sequence: AtomicUsize,
    word: AtomicU32,
    port: AtomicU32,
    time: AtomicU64,
}

/// Lock-free, bounded multi-producer single-consumer ring buffer for [`LiveMidiEvent`].
/// Never blocks, never allocates during push/pop, and uses zero `unsafe`.
pub struct MidiInputRing {
    slots: Box<[Slot]>,
    mask: usize,
    head: CachePadded<AtomicUsize>,
    tail: CachePadded<AtomicUsize>,
    dropped: CachePadded<AtomicUsize>,
}

impl MidiInputRing {
    pub fn new(capacity: usize) -> Arc<Self> {
        let n = capacity.max(2).next_power_of_two();
        let slots = (0..n)
            .map(|i| Slot { sequence: AtomicUsize::new(i), word: AtomicU32::new(0), port: AtomicU32::new(0), time: AtomicU64::new(0) })
            .collect();
        Arc::new(Self {
            slots,
            mask: n - 1,
            head: CachePadded(AtomicUsize::new(0)),
            tail: CachePadded(AtomicUsize::new(0)),
            dropped: CachePadded(AtomicUsize::new(0)),
        })
    }

    /// Push an event into the ring.
    /// Thread-safe for multiple concurrent producers (e.g. multiple MIDI ports and UI thread).
    /// Returns `true` on success, or `false` if the buffer is full (event dropped).
    pub fn push(&self, event: LiveMidiEvent) -> bool {
        let word = event.pack_word();
        let port = event.port_id;
        let time = event.timestamp_us;
        let mut head = self.head.load(Ordering::Relaxed);
        loop {
            let idx = head & self.mask;
            let Some(slot) = self.slots.get(idx) else {
                return false;
            };
            let seq = slot.sequence.load(Ordering::Acquire);
            let diff = (seq as isize).wrapping_sub(head as isize);
            if diff == 0 {
                match self.head.compare_exchange_weak(head, head.wrapping_add(1), Ordering::Relaxed, Ordering::Relaxed) {
                    Ok(_) => {
                        slot.word.store(word, Ordering::Relaxed);
                        slot.port.store(port, Ordering::Relaxed);
                        slot.time.store(time, Ordering::Relaxed);
                        slot.sequence.store(head.wrapping_add(1), Ordering::Release);
                        return true;
                    }
                    Err(actual) => head = actual,
                }
            } else if diff < 0 {
                self.dropped.fetch_add(1, Ordering::Relaxed);
                return false;
            } else {
                head = self.head.load(Ordering::Relaxed);
            }
        }
    }

    /// Pop a single event from the ring.
    /// Expected to be called from the single consumer (audio thread).
    pub fn pop(&self) -> Option<LiveMidiEvent> {
        let tail = self.tail.load(Ordering::Relaxed);
        let idx = tail & self.mask;
        let slot = self.slots.get(idx)?;
        let seq = slot.sequence.load(Ordering::Acquire);
        let diff = (seq as isize).wrapping_sub(tail.wrapping_add(1) as isize);
        if diff == 0 {
            let word = slot.word.load(Ordering::Relaxed);
            let port = slot.port.load(Ordering::Relaxed);
            let time = slot.time.load(Ordering::Relaxed);
            slot.sequence.store(tail.wrapping_add(self.mask).wrapping_add(1), Ordering::Release);
            self.tail.store(tail.wrapping_add(1), Ordering::Release);
            Some(LiveMidiEvent::unpack_word(port, time, word))
        } else {
            None
        }
    }

    /// Drain all pending events calling `f` for each event in order.
    pub fn drain(&self, mut f: impl FnMut(LiveMidiEvent)) {
        while let Some(ev) = self.pop() {
            f(ev);
        }
    }

    /// Total events dropped due to a full buffer.
    pub fn dropped(&self) -> usize {
        self.dropped.load(Ordering::Relaxed)
    }

    /// Current approximate number of queued events.
    pub fn len(&self) -> usize {
        let head = self.head.load(Ordering::Relaxed);
        let tail = self.tail.load(Ordering::Relaxed);
        head.saturating_sub(tail)
    }

    /// Returns `true` if the ring has no events queued.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[repr(align(64))]
struct RecordSlot {
    sequence: AtomicUsize,
    track: AtomicU64,
    at_sample: AtomicI64,
    word: AtomicU32,
    port: AtomicU32,
    time: AtomicU64,
}

/// Lock-free, bounded multi-producer single-consumer ring buffer for [`RecordedMidiEvent`].
/// Never blocks, never allocates during push/pop, and uses zero `unsafe`.
pub struct MidiRecordRing {
    slots: Box<[RecordSlot]>,
    mask: usize,
    head: CachePadded<AtomicUsize>,
    tail: CachePadded<AtomicUsize>,
    dropped: CachePadded<AtomicUsize>,
}

impl MidiRecordRing {
    pub fn new(capacity: usize) -> Arc<Self> {
        let n = capacity.max(2).next_power_of_two();
        let slots = (0..n)
            .map(|i| RecordSlot {
                sequence: AtomicUsize::new(i),
                track: AtomicU64::new(0),
                at_sample: AtomicI64::new(0),
                word: AtomicU32::new(0),
                port: AtomicU32::new(0),
                time: AtomicU64::new(0),
            })
            .collect();
        Arc::new(Self {
            slots,
            mask: n - 1,
            head: CachePadded(AtomicUsize::new(0)),
            tail: CachePadded(AtomicUsize::new(0)),
            dropped: CachePadded(AtomicUsize::new(0)),
        })
    }

    /// Push a recorded event into the ring.
    /// Thread-safe for real-time audio threads.
    pub fn push(&self, event: RecordedMidiEvent) -> bool {
        let word = event.event.pack_word();
        let port = event.event.port_id;
        let time = event.event.timestamp_us;
        let track = event.track_id;
        let at_sample = event.at_sample;
        let mut head = self.head.load(Ordering::Relaxed);
        loop {
            let idx = head & self.mask;
            let Some(slot) = self.slots.get(idx) else {
                return false;
            };
            let seq = slot.sequence.load(Ordering::Acquire);
            let diff = (seq as isize).wrapping_sub(head as isize);
            if diff == 0 {
                match self.head.compare_exchange_weak(head, head.wrapping_add(1), Ordering::Relaxed, Ordering::Relaxed) {
                    Ok(_) => {
                        slot.track.store(track, Ordering::Relaxed);
                        slot.at_sample.store(at_sample, Ordering::Relaxed);
                        slot.word.store(word, Ordering::Relaxed);
                        slot.port.store(port, Ordering::Relaxed);
                        slot.time.store(time, Ordering::Relaxed);
                        slot.sequence.store(head.wrapping_add(1), Ordering::Release);
                        return true;
                    }
                    Err(actual) => head = actual,
                }
            } else if diff < 0 {
                self.dropped.fetch_add(1, Ordering::Relaxed);
                return false;
            } else {
                head = self.head.load(Ordering::Relaxed);
            }
        }
    }

    /// Pop a single recorded event from the ring.
    pub fn pop(&self) -> Option<RecordedMidiEvent> {
        let tail = self.tail.load(Ordering::Relaxed);
        let idx = tail & self.mask;
        let slot = self.slots.get(idx)?;
        let seq = slot.sequence.load(Ordering::Acquire);
        let diff = (seq as isize).wrapping_sub(tail.wrapping_add(1) as isize);
        if diff == 0 {
            let track_id = slot.track.load(Ordering::Relaxed);
            let at_sample = slot.at_sample.load(Ordering::Relaxed);
            let word = slot.word.load(Ordering::Relaxed);
            let port = slot.port.load(Ordering::Relaxed);
            let time = slot.time.load(Ordering::Relaxed);
            slot.sequence.store(tail.wrapping_add(self.mask).wrapping_add(1), Ordering::Release);
            self.tail.store(tail.wrapping_add(1), Ordering::Release);
            let event = LiveMidiEvent::unpack_word(port, time, word);
            Some(RecordedMidiEvent { track_id, at_sample, event })
        } else {
            None
        }
    }

    /// Drain all pending events calling `f` for each event in order.
    pub fn drain(&self, mut f: impl FnMut(RecordedMidiEvent)) {
        while let Some(ev) = self.pop() {
            f(ev);
        }
    }

    /// Total events dropped due to a full buffer.
    pub fn dropped(&self) -> usize {
        self.dropped.load(Ordering::Relaxed)
    }

    /// Current approximate number of queued events.
    pub fn len(&self) -> usize {
        let head = self.head.load(Ordering::Relaxed);
        let tail = self.tail.load(Ordering::Relaxed);
        head.saturating_sub(tail)
    }

    /// Returns `true` if the ring has no events queued.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Manages active MIDI recording for armed tracks, translating captured events
/// into timeline-aligned notes and controllers.
pub struct MidiRecorder {
    ring: Arc<MidiRecordRing>,
    armed_tracks: Vec<TrackId>,
    record_start: i64,
    active_notes: HashMap<(TrackId, u8, u8), (i64, u8)>,
    notes: HashMap<TrackId, Vec<(i64, i64, u8, u8, u8, u8)>>,
    ctrls: HashMap<TrackId, Vec<(i64, u8, CtrlKind, i32)>>,
}

impl MidiRecorder {
    pub fn new(ring: Arc<MidiRecordRing>, armed_tracks: Vec<TrackId>, record_start: i64) -> Self {
        Self { ring, armed_tracks, record_start, active_notes: HashMap::new(), notes: HashMap::new(), ctrls: HashMap::new() }
    }

    pub fn poll(&mut self, s: &Session) {
        while let Some(ev) = self.ring.pop() {
            self.process_event(s, ev);
        }
    }

    pub fn process_event(&mut self, s: &Session, ev: RecordedMidiEvent) {
        let track_id = TrackId(ev.track_id);
        if !self.armed_tracks.contains(&track_id) {
            return;
        }

        if (ev.event.is_note_on() || ev.event.is_note_off()) && s.edit.flag("midi.filter.notes") {
            return;
        }
        if ev.event.is_control_change() && s.edit.flag("midi.filter.cc") {
            return;
        }
        if ev.event.is_pitch_bend() && s.edit.flag("midi.filter.pitch_bend") {
            return;
        }
        if ev.event.is_channel_pressure() && s.edit.flag("midi.filter.mono_aftertouch") {
            return;
        }
        if ev.event.is_poly_pressure() && s.edit.flag("midi.filter.poly_aftertouch") {
            return;
        }
        if ev.event.is_program_change() && s.edit.flag("midi.filter.program") {
            return;
        }

        if ev.event.is_note_on() {
            let pitch = ev.event.note_pitch();
            let vel = ev.event.note_velocity();
            let ch = ev.event.channel();
            if let Some((start, old_vel)) = self.active_notes.remove(&(track_id, ch, pitch)) {
                let len = (ev.at_sample.saturating_sub(start)).max(1);
                self.notes.entry(track_id).or_default().push((start, len, pitch, old_vel, 64, ch));
            }
            self.active_notes.insert((track_id, ch, pitch), (ev.at_sample, vel));
        } else if ev.event.is_note_off() {
            let pitch = ev.event.note_pitch();
            let rel_vel = ev.event.note_velocity();
            let ch = ev.event.channel();
            if let Some((start, vel)) = self.active_notes.remove(&(track_id, ch, pitch)) {
                let len = (ev.at_sample.saturating_sub(start)).max(1);
                self.notes.entry(track_id).or_default().push((start, len, pitch, vel, rel_vel, ch));
            }
        } else if ev.event.is_control_change() {
            let ctrl = ev.event.cc_controller();
            let val = ev.event.cc_value() as i32;
            self.ctrls.entry(track_id).or_default().push((ev.at_sample, ev.event.channel(), CtrlKind::Cc(ctrl), val));
        } else if ev.event.is_pitch_bend() {
            let val = ev.event.pitch_bend_value();
            self.ctrls.entry(track_id).or_default().push((ev.at_sample, ev.event.channel(), CtrlKind::PitchBend, val));
        } else if ev.event.is_channel_pressure() {
            let val = (ev.event.data[1] & 0x7F) as i32;
            self.ctrls.entry(track_id).or_default().push((ev.at_sample, ev.event.channel(), CtrlKind::ChannelPressure, val));
        } else if ev.event.is_poly_pressure() {
            let pitch = ev.event.data[1] & 0x7F;
            let val = (ev.event.data[2] & 0x7F) as i32;
            self.ctrls.entry(track_id).or_default().push((ev.at_sample, ev.event.channel(), CtrlKind::PolyPressure(pitch), val));
        } else if ev.event.is_program_change() {
            let val = (ev.event.data[1] & 0x7F) as i32;
            self.ctrls.entry(track_id).or_default().push((ev.at_sample, ev.event.channel(), CtrlKind::Program, val));
        }
    }

    /// Query the current live notes (both completed and sounding) for visualization.
    pub fn live_notes(&self, track: TrackId, current_pos: i64) -> Vec<(i64, i64, u8)> {
        let mut out = Vec::new();
        if let Some(list) = self.notes.get(&track) {
            for &(start, len, pitch, _, _, _) in list {
                out.push((start, len, pitch));
            }
        }
        for (&(t, _, pitch), &(start, _)) in &self.active_notes {
            if t == track {
                let len = (current_pos.saturating_sub(start)).max(1);
                out.push((start, len, pitch));
            }
        }
        out
    }

    /// Finish recording at `stop_sample` and assemble completed [`MidiTake`]s for all armed tracks.
    pub fn finish(&mut self, s: &Session, stop_sample: i64) -> Vec<MidiTake> {
        self.poll(s);
        for ((track_id, ch, pitch), (start, vel)) in self.active_notes.drain() {
            let len = (stop_sample.saturating_sub(start)).max(1);
            self.notes.entry(track_id).or_default().push((start, len, pitch, vel, 64, ch));
        }

        let clip_start = self.record_start;
        let clip_len = (stop_sample.saturating_sub(clip_start)).max(1);
        let sr = s.sample_rate;
        let mut takes = Vec::new();

        for track_id in &self.armed_tracks {
            let mut notes = Vec::new();
            if let Some(track_notes) = self.notes.get(track_id) {
                for &(start, len, pitch, vel, rel_vel, ch) in track_notes {
                    let note_start_ticks = s.tempo.samples_to_ticks(start, sr).saturating_sub(s.tempo.samples_to_ticks(clip_start, sr));
                    let note_end_ticks =
                        s.tempo.samples_to_ticks(start.saturating_add(len), sr).saturating_sub(s.tempo.samples_to_ticks(clip_start, sr));
                    let note_len_ticks = (note_end_ticks.saturating_sub(note_start_ticks)).max(1);
                    notes.push(Note {
                        pitch,
                        velocity: vel,
                        release_velocity: rel_vel,
                        channel: ch,
                        start: note_start_ticks.max(0),
                        length: note_len_ticks,
                    });
                }
            }

            let mut ctrls = Vec::new();
            if let Some(track_ctrls) = self.ctrls.get(track_id) {
                for &(at, ch, kind, val) in track_ctrls {
                    let tick = s.tempo.samples_to_ticks(at, sr).saturating_sub(s.tempo.samples_to_ticks(clip_start, sr));
                    ctrls.push(CtrlEvent { tick: tick.max(0), channel: ch, kind, value: val });
                }
            }

            let mut sequence = Sequence { notes, ctrls };
            sequence.sort();

            takes.push(MidiTake { track_id: track_id.0, start: clip_start, length: clip_len, sequence });
        }

        takes
    }
}

/// Information about a detected physical or system MIDI input device.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct MidiDeviceInfo {
    pub id: u32,
    pub name: String,
    pub connected: bool,
}

/// Query the OS for available MIDI input devices.
pub fn list_input_devices() -> Vec<MidiDeviceInfo> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let Ok(midi_in) = MidiInput::new("SoundCraft") else {
            return Vec::new();
        };
        let mut devices = Vec::new();
        for (i, port) in midi_in.ports().iter().enumerate() {
            let name = midi_in.port_name(port).unwrap_or_else(|_| format!("MIDI Port {i}"));
            devices.push(MidiDeviceInfo { id: i as u32, name, connected: false });
        }
        devices
    }
    #[cfg(target_arch = "wasm32")]
    {
        Vec::new()
    }
}

/// Manages physical MIDI hardware connections and streams input into a [`MidiInputRing`].
pub struct MidiManager {
    ring: Arc<MidiInputRing>,
    #[cfg(not(target_arch = "wasm32"))]
    connections: HashMap<String, MidiInputConnection<()>>,
}

impl MidiManager {
    pub fn new(ring: Arc<MidiInputRing>) -> Self {
        Self {
            ring,
            #[cfg(not(target_arch = "wasm32"))]
            connections: HashMap::new(),
        }
    }

    pub fn ring(&self) -> &Arc<MidiInputRing> {
        &self.ring
    }

    /// Enumerate available MIDI input devices and mark whether they are currently connected.
    pub fn list_devices(&self) -> Vec<MidiDeviceInfo> {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let mut devices = list_input_devices();
            for dev in &mut devices {
                dev.connected = self.connections.contains_key(&dev.name);
            }
            devices
        }
        #[cfg(target_arch = "wasm32")]
        {
            list_input_devices()
        }
    }

    /// Names of all currently connected devices.
    pub fn connected_devices(&self) -> Vec<String> {
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.connections.keys().cloned().collect()
        }
        #[cfg(target_arch = "wasm32")]
        {
            Vec::new()
        }
    }

    /// Connect to a specific input device by name.
    pub fn connect(&mut self, device_name: &str) -> Result<(), String> {
        #[cfg(not(target_arch = "wasm32"))]
        {
            if self.connections.contains_key(device_name) {
                return Ok(());
            }
            let mut midi_in = MidiInput::new("SoundCraft Device").map_err(|e| format!("Failed to initialize MIDI input: {e}"))?;
            midi_in.ignore(Ignore::ActiveSense);
            let ports = midi_in.ports();
            let mut matched = None;
            for (idx, port) in ports.iter().enumerate() {
                if let Ok(name) = midi_in.port_name(port)
                    && name == device_name
                {
                    matched = Some((idx as u32, idx));
                    break;
                }
            }
            let (port_id, port_idx) = matched.ok_or_else(|| format!("MIDI device “{device_name}” not found"))?;
            let port = ports.get(port_idx).ok_or_else(|| format!("Port index {port_idx} out of range"))?;
            let ring = Arc::clone(&self.ring);
            let conn = midi_in
                .connect(
                    port,
                    device_name,
                    move |_timestamp_us, bytes, _| {
                        let ts = monotonic_now_us();
                        if let Some(event) = parse_midi_message(port_id, ts, bytes) {
                            ring.push(event);
                        }
                    },
                    (),
                )
                .map_err(|e| format!("Failed to connect to “{device_name}”: {e}"))?;
            self.connections.insert(device_name.to_string(), conn);
            Ok(())
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = device_name;
            Err("MIDI hardware I/O is not supported on WebAssembly".into())
        }
    }

    /// Disconnect from a device.
    pub fn disconnect(&mut self, device_name: &str) {
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.connections.remove(device_name);
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = device_name;
        }
    }

    /// Sync active connections with a desired list of enabled device names.
    pub fn sync_enabled(&mut self, enabled: &[String]) {
        #[cfg(not(target_arch = "wasm32"))]
        {
            // Remove disconnected devices
            self.connections.retain(|name, _| enabled.contains(name));
            // Connect newly enabled devices
            for name in enabled {
                if !self.connections.contains_key(name) {
                    let _ = self.connect(name);
                }
            }
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = enabled;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ring_push_pop_fifo() {
        let ring = MidiInputRing::new(4);
        assert!(ring.is_empty());
        assert_eq!(ring.len(), 0);

        let e1 = LiveMidiEvent { port_id: 1, timestamp_us: 100, len: 3, data: [0x90, 60, 100] };
        let e2 = LiveMidiEvent { port_id: 1, timestamp_us: 200, len: 3, data: [0x80, 60, 64] };

        assert!(ring.push(e1));
        assert!(ring.push(e2));
        assert_eq!(ring.len(), 2);

        assert_eq!(ring.pop(), Some(e1));
        assert_eq!(ring.pop(), Some(e2));
        assert_eq!(ring.pop(), None);
        assert!(ring.is_empty());
    }

    #[test]
    fn ring_full_drops_events() {
        let ring = MidiInputRing::new(2); // next power of two is 2
        let ev = LiveMidiEvent { port_id: 0, timestamp_us: 1, len: 3, data: [0x90, 60, 100] };

        assert!(ring.push(ev));
        assert!(ring.push(ev));
        assert!(!ring.push(ev), "ring must reject when full");
        assert_eq!(ring.dropped(), 1);

        assert!(ring.pop().is_some());
        assert!(ring.push(ev), "push succeeds after popping");
    }

    #[test]
    fn ring_drain() {
        let ring = MidiInputRing::new(8);
        for i in 0..5 {
            ring.push(LiveMidiEvent { port_id: i, timestamp_us: i as u64 * 10, len: 3, data: [0x90, i as u8, 100] });
        }

        let mut drained = Vec::new();
        ring.drain(|e| drained.push(e.port_id));
        assert_eq!(drained, vec![0, 1, 2, 3, 4]);
        assert!(ring.is_empty());
    }

    #[test]
    fn parse_midi_messages() {
        // Note On
        let ev = parse_midi_message(0, 50, &[0x90, 60, 100]).expect("Note On parsed");
        assert!(ev.is_note_on());
        assert!(!ev.is_note_off());
        assert_eq!(ev.note_pitch(), 60);
        assert_eq!(ev.note_velocity(), 100);
        assert_eq!(ev.channel(), 0);

        // Note On with velocity 0 is Note Off
        let ev0 = parse_midi_message(0, 60, &[0x90, 60, 0]).expect("Note On vel 0 parsed");
        assert!(!ev0.is_note_on());
        assert!(ev0.is_note_off());
        assert_eq!(ev0.note_pitch(), 60);

        // Standard Note Off (0x80)
        let ev_off = parse_midi_message(0, 70, &[0x80, 64, 64]).expect("Note Off parsed");
        assert!(ev_off.is_note_off());
        assert_eq!(ev_off.note_pitch(), 64);

        // Control Change (0xB0)
        let ev_cc = parse_midi_message(1, 80, &[0xB0, 7, 127]).expect("CC parsed");
        assert_eq!(ev_cc.len, 3);
        assert_eq!(ev_cc.data, [0xB0, 7, 127]);

        // Program Change (0xC0, 2 bytes)
        let ev_pc = parse_midi_message(1, 90, &[0xC0, 10]).expect("Program Change parsed");
        assert_eq!(ev_pc.len, 2);
        assert_eq!(ev_pc.data[0], 0xC0);
        assert_eq!(ev_pc.data[1], 10);

        // Active Sensing (0xFE) ignored
        assert_eq!(parse_midi_message(0, 100, &[0xFE]), None);

        // Empty bytes
        assert_eq!(parse_midi_message(0, 100, &[]), None);
    }

    #[test]
    fn enumeration_does_not_panic() {
        let devices = list_input_devices();
        // Even if no MIDI devices are connected, it should return a Vec without error
        let _ = devices.len();
    }

    #[test]
    fn midi_manager_initialization() {
        let ring = MidiInputRing::new(16);
        let mut mgr = MidiManager::new(ring);
        assert_eq!(mgr.connected_devices(), Vec::<String>::new());
        mgr.sync_enabled(&[]);
        assert_eq!(mgr.connected_devices(), Vec::<String>::new());
    }

    #[test]
    fn record_ring_push_pop_fifo() {
        let ring = MidiRecordRing::new(4);
        let ev1 =
            RecordedMidiEvent { track_id: 1, at_sample: 100, event: LiveMidiEvent { port_id: 0, timestamp_us: 10, len: 3, data: [0x90, 60, 100] } };
        let ev2 =
            RecordedMidiEvent { track_id: 2, at_sample: 200, event: LiveMidiEvent { port_id: 0, timestamp_us: 20, len: 3, data: [0x80, 60, 0] } };
        assert!(ring.push(ev1));
        assert!(ring.push(ev2));
        assert_eq!(ring.len(), 2);
        assert_eq!(ring.pop(), Some(ev1));
        assert_eq!(ring.pop(), Some(ev2));
        assert_eq!(ring.pop(), None);
    }

    #[test]
    fn midi_recorder_captures_notes_and_ctrls() {
        let ring = MidiRecordRing::new(16);
        let track = TrackId(10);
        let mut rec = MidiRecorder::new(Arc::clone(&ring), vec![track], 0);
        let s = Session::default();

        // Note 1: start at 1000, end at 3400
        assert!(ring.push(RecordedMidiEvent {
            track_id: 10,
            at_sample: 1000,
            event: LiveMidiEvent { port_id: 0, timestamp_us: 10, len: 3, data: [0x90, 60, 100] },
        }));
        assert!(ring.push(RecordedMidiEvent {
            track_id: 10,
            at_sample: 2000,
            event: LiveMidiEvent { port_id: 0, timestamp_us: 20, len: 3, data: [0xB0, 7, 120] },
        }));
        assert!(ring.push(RecordedMidiEvent {
            track_id: 10,
            at_sample: 3400,
            event: LiveMidiEvent { port_id: 0, timestamp_us: 34, len: 3, data: [0x80, 60, 64] },
        }));
        // Note 2: started at 4000, never released before stop at 8000
        assert!(ring.push(RecordedMidiEvent {
            track_id: 10,
            at_sample: 4000,
            event: LiveMidiEvent { port_id: 0, timestamp_us: 40, len: 3, data: [0x90, 64, 90] },
        }));

        let takes = rec.finish(&s, 8000);
        assert_eq!(takes.len(), 1);
        let take = &takes[0];
        assert_eq!(take.track_id, 10);
        assert_eq!(take.start, 0);
        assert_eq!(take.length, 8000);
        assert_eq!(take.sequence.notes.len(), 2);
        assert_eq!(take.sequence.ctrls.len(), 1);

        // First note: pitch 60, vel 100, rel 64
        assert_eq!(take.sequence.notes[0].pitch, 60);
        assert_eq!(take.sequence.notes[0].velocity, 100);
        assert_eq!(take.sequence.notes[0].release_velocity, 64);
        assert!(take.sequence.notes[0].start > 0);
        assert!(take.sequence.notes[0].length > 0);

        // Second note: pitch 64, vel 90, closed at 8000
        assert_eq!(take.sequence.notes[1].pitch, 64);
        assert_eq!(take.sequence.notes[1].velocity, 90);
        assert!(take.sequence.notes[1].start > take.sequence.notes[0].start);

        // CC: CC 7, value 120
        assert_eq!(take.sequence.ctrls[0].kind, CtrlKind::Cc(7));
        assert_eq!(take.sequence.ctrls[0].value, 120);
    }

    #[test]
    fn midi_recorder_respects_filter_flags() {
        let ring = MidiRecordRing::new(16);
        let track = TrackId(10);
        let mut rec = MidiRecorder::new(Arc::clone(&ring), vec![track], 0);
        let mut s = Session::default();
        s.edit.set_flag("midi.filter.notes", true);

        // Push Note On and CC
        assert!(ring.push(RecordedMidiEvent {
            track_id: 10,
            at_sample: 1000,
            event: LiveMidiEvent { port_id: 0, timestamp_us: 10, len: 3, data: [0x90, 60, 100] },
        }));
        assert!(ring.push(RecordedMidiEvent {
            track_id: 10,
            at_sample: 2000,
            event: LiveMidiEvent { port_id: 0, timestamp_us: 20, len: 3, data: [0xB0, 7, 120] },
        }));

        let takes = rec.finish(&s, 5000);
        assert_eq!(takes.len(), 1);
        // Notes were filtered, only CC remains
        assert_eq!(takes[0].sequence.notes.len(), 0);
        assert_eq!(takes[0].sequence.ctrls.len(), 1);
    }

    #[test]
    fn cache_line_alignment_prevents_false_sharing() {
        assert!(std::mem::align_of::<Slot>() >= 64);
        assert!(std::mem::align_of::<RecordSlot>() >= 64);
        assert_eq!(std::mem::size_of::<Slot>() % 64, 0);
        assert_eq!(std::mem::size_of::<RecordSlot>() % 64, 0);
        assert!(std::mem::align_of::<CachePadded<std::sync::atomic::AtomicUsize>>() >= 64);
    }

    #[test]
    fn staccato_intra_block_duration_preserved() {
        let ring = MidiRecordRing::new(16);
        let track = TrackId(10);
        let mut rec = MidiRecorder::new(Arc::clone(&ring), vec![track], 0);
        let s = Session::default();

        // Staccato note played in same 512-sample block: Note-on at sample 100, Note-off at sample 250 (150 samples duration)
        assert!(ring.push(RecordedMidiEvent {
            track_id: 10,
            at_sample: 100,
            event: LiveMidiEvent { port_id: 0, timestamp_us: 10, len: 3, data: [0x90, 60, 100] },
        }));
        assert!(ring.push(RecordedMidiEvent {
            track_id: 10,
            at_sample: 250,
            event: LiveMidiEvent { port_id: 0, timestamp_us: 25, len: 3, data: [0x80, 60, 64] },
        }));

        let takes = rec.finish(&s, 512);
        assert_eq!(takes.len(), 1);
        let note = &takes[0].sequence.notes[0];
        // At 120 bpm 48kHz, 150 samples is ~3 ticks (much greater than 1 tick)
        assert!(note.length > 1, "staccato note duration must be preserved rather than collapsed to 1 tick: got {}", note.length);
    }
}
