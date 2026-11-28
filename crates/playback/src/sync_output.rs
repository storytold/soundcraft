//! One native worker owns every external MIDI output route (today the track note output) and
//! one connection per destination label, each bound to the saved native port ID it was opened
//! with. A port carries one route of each [`RouteKind`], so several kinds of traffic share one
//! connection (a WinMM port can usually be opened only once).
//!
//! The worker also plays live MIDI input through to the tracks' outputs (MIDI Thru,
//! [`crate::thru`]). The input threads wake it for each message ([`MidiFeed::attach_thru`]), so
//! its waits are parks that a message cuts short, and thru goes out as soon as it arrives,
//! outside the deadline queue.
use crate::midi::{MidiFeed, SentBy};
use crate::sync_clock::{SessionRevision, delivery_offset};
use crate::thru::Thru;
use crate::{
    Cmd, MidiSyncStatus, Session,
    sync_clock::{CallbackMetrics, Publisher, SyncEncoder, View},
    sync_status::PortTiming,
};
mod ports;
use midir::MidiOutputConnection;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicU64, Ordering::Relaxed},
    mpsc::{Receiver, Sender, channel},
};
use std::time::{Duration, Instant};

pub(crate) fn output_ids(saved: &BTreeMap<String, String>) -> Result<BTreeMap<String, String>, String> {
    ports::available(saved)
}

/// Extension point for OS-timestamped MIDI backends. The native midir backend
/// sends immediately; each port retains future messages until their deadline.
trait SyncPort: Send {
    fn send_at(&mut self, at: Instant, bytes: &[u8]) -> Result<Instant, String>;
}
impl SyncPort for MidiOutputConnection {
    fn send_at(&mut self, _at: Instant, bytes: &[u8]) -> Result<Instant, String> {
        let handed = Instant::now();
        self.send(bytes).map_err(|e| e.to_string())?;
        Ok(handed)
    }
}
/// The kinds of traffic a destination port can carry at once, each through its own encoder.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RouteKind {
    /// Clip notes and controllers of the MIDI and instrument tracks routed to the port.
    Notes,
}
const ROUTE_KINDS: usize = 1;
impl RouteKind {
    const ALL: [RouteKind; ROUTE_KINDS] = [RouteKind::Notes];
    fn index(self) -> usize {
        self as usize
    }
}
/// A message that ends sound: Note-Off (or Note-On at velocity 0), sustain pedal up (any value
/// below 64), All Sound Off or All Notes Off. A full queue never evicts one (MIDI 1.0 p24: send
/// each Note-Off).
fn releases(bytes: &[u8]) -> bool {
    match bytes {
        [status, _, velocity, ..] if status & 0xf0 == 0x90 => *velocity == 0,
        [status, ..] if status & 0xf0 == 0x80 => true,
        [status, 64, value, ..] if status & 0xf0 == 0xb0 => *value < 64,
        [status, 120 | 123, ..] if status & 0xf0 == 0xb0 => true,
        _ => false,
    }
}
#[derive(Default)]
struct Stats {
    #[cfg(test)]
    iterations: AtomicU64,
    connected: Mutex<BTreeSet<String>>,
    priority: AtomicU64,
    sent: AtomicU64,
    pending_transition_timeouts: AtomicU64,
    oversized_dropped: AtomicU64,
    session_backlog_drops: AtomicU64,
    buckets: [AtomicU64; 7],
    max: AtomicU64,
    /// MIDI Thru: messages handed to a port, and how long after they arrived (same buckets).
    thru_sent: AtomicU64,
    thru_buckets: [AtomicU64; 7],
    thru_max: AtomicU64,
}
impl Stats {
    /// A thru message was handed to its port `us` microseconds after it arrived.
    fn record_thru(&self, us: u64) {
        let bucket = BOUNDS.partition_point(|&v| v < us);
        self.thru_buckets[bucket].fetch_add(1, Relaxed);
        self.thru_sent.fetch_add(1, Relaxed);
        self.thru_max.fetch_max(us, Relaxed);
    }
    fn connected(&self) -> std::sync::MutexGuard<'_, BTreeSet<String>> {
        self.connected.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }
    fn record(&self, at: Instant, handed: Instant) {
        let us = handed.saturating_duration_since(at).as_micros() as u64;
        let bucket = BOUNDS.partition_point(|&v| v < us);
        self.buckets[bucket].fetch_add(1, Relaxed);
        self.sent.fetch_add(1, Relaxed);
        if us > self.max.load(Relaxed) {
            self.max.fetch_max(us, Relaxed);
        }
    }
}
fn effective_offset(offset: i64, sr: soundcraft_time::SampleRate) -> i64 {
    let max = crate::max_sync_offset_samples(sr);
    offset.clamp(-max, max)
}
/// Joins the failures of several destinations in [`MidiSyncStatus::error`].
const FAILURE_SEPARATOR: &str = "; ";
const BOUNDS: [u64; 6] = [500, 1000, 2000, 5000, 10_000, 20_000];
/// The `p`th percentile of the values counted in `buckets` ([`BOUNDS`]), the largest being
/// `max`. Only the buckets are known, so the value is estimated as if a bucket's values were
/// spread evenly from its lower bound to its upper bound, or to `max` when that is lower: never
/// above `max`, and not the bucket's ceiling (which read 500 µs for latencies of 0.2 ms).
fn percentile(buckets: [u64; 7], p: u64, max: u64) -> u64 {
    let target = buckets.iter().sum::<u64>().saturating_mul(p).div_ceil(100);
    if target == 0 {
        return 0;
    }
    let mut before = 0;
    for (i, &n) in buckets.iter().enumerate() {
        if n > 0 && before + n >= target {
            let low = if i == 0 { 0 } else { BOUNDS.get(i - 1).copied().unwrap_or(0) }.min(max);
            let high = BOUNDS.get(i).copied().unwrap_or(max).min(max);
            return low + (high - low).saturating_mul(target - before) / n;
        }
        before += n;
    }
    max
}
#[derive(Clone)]
struct WorkerSender {
    tx: Sender<Update>,
    worker: std::thread::Thread,
}
impl WorkerSender {
    fn send(&self, update: Update) -> Result<(), std::sync::mpsc::SendError<Update>> {
        self.tx.send(update)?;
        // A timed-out transition can park on a late presentation as well as on control.
        self.worker.unpark();
        Ok(())
    }
}
/// The destinations of one [`RouteKind`]: each port label with its signed offset in samples,
/// and whether the route is delay-compensated.
#[derive(Clone, Default, PartialEq, Eq)]
struct Routes {
    ports: Vec<(String, i64)>,
    compensate: bool,
}
#[derive(Default)]
pub(crate) struct Output {
    port_registry: Arc<ports::Registry>,
    view: View,
    metrics: Arc<CallbackMetrics>,
    session: Option<Arc<Session>>,
    generation: u64,
    routes: [Routes; ROUTE_KINDS],
    stats: Arc<Stats>,
    trouble: Arc<Mutex<Option<String>>>,
    worker: Option<(WorkerSender, std::thread::JoinHandle<()>)>,
    retry_spawn_at: Option<Instant>,
    /// Live MIDI input, for MIDI Thru.
    feed: Option<Arc<MidiFeed>>,
}
enum Update {
    /// Live MIDI input to play through to the tracks' outputs (MIDI Thru).
    Feed(Option<Arc<MidiFeed>>),
    Session(Arc<Session>),
    /// Replace the ports of one route kind. Each newly routed port gets its own encoder.
    Routes(RouteKind, Vec<(String, i64)>, bool, fn(&str) -> Box<dyn SyncEncoder>),
    /// Send these messages now on every connected port (All MIDI Notes Off).
    Panic(fn(&mut dyn FnMut(&[u8]))),
    Wake(Option<(u64, bool)>),
}
impl Output {
    pub(crate) fn restore_port_ids(&self, ids: &BTreeMap<String, String>) {
        self.port_registry.restore(ids);
    }
    pub(crate) fn opened_port_ids(&self) -> Option<BTreeMap<String, String>> {
        self.port_registry.opened()
    }
    pub(crate) fn configure(&mut self, s: &Arc<Session>, audio: &Sender<Cmd>) -> u64 {
        if self.session.is_none() {
            let _ = audio.send(Cmd::Clock(Some(Publisher::new(self.view.clone(), self.metrics.clone()))));
        }
        self.generation = self.generation.wrapping_add(1);
        let revision = SessionRevision { generation: self.generation, doc: s.clone(), remap: true };
        let active = !self.stats.connected().is_empty();
        if self.view.lock().unwrap_or_else(std::sync::PoisonError::into_inner).register(revision, active) {
            self.stats.session_backlog_drops.fetch_add(1, Relaxed);
        }
        self.session = Some(s.clone());
        if let Some((tx, _)) = &self.worker {
            let _ = tx.send(Update::Session(s.clone()));
        }
        self.generation
    }
    fn send(&mut self, u: Update) -> bool {
        if self.worker.is_none() {
            let Some(s) = self.session.clone() else { return false };
            if self.retry_spawn_at.is_some_and(|at| Instant::now() < at) {
                return false;
            }
            let (tx, rx) = channel();
            let view = self.view.clone();
            let trouble = self.trouble.clone();
            let stats = self.stats.clone();
            let registry = self.port_registry.clone();
            match std::thread::Builder::new()
                .name("MIDI output".into())
                .spawn(move || drive_ports(view, s, rx, trouble, stats, ports::Native { registry, scanner: None }))
            {
                Ok(thread) => {
                    let tx = WorkerSender { tx, worker: thread.thread().clone() };
                    if self.feed.is_some() {
                        let _ = tx.send(Update::Feed(self.feed.clone()));
                    }
                    self.worker = Some((tx, thread));
                }
                Err(e) => {
                    *self.trouble.lock().unwrap_or_else(std::sync::PoisonError::into_inner) =
                        Some(format!("MIDI output worker could not start: {e}"));
                    self.retry_spawn_at = Some(Instant::now() + Duration::from_secs(1));
                    return false;
                }
            }
        }
        let queued = self.worker.as_ref().is_some_and(|(tx, _)| tx.send(u).is_ok());
        if !queued {
            self.worker = None;
        }
        queued
    }
    /// Route `kind` to these ports, each with its signed offset in samples; `make` builds the
    /// encoder of a newly routed port from its label. Whether the worker accepted them: a transient startup
    /// failure can be retried with the same settings.
    pub(crate) fn set_routes(
        &mut self,
        kind: RouteKind,
        ports: Vec<(String, i64)>,
        compensate: bool,
        make: fn(&str) -> Box<dyn SyncEncoder>,
    ) -> bool {
        let next = Routes { ports, compensate };
        let routes = &mut self.routes[kind.index()];
        if self.worker.as_ref().is_some_and(|(_, task)| !task.is_finished()) && *routes == next {
            return true;
        }
        *routes = next.clone();
        (next.ports.is_empty() && self.worker.is_none()) || self.send(Update::Routes(kind, next.ports, compensate, make))
    }
    /// Play live input from `feed` through to the tracks' MIDI outputs (MIDI Thru), from the
    /// worker that owns the ports (started with the first route).
    pub(crate) fn set_feed(&mut self, feed: Arc<MidiFeed>) {
        if self.feed.as_ref().is_some_and(|f| Arc::ptr_eq(f, &feed)) {
            return;
        }
        self.feed = Some(feed.clone());
        if let Some((tx, _)) = &self.worker {
            let _ = tx.send(Update::Feed(Some(feed)));
        }
    }
    /// Send `messages` at once on every connected port. Returns how many ports are connected.
    pub(crate) fn panic(&mut self, messages: fn(&mut dyn FnMut(&[u8]))) -> usize {
        let connected = self.stats.connected().len();
        if self.worker.is_some() {
            self.send(Update::Panic(messages));
        }
        connected
    }
    pub(crate) fn send_transport(&self, audio: &Sender<Cmd>, command: Cmd) {
        let Some((tx, _)) = &self.worker else {
            let _ = audio.send(command);
            return;
        };
        // The callback may publish before the worker processes this message. Capture
        // its baseline before even enqueueing the corresponding audio command.
        let before = self.view.lock().unwrap_or_else(std::sync::PoisonError::into_inner).transport_state();
        let already_stopped = matches!(command, Cmd::Stop) && before.is_some_and(|(_, playing)| !playing);
        if audio.send(command).is_ok() && !already_stopped {
            let _ = tx.send(Update::Wake(before));
        }
    }
    pub(crate) fn trouble(&self) -> Option<String> {
        self.trouble.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone()
    }
    /// The audio device has not reported its output timing since it was opened, so compensated
    /// output could not be timed.
    pub(crate) fn timing_unknown(&self) -> bool {
        self.metrics.lead() == (None, crate::OutputLeadSource::Unknown)
    }
    pub(crate) fn timing_issue(&self) -> Option<crate::OutputTimingIssue> {
        if !self.routes.iter().any(|r| r.compensate && !r.ports.is_empty()) {
            return None;
        }
        match self.metrics.lead() {
            (Some(lead_us), crate::OutputLeadSource::Retained) => Some(crate::OutputTimingIssue::Retained { lead_us }),
            (None, crate::OutputLeadSource::Unknown) => Some(crate::OutputTimingIssue::Unknown),
            _ => None,
        }
    }
    pub(crate) fn status(&self) -> MidiSyncStatus {
        let buckets = self.stats.buckets.each_ref().map(|v| v.load(Relaxed));
        let max = self.stats.max.load(Relaxed);
        let (lead, lead_source) = self.metrics.lead();
        let (sr, system_delay, system_rate) =
            self.view.lock().unwrap_or_else(std::sync::PoisonError::into_inner).mixer_delay().unwrap_or_else(|| {
                let sr = self.session.as_ref().map_or_else(soundcraft_time::SampleRate::default, |s| s.sample_rate);
                (sr, 0, sr.as_f64())
            });
        let timing = |name: &str, offset, compensate| {
            let available = if compensate { lead } else { Some(0) };
            let delay = available.map(|us| delivery_offset(Duration::from_micros(us), system_delay, system_rate, sr, compensate, offset));
            PortTiming {
                connected: self.stats.connected().contains(name),
                requested_offset_samples: offset,
                offset_samples: effective_offset(offset, sr),
                system_delay_samples: system_delay as u64,
                device_lead_us: lead,
                lead_source,
                notes_enabled: false,
                advance_limited: delay.is_some_and(|(_, limited)| limited),
                effective_total_us: delay.map(|(total, _)| (total * 1e6).round() as i64),
                notes_total_us: None,
            }
        };
        let mut ports = BTreeMap::new();
        for kind in RouteKind::ALL {
            let routes = &self.routes[kind.index()];
            for (name, offset) in &routes.ports {
                let t = timing(name, *offset, routes.compensate);
                let (total, limited) = (t.effective_total_us, t.advance_limited);
                let entry = ports.entry(name.clone()).or_insert(t);
                entry.advance_limited |= limited;
                match kind {
                    RouteKind::Notes => {
                        entry.notes_enabled = true;
                        entry.notes_total_us = total;
                    }
                }
            }
        }
        MidiSyncStatus {
            sent: self.stats.sent.load(Relaxed),
            pending_transition_timeouts: self.stats.pending_transition_timeouts.load(Relaxed),
            oversized_dropped: self.stats.oversized_dropped.load(Relaxed),
            error: self.trouble(),
            session_backlog_drops: self.stats.session_backlog_drops.load(Relaxed),
            lateness_buckets: buckets,
            lateness_p50_us: percentile(buckets, 50, max),
            lateness_p95_us: percentile(buckets, 95, max),
            lateness_max_us: max,
            reported_lead_us: lead,
            lead_source,
            callback_gap_max_us: self.metrics.max_gap_us.load(Relaxed),
            callback_gaps_over_two_buffers: self.metrics.late_buffers.load(Relaxed),
            lead_clamps: self.metrics.clamps.load(Relaxed),
            thru: {
                let buckets = self.stats.thru_buckets.each_ref().map(|v| v.load(Relaxed));
                let max = self.stats.thru_max.load(Relaxed);
                crate::sync_status::ThruStatus {
                    sent: self.stats.thru_sent.load(Relaxed),
                    echoes_dropped: self.feed.as_ref().map_or(0, |f| f.thru_echoes()),
                    latency_buckets: buckets,
                    latency_p50_us: percentile(buckets, 50, max),
                    latency_p95_us: percentile(buckets, 95, max),
                    latency_max_us: max,
                }
            },
            priority: match self.stats.priority.load(Relaxed) {
                1 => "raised",
                2 => "normal (raise unavailable)",
                _ => "inactive",
            },
            ports,
        }
    }
}
impl Drop for Output {
    fn drop(&mut self) {
        if let Some((tx, worker)) = self.worker.take() {
            drop(tx);
            worker.thread().unpark();
            let _ = worker.join();
        }
    }
}

struct Route {
    map: Option<SessionRevision>,
    offset: i64,
    compensate: bool,
    encoder: Box<dyn SyncEncoder>,
}
struct Port<C> {
    link: Option<C>,
    /// When to try to open the port again or, while it is open, to check that it is still listed.
    retry: Instant,
    /// Whether the port was ever open, to log a reconnection as such.
    opened: bool,
    routes: [Option<Route>; ROUTE_KINDS],
    future: Vec<Event>,
    timing_epochs: [u64; ROUTE_KINDS],
    waiting_for_timing: bool,
    failure: Option<String>,
    /// The live input, told of what the clips send here for its echo check
    /// ([`MidiFeed::sent`]): a port that is also an input may send it straight back.
    echo: Option<Arc<MidiFeed>>,
    /// The keys MIDI Thru has sounding here, bit `k` of entry `c` for key `k` on channel `c`.
    /// The clips and Thru share a key on a channel: the device hears its Note-Off only when the
    /// last of them lets go, as a device ends notes by key.
    thru_keys: [u128; 16],
    /// The channels where MIDI Thru has the sustain pedal down, bit `c` for channel `c`. Shared
    /// with the clips as a key is: the pedal goes up when the last of them lets go of it.
    thru_pedal: u16,
}
/// The channel and key of a Note-Off (or Note-On at velocity 0).
fn note_off(bytes: &[u8]) -> Option<(u8, u8)> {
    match bytes {
        [status, key, velocity, ..] if status & 0xf0 == 0x90 && *velocity == 0 => Some((status & 0x0f, key & 0x7f)),
        [status, key, ..] if status & 0xf0 == 0x80 => Some((status & 0x0f, key & 0x7f)),
        _ => None,
    }
}
/// The channel of a sustain pedal message (CC 64) and whether it puts the pedal down (64 or more).
fn pedal(bytes: &[u8]) -> Option<(u8, bool)> {
    match bytes {
        [status, 64, value, ..] if status & 0xf0 == 0xb0 => Some((status & 0x0f, *value >= 64)),
        _ => None,
    }
}
/// Whether MIDI Thru has the key of this clip Note-Off sounding (`keys`, [`Port::thru_keys`]), or
/// the pedal of this clip pedal up down (`pedal`, [`Port::thru_pedal`]): the clip's message is
/// held back, and Thru's own ends the note or lifts the pedal.
fn thru_holds(keys: &[u128; 16], pedal_down: u16, bytes: &[u8]) -> bool {
    if let Some((channel, false)) = pedal(bytes) {
        return pedal_down & (1 << channel) != 0;
    }
    note_off(bytes).is_some_and(|(channel, key)| keys[usize::from(channel)] & (1u128 << key) != 0)
}
/// Most future events a port holds. When full, an event that is not a release makes room; when
/// every queued event is a release, an event that is not one is dropped. Releases are always
/// kept.
const QUEUE: usize = 256;
#[derive(Clone, Copy)]
struct Event {
    route: usize,
    transport: (u64, bool),
    at: Instant,
    len: usize,
    bytes: [u8; 16],
}
impl Event {
    fn new(route: usize, transport: (u64, bool), at: Instant, msg: &[u8]) -> Option<Self> {
        let mut bytes = [0; 16];
        bytes.get_mut(..msg.len())?.copy_from_slice(msg);
        Some(Self { route, transport, at, len: msg.len(), bytes })
    }
}
impl<C: SyncPort> Port<C> {
    /// Close the port after `failure`; it is opened again after a second.
    fn disconnect(&mut self, name: &str, failure: String, now: Instant, stats: &Stats) {
        log::warn!("{failure}");
        self.failure = Some(failure);
        stats.connected().remove(name);
        self.link = None;
        self.future.clear();
        self.retry = now + Duration::from_secs(1);
    }

    fn poll(&mut self, name: &str, now: Instant, session: &Session, view: &View, stats: &Stats, pending: &mut Vec<Event>) -> Option<Instant> {
        self.waiting_for_timing = false;
        let Some(port) = &mut self.link else { return Some(self.retry) };
        let mut next = None;
        let mut soon = |at: Instant| next = Some(next.map_or(at, |t: Instant| t.min(at)));
        pending.clear();
        self.future.retain(|event| {
            if event.at <= now {
                pending.push(*event);
                false
            } else {
                soon(event.at);
                true
            }
        });
        for (route_index, route) in self.routes.iter_mut().enumerate() {
            let Some(r) = route else { continue };
            let (timing_epoch, block, future, revision, unversioned) = {
                let mut timeline = view.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                let (block, future) = timeline.read(now, r.compensate, r.offset);
                if r.compensate && block.is_none() && future.is_none() {
                    timeline.wait_for_timing();
                    self.waiting_for_timing = true;
                }
                let changed = block
                    .and_then(|b| timeline.map(b.generation))
                    .filter(|revision| r.map.as_ref().is_none_or(|old| old.generation != revision.generation))
                    .cloned();
                if future.is_none()
                    && let Some(b) = block
                    && !(b.generation == 0 && timeline.unversioned())
                    && !r.map.as_ref().is_some_and(|old| old.generation == b.generation)
                    && timeline.map(b.generation).is_none()
                {
                    timeline.wait_for_revision();
                    self.waiting_for_timing = true;
                }
                (timeline.timing_epoch, block, future, changed, timeline.unversioned())
            };
            if r.compensate && self.timing_epochs[route_index] != timing_epoch {
                self.timing_epochs[route_index] = timing_epoch;
                self.future.retain(|event| event.route != route_index);
                pending.retain(|event| event.route != route_index);
                r.encoder.timing_lost();
            }
            if let Some(new) = revision {
                if new.remap
                    && let Some(old) = &r.map
                {
                    r.encoder.remap(&old.doc, &new.doc);
                }
                r.map = Some(new);
            }
            if let Some(at) = future {
                soon(at);
            }
            if let Some(b) = block {
                let transport = (b.pass, b.playing);
                let keep = |e: &Event| e.route != route_index || e.transport == transport;
                self.future.retain(keep);
                pending.retain(keep);
                let session = match &r.map {
                    Some(rev) if rev.generation == b.generation => &*rev.doc,
                    None if unversioned && b.generation == 0 => session,
                    _ => {
                        self.future.retain(|event| event.route != route_index);
                        pending.retain(|event| event.route != route_index);
                        // A later callback may carry a retained revision. Wait for it without
                        // sending old audio positions coded against the latest control document.
                        continue;
                    }
                };
                if let Some(at) = r.encoder.update(b, now, session, &mut |at, m| {
                    let Some(event) = Event::new(route_index, transport, at, m) else {
                        stats.oversized_dropped.fetch_add(1, Relaxed);
                        self.failure = Some(format!("{name}: MIDI message longer than 16 bytes dropped"));
                        return;
                    };
                    if at <= now {
                        pending.push(event);
                    } else {
                        if self.future.len() >= QUEUE {
                            self.failure = Some(format!("{name}: MIDI output deadline queue is full"));
                            if let Some(index) = self.future.iter().position(|e| !releases(&e.bytes[..e.len])) {
                                self.future.remove(index);
                            } else if !releases(m) {
                                return;
                            }
                        }
                        self.future.push(event);
                        soon(at);
                    }
                }) {
                    soon(at);
                }
            }
        }
        // A stable sort keeps the emission order of equal deadlines.
        pending.sort_by_key(|m| m.at);
        for e in pending.iter() {
            if thru_holds(&self.thru_keys, self.thru_pedal, &e.bytes[..e.len]) {
                continue;
            }
            if let Some(f) = &self.echo {
                // Before it leaves: its echo may be back before the send returns.
                f.sent(name, &e.bytes[..e.len], SentBy::Clips);
            }
            match port.send_at(e.at, &e.bytes[..e.len]) {
                Ok(handed) => stats.record(e.at, handed),
                Err(err) => {
                    // Reopening resets the encoders, which release what the device may still hold.
                    self.disconnect(name, format!("{name}: {err}"), now, stats);
                    return Some(self.retry);
                }
            }
        }
        for r in self.routes.iter_mut().flatten() {
            r.encoder.delivered();
        }
        next
    }
}
fn set_route<C: SyncPort>(map: &mut BTreeMap<String, Port<C>>, index: usize, name: String, route: Route) {
    let port = map.entry(name).or_insert_with(|| Port {
        link: None,
        retry: Instant::now(),
        routes: std::array::from_fn(|_| None),
        future: Vec::new(),
        timing_epochs: [0; ROUTE_KINDS],
        waiting_for_timing: false,
        failure: None,
        opened: false,
        echo: None,
        thru_keys: [0; 16],
        thru_pedal: 0,
    });
    port.routes[index] = Some(route);
}
/// Send `msg` now on a connected port. A failure disconnects it; the reconnect resets its
/// encoders.
fn send_now<C: SyncPort>(name: &str, port: &mut Port<C>, msg: &[u8], stats: &Stats) {
    let now = Instant::now();
    if thru_holds(&port.thru_keys, port.thru_pedal, msg) {
        return;
    }
    if let Some(link) = &mut port.link {
        if let Some(f) = &port.echo {
            f.sent(name, msg, SentBy::Clips);
        }
        match link.send_at(now, msg) {
            Ok(handed) => stats.record(now, handed),
            Err(error) => port.disconnect(name, format!("{name}: {error}"), now, stats),
        }
    }
}
/// Send a MIDI Thru message now on the port `name`, if it is open: it is not queued, since a
/// late note is worse than none when playing live. `arrived` is when its input arrived
/// ([`MidiFeed::clock`]), for the latency diagnostics; releases thru makes on its own have none.
/// A failure disconnects the port.
fn send_thru<C: SyncPort>(ports: &mut BTreeMap<String, Port<C>>, name: &str, msg: &[u8], feed: &MidiFeed, stats: &Stats, arrived: Option<i64>) {
    if let Some(p) = ports.get_mut(name) {
        send_thru_on(name, p, msg, feed, stats, arrived);
    }
}
/// [`send_thru`] on the port `p` named `name`.
fn send_thru_on<C: SyncPort>(name: &str, p: &mut Port<C>, msg: &[u8], feed: &MidiFeed, stats: &Stats, arrived: Option<i64>) {
    if p.link.is_none() {
        return;
    }
    if let Some((channel, key)) = note_off(msg) {
        p.thru_keys[usize::from(channel)] &= !(1u128 << key);
        // The clips still sound this key here: their own Note-Off ends it.
        if p.routes.iter().flatten().any(|r| r.encoder.holds(channel, key)) {
            return;
        }
    } else if let [status, key, _, ..] = msg
        && status & 0xf0 == 0x90
    {
        p.thru_keys[usize::from(status & 0x0f)] |= 1u128 << (key & 0x7f);
    } else if let Some((channel, down)) = pedal(msg) {
        if down {
            p.thru_pedal |= 1 << channel;
        } else {
            p.thru_pedal &= !(1 << channel);
            // The clips still hold the pedal here: their own pedal up lifts it.
            if p.routes.iter().flatten().any(|r| r.encoder.holds_pedal(channel)) {
                return;
            }
        }
    }
    let Some(link) = &mut p.link else { return };
    // Before it leaves: its echo may be back before this returns.
    feed.sent(name, msg, SentBy::Thru);
    match link.send_at(Instant::now(), msg) {
        Ok(_) => {
            if let Some(at) = arrived {
                stats.record_thru(u64::try_from(feed.clock().saturating_sub(at)).unwrap_or(0));
            }
        }
        Err(error) => p.disconnect(name, format!("{name}: {error}"), Instant::now(), stats),
    }
}
/// The worker's MIDI Thru: the live input, what thru holds on the ports, and whether the
/// transport was playing (a Stop lets go of it).
#[derive(Default)]
struct LiveThru {
    feed: Option<Arc<MidiFeed>>,
    thru: Thru,
    was_playing: bool,
}
impl LiveThru {
    /// Take live input from `next` from now on. What the old input held is let go of: its keys'
    /// releases will not come from the new one.
    fn set_feed<C: SyncPort>(&mut self, next: Option<Arc<MidiFeed>>, ports: &mut BTreeMap<String, Port<C>>, stats: &Stats) {
        if let Some(old) = self.feed.take() {
            old.detach_thru(std::thread::current().id());
            self.thru.release_all(&mut |name, msg| send_thru(ports, name, msg, &old, stats, None));
        }
        if let Some(f) = &next {
            f.attach_thru(std::thread::current());
        }
        self.feed = next;
    }
    /// The ports that lost their last route are about to close: end what thru holds there while
    /// they are still open. Once that went out, a later reopen has nothing to quiet.
    fn release_closing<C: SyncPort>(&mut self, ports: &mut BTreeMap<String, Port<C>>, stats: &Stats) {
        let Some(f) = &self.feed else { return };
        for (name, p) in ports.iter_mut().filter(|(_, p)| p.routes.iter().all(Option::is_none)) {
            self.thru.release_dest(name, &mut |_, msg| send_thru_on(name, p, msg, f, stats, None));
            if p.link.is_some() {
                self.thru.reopened(name);
            }
        }
    }
    /// The port `name` is about to close as it is no longer listed or its label is bound to
    /// another port now: end what thru holds there while it is open, so that a device still
    /// there does not keep it sounding. Sent to a port that is gone, it goes nowhere, so the
    /// channels are still quieted when the label opens again ([`Self::reopened`]).
    fn release_leaving<C: SyncPort>(&mut self, name: &str, p: &mut Port<C>, stats: &Stats) {
        if let Some(f) = &self.feed {
            self.thru.release_dest(name, &mut |_, msg| send_thru_on(name, p, msg, f, stats, None));
        }
    }
    /// Every thru note off and pedal up (All MIDI Notes Off).
    fn release_all<C: SyncPort>(&mut self, ports: &mut BTreeMap<String, Port<C>>, stats: &Stats) {
        if let Some(f) = &self.feed {
            self.thru.release_all(&mut |name, msg| send_thru(ports, name, msg, f, stats, None));
        }
    }
    /// Send what arrived, and let go of what lost ports, a Stop or a change of the session end.
    fn service<C: SyncPort>(&mut self, ports: &mut BTreeMap<String, Port<C>>, stats: &Stats, session: &Session, view: &View, now: Instant) {
        let Some(f) = &self.feed else { return };
        let thru = &mut self.thru;
        let lost = f.take_thru_lost();
        if lost != 0 {
            thru.release_ports(lost, &mut |name, msg| send_thru(ports, name, msg, f, stats, None));
        }
        let mut at = None;
        while let Some((arrived, e)) = f.next_thru() {
            let at = *at.get_or_insert_with(|| play_position(view, now));
            thru.route(session, f.ports(), at, &e, &mut |name, msg| send_thru(ports, name, msg, f, stats, Some(arrived)));
        }
        let playing = view.lock().unwrap_or_else(std::sync::PoisonError::into_inner).transport_state().is_some_and(|(_, playing)| playing);
        if std::mem::replace(&mut self.was_playing, playing) && !playing {
            thru.release_all(&mut |name, msg| send_thru(ports, name, msg, f, stats, None));
        }
        if thru.holds() {
            let at = at.unwrap_or_else(|| play_position(view, now));
            thru.follow(session, f.ports(), at, &mut |name, msg| send_thru(ports, name, msg, f, stats, None));
        }
    }
    /// The port `name` was opened again: the device may still hold what thru played before it
    /// went. Quiet those channels (the clip encoder does the same for its own).
    /// A send that fails closes the port, and the sends after it go nowhere: then every one of
    /// these channels is quieted again when the port next opens.
    fn reopened<C: SyncPort>(&mut self, name: &str, p: &mut Port<C>, stats: &Stats) {
        p.thru_keys = [0; 16];
        p.thru_pedal = 0;
        let channels = self.thru.reopened(name);
        for channel in (0..16u8).filter(|c| channels & (1 << c) != 0) {
            send_now(name, p, &[0xb0 | channel, 64, 0], stats);
            send_now(name, p, &[0xb0 | channel, 123, 0], stats);
        }
        if p.link.is_none() {
            self.thru.unquieted(name, channels);
        }
    }
}
fn same_feed(a: Option<&Arc<MidiFeed>>, b: Option<&Arc<MidiFeed>>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => Arc::ptr_eq(a, b),
        (None, None) => true,
        _ => false,
    }
}
/// The play position for MIDI Thru's mute check (mute automation): where the last presented
/// block puts `now`, 0 before the first.
fn play_position(view: &View, now: Instant) -> i64 {
    view.lock().unwrap_or_else(std::sync::PoisonError::into_inner).read(now, false, 0).0.map_or(0, |b| b.sample_index_at(now))
}
/// Stop the route `index` of `port`. Its notes end one by one; its pedal up and All Notes Off are
/// not sent on a channel where MIDI Thru holds a key or the pedal, which the player still holds
/// (a route replaced by an offset change, say). All MIDI Notes Off, and a port that closes, end
/// Thru's notes too.
fn clear_route<C: SyncPort>(name: &str, port: &mut Port<C>, index: usize, stats: &Stats) {
    port.future.retain(|event| event.route != index);
    if let Some(mut route) = port.routes[index].take() {
        route.encoder.stop(&mut |msg| {
            if let [status, 123, ..] = msg
                && status & 0xf0 == 0xb0
                && (port.thru_keys[usize::from(status & 0x0f)] != 0 || port.thru_pedal & (1 << (status & 0x0f)) != 0)
            {
                return;
            }
            send_now(name, port, msg, stats);
        });
    }
}
/// The port `name` was opened again: send the clips' quieting now ([`SyncEncoder::quiet`]),
/// before MIDI Thru plays there. Run after [`LiveThru::reopened`], which lets go of what Thru held.
/// A send that fails closes the port; the encoders' next [`SyncEncoder::reset`] keeps what was
/// not delivered to quiet on the next reopen.
fn quiet_clips<C: SyncPort>(name: &str, p: &mut Port<C>, stats: &Stats) {
    let mut routes = std::mem::replace(&mut p.routes, std::array::from_fn(|_| None));
    for r in routes.iter_mut().flatten() {
        r.encoder.quiet(&mut |msg| send_now(name, p, msg, stats));
    }
    p.routes = routes;
}
/// Below this, the last stretch of a wait is one plain sleep.
const FINE_SLEEP: Duration = Duration::from_micros(200);

/// Sleep until `at`, or until live MIDI input is waiting for thru (`thru`): the input threads
/// unpark the worker. A system may wake a sleeping thread late by a share of the time asked for
/// (on the macOS test machine half of it: a 4 ms sleep took 6 ms), so sleep in halving steps;
/// the last one is at most [`FINE_SLEEP`] long.
fn sleep_until(at: Instant, thru: Option<&MidiFeed>) {
    loop {
        if thru.is_some_and(MidiFeed::thru_pending) {
            return;
        }
        let left = at.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return;
        }
        std::thread::park_timeout(if left <= FINE_SLEEP { left } else { left / 2 });
    }
}

/// With a timed-out transport command the worker can be woken by either a control
/// message or the first changed presentation. The thread park token survives a
/// publication between reading the channel and entering the wait.
///
/// With live input attached for thru (`thru`), the worker never blocks on the channel alone: it
/// parks, which both a control message and a live message end.
fn wait_for_update(
    rx: &Receiver<Update>,
    next: Option<Instant>,
    connected: bool,
    wait_for_late_block: bool,
    thru: Option<&MidiFeed>,
) -> Result<Option<Update>, ()> {
    if let Some(at) = next {
        let delay = at.saturating_duration_since(Instant::now());
        if !connected && delay > Duration::from_millis(100) {
            return match rx.recv_timeout((delay - Duration::from_millis(30)).min(Duration::from_millis(250))) {
                Ok(update) => Ok(Some(update)),
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => Ok(None),
                Err(_) => Err(()),
            };
        }
        // Keep native sub-millisecond precision. Check control and shutdown after at
        // most 10 ms of requested sleep; distant events stay queued across checks.
        sleep_until(Instant::now() + delay.min(Duration::from_millis(10)), thru);
    } else if !wait_for_late_block && thru.is_none() {
        return rx.recv().map(Some).map_err(|_| ());
    }
    let receive = || match rx.try_recv() {
        Ok(update) => Ok(Some(update)),
        Err(std::sync::mpsc::TryRecvError::Empty) => Ok(None),
        Err(_) => Err(()),
    };
    let update = receive()?;
    if update.is_none() && next.is_none() && !thru.is_some_and(MidiFeed::thru_pending) {
        std::thread::park();
        // A single notification can mean either a published block or a queued
        // command. Consume the command before polling the timeline again.
        return receive();
    }
    Ok(update)
}

/// Names every failing destination, not only the first, so several unavailable
/// ports all appear in the status line and diagnostics. Reuses `buffer` so the
/// worker loop does not allocate while the failures stay the same.
fn join_failures<'a, C>(ports: &BTreeMap<String, Port<C>>, buffer: &'a mut String) -> Option<&'a str> {
    buffer.clear();
    for p in ports.values() {
        let Some(err) = p.failure.as_deref().or_else(|| p.routes.iter().flatten().find_map(|r| r.encoder.error())) else { continue };
        if !buffer.split(FAILURE_SEPARATOR).any(|known| known == err) {
            if !buffer.is_empty() {
                buffer.push_str(FAILURE_SEPARATOR);
            }
            buffer.push_str(err);
        }
    }
    (!buffer.is_empty()).then_some(buffer.as_str())
}

/// Once a second, close an open port that is no longer `listed` (when the list is known); open a
/// closed one when its retry is due, resetting its encoders. A port closed because its label is
/// now bound to another port may still be there: its encoders release their notes on it first
/// (sent to a port that is gone, they go nowhere). The reopen still quiets what they played.
fn keep_open<C: SyncPort>(
    name: &str,
    p: &mut Port<C>,
    listed: Option<&BTreeSet<String>>,
    now: Instant,
    stats: &Stats,
    open: &mut impl ports::Connect<C>,
) {
    if p.link.is_some() && now >= p.retry {
        if listed.is_some_and(|listed| !listed.contains(name)) {
            p.future.clear();
            let mut routes = std::mem::replace(&mut p.routes, std::array::from_fn(|_| None));
            for r in routes.iter_mut().flatten() {
                r.encoder.stop(&mut |msg| send_now(name, p, msg, stats));
            }
            p.routes = routes;
            p.disconnect(name, format!("MIDI output {name} unavailable"), now, stats);
            // Try again at once: a port back under a new ID is listed already.
            p.retry = now;
        } else {
            p.retry = now + Duration::from_secs(1);
        }
    }
    if p.link.is_none() && now >= p.retry {
        match open.open(name) {
            Ok((out, warn)) => {
                if p.opened {
                    log::info!("MIDI output {name}: reconnected");
                } else {
                    log::info!("MIDI output: {name}");
                }
                stats.connected().insert(name.to_owned());
                p.link = Some(out);
                p.opened = true;
                p.failure = warn;
                for r in p.routes.iter_mut().flatten() {
                    r.encoder.reset();
                }
            }
            Err(e) => {
                if p.failure.as_ref() != Some(&e) {
                    if e.contains(name) {
                        log::warn!("{e}");
                    } else {
                        log::warn!("MIDI output {name}: {e}");
                    }
                }
                p.failure = Some(e);
            }
        }
        p.retry = now + Duration::from_secs(1);
    }
}

/// [`drive_ports`] with ports that are opened by `open` and never checked against a port list.
#[cfg(test)]
fn drive<C: SyncPort>(
    view: View,
    session: Arc<Session>,
    rx: Receiver<Update>,
    trouble: Arc<Mutex<Option<String>>>,
    stats: Arc<Stats>,
    open: impl FnMut(&str) -> Result<(C, Option<String>), String>,
) {
    drive_ports(view, session, rx, trouble, stats, open);
}

/// The worker: open each routed port through `open`, keep it open while it is listed, and send
/// what its encoders produce at their deadlines until the [`Output`] goes away.
fn drive_ports<C: SyncPort>(
    view: View,
    mut session: Arc<Session>,
    rx: Receiver<Update>,
    trouble: Arc<Mutex<Option<String>>>,
    stats: Arc<Stats>,
    mut open: impl ports::Connect<C>,
) {
    // Use a safe best-effort API. Hosts that deny a negative nice level keep
    // their normal scheduling policy and expose that in the diagnostics.
    let priority = thread_priority::ThreadPriorityValue::try_from(68u8)
        .ok()
        .is_some_and(|value| thread_priority::ThreadPriority::Crossplatform(value).set_for_current().is_ok());
    stats.priority.store(if priority { 1 } else { 2 }, Relaxed);
    let mut ports = BTreeMap::<String, Port<C>>::new();
    let mut awake = Instant::now();
    let mut transition = None;
    let mut wait_for_late_block = false;
    let state = || view.lock().unwrap_or_else(std::sync::PoisonError::into_inner).transport_state();
    let mut update = None;
    let mut pending = Vec::<Event>::new();
    let mut failures = String::new();
    let mut live = LiveThru::default();
    loop {
        #[cfg(test)]
        stats.iterations.fetch_add(1, Relaxed);
        let now = Instant::now();
        if let Some(u) = update.take() {
            // Local transport and setup commands may precede the callback that publishes
            // their new timeline.
            awake = now + Duration::from_millis(150);
            match u {
                Update::Session(s) => session = s,
                Update::Feed(next) => live.set_feed(next, &mut ports, &stats),
                Update::Routes(kind, dest, c, make) => {
                    let index = kind.index();
                    for (name, p) in &mut ports {
                        if !dest.iter().any(|(n, _)| n == name) {
                            clear_route(name, p, index, &stats);
                        }
                    }
                    for (n, o) in dest {
                        if ports.get(&n).and_then(|p| p.routes[index].as_ref()).is_some_and(|r| r.offset == o && r.compensate == c) {
                            continue;
                        }
                        if let Some(p) = ports.get_mut(&n) {
                            clear_route(&n, p, index, &stats);
                        }
                        let encoder = make(&n);
                        set_route(&mut ports, index, n, Route { map: None, offset: o, compensate: c, encoder });
                    }
                }
                Update::Panic(messages) => {
                    live.release_all(&mut ports, &stats);
                    for (name, p) in &mut ports {
                        p.future.retain(|e| releases(&e.bytes[..e.len]));
                        let mut routes = std::mem::replace(&mut p.routes, std::array::from_fn(|_| None));
                        for r in routes.iter_mut().flatten() {
                            r.encoder.panic(&mut |msg| send_now(name, p, msg, &stats));
                        }
                        p.routes = routes;
                        messages(&mut |msg| send_now(name, p, msg, &stats));
                    }
                }
                Update::Wake(before) => transition = Some((before, now + Duration::from_secs(2))),
            }
            live.release_closing(&mut ports, &stats);
            ports.retain(|_, p| p.routes.iter().any(Option::is_some));
            stats.connected().retain(|n| ports.contains_key(n));
            for p in ports.values_mut() {
                if !same_feed(p.echo.as_ref(), live.feed.as_ref()) {
                    p.echo.clone_from(&live.feed);
                }
            }
        }
        let mut next = None;
        let mut soon = |at: Instant| next = Some(next.map_or(at, |t: Instant| t.min(at)));
        // Sample before polling so a callback racing this iteration cannot cancel
        // the final read of its newly published transport state.
        if wait_for_late_block {
            wait_for_late_block = view.lock().unwrap_or_else(std::sync::PoisonError::into_inner).late_transport_waiter.is_some();
        }
        let observed_state = transition.and_then(|_| state());
        live.service(&mut ports, &stats, &session, &view, now);
        // A removed device gives no send error (CoreMIDI drops what is sent to a destination
        // that is gone), so once a second the open ports are checked against the port list.
        let due: BTreeSet<String> = ports.iter().filter(|(_, p)| p.link.is_some() && now >= p.retry).map(|(name, _)| name.clone()).collect();
        let listed = if due.is_empty() { None } else { open.listed(&due) };
        for (name, p) in &mut ports {
            let was_open = p.link.is_some();
            // Closed below as no longer listed, or as its label is bound to another port now,
            // which keep_open may open at once.
            let leaving = was_open && now >= p.retry && listed.as_ref().is_some_and(|l| !l.contains(name));
            if leaving {
                live.release_leaving(name, p, &stats);
            }
            keep_open(name, p, listed.as_ref(), now, &stats, &mut open);
            if (leaving || !was_open) && p.link.is_some() {
                live.reopened(name, p, &stats);
                quiet_clips(name, p, &stats);
            }
            if p.link.is_some() && open.lists() {
                soon(p.retry);
            }
            if let Some(at) = p.poll(name, now, &session, &view, &stats, &mut pending) {
                soon(at);
            }
        }
        if let Some(generation) = ports
            .values()
            .filter(|p| p.link.is_some())
            .flat_map(|p| p.routes.iter().flatten())
            .map(|route| route.map.as_ref().map_or(0, |r| r.generation))
            .min()
        {
            view.lock().unwrap_or_else(std::sync::PoisonError::into_inner).retire_before(generation);
        }
        {
            let err = join_failures(&ports, &mut failures);
            let mut old = trouble.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            if old.as_deref() != err {
                *old = err.map(str::to_owned);
            }
        }
        ports.retain(|name, p| {
            let keep = p.routes.iter().any(Option::is_some);
            if !keep {
                stats.connected().remove(name);
            }
            keep
        });
        let connected = ports.values().any(|p| p.link.is_some());
        if let Some((previous, deadline)) = transition {
            if !connected || observed_state != previous {
                transition = None;
            } else if now >= deadline {
                let mut timeline = view.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                if timeline.transport_state() == previous {
                    stats.pending_transition_timeouts.fetch_add(1, Relaxed);
                    timeline.late_transport_waiter = Some(std::thread::current());
                    wait_for_late_block = true;
                } else {
                    // The callback published between polling and arming; read it immediately.
                    soon(now);
                }
                transition = None;
            }
        }
        if connected && (now < awake || transition.is_some()) {
            soon(now + Duration::from_millis(1));
        }
        let waiting_for_timing = ports.values().any(|p| p.waiting_for_timing);
        match wait_for_update(&rx, next, connected, wait_for_late_block || waiting_for_timing, live.feed.as_deref()) {
            Ok(next_update) => update = next_update,
            Err(()) => break,
        }
    }
    view.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clear_waiters();
    live.set_feed(None, &mut ports, &stats);
    for (name, p) in &mut ports {
        for index in 0..ROUTE_KINDS {
            clear_route(name, p, index, &stats);
        }
    }
}

#[cfg(test)]
#[path = "sync_output_tests.rs"]
mod tests;
