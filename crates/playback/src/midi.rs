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
//!   drops what it had from that port from before the loss, so no note can be left sounding.
//!   The time of the loss is kept with the mark, for each port ([`MidiFeed::lost_at`]): what the port sends
//!   after it (a replugged device, a queue that has room again) is played and recorded as usual,
//!   after the release. A loss from a full queue is also written into the queue itself, ahead of
//!   the next event that fits: the audio thread meets it at its exact place, also when it had
//!   already looked at the marks for this callback.
//! - A connection that is closed is shut off first ([`MidiFeed::close_port`]): what its device
//!   thread still delivers after that is discarded, whether or not the system waits for that
//!   thread when a connection is dropped.
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

    /// The `status` of an entry that says loop recording went back to the start of the loop
    /// here: what follows was played in the next round. Without it, a key pressed in one round
    /// and released later in the next would look like one short note.
    pub const WRAP: u8 = 0xFE;

    /// Whether this entry marks dropped events.
    pub fn is_lost(&self) -> bool {
        self.status == Self::LOST
    }

    /// Whether this entry marks the loop going round.
    pub fn is_wrap(&self) -> bool {
        self.status == Self::WRAP
    }
}

/// The place of the bit of `port` in the lost-ports mask ([`midi_port_bit`]): ports without an
/// index of their own share the last one.
fn lost_bit(port: u8) -> usize {
    usize::from(port).min(63)
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
    /// Device threads write one at a time. Holds the ports ([`midi_port_bit`]) whose loss from
    /// a full queue is not written into the queue yet.
    writers: Mutex<u64>,
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
    /// When each port last lost input ([`Self::now`]), by the place of its bit in `lost`;
    /// `i64::MIN` before the first time.
    lost_at: Vec<AtomicI64>,
    /// Events dropped on the way to the audio thread so far.
    dropped: AtomicU64,
    /// Recorded events the UI thread never got, until it asks.
    recorded_lost: AtomicU32,
    /// The last recorded event was dropped: the next one is preceded by a [`RecordedMidi::LOST`].
    record_gap: AtomicBool,
    /// The last record pass ([`Self::end_pass`]) whose MIDI the audio thread has finished writing
    /// down.
    captures_done: AtomicU64,
    /// Record passes ended so far: the number of the last one.
    passes: AtomicU64,
    /// When the last record pass was ended ([`Self::end_pass`]), on the clock of the stamps.
    pass_end: AtomicI64,
    /// When the last record pass was begun ([`Self::begin_pass`]); `i64::MIN` before the first.
    pass_begin: AtomicI64,
    /// A pass was begun whose start command the audio thread has not read yet.
    begin_pending: AtomicBool,
    /// When each port last had an event dropped by the full queue, by the place of its bit in
    /// `lost`; `i64::MIN` before the first time. Kept apart from `lost_at`, which a later close
    /// of the port moves on.
    overflow_at: Vec<AtomicI64>,
    /// Where the audio thread's playhead stood when it finished `stop_pass`.
    stop_at: AtomicI64,
    stop_pass: AtomicU64,
    /// A clock set by a test in place of the real one (`i64::MIN`: the real one).
    #[cfg(test)]
    test_clock: AtomicI64,
}

impl Default for MidiFeed {
    fn default() -> Self {
        MidiFeed {
            ports: Arc::new(MidiPorts::default()),
            incoming: Ring::new(INCOMING),
            writers: Mutex::new(0),
            recorded: Ring::new(RECORDED),
            #[cfg(not(target_arch = "wasm32"))]
            epoch: Instant::now(),
            connected: AtomicUsize::new(0),
            activity: AtomicU64::new(0),
            lost: AtomicU64::new(0),
            lost_at: (0..64).map(|_| AtomicI64::new(i64::MIN)).collect(),
            dropped: AtomicU64::new(0),
            recorded_lost: AtomicU32::new(0),
            record_gap: AtomicBool::new(false),
            captures_done: AtomicU64::new(0),
            passes: AtomicU64::new(0),
            pass_end: AtomicI64::new(i64::MAX),
            pass_begin: AtomicI64::new(i64::MIN),
            begin_pending: AtomicBool::new(false),
            overflow_at: (0..64).map(|_| AtomicI64::new(i64::MIN)).collect(),
            stop_at: AtomicI64::new(0),
            stop_pass: AtomicU64::new(0),
            #[cfg(test)]
            test_clock: AtomicI64::new(i64::MIN),
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
        #[cfg(test)]
        {
            let set = self.test_clock.load(Ordering::Relaxed);
            if set != i64::MIN {
                return set;
            }
        }
        #[cfg(not(target_arch = "wasm32"))]
        return i64::try_from(self.epoch.elapsed().as_micros()).unwrap_or(i64::MAX);
        #[cfg(target_arch = "wasm32")]
        0
    }

    /// Stop the clock at `micros`, for tests: stamps and ages no longer depend on how fast the
    /// test runs.
    #[cfg(test)]
    pub(crate) fn set_clock(&self, micros: i64) {
        self.test_clock.store(micros, Ordering::Relaxed);
    }

    /// A message arrived from `port` (called on a MIDI system thread). Channel messages go on to
    /// the audio thread; system messages (clock, SysEx, active sensing) are dropped.
    ///
    /// The bytes come from a device, real or virtual, and are not trusted: a message that does
    /// not start with a channel status byte or is shorter than its kind needs is dropped, data
    /// bytes are masked to 7 bits, and bytes past the message are ignored. Nothing downstream
    /// (the engine, instruments, the recorded take) sees anything else.
    pub fn receive(&self, port: u8, bytes: &[u8]) {
        self.receive_from(None, port, bytes);
    }

    /// [`Self::receive`] for a connection that can be shut off: once `open` is false
    /// ([`Self::close_port`]) the message is discarded.
    pub fn receive_while_open(&self, open: &AtomicBool, port: u8, bytes: &[u8]) {
        self.receive_from(Some(open), port, bytes);
    }

    fn receive_from(&self, open: Option<&AtomicBool>, port: u8, bytes: &[u8]) {
        let Some(&status) = bytes.first().filter(|s| (0x80..0xF0).contains(*s)) else { return };
        // Program change and channel pressure have one data byte, the rest two.
        let needs = if matches!(status & 0xF0, 0xC0 | 0xD0) { 2 } else { 3 };
        if bytes.len() < needs {
            return;
        }
        let data = |i: usize| bytes.get(i).copied().unwrap_or(0) & 0x7F;
        let e = pack(&LiveMidi { offset: 0, port, status, data1: data(1), data2: data(2) });
        {
            let mut owed = self.writers.lock().unwrap_or_else(PoisonError::into_inner);
            // Shut off under the same lock that `close_port` holds: a message is either in the
            // queue before the port's loss is marked, or it is discarded here.
            if open.is_some_and(|o| !o.load(Ordering::Acquire)) {
                return;
            }
            let now = self.now();
            // Losses from a full queue go into the queue first, where they happened: ahead of
            // the first event that fits again. Each mark carries the time its port's events
            // were dropped at (nothing of that port got into the queue since), so the audio
            // thread can tell it from another loss of the same port, such as a later close.
            let announce = *owed;
            for bit in (0..64u8).filter(|b| announce & (1u64 << b) != 0) {
                let port = if bit == 63 { u8::MAX } else { bit };
                if !self.incoming.push(self.overflow_at(port).min(now), lost_entry(port)) {
                    break;
                }
                *owed &= !(1u64 << bit);
            }
            if *owed != 0 || !self.incoming.push(now, e) {
                // The queue is full. This may be a release, so the port's notes have to go.
                self.dropped.fetch_add(1, Ordering::Relaxed);
                // One time for the drop and for the loss it is: the mark in the queue and the
                // port's flag are then known to stand for the same loss.
                if let Some(at) = self.overflow_at.get(lost_bit(port)) {
                    at.fetch_max(now, Ordering::AcqRel);
                }
                self.port_lost_at(port, now);
                *owed |= midi_port_bit(port);
            }
        }
        let count = (self.activity.load(Ordering::Relaxed) >> 32).wrapping_add(1) & 0xFFFF_FFFF;
        self.activity.store(count << 32 | u64::from(e), Ordering::Relaxed);
    }

    /// The connection of `port` that delivers through `open` ([`Self::receive_while_open`]) is
    /// being closed: shut it off and mark the port as having lost its input, in one step. What
    /// the connection delivered before is in the queue with an earlier time than the loss; what
    /// its device thread delivers from now on is discarded. Dropping the connection afterwards
    /// may or may not wait for that thread (the system decides): it no longer matters.
    pub fn close_port(&self, open: &AtomicBool, port: u8) {
        let _writers = self.writers.lock().unwrap_or_else(PoisonError::into_inner);
        open.store(false, Ordering::Release);
        self.port_lost(port);
    }

    /// Events of `port` did not (or will not) reach the audio thread: it releases what the port
    /// holds. Called when the queue is full and when a port closes.
    pub fn port_lost(&self, port: u8) {
        self.port_lost_at(port, self.now());
    }

    /// [`Self::port_lost`] at the time `now`.
    fn port_lost_at(&self, port: u8, now: i64) {
        // The time first: whoever sees the port's bit also sees a time that covers this loss.
        if let Some(at) = self.lost_at.get(lost_bit(port)) {
            at.fetch_max(now, Ordering::AcqRel);
        }
        self.lost.fetch_or(midi_port_bit(port), Ordering::Release);
    }

    /// Audio thread: the ports that lost events since the last call ([`midi_port_bit`]). Ask
    /// before taking events with [`Self::next`], then read [`Self::lost_at`]: the events of
    /// those ports stamped up to that time preceded the loss and are dropped with it, later ones
    /// came after it.
    pub(crate) fn take_lost(&self) -> u64 {
        self.lost.swap(0, Ordering::AcqRel)
    }

    /// Mark `lost` again, as [`Self::take_lost`] returned it: for tests that play out a loss
    /// the audio thread learns of one callback late.
    #[cfg(test)]
    pub(crate) fn put_back_lost(&self, lost: u64) {
        self.lost.fetch_or(lost, Ordering::Release);
    }

    /// Audio thread: when `port` last lost input, on the clock of the events' stamps. Each port
    /// has its own time: a loss on one port says nothing about what another sent before it. An
    /// event of a lost port with a later stamp arrived after the loss. One with the same stamp
    /// counts as before it: dropping a fresh event is safe, playing a note-on from before the
    /// release is not.
    pub(crate) fn lost_at(&self, port: u8) -> i64 {
        self.lost_at.get(lost_bit(port)).map_or(i64::MIN, |at| at.load(Ordering::Acquire))
    }

    /// Audio thread: when the full queue last dropped an event of `port` (`i64::MIN`: never).
    /// The mark of that loss in the queue is stamped with this time.
    pub(crate) fn overflow_at(&self, port: u8) -> i64 {
        self.overflow_at.get(lost_bit(port)).map_or(i64::MIN, |at| at.load(Ordering::Acquire))
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

    /// Audio thread: the next event and when it arrived ([`Self::clock`], microseconds). An entry
    /// with the status [`RecordedMidi::LOST`] is not an event: events of its port (of the ports
    /// without an index, for `u8::MAX`) were dropped at this place in the queue, at the time it
    /// is stamped with ([`Self::lost_at`] as it was when the mark was written).
    pub(crate) fn next(&self) -> Option<(i64, LiveMidi)> {
        self.incoming.pop().map(|(stamp, e)| (stamp, unpack(e)))
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

    /// Audio thread: loop recording went back to the start of the loop, which ends at `at`.
    pub(crate) fn record_wrap(&self, at: Samples) {
        self.record_entry(at, pack(&LiveMidi { offset: 0, port: u8::MAX, status: RecordedMidi::WRAP, data1: 0, data2: 0 }));
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

    /// UI thread: a record pass begins now, just before the audio thread is told to record.
    /// While the transport is already running, what arrives from here on belongs to the pass,
    /// also when the audio thread takes it in before it reads that command.
    pub fn begin_pass(&self) {
        // The time first: whoever sees the pass as begun also sees when.
        self.pass_begin.store(self.now(), Ordering::Release);
        self.begin_pending.store(true, Ordering::Release);
    }

    /// Audio thread: since when arriving events belong to the pass that was last begun. It
    /// holds for the whole pass: what was played before Record stays out of the take also when
    /// the audio thread takes it in after it has read the start command.
    pub(crate) fn record_from(&self) -> i64 {
        self.pass_begin.load(Ordering::Acquire)
    }

    /// Audio thread: whether a pass was begun whose start command has not been read yet.
    pub(crate) fn begin_pending(&self) -> bool {
        self.begin_pending.load(Ordering::Acquire)
    }

    /// Audio thread: the command that starts the pass was read.
    pub(crate) fn pass_begun(&self) {
        self.begin_pending.store(false, Ordering::Release);
    }

    /// UI thread: a record pass ends. Returns its number, to stop the recording with
    /// ([`crate::Player::end_recording`]) and to wait for ([`Self::captured`]). Numbers only go
    /// up, so the late answer to an earlier pass is never taken for this one.
    pub fn end_pass(&self) -> u64 {
        // The cutoff is in force from this one store on ([`Self::record_until`]).
        self.pass_end.store(self.now(), Ordering::Release);
        self.passes.fetch_add(1, Ordering::AcqRel).saturating_add(1)
    }

    /// Audio thread: up to when arriving events belong to the take. While an ended pass is not
    /// confirmed yet that is the time it was ended at: what a device sends after the Stop is
    /// played but not recorded, however long the audio thread takes to hear of the Stop.
    pub(crate) fn record_until(&self) -> i64 {
        self.pass_end.load(Ordering::Acquire)
    }

    /// Audio thread: keep the device threads from putting anything into the queue until the
    /// guard is dropped, so that the queue as it is read next holds everything that was stamped
    /// so far (a device thread stamps a message and puts it in under this lock). Does not wait:
    /// when a device thread is in there right now it tries a few times, and gives up (`None`)
    /// rather than hold the audio thread up.
    pub(crate) fn hold_writers(&self) -> Option<std::sync::MutexGuard<'_, u64>> {
        for _ in 0..256 {
            match self.writers.try_lock() {
                Ok(guard) => return Some(guard),
                Err(std::sync::TryLockError::Poisoned(p)) => return Some(p.into_inner()),
                Err(std::sync::TryLockError::WouldBlock) => std::hint::spin_loop(),
            }
        }
        None
    }

    /// Audio thread: recording was switched off for `pass`, and everything played before that is
    /// recorded. `at` is where the playhead stood then: the positions in the take, and the marks
    /// of the loop going round, are all from before it.
    pub(crate) fn capture_done(&self, pass: u64, at: Samples) {
        self.stop_at.store(at, Ordering::Release);
        self.stop_pass.store(pass, Ordering::Release);
        // The last pass that was ended: its cutoff goes, before the UI can see the answer and
        // begin another pass.
        if pass >= self.passes.load(Ordering::Acquire) {
            self.pass_end.store(i64::MAX, Ordering::Release);
        }
        self.captures_done.fetch_max(pass, Ordering::AcqRel);
    }

    /// Where the pass stopped, once the audio thread has confirmed it ([`Self::captured`]): the
    /// playhead as the take saw it. The player's position may be further on by then (in the
    /// next round of a loop, which the take knows nothing about). `None` while the pass is not
    /// confirmed, or when it was let go without an answer ([`Self::forgive_passes`]).
    pub fn stopped_at(&self, pass: u64) -> Option<Samples> {
        (self.captured(pass) && self.stop_pass.load(Ordering::Acquire) == pass).then(|| self.stop_at.load(Ordering::Acquire))
    }

    /// Whether the audio thread has seen the end of `pass` ([`Self::end_pass`]) and has written
    /// down all that was played up to it: collect the take ([`Self::take_recorded`]) once this
    /// is true.
    pub fn captured(&self, pass: u64) -> bool {
        self.captures_done.load(Ordering::Acquire) >= pass
    }

    /// Whether a record pass that was ended ([`Self::end_pass`]) is not confirmed yet. The audio
    /// thread takes the MIDI that arrived since its last callback in one go, with the state of
    /// the pass it knows about: a pass started and stopped before it runs again would have its
    /// notes written at the old pass's position. So no pass that records MIDI starts while this
    /// is true.
    pub fn pass_owed(&self) -> bool {
        self.captures_done.load(Ordering::Acquire) < self.passes.load(Ordering::Acquire)
    }

    /// The audio engine that owed the passes ended so far is gone (another one takes the feed):
    /// nothing is owed any more.
    pub fn forgive_passes(&self) {
        self.pass_end.store(i64::MAX, Ordering::Release);
        self.captures_done.fetch_max(self.passes.load(Ordering::Acquire), Ordering::AcqRel);
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
    /// A given list of port names stands in for the system's (see [`MidiInputs::with_port_names`]).
    stand_in: Option<Vec<String>>,
}

impl MidiInputs {
    /// Connects to every input port except the `disabled` ones.
    pub fn open(disabled: Vec<String>) -> MidiInputs {
        let mut m = MidiInputs {
            feed: Arc::new(MidiFeed::default()),
            disabled,
            ports: Vec::new(),
            error: None,
            backend: backend::Backend::default(),
            stand_in: None,
        };
        m.rescan();
        m
    }

    /// Inputs that list `names` as their ports instead of asking the system, for tests and for
    /// running without MIDI hardware. No device is opened; an enabled port counts as open, and
    /// messages come in through the feed ([`MidiFeed::receive`]). Switches, rescans and the
    /// release of a port that goes away work as with real devices.
    pub fn with_port_names(disabled: Vec<String>, names: Vec<String>) -> MidiInputs {
        let mut m = MidiInputs {
            feed: Arc::new(MidiFeed::default()),
            disabled,
            ports: Vec::new(),
            error: None,
            backend: backend::Backend::default(),
            stand_in: Some(names),
        };
        m.rescan();
        m
    }

    /// The devices of [`MidiInputs::with_port_names`] changed (one was plugged in or removed);
    /// the next [`MidiInputs::rescan`] sees it. Nothing happens on inputs that use the system's.
    pub fn set_port_names(&mut self, names: Vec<String>) {
        if let Some(n) = &mut self.stand_in {
            *n = names;
        }
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
        let found = match &self.stand_in {
            Some(names) => Ok(names
                .iter()
                .map(|name| {
                    let enabled = !self.disabled.contains(name);
                    // Give the port its index, as connecting to a device does.
                    self.feed.ports().assign(name);
                    MidiPort { name: name.clone(), enabled, open: enabled }
                })
                .collect()),
            None => self.backend.sync(&self.disabled, &self.feed),
        };
        match found {
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
        for gone in closed_ports(&before, &self.ports) {
            self.feed.port_lost(self.feed.ports().assign(gone));
        }
        self.feed.set_connected(self.ports.iter().filter(|p| p.open).count());
        before != self.ports
    }

    /// Close every connection and open them again (after a device was replugged in place).
    pub fn reconnect(&mut self) {
        // Close first: once the old connections are gone, nothing more arrives through them.
        // Only then say that their ports lost input, so that no note-on can slip in after the
        // release it stands for; then open them again.
        self.backend.close_all(&self.feed);
        self.backend = backend::Backend::default();
        for p in self.ports.iter().filter(|p| p.open) {
            self.feed.port_lost(self.feed.ports().assign(&p.name));
        }
        self.rescan();
    }
}

/// The names of the ports that were open in `before` and are not open in `after` (unplugged,
/// switched off, or failed).
fn closed_ports<'a>(before: &'a [MidiPort], after: &'a [MidiPort]) -> impl Iterator<Item = &'a str> {
    before.iter().filter(|b| b.open && !after.iter().any(|p| p.open && p.name == b.name)).map(|b| b.name.as_str())
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

/// Close the connections in `open` (`(identity, name, connection)`) whose port is not in `listed`
/// (`(identity, system name)`) any more or was switched off, and tell `feed` that each of those
/// ports lost its input ([`MidiFeed::port_lost`]): the keys held through it have no release
/// coming. That is by connection, not by name: a port that went away and came back with another
/// identity keeps its name, and the list of open port names never shows it was gone.
///
/// Each connection is shut off and its port marked in one step ([`MidiFeed::close_port`]), then
/// dropped. Nothing its device thread delivers after that step gets into the queue, so no
/// note-on can follow the release, whether or not dropping the connection waits for that thread
/// (ALSA joins it; CoreMIDI and WinMM do not promise to).
#[cfg(any(test, target_os = "linux", target_os = "macos", target_os = "windows"))]
fn retire<C>(open: &mut Vec<(String, String, Gate, C)>, listed: &[(String, String)], disabled: &[String], feed: &MidiFeed) {
    let (keep, gone): (Vec<_>, Vec<_>) =
        std::mem::take(open).into_iter().partition(|(id, name, ..)| listed.iter().any(|(l, _)| l == id) && !disabled.contains(name));
    *open = keep;
    for (_, name, gate, connection) in gone {
        feed.close_port(&gate, feed.ports().assign(&name));
        drop(connection);
    }
}

/// Whether a connection still delivers: its callback asks under the feed's lock
/// ([`MidiFeed::receive_while_open`]), and [`MidiFeed::close_port`] says no.
#[cfg(any(test, target_os = "linux", target_os = "macos", target_os = "windows"))]
type Gate = Arc<AtomicBool>;

/// The number of an ALSA loopback port ("Midi Through Port-0").
#[cfg(any(test, target_os = "linux", target_os = "macos", target_os = "windows"))]
fn through_number(name: &str) -> Option<&str> {
    name.strip_prefix("Midi Through Port-").filter(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
}

/// The names the ports go by, kept per port. `found` lists the ports that are there now, each
/// with what tells it apart from the others (`identity`: the system's id for the port) and the
/// name the system gives it. A port keeps the name it got for as long as it is there, whatever
/// happens to the others: with two keyboards called "Keys", unplugging the first leaves the
/// second as "Keys (2)", connected, and the tracks that listen to it still do.
///
/// A new port gets its tidy name, made unique among the names in use: a second "Keys" becomes
/// "Keys (2)". The system's loopback port is plain "Midi Through" while it is the only one,
/// "Midi Through 0", "Midi Through 1" otherwise. `known` holds `(identity, name)` and loses the
/// ports that went away.
#[cfg(any(test, target_os = "linux", target_os = "macos", target_os = "windows"))]
fn assign_names(known: &mut Vec<(String, String)>, found: &[(String, String)]) -> Vec<String> {
    known.retain(|(identity, _)| found.iter().any(|(f, _)| f == identity));
    let tidy: Vec<String> = found.iter().map(|(_, raw)| tidy_port_name(raw)).collect();
    let through = tidy.iter().filter(|n| through_number(n).is_some()).count();
    for ((identity, _), base) in found.iter().zip(tidy) {
        if known.iter().any(|(k, _)| k == identity) {
            continue;
        }
        let base = match through_number(&base) {
            Some(_) if through == 1 => "Midi Through".to_string(),
            Some(n) => format!("Midi Through {n}"),
            None => base,
        };
        let mut name = base.clone();
        let mut n = 2;
        while known.iter().any(|(_, used)| *used == name) {
            name = format!("{base} ({n})");
            n += 1;
        }
        known.push((identity.clone(), name));
    }
    found.iter().map(|(identity, _)| known.iter().find(|(k, _)| k == identity).map(|(_, name)| name.clone()).unwrap_or_default()).collect()
}

/// The names a first scan gives the ports with these system names, in order.
#[cfg(test)]
fn unique_names(raw: impl IntoIterator<Item = String>) -> Vec<String> {
    let found: Vec<(String, String)> = raw.into_iter().enumerate().map(|(i, r)| (i.to_string(), r)).collect();
    assign_names(&mut Vec::new(), &found)
}
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
mod backend {
    use super::{Gate, MidiFeed, MidiPort, assign_names};
    use midir::{Ignore, MidiInput, MidiInputConnection, MidiInputPort};
    use std::panic::{AssertUnwindSafe, catch_unwind};
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;

    const CLIENT: &str = "SoundCraft";

    #[derive(Default)]
    pub(super) struct Backend {
        /// Lists the ports; kept, so a rescan does not make a new client each time.
        scanner: Option<MidiInput>,
        /// The name each port that is there goes by: `(identity, name)`, see `assign_names`.
        names: Vec<(String, String)>,
        /// The open connections: `(identity, name, gate, connection)`.
        open: Vec<(String, String, Gate, MidiInputConnection<()>)>,
    }

    /// What tells a port apart from another with the same name: the system's id for it (an ALSA
    /// address, a CoreMIDI unique id, a Windows device id), with its name.
    fn identity(port: &MidiInputPort, raw: &str) -> String {
        format!("{}\u{1f}{raw}", port.id())
    }

    impl Backend {
        /// Shut every connection off and mark its port as having lost its input, before the
        /// connections are dropped.
        pub(super) fn close_all(&mut self, feed: &MidiFeed) {
            for (_, name, gate, _) in &self.open {
                feed.close_port(gate, feed.ports().assign(name));
            }
        }

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
            let listed: Vec<(String, String)> = found
                .iter()
                .map(|p| {
                    let raw = scanner.port_name(p).unwrap_or_default();
                    (identity(p, &raw), raw)
                })
                .collect();
            let names = assign_names(&mut self.names, &listed);
            self.scanner = Some(scanner);
            // A connection belongs to its port, not to a name: it stays while that port is there
            // and switched on, and no other port takes it over.
            // One that goes says so, before anything is opened: a device replugged between two
            // scans comes back under the same name, and its old connection's releases never
            // come. What the new connection sends is stamped after this and is kept.
            super::retire(&mut self.open, &listed, disabled, feed);
            let mut out = Vec::with_capacity(names.len());
            for ((name, port), (id, _)) in names.into_iter().zip(found.iter()).zip(listed.iter()) {
                let enabled = !disabled.contains(&name);
                let mut open = self.open.iter().any(|(i, ..)| i == id);
                if enabled && !open {
                    match connect(&name, port, feed) {
                        Ok((gate, c)) => {
                            log::info!("MIDI input: {name}");
                            self.open.push((id.clone(), name.clone(), gate, c));
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

    fn connect(name: &str, port: &MidiInputPort, feed: &Arc<MidiFeed>) -> Result<(Gate, MidiInputConnection<()>), String> {
        let mut input = MidiInput::new(CLIENT).map_err(|e| e.to_string())?;
        // Clock, SysEx and active sensing are not used; do not wake up for them.
        input.ignore(Ignore::All);
        let index = feed.ports().assign(name);
        let feed = Arc::clone(feed);
        let gate: Gate = Arc::new(AtomicBool::new(true));
        let open = Arc::clone(&gate);
        let connection =
            input.connect(port, "in", move |_, bytes, ()| feed.receive_while_open(&open, index, bytes), ()).map_err(|e| e.to_string())?;
        Ok((gate, connection))
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
mod backend {
    use super::{MidiFeed, MidiPort};
    use std::sync::Arc;

    #[derive(Default)]
    pub(super) struct Backend;

    impl Backend {
        pub(super) fn close_all(&mut self, _feed: &MidiFeed) {}

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
    fn a_port_keeps_its_name_when_another_with_the_same_name_goes_away() {
        let port = |id: &str, raw: &str| (id.to_string(), raw.to_string());
        let mut known = Vec::new();
        // Two keyboards that call themselves the same, and a pad.
        let (a, b, pad) = (port("20:0", "Keys 20:0"), port("24:0", "Keys 24:0"), port("28:0", "Pad 28:0"));
        assert_eq!(assign_names(&mut known, &[a.clone(), b.clone(), pad.clone()]), ["Keys", "Keys (2)", "Pad"]);
        // The first is unplugged: the second is still "Keys (2)", so its connection and the
        // tracks that listen to it stay with it.
        assert_eq!(assign_names(&mut known, &[b.clone(), pad.clone()]), ["Keys (2)", "Pad"]);
        // Another "Keys" arrives (the first one again, at a new address): the free name is its.
        let c = port("32:0", "Keys 32:0");
        assert_eq!(assign_names(&mut known, &[b.clone(), pad.clone(), c.clone()]), ["Keys (2)", "Pad", "Keys"]);
        // The order the system lists them in does not matter.
        assert_eq!(assign_names(&mut known, &[c, pad, b]), ["Keys", "Pad", "Keys (2)"]);
        assert_eq!(known.len(), 3);
        assert!(assign_names(&mut known, &[]).is_empty() && known.is_empty(), "ports that went away are forgotten");
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
        let got: Vec<LiveMidi> = std::iter::from_fn(|| f.next())
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
    fn malformed_messages_from_a_device_are_dropped_or_masked() {
        let f = MidiFeed::default();
        // Too short for their kind, not a status byte, system messages, nothing at all.
        for bad in
            [&[0x90][..], &[0x90, 60], &[0xB0, 7], &[0xE0, 1], &[0xC0], &[0xD0], &[60, 100, 0], &[0x7F], &[0xF0, 1, 2, 0xF7], &[0xFF, 0, 0], &[]]
        {
            f.receive(0, bad);
        }
        assert_eq!((f.activity().0, f.dropped(), f.take_lost()), (0, 0, 0), "none of them counts");
        // Data bytes above 127 are masked, bytes past the message are ignored, and a short kind
        // keeps its one data byte.
        f.receive(0, &[0x9F, 0xFF, 0x80, 0x55, 0x66]);
        f.receive(0, &[0xC2, 0x85, 0x99]);
        f.receive(0, &[0xD0, 0x40]);
        let got: Vec<(u8, u8, u8)> = std::iter::from_fn(|| f.next()).map(|(_, e)| (e.status, e.data1, e.data2)).collect();
        assert_eq!(got, [(0x9F, 0x7F, 0x00), (0xC2, 0x05, 0x19), (0xD0, 0x40, 0x00)]);
    }

    #[test]
    fn ports_that_close_are_told_apart_from_ports_that_stay_or_were_never_open() {
        let port = |name: &str, enabled, open| MidiPort { name: name.to_string(), enabled, open };
        let before = [port("Keys", true, true), port("Pad", true, true), port("Off", false, false), port("Stays", true, true)];
        // Keys is unplugged, Pad is switched off, a new one arrives.
        let after = [port("Pad", false, false), port("Off", false, false), port("Stays", true, true), port("New", true, true)];
        assert_eq!(closed_ports(&before, &after).collect::<Vec<_>>(), ["Keys", "Pad"]);
        assert_eq!(closed_ports(&before, &[]).count(), 3, "the MIDI service failed: every open port is gone");
        assert_eq!(closed_ports(&[], &after).count(), 0);
    }

    /// End to end with the system's MIDI service: a virtual device shows up in the port list, what
    /// it sends arrives, and when it goes away its port is closed and marked as having lost input.
    /// Skipped where there is no MIDI service (a CI machine without the ALSA sequencer).
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn a_virtual_device_is_found_heard_and_followed_when_it_goes_away() {
        use midir::os::unix::VirtualOutput;
        use std::time::{Duration, Instant};
        let name = format!("soundcraft-test-{}", std::process::id());
        let device = midir::MidiOutput::new("SoundCraft test device").ok().and_then(|o| o.create_virtual(&name).ok());
        let Some(mut device) = device else {
            eprintln!("skipped: this machine has no MIDI service to make a virtual device with");
            return;
        };
        let until = |what: &mut dyn FnMut() -> bool| {
            let start = Instant::now();
            while !what() {
                if start.elapsed() > Duration::from_secs(5) {
                    return false;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            true
        };
        let mut inputs = MidiInputs::open(Vec::new());
        assert_eq!(inputs.error(), None);
        let found = |inputs: &MidiInputs| inputs.ports().iter().find(|p| p.name.contains(&name)).cloned();
        assert!(until(&mut || inputs.rescan() || found(&inputs).is_some()), "the device is listed");
        let port = found(&inputs).unwrap();
        assert!(port.enabled && port.open, "{port:?}");
        assert!(inputs.feed().connected() >= 1);
        let index = inputs.feed().ports().assign(&port.name);
        // What it sends arrives, tagged with its port; a system message does not.
        let feed = Arc::clone(inputs.feed());
        let before = feed.activity().0;
        device.send(&[0xF8]).unwrap();
        device.send(&[0x92, 60, 100]).unwrap();
        assert!(until(&mut || feed.activity().0 > before), "the note arrives");
        let last = feed.activity().1.unwrap();
        assert_eq!((last.port, last.status, last.data1, last.data2), (index, 0x92, 60, 100));
        // Switched off: closed, and what it held is to be released. On again: open.
        feed.take_lost();
        inputs.set_enabled(&port.name, false);
        assert_eq!(found(&inputs).map(|p| (p.enabled, p.open)), Some((false, false)));
        assert_ne!(feed.take_lost() & midi_port_bit(index), 0);
        assert_eq!(inputs.disabled(), std::slice::from_ref(&port.name));
        inputs.set_enabled(&port.name, true);
        assert_eq!(found(&inputs).map(|p| (p.enabled, p.open)), Some((true, true)));
        // Unplugged: it leaves the list on a rescan, and its port is marked.
        feed.take_lost();
        drop(device);
        assert!(until(&mut || {
            inputs.rescan();
            found(&inputs).is_none()
        }));
        assert_ne!(feed.take_lost() & midi_port_bit(index), 0, "a device that goes away lets go of its notes");
    }

    #[test]
    fn a_connection_that_is_closed_marks_its_port_even_when_the_name_stays() {
        let feed = MidiFeed::default();
        let (keys, pad, off) = (feed.ports().assign("Keys"), feed.ports().assign("Pad"), feed.ports().assign("Off"));
        let id = |a: &str, b: &str| (a.to_string(), b.to_string());
        let gate = || Arc::new(AtomicBool::new(true));
        let mut open = vec![
            ("20:0".to_string(), "Keys".to_string(), gate(), ()),
            ("24:0".to_string(), "Pad".to_string(), gate(), ()),
            ("28:0".to_string(), "Off".to_string(), gate(), ()),
        ];
        let pad_gate = Arc::clone(&open[1].2);
        // Keys was unplugged and plugged in again between two scans: the same name at another
        // address. Pad is as before. Off was switched off.
        let listed = [id("32:0", "Keys"), id("24:0", "Pad"), id("28:0", "Off")];
        retire(&mut open, &listed, &["Off".to_string()], &feed);
        assert_eq!(open.iter().map(|(id, ..)| id.as_str()).collect::<Vec<_>>(), ["24:0"], "the old Keys connection is closed");
        assert!(pad_gate.load(Ordering::Acquire), "the connection that stays still delivers");
        assert_eq!(feed.take_lost(), midi_port_bit(keys) | midi_port_bit(off), "and what it held is released; Pad ({pad}) keeps its notes");
        // The lists of open ports before and after both show an open "Keys": nothing to see there.
        let port = |name: &str| MidiPort { name: name.to_string(), enabled: true, open: true };
        assert_eq!(closed_ports(&[port("Keys"), port("Pad")], &[port("Keys"), port("Pad")]).count(), 0);
    }

    #[test]
    fn what_a_connection_delivers_while_it_closes_never_follows_the_loss() {
        /// A connection whose device thread delivers one more note-on while it is dropped, as
        /// a system that does not wait for that thread allows.
        struct Closing(Arc<MidiFeed>, Gate, u8);
        impl Drop for Closing {
            fn drop(&mut self) {
                std::thread::sleep(std::time::Duration::from_millis(2));
                self.0.receive_while_open(&self.1, self.2, &[0x90, 60, 100]);
            }
        }
        let feed = Arc::new(MidiFeed::default());
        let keys = feed.ports().assign("Keys");
        let gate: Gate = Arc::new(AtomicBool::new(true));
        // Open, it delivers.
        feed.receive_while_open(&gate, keys, &[0x90, 50, 100]);
        let mut open = vec![("20:0".to_string(), "Keys".to_string(), Arc::clone(&gate), Closing(Arc::clone(&feed), Arc::clone(&gate), keys))];
        retire(&mut open, &[], &[], &feed);
        assert_eq!(feed.take_lost(), midi_port_bit(keys));
        let (stamp, e) = feed.next().unwrap();
        assert_eq!(e.data1, 50);
        assert!(stamp <= feed.lost_at(keys), "what came before the close is from before the loss");
        assert!(feed.next().is_none(), "the note-on delivered during the close is discarded: nothing can play after the release");
        assert_eq!(feed.lost_at(keys + 1), i64::MIN, "another port has no loss");
    }

    #[test]
    fn a_loss_from_a_full_queue_is_written_into_the_queue_where_it_happened() {
        let f = MidiFeed::default();
        for _ in 0..INCOMING {
            f.receive(1, &[0xB0, 1, 0]);
        }
        // Two ports lose a release each; the second drop cannot write the first one's mark.
        f.receive(0, &[0x80, 60, 0]);
        f.receive(2, &[0x80, 62, 0]);
        assert_eq!(f.dropped(), 2);
        assert_eq!(std::iter::from_fn(|| f.next()).take(INCOMING - 2).count(), INCOMING - 2);
        // Room again: the marks come first, then what is played after the loss.
        f.receive(0, &[0x90, 64, 100]);
        let got: Vec<(u8, u8, u8)> = std::iter::from_fn(|| f.next()).map(|(_, e)| (e.port, e.status, e.data1)).collect();
        assert_eq!(got, [(1, 0xB0, 1), (1, 0xB0, 1), (0, RecordedMidi::LOST, 0), (2, RecordedMidi::LOST, 0), (0, 0x90, 64)]);
        assert_eq!(f.dropped(), 2, "nothing more was dropped");
        // Written once.
        f.receive(0, &[0x80, 64, 0]);
        assert_eq!(std::iter::from_fn(|| f.next()).map(|(_, e)| e.status).collect::<Vec<_>>(), [0x80]);
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
        assert_eq!(std::iter::from_fn(|| f.next()).count(), INCOMING);
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
