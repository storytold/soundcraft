//! MIDI device input (midir: ALSA sequencer on Linux, CoreMIDI on macOS, WinMM on Windows).
//!
//! [`MidiInputs`] (UI thread) keeps a connection to every enabled input port and follows devices
//! being plugged in and removed ([`MidiInputs::rescan`]). Each connection's callback runs on a
//! thread of the MIDI system and puts channel messages, stamped with their arrival time, into
//! the [`MidiFeed`]. The audio callback takes them out without locking or allocating
//! ([`crate::Player::set_midi`]), plays them through the mix engine's MIDI thru and, while
//! recording, writes them back with their timeline position for the UI thread to collect
//! ([`MidiFeed::take_recorded`]).
//!
//! # When a queue is full
//!
//! Both queues have a fixed size, and a dropped event may be a note-off or a pedal release. So a
//! drop is never silent:
//!
//! - An event that does not reach the audio thread marks its port as having lost events
//!   ([`MidiFeed::port_lost`]; a port that closes does the same, as its releases will not come).
//!   The audio thread then releases every note and pedal that port holds on every track, and
//!   drops what it still had queued from it, so no note can be left sounding.
//! - While recording, that point goes into the take as a [`RecordedMidi::LOST`] entry: the notes
//!   the port held end there. Recorded events that do not fit the queue to the UI thread are
//!   counted ([`MidiFeed::take_recorded_lost`]) and followed by such an entry for all ports.
//!   The UI reports a take with either as incomplete.
//!
//! On other systems (FreeBSD, the web) there is no MIDI input: the port list stays empty.

use soundcraft_mix::{LiveMidi, MidiPorts, midi_port_bit};
use soundcraft_time::Samples;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
#[cfg(not(target_arch = "wasm32"))]
use std::time::Instant;

/// Events waiting for the audio thread. A callback takes them all, so this covers one callback.
/// More than that marks the port as having lost events (see the module text).
pub(crate) const INCOMING: usize = 1024;
/// Recorded events waiting for the UI thread, which collects every frame.
const RECORDED: usize = 16_384;

/// A fixed-size queue of stamped events for one reader and one writer at a time. Lock-free.
struct Ring {
    stamps: Vec<AtomicI64>,
    events: Vec<AtomicU32>,
    write: AtomicUsize,
    read: AtomicUsize,
}

impl Ring {
    fn new(capacity: usize) -> Ring {
        let n = capacity.next_power_of_two();
        Ring {
            stamps: (0..n).map(|_| AtomicI64::new(0)).collect(),
            events: (0..n).map(|_| AtomicU32::new(0)).collect(),
            write: AtomicUsize::new(0),
            read: AtomicUsize::new(0),
        }
    }

    /// False when full (the event is dropped).
    fn push(&self, stamp: i64, event: u32) -> bool {
        let n = self.events.len();
        let w = self.write.load(Ordering::Relaxed);
        if w.wrapping_sub(self.read.load(Ordering::Acquire)) >= n {
            return false;
        }
        let (Some(s), Some(e)) = (self.stamps.get(w % n), self.events.get(w % n)) else { return false };
        s.store(stamp, Ordering::Relaxed);
        e.store(event, Ordering::Relaxed);
        self.write.store(w.wrapping_add(1), Ordering::Release);
        true
    }

    fn pop(&self) -> Option<(i64, u32)> {
        let n = self.events.len();
        let r = self.read.load(Ordering::Relaxed);
        if r == self.write.load(Ordering::Acquire) {
            return None;
        }
        let out = (self.stamps.get(r % n)?.load(Ordering::Relaxed), self.events.get(r % n)?.load(Ordering::Relaxed));
        self.read.store(r.wrapping_add(1), Ordering::Release);
        Some(out)
    }
}

fn pack(e: &LiveMidi) -> u32 {
    u32::from_be_bytes([e.port, e.status, e.data1, e.data2])
}

fn unpack(v: u32) -> LiveMidi {
    let [port, status, data1, data2] = v.to_be_bytes();
    LiveMidi { offset: 0, port, status, data1, data2 }
}

/// A recorded MIDI message at its timeline position, or (status [`RecordedMidi::LOST`]) the point
/// where events of `port` were dropped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RecordedMidi {
    pub at: Samples,
    /// Index into the feed's [`MidiPorts`].
    pub port: u8,
    pub status: u8,
    pub data1: u8,
    pub data2: u8,
}

impl RecordedMidi {
    /// The `status` of an entry that stands for dropped events instead of a message: events of
    /// `port` (of every port when it is `u8::MAX`) were lost at `at`. Notes those ports held end
    /// there, and the take is incomplete.
    pub const LOST: u8 = 0xFF;

    /// Whether this entry marks dropped events.
    pub fn is_lost(&self) -> bool {
        self.status == Self::LOST
    }
}

/// The recorded entry for events dropped on `port`.
fn lost_entry(port: u8) -> u32 {
    pack(&LiveMidi { offset: 0, port, status: RecordedMidi::LOST, data1: 0, data2: 0 })
}

/// The path of MIDI events from the device threads to the audio thread, and of recorded events
/// from the audio thread to the UI.
pub struct MidiFeed {
    ports: Arc<MidiPorts>,
    incoming: Ring,
    /// Device threads write one at a time.
    writers: Mutex<()>,
    recorded: Ring,
    /// When the feed was made. Not on the web: there is no MIDI input there, and no clock in std.
    #[cfg(not(target_arch = "wasm32"))]
    epoch: Instant,
    /// Open input ports.
    connected: AtomicUsize,
    /// Events received so far (low 32 bits: the last one), for the "is my keyboard seen" readout.
    activity: AtomicU64,
    /// Ports ([`midi_port_bit`]) whose events did not all reach the audio thread, until it asks.
    lost: AtomicU64,
    /// Events dropped on the way to the audio thread so far.
    dropped: AtomicU64,
    /// Recorded events the UI thread never got, until it asks.
    recorded_lost: AtomicU32,
    /// The last recorded event was dropped: the next one is preceded by a [`RecordedMidi::LOST`].
    record_gap: AtomicBool,
}

impl Default for MidiFeed {
    fn default() -> Self {
        MidiFeed {
            ports: Arc::new(MidiPorts::default()),
            incoming: Ring::new(INCOMING),
            writers: Mutex::new(()),
            recorded: Ring::new(RECORDED),
            #[cfg(not(target_arch = "wasm32"))]
            epoch: Instant::now(),
            connected: AtomicUsize::new(0),
            activity: AtomicU64::new(0),
            lost: AtomicU64::new(0),
            dropped: AtomicU64::new(0),
            recorded_lost: AtomicU32::new(0),
            record_gap: AtomicBool::new(false),
        }
    }
}

impl MidiFeed {
    /// Names of the ports events come from, by [`LiveMidi::port`].
    pub fn ports(&self) -> &Arc<MidiPorts> {
        &self.ports
    }

    /// How many input ports are open.
    pub fn connected(&self) -> usize {
        self.connected.load(Ordering::Relaxed)
    }

    pub(crate) fn set_connected(&self, ports: usize) {
        self.connected.store(ports, Ordering::Relaxed);
    }

    /// Microseconds since the feed was made.
    fn now(&self) -> i64 {
        #[cfg(not(target_arch = "wasm32"))]
        return i64::try_from(self.epoch.elapsed().as_micros()).unwrap_or(i64::MAX);
        #[cfg(target_arch = "wasm32")]
        0
    }

    /// A message arrived from `port` (called on a MIDI system thread). Channel messages go on to
    /// the audio thread; system messages (clock, SysEx, active sensing) are dropped.
    pub fn receive(&self, port: u8, bytes: &[u8]) {
        let Some(&status) = bytes.first().filter(|s| (0x80..0xF0).contains(*s)) else { return };
        let data = |i: usize| bytes.get(i).copied().unwrap_or(0) & 0x7F;
        let e = pack(&LiveMidi { offset: 0, port, status, data1: data(1), data2: data(2) });
        {
            let _one = self.writers.lock().unwrap_or_else(PoisonError::into_inner);
            if !self.incoming.push(self.now(), e) {
                // The queue is full. This may be a release, so the port's notes have to go.
                self.dropped.fetch_add(1, Ordering::Relaxed);
                self.port_lost(port);
            }
        }
        let count = (self.activity.load(Ordering::Relaxed) >> 32).wrapping_add(1) & 0xFFFF_FFFF;
        self.activity.store(count << 32 | u64::from(e), Ordering::Relaxed);
    }

    /// Events of `port` did not (or will not) reach the audio thread: it releases what the port
    /// holds. Called when the queue is full and when a port closes.
    pub fn port_lost(&self, port: u8) {
        self.lost.fetch_or(midi_port_bit(port), Ordering::Release);
    }

    /// Audio thread: the ports that lost events since the last call ([`midi_port_bit`]). Ask
    /// before taking events with [`Self::next`]: what preceded a loss is then already taken, or
    /// is taken right after, and can be dropped with it.
    pub(crate) fn take_lost(&self) -> u64 {
        self.lost.swap(0, Ordering::AcqRel)
    }

    /// Events dropped on the way to the audio thread so far (a full queue).
    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }

    /// How many events arrived so far, and the last one.
    pub fn activity(&self) -> (u32, Option<LiveMidi>) {
        let a = self.activity.load(Ordering::Relaxed);
        let count = u32::try_from(a >> 32).unwrap_or(u32::MAX);
        (count, (count > 0).then(|| unpack(u32::try_from(a & 0xFFFF_FFFF).unwrap_or(0))))
    }

    /// Audio thread: the next event and how long ago it arrived (microseconds).
    pub(crate) fn next(&self, now: i64) -> Option<(i64, LiveMidi)> {
        self.incoming.pop().map(|(stamp, e)| (now.saturating_sub(stamp).max(0), unpack(e)))
    }

    pub(crate) fn clock(&self) -> i64 {
        self.now()
    }

    /// Audio thread: an event played while recording, at timeline position `at`.
    pub(crate) fn record(&self, at: Samples, e: &LiveMidi) {
        self.record_entry(at, pack(e));
    }

    /// Audio thread: events of the ports in `lost` ([`midi_port_bit`]) were dropped at `at`.
    pub(crate) fn record_lost(&self, at: Samples, lost: u64) {
        // The top bit stands for the ports without an index: that is any of them.
        if lost >> 63 != 0 {
            self.record_entry(at, lost_entry(u8::MAX));
            return;
        }
        for port in (0..63u8).filter(|p| lost & midi_port_bit(*p) != 0) {
            self.record_entry(at, lost_entry(port));
        }
    }

    /// Put a recorded entry in the queue to the UI thread. One that does not fit is counted, and
    /// the next that fits is preceded by a [`RecordedMidi::LOST`] for all ports.
    fn record_entry(&self, at: Samples, entry: u32) {
        if self.record_gap.load(Ordering::Relaxed) {
            if !self.recorded.push(at, lost_entry(u8::MAX)) {
                self.recorded_lost.fetch_add(1, Ordering::Relaxed);
                return;
            }
            self.record_gap.store(false, Ordering::Relaxed);
        }
        if !self.recorded.push(at, entry) {
            self.recorded_lost.fetch_add(1, Ordering::Relaxed);
            self.record_gap.store(true, Ordering::Relaxed);
        }
    }

    /// UI thread: how many recorded events were dropped since the last call (the UI did not
    /// collect them in time). The take is incomplete when this is not zero.
    pub fn take_recorded_lost(&self) -> u32 {
        self.recorded_lost.swap(0, Ordering::Relaxed)
    }

    /// UI thread: move the recorded events so far to the end of `out`.
    pub fn take_recorded(&self, out: &mut Vec<RecordedMidi>) {
        while let Some((at, e)) = self.recorded.pop() {
            let e = unpack(e);
            out.push(RecordedMidi { at, port: e.port, status: e.status, data1: e.data1, data2: e.data2 });
        }
    }
}

/// An input port as the Setup dialog shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MidiPort {
    pub name: String,
    /// Not switched off by the user.
    pub enabled: bool,
    /// Connected and delivering events.
    pub open: bool,
}

/// The MIDI input ports of this machine and the connections to the enabled ones.
pub struct MidiInputs {
    feed: Arc<MidiFeed>,
    /// Ports the user switched off, by name.
    disabled: Vec<String>,
    ports: Vec<MidiPort>,
    error: Option<String>,
    backend: backend::Backend,
}

impl MidiInputs {
    /// Connects to every input port except the `disabled` ones.
    pub fn open(disabled: Vec<String>) -> MidiInputs {
        let mut m =
            MidiInputs { feed: Arc::new(MidiFeed::default()), disabled, ports: Vec::new(), error: None, backend: backend::Backend::default() };
        m.rescan();
        m
    }

    /// What the audio engine reads ([`crate::Player::set_midi`]).
    pub fn feed(&self) -> &Arc<MidiFeed> {
        &self.feed
    }

    /// The ports found by the last scan.
    pub fn ports(&self) -> &[MidiPort] {
        &self.ports
    }

    /// Why there is no MIDI input, when the system's MIDI could not be opened.
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    pub fn disabled(&self) -> &[String] {
        &self.disabled
    }

    /// Switch a port on or off (it stays off across rescans and, through the prefs, launches).
    pub fn set_enabled(&mut self, name: &str, on: bool) {
        self.disabled.retain(|d| d != name);
        if !on {
            self.disabled.push(name.to_string());
        }
        self.rescan();
    }

    /// Look for ports again: open the new ones, close the ones that went away or were switched
    /// off. True when the list changed.
    pub fn rescan(&mut self) -> bool {
        let before = std::mem::take(&mut self.ports);
        match self.backend.sync(&self.disabled, &self.feed) {
            Ok(ports) => {
                self.ports = ports;
                self.error = None;
            }
            Err(e) => {
                if self.error.as_deref() != Some(e.as_str()) {
                    log::warn!("MIDI input: {e}");
                }
                self.error = Some(e);
            }
        }
        // A port that is no longer open sends no releases: let go of what it holds.
        for gone in before.iter().filter(|b| b.open && !self.ports.iter().any(|p| p.open && p.name == b.name)) {
            self.feed.port_lost(self.feed.ports().assign(&gone.name));
        }
        self.feed.set_connected(self.ports.iter().filter(|p| p.open).count());
        before != self.ports
    }

    /// Close every connection and open them again (after a device was replugged in place).
    pub fn reconnect(&mut self) {
        // Events may pass unseen between closing and opening.
        for p in self.ports.iter().filter(|p| p.open) {
            self.feed.port_lost(self.feed.ports().assign(&p.name));
        }
        self.backend = backend::Backend::default();
        self.rescan();
    }
}

#[cfg(any(test, target_os = "linux", target_os = "macos", target_os = "windows"))]
/// A port name without the parts that change between sessions or say things twice. ALSA names
/// read `Client:Port name 28:0`: the address goes, and so does a client name the port repeats.
fn tidy_port_name(raw: &str) -> String {
    let mut s = raw.trim();
    if let Some((head, tail)) = s.rsplit_once(' ')
        && let Some((client, port)) = tail.split_once(':')
        && !client.is_empty()
        && !port.is_empty()
        && client.bytes().chain(port.bytes()).all(|b| b.is_ascii_digit())
    {
        s = head.trim_end();
    }
    if let Some((client, port)) = s.split_once(':')
        && !client.trim().is_empty()
        && port.trim_start().starts_with(client.trim())
    {
        s = port.trim();
    }
    if s.is_empty() { "MIDI input".to_string() } else { s.to_string() }
}

/// The number of an ALSA loopback port ("Midi Through Port-0").
#[cfg(any(test, target_os = "linux", target_os = "macos", target_os = "windows"))]
fn through_number(name: &str) -> Option<&str> {
    name.strip_prefix("Midi Through Port-").filter(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
}

/// Tidy names, made unique: a second "Keys" becomes "Keys (2)". The system's loopback port is
/// plain "Midi Through" while it is the only one, "Midi Through 0", "Midi Through 1" otherwise.
#[cfg(any(test, target_os = "linux", target_os = "macos", target_os = "windows"))]
fn unique_names(raw: impl IntoIterator<Item = String>) -> Vec<String> {
    let tidy: Vec<String> = raw.into_iter().map(|r| tidy_port_name(&r)).collect();
    let through = tidy.iter().filter(|n| through_number(n).is_some()).count();
    let mut out: Vec<String> = Vec::new();
    for base in tidy {
        let base = match through_number(&base) {
            Some(_) if through == 1 => "Midi Through".to_string(),
            Some(n) => format!("Midi Through {n}"),
            None => base,
        };
        let mut name = base.clone();
        let mut n = 2;
        while out.contains(&name) {
            name = format!("{base} ({n})");
            n += 1;
        }
        out.push(name);
    }
    out
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
mod backend {
    use super::{MidiFeed, MidiPort, unique_names};
    use midir::{Ignore, MidiInput, MidiInputConnection, MidiInputPort};
    use std::panic::{AssertUnwindSafe, catch_unwind};
    use std::sync::Arc;

    const CLIENT: &str = "SoundCraft";

    #[derive(Default)]
    pub(super) struct Backend {
        /// Lists the ports; kept, so a rescan does not make a new client each time.
        scanner: Option<MidiInput>,
        open: Vec<(String, MidiInputConnection<()>)>,
    }

    impl Backend {
        /// Open the enabled ports that are not open yet, close the rest, and return the list.
        pub(super) fn sync(&mut self, disabled: &[String], feed: &Arc<MidiFeed>) -> Result<Vec<MidiPort>, String> {
            // midir panics on a few failures of the MIDI system (it unwraps them); those become
            // errors here.
            catch_unwind(AssertUnwindSafe(|| self.sync_inner(disabled, feed))).unwrap_or_else(|_| {
                self.scanner = None;
                Err("the system's MIDI service failed".to_string())
            })
        }

        fn sync_inner(&mut self, disabled: &[String], feed: &Arc<MidiFeed>) -> Result<Vec<MidiPort>, String> {
            let scanner = match self.scanner.take() {
                Some(s) => s,
                None => MidiInput::new(CLIENT).map_err(|e| format!("MIDI is not available: {e}"))?,
            };
            let found: Vec<MidiInputPort> = scanner.ports();
            let names = unique_names(found.iter().map(|p| scanner.port_name(p).unwrap_or_default()));
            self.scanner = Some(scanner);
            self.open.retain(|(n, _)| names.contains(n) && !disabled.contains(n));
            let mut out = Vec::with_capacity(names.len());
            for (name, port) in names.into_iter().zip(found.iter()) {
                let enabled = !disabled.contains(&name);
                let mut open = self.open.iter().any(|(n, _)| *n == name);
                if enabled && !open {
                    match connect(&name, port, feed) {
                        Ok(c) => {
                            log::info!("MIDI input: {name}");
                            self.open.push((name.clone(), c));
                            open = true;
                        }
                        Err(e) => log::warn!("MIDI input {name}: {e}"),
                    }
                }
                out.push(MidiPort { name, enabled, open });
            }
            Ok(out)
        }
    }

    fn connect(name: &str, port: &MidiInputPort, feed: &Arc<MidiFeed>) -> Result<MidiInputConnection<()>, String> {
        let mut input = MidiInput::new(CLIENT).map_err(|e| e.to_string())?;
        // Clock, SysEx and active sensing are not used; do not wake up for them.
        input.ignore(Ignore::All);
        let index = feed.ports().assign(name);
        let feed = Arc::clone(feed);
        input.connect(port, "in", move |_, bytes, ()| feed.receive(index, bytes), ()).map_err(|e| e.to_string())
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
mod backend {
    use super::{MidiFeed, MidiPort};
    use std::sync::Arc;

    #[derive(Default)]
    pub(super) struct Backend;

    impl Backend {
        pub(super) fn sync(&mut self, _disabled: &[String], _feed: &Arc<MidiFeed>) -> Result<Vec<MidiPort>, String> {
            Err("MIDI devices are not supported on this system yet".to_string())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn port_names_lose_addresses_and_repeated_client_names() {
        for (raw, want) in [
            ("Launchkey MK3:Launchkey MK3 MIDI 1 28:0", "Launchkey MK3 MIDI 1"),
            ("Midi Through:Midi Through Port-0 14:0", "Midi Through Port-0"),
            ("VMPK Output:out 130:0", "VMPK Output:out"),
            ("IAC Driver Bus 1", "IAC Driver Bus 1"),
            ("Piano 2", "Piano 2"),
            ("  ", "MIDI input"),
        ] {
            assert_eq!(tidy_port_name(raw), want);
        }
        assert_eq!(unique_names(["Keys 20:0".to_string(), "Keys 24:0".into(), "Pad".into(), "Keys".into()]), ["Keys", "Keys (2)", "Pad", "Keys (3)"]);
        assert_eq!(unique_names(["Midi Through:Midi Through Port-0 14:0".to_string(), "Pad".into()]), ["Midi Through", "Pad"]);
        assert_eq!(
            unique_names(["Midi Through:Midi Through Port-0 14:0".to_string(), "Midi Through:Midi Through Port-1 14:1".into()]),
            ["Midi Through 0", "Midi Through 1"]
        );
    }

    #[test]
    fn the_feed_passes_channel_messages_in_order_and_drops_the_rest() {
        let f = MidiFeed::default();
        assert_eq!(f.activity(), (0, None));
        f.receive(3, &[0x90, 60, 100]);
        f.receive(3, &[0xF8]);
        f.receive(3, &[]);
        f.receive(1, &[0xC5, 7]);
        f.receive(3, &[0x80, 60, 0xFF]);
        let now = f.clock();
        let got: Vec<LiveMidi> = std::iter::from_fn(|| f.next(now))
            .map(|(age, e)| {
                assert!(age >= 0);
                e
            })
            .collect();
        let ev = |port, status, data1, data2| LiveMidi { offset: 0, port, status, data1, data2 };
        assert_eq!(got, [ev(3, 0x90, 60, 100), ev(1, 0xC5, 7, 0), ev(3, 0x80, 60, 0x7F)]);
        assert_eq!(f.activity(), (3, Some(ev(3, 0x80, 60, 0x7F))));
    }

    #[test]
    fn a_full_ring_drops_new_events_and_recovers() {
        let r = Ring::new(4);
        assert!((0..4).all(|i| r.push(i, 1)));
        assert!(!r.push(9, 1));
        assert_eq!(r.pop(), Some((0, 1)));
        assert!(r.push(4, 2));
        assert_eq!(std::iter::from_fn(|| r.pop()).map(|(s, _)| s).collect::<Vec<_>>(), [1, 2, 3, 4]);
    }

    #[test]
    fn a_full_input_queue_marks_the_port_and_counts_the_drop() {
        let f = MidiFeed::default();
        // A key and the pedal go down, then the queue fills and the releases do not fit.
        f.receive(2, &[0x90, 60, 100]);
        f.receive(2, &[0xB0, 64, 127]);
        for _ in 0..INCOMING - 2 {
            f.receive(5, &[0xB0, 1, 0]);
        }
        assert_eq!((f.dropped(), f.take_lost()), (0, 0));
        f.receive(2, &[0x80, 60, 0]);
        f.receive(2, &[0xB0, 64, 0]);
        f.receive(u8::MAX, &[0x80, 1, 0]);
        f.receive(2, &[0xF8]);
        assert_eq!(f.dropped(), 3, "system messages are not counted");
        assert_eq!(f.take_lost(), midi_port_bit(2) | midi_port_bit(u8::MAX));
        assert_eq!(f.take_lost(), 0, "told once");
        let now = f.clock();
        assert_eq!(std::iter::from_fn(|| f.next(now)).count(), INCOMING);
        f.receive(2, &[0x90, 61, 100]);
        assert_eq!((f.dropped(), f.take_lost()), (3, 0), "the queue works again");
    }

    #[test]
    fn a_full_recorded_queue_is_counted_and_leaves_a_lost_entry() {
        let f = MidiFeed::default();
        let e = LiveMidi { offset: 0, port: 1, status: 0x90, data1: 60, data2: 100 };
        let capacity = RECORDED.next_power_of_two();
        for at in 0..capacity + 3 {
            f.record(at as i64, &e);
        }
        let mut out = Vec::new();
        f.take_recorded(&mut out);
        assert_eq!((out.len(), f.take_recorded_lost(), f.take_recorded_lost()), (capacity, 3, 0));
        // The next event that fits comes after an entry that ends every held note.
        f.record(99, &LiveMidi { status: 0x80, ..e });
        out.clear();
        f.take_recorded(&mut out);
        assert_eq!(out.len(), 2);
        assert!(out[0].is_lost() && out[0].port == u8::MAX && out[0].at == 99);
        assert_eq!((out[1].status, out[1].is_lost()), (0x80, false));
        // Ports that lost input get an entry each; a port without an index means all of them.
        f.record_lost(7, midi_port_bit(0) | midi_port_bit(4));
        f.record_lost(8, midi_port_bit(3) | midi_port_bit(u8::MAX));
        out.clear();
        f.take_recorded(&mut out);
        assert_eq!(out.iter().map(|e| (e.at, e.port, e.is_lost())).collect::<Vec<_>>(), [(7, 0, true), (7, 4, true), (8, u8::MAX, true)]);
    }

    #[test]
    fn recorded_events_come_back_with_their_position() {
        let f = MidiFeed::default();
        f.record(48_000, &LiveMidi { offset: 9, port: 2, status: 0x91, data1: 64, data2: 90 });
        let mut out = Vec::new();
        f.take_recorded(&mut out);
        f.take_recorded(&mut out);
        assert_eq!(out, [RecordedMidi { at: 48_000, port: 2, status: 0x91, data1: 64, data2: 90 }]);
    }
}
