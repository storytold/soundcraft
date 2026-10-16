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
//! On other systems (FreeBSD, the web) there is no MIDI input: the port list stays empty.

use soundcraft_mix::{LiveMidi, MidiPorts};
use soundcraft_time::Samples;
use std::sync::atomic::{AtomicI64, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
#[cfg(not(target_arch = "wasm32"))]
use std::time::Instant;

/// Events waiting for the audio thread. A callback takes them all, so this covers one callback.
const INCOMING: usize = 1024;
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

/// A recorded MIDI message at its timeline position.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RecordedMidi {
    pub at: Samples,
    /// Index into the feed's [`MidiPorts`].
    pub port: u8,
    pub status: u8,
    pub data1: u8,
    pub data2: u8,
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
            self.incoming.push(self.now(), e);
        }
        let count = (self.activity.load(Ordering::Relaxed) >> 32).wrapping_add(1) & 0xFFFF_FFFF;
        self.activity.store(count << 32 | u64::from(e), Ordering::Relaxed);
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
        self.recorded.push(at, pack(e));
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
        self.feed.set_connected(self.ports.iter().filter(|p| p.open).count());
        before != self.ports
    }

    /// Close every connection and open them again (after a device was replugged in place).
    pub fn reconnect(&mut self) {
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
    fn recorded_events_come_back_with_their_position() {
        let f = MidiFeed::default();
        f.record(48_000, &LiveMidi { offset: 9, port: 2, status: 0x91, data1: 64, data2: 90 });
        let mut out = Vec::new();
        f.take_recorded(&mut out);
        f.take_recorded(&mut out);
        assert_eq!(out, [RecordedMidi { at: 48_000, port: 2, status: 0x91, data1: 64, data2: 90 }]);
    }
}
