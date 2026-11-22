//! A shared scheduler timestamps sync traffic. Each named native port has one ordered delivery thread.
use crate::sync_clock::{SessionRevision, delivery_offset};
use crate::{
    Cmd, MidiSyncStatus, Session,
    beat_clock::Cursor,
    sync_clock::{CallbackMetrics, Publisher, SyncEncoder, View},
    sync_status::PortTiming,
};
mod ports;
use midir::MidiOutputConnection;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicU64, Ordering::Relaxed},
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
/// The scheduler can submit a small batch of due packets. A native port
/// acknowledges actual handoffs on its own thread. While a batch is outstanding
/// only that destination pauses; reconnection retries unacknowledged cleanup.
trait Destination: Send {
    fn submit(&mut self, at: Instant, bytes: &[u8], retry: bool, stats: &Stats) -> Result<(), String>;
    fn ready(&mut self, _cleanup: &mut VecDeque<Vec<u8>>) -> Result<bool, String> {
        Ok(true)
    }
    fn flush(&mut self) {}
    fn busy(&self) -> bool {
        false
    }
}
impl<C: SyncPort> Destination for C {
    fn submit(&mut self, at: Instant, bytes: &[u8], retry: bool, stats: &Stats) -> Result<(), String> {
        let handed = self.send_at(at, bytes)?;
        stats.delivered(at, handed, bytes, retry);
        Ok(())
    }
}

mod port_io;
use port_io::PortIo;

#[derive(Default)]
struct Stats {
    #[cfg(test)]
    iterations: AtomicU64,
    connected: Mutex<BTreeSet<String>>,
    priority: AtomicU64,
    sent: AtomicU64,
    pending_transition_timeouts: AtomicU64,
    oversized_dropped: AtomicU64,
    relaunches: AtomicU64,
    catch_up_sends: AtomicU64,
    stale_dropped: AtomicU64,
    session_backlog_drops: AtomicU64,
    buckets: [AtomicU64; 7],
    max: AtomicU64,
}
impl Stats {
    fn connected(&self) -> std::sync::MutexGuard<'_, BTreeSet<String>> {
        self.connected.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }
    fn delivered(&self, at: Instant, handed: Instant, bytes: &[u8], cleanup_or_stop: bool) {
        self.record(at, handed);
        if !cleanup_or_stop && matches!(bytes.first(), Some(0xf0 | 0xf2)) {
            self.relaunches.fetch_add(1, Relaxed);
        }
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
const BOUNDS: [u64; 6] = [500, 1000, 2000, 5000, 10_000, 20_000];
fn percentile(buckets: [u64; 7], p: u64, max: u64) -> u64 {
    let target = buckets.iter().sum::<u64>().saturating_mul(p).div_ceil(100);
    if target == 0 {
        return 0;
    }
    let mut sum = 0;
    for (i, n) in buckets.iter().enumerate() {
        sum += n;
        if sum >= target {
            return BOUNDS.get(i).copied().unwrap_or(max);
        }
    }
    max
}
#[derive(Clone)]
struct WorkerSender {
    tx: Sender<Update>,
    worker: std::thread::Thread,
}
impl WorkerSender {
    fn send_coalesced(&self, pending: &Arc<AtomicBool>, update: impl FnOnce(Arc<AtomicBool>) -> Update) {
        if !pending.swap(true, Relaxed) && self.send(update(pending.clone())).is_err() {
            pending.store(false, Relaxed);
        }
    }
    fn send(&self, update: Update) -> Result<(), std::sync::mpsc::SendError<Update>> {
        self.tx.send(update)?;
        // A timed-out transition can park on a late presentation as well as on control.
        self.worker.unpark();
        Ok(())
    }
}
#[derive(Default)]
pub(crate) struct Output {
    port_registry: Arc<ports::Registry>,
    chase_feed: std::sync::Weak<crate::midi::MidiFeed>,
    view: View,
    metrics: Arc<CallbackMetrics>,
    session: Option<Arc<Session>>,
    generation: u64,
    beat: Vec<(String, i64)>,
    compensated: bool,
    beats_applied: bool,
    additional: Option<(String, i64, bool)>,
    stats: Arc<Stats>,
    trouble: Arc<Mutex<Option<String>>>,
    wake_tx: Arc<Mutex<Option<WorkerSender>>>,
    pending_wake: Arc<AtomicBool>,
    worker: Option<(WorkerSender, std::thread::JoinHandle<()>)>,
    retry_spawn_at: Option<Instant>,
}
enum Update {
    Session,
    Beats(Vec<(String, i64)>, bool),
    Encoder(Option<(String, i64)>, bool, Box<dyn SyncEncoder>),
    Wake(Option<(u64, bool)>),
    InputWake(Arc<AtomicBool>),
    TransportWake(Arc<AtomicBool>, Option<(u64, bool)>),
}
impl Output {
    pub(crate) fn restore_port_ids(&self, ids: &BTreeMap<String, String>) {
        self.port_registry.restore(ids);
    }
    pub(crate) fn opened_port_ids(&self) -> Option<BTreeMap<String, String>> {
        self.port_registry.opened()
    }
    pub(crate) fn configure(&mut self, s: &Arc<Session>, audio: &Sender<Cmd>, accepted_grid: Option<&Arc<Session>>) -> u64 {
        if self.session.is_none() {
            let _ = audio.send(Cmd::Clock(Some(Publisher::new(self.view.clone(), self.metrics.clone()))));
        }
        self.generation = self.generation.wrapping_add(1);
        let revision = SessionRevision { generation: self.generation, doc: s.clone(), remap: accepted_grid.is_none() };
        let connected = self.stats.connected();
        let active = !connected.is_empty();
        let mut timeline = self.view.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(grid) = accepted_grid {
            timeline.register_grid(revision, grid, active);
        } else if timeline.register(revision, active) {
            self.stats.session_backlog_drops.fetch_add(1, Relaxed);
        }
        drop(timeline);
        drop(connected);
        self.session = Some(s.clone());
        if let Some((tx, _)) = &self.worker {
            let _ = tx.send(Update::Session);
        }
        self.generation
    }

    /// The fixed audio snapshot pool can fill while every queued buffer has
    /// its own tempo. A host retry also reclaims maps if no MIDI worker is polling.
    pub(crate) fn reclaim_grid_snapshots(&self, accepted: &Arc<Session>) {
        let connected = self.stats.connected();
        let mut timeline = self.view.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        timeline.reclaim_grid_history(accepted, !connected.is_empty(), Instant::now());
        timeline.discard_unpresented_maps();
    }
    fn send(&mut self, u: Update) -> bool {
        if self.worker.is_none() {
            self.beats_applied = false;
            let Some(s) = self.session.clone() else { return false };
            if self.retry_spawn_at.is_some_and(|at| Instant::now() < at) {
                return false;
            }
            let (tx, rx) = channel();
            let view = self.view.clone();
            let trouble = self.trouble.clone();
            let stats = self.stats.clone();
            let registry = self.port_registry.clone();
            match std::thread::Builder::new().name("MIDI sync output".into()).spawn(move || {
                let port_stats = stats.clone();
                drive(view, s, rx, trouble, stats, move |name| {
                    let (connection, warning) = registry.connect(name)?;
                    Ok((PortIo::new(connection, port_stats.clone())?, warning))
                })
            }) {
                Ok(thread) => {
                    let tx = WorkerSender { tx, worker: thread.thread().clone() };
                    *self.wake_tx.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(tx.clone());
                    self.worker = Some((tx, thread));
                }
                Err(e) => {
                    *self.trouble.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(format!("MIDI sync worker could not start: {e}"));
                    self.retry_spawn_at = Some(Instant::now() + Duration::from_secs(1));
                    return false;
                }
            }
        }
        let queued = self.worker.as_ref().is_some_and(|(tx, _)| tx.send(u).is_ok());
        if !queued {
            self.worker = None;
            *self.wake_tx.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = None;
        }
        queued
    }
    pub(crate) fn beats(&mut self, ports: Vec<(String, i64)>, compensate: bool) -> bool {
        if self.beats_applied
            && self.worker.as_ref().is_some_and(|(_, task)| !task.is_finished())
            && self.beat == ports
            && self.compensated == compensate
        {
            return true;
        }
        self.beat = ports.clone();
        self.compensated = compensate;
        self.beats_applied = (ports.is_empty() && self.worker.is_none()) || self.send(Update::Beats(ports, compensate));
        self.beats_applied
    }
    pub(crate) fn encoder(&mut self, port: Option<(String, i64)>, compensate: bool, encoder: Box<dyn SyncEncoder>) -> bool {
        if port.is_none() && self.additional.is_none() && self.worker.is_none() {
            return true;
        }
        let accepted = self.send(Update::Encoder(port.clone(), compensate, encoder));
        if accepted {
            self.additional = port.map(|(name, offset)| (name, offset, compensate));
        }
        accepted
    }
    pub(crate) fn waker(&self) -> Arc<dyn Fn() + Send + Sync> {
        let dest = self.wake_tx.clone();
        let pending = self.pending_wake.clone();
        Arc::new(move || {
            if !pending.load(Relaxed)
                && let Some(tx) = dest.lock().unwrap_or_else(std::sync::PoisonError::into_inner).as_ref()
            {
                tx.send_coalesced(&pending, Update::InputWake);
            }
        })
    }
    pub(crate) fn attach_mtc_feed(&mut self, feed: &Arc<crate::midi::MidiFeed>) -> crate::midi::TransportWaker {
        self.chase_feed = Arc::downgrade(feed);
        self.transport_waker()
    }

    /// MidiFeed invokes this while it still holds the reader, before the callback
    /// can consume the new roll, stop or locate and publish its transport state.
    pub(crate) fn transport_waker(&self) -> crate::midi::TransportWaker {
        let dest = self.wake_tx.clone();
        let pending = Arc::<AtomicBool>::default();
        let view = self.view.clone();
        Arc::new(move |event| {
            if pending.load(Relaxed) {
                return;
            }
            if let Some(tx) = dest.lock().unwrap_or_else(std::sync::PoisonError::into_inner).as_ref() {
                let before = view.lock().unwrap_or_else(std::sync::PoisonError::into_inner).transport_state();
                if !matches!(
                    (event, before),
                    (crate::mtc_input::ChaseEvent::Roll(..), Some((_, true))) | (crate::mtc_input::ChaseEvent::Stop, Some((_, false)))
                ) {
                    tx.send_coalesced(&pending, |flag| Update::TransportWake(flag, before));
                }
            }
        })
    }
    pub(crate) fn send_transport(&self, audio: &Sender<Cmd>, command: Cmd) -> bool {
        let Some((tx, _)) = &self.worker else {
            return audio.send(command).is_ok();
        };
        // The callback may publish before the worker processes this message. Capture
        // its baseline before even enqueueing the corresponding audio command.
        let before = self.view.lock().unwrap_or_else(std::sync::PoisonError::into_inner).transport_state();
        let already_stopped = matches!(command, Cmd::Stop) && before.is_some_and(|(_, playing)| !playing);
        let queued = audio.send(command).is_ok();
        if queued && !already_stopped {
            let _ = tx.send(Update::Wake(before));
        }
        queued
    }
    pub(crate) fn reported_lead_us(&self) -> Option<u64> {
        self.metrics.lead().0
    }
    pub(crate) fn trouble(&self) -> Option<String> {
        self.trouble.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone()
    }
    pub(crate) fn presented(&self) -> Option<i64> {
        let now = Instant::now();
        self.view.lock().unwrap_or_else(std::sync::PoisonError::into_inner).read(now, true, 0).0.map(|b| b.sample_index_at(now))
    }
    pub(crate) fn clock_follow_sample(&self) -> Option<(u64, Instant, f64)> {
        self.view.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clock_follow_sample()
    }
    pub(crate) fn timing_issue(&self) -> Option<crate::OutputTimingIssue> {
        let clock = self.compensated && !self.beat.is_empty();
        let encoder = self.additional.as_ref().is_some_and(|(_, _, compensated)| *compensated);
        let chasing = self.chase_feed.upgrade().is_some_and(|feed| feed.mtc_online());
        if !clock && !encoder && !chasing {
            return None;
        }
        match self.metrics.lead() {
            (Some(lead_us), crate::OutputLeadSource::Retained) => Some(crate::OutputTimingIssue::Retained { lead_us }),
            (None, crate::OutputLeadSource::Unknown) => Some(crate::OutputTimingIssue::Unknown),
            _ => None,
        }
    }
    pub(crate) fn reported_timing_count(&self) -> u64 {
        self.metrics.fresh_lead_reports.load(Relaxed)
    }
    pub(crate) fn status(&self) -> MidiSyncStatus {
        let buckets = self.stats.buckets.each_ref().map(|v| v.load(Relaxed));
        let max = self.stats.max.load(Relaxed);
        let (lead, lead_source) = self.metrics.lead();
        let (sr, system_delay) = self
            .view
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .mixer_delay()
            .unwrap_or_else(|| (self.session.as_ref().map_or_else(soundcraft_time::SampleRate::default, |s| s.sample_rate), 0));
        let timing = |name: &str, offset, compensate| {
            let available = if compensate { lead } else { Some(0) };
            let delay = available.map(|us| delivery_offset(Duration::from_micros(us), system_delay, sr, compensate, offset));
            PortTiming {
                connected: self.stats.connected().contains(name),
                requested_offset_samples: offset,
                offset_samples: effective_offset(offset, sr),
                system_delay_samples: system_delay as u64,
                device_lead_us: lead,
                lead_source,
                beat_clock_enabled: false,
                mtc_enabled: false,
                advance_limited: delay.is_some_and(|(_, limited)| limited),
                effective_total_us: delay.map(|(total, _)| (total * 1e6).round() as i64),
                beat_clock_total_us: None,
                mtc_total_us: None,
            }
        };
        let mut ports = BTreeMap::new();
        for (name, offset) in &self.beat {
            let mut t = timing(name, *offset, self.compensated);
            t.beat_clock_enabled = true;
            t.beat_clock_total_us = t.effective_total_us;
            ports.insert(name.clone(), t);
        }
        if let Some((name, offset, compensate)) = &self.additional {
            let t = timing(name, *offset, *compensate);
            let total = t.effective_total_us;
            let limited = t.advance_limited;
            let entry = ports.entry(name.clone()).or_insert(t);
            entry.mtc_enabled = true;
            entry.mtc_total_us = total;
            entry.advance_limited |= limited;
        }
        MidiSyncStatus {
            sent: self.stats.sent.load(Relaxed),
            pending_transition_timeouts: self.stats.pending_transition_timeouts.load(Relaxed),
            oversized_dropped: self.stats.oversized_dropped.load(Relaxed),
            error: self.trouble(),
            session_backlog_drops: self.stats.session_backlog_drops.load(Relaxed),
            relaunches: self.stats.relaunches.load(Relaxed),
            catch_up_sends: self.stats.catch_up_sends.load(Relaxed),
            stale_dropped: self.stats.stale_dropped.load(Relaxed),
            lateness_buckets: buckets,
            lateness_p50_us: percentile(buckets, 50, max),
            lateness_p95_us: percentile(buckets, 95, max),
            lateness_max_us: max,
            reported_lead_us: lead,
            lead_source,
            callback_gap_max_us: self.metrics.max_gap_us.load(Relaxed),
            callback_gaps_over_two_buffers: self.metrics.late_buffers.load(Relaxed),
            lead_clamps: self.metrics.clamps.load(Relaxed),
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
        self.wake_tx.lock().unwrap_or_else(std::sync::PoisonError::into_inner).take();
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
    retry: Instant,
    routes: [Option<Route>; 2],
    future: Vec<Event>,
    cleanup: VecDeque<Vec<u8>>,
    timing_epochs: [u64; 2],
    waiting_for_timing: bool,
    failure: Option<String>,
}
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
impl<C: Destination> Port<C> {
    fn flush_final_cleanup(&mut self, stats: &Stats) {
        let Some(link) = &mut self.link else { return };
        while let Some(bytes) = self.cleanup.front() {
            if link.submit(Instant::now(), bytes, true, stats).is_err() {
                break;
            }
            self.cleanup.pop_front();
        }
        link.flush();
    }

    fn disconnect(&mut self, name: &str, error: String, now: Instant, stats: &Stats) {
        self.failure = Some(format!("{name}: {error}"));
        stats.connected().remove(name);
        self.link = None;
        self.future.clear();
        for route in self.routes.iter_mut().flatten() {
            route.map = None;
        }
        self.retry = now + Duration::from_secs(1);
    }

    fn poll(&mut self, name: &str, now: Instant, session: &Session, view: &View, stats: &Stats, pending: &mut Vec<Event>) -> Option<Instant> {
        self.waiting_for_timing = false;
        let Some(port) = &mut self.link else { return Some(self.retry) };
        match port.ready(&mut self.cleanup) {
            Ok(true) => {}
            Ok(false) => return Some(now + Duration::from_millis(1)),
            Err(error) => {
                self.disconnect(name, error, now, stats);
                return Some(self.retry);
            }
        }
        let mut next = None;
        let mut soon = |at: Instant| next = Some(next.map_or(at, |t: Instant| t.min(at)));
        while let Some(msg) = self.cleanup.front() {
            match port.submit(now, msg, true, stats) {
                Ok(()) => {
                    self.cleanup.pop_front();
                }
                Err(error) => {
                    self.disconnect(name, error, now, stats);
                    return Some(self.retry);
                }
            }
        }
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
            let (timing_epoch, block, future, revision, unconfigured) = {
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
                    && !(b.generation == 0 && timeline.unconfigured())
                    && !r.map.as_ref().is_some_and(|old| old.generation == b.generation)
                    && timeline.map(b.generation).is_none()
                {
                    timeline.wait_for_revision();
                    self.waiting_for_timing = true;
                }
                (timeline.timing_epoch, block, future, changed, timeline.unconfigured())
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
                    None if unconfigured && b.generation == 0 => session,
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
                        if self.future.len() == 256 {
                            self.future.remove(0);
                            self.failure = Some(format!("{name}: MIDI sync deadline queue is full"));
                        }
                        self.future.push(event);
                        soon(at);
                    }
                }) {
                    soon(at);
                } else if b.playing && !self.future.iter().any(|e| e.route == route_index) {
                    soon(now + Duration::from_millis(1));
                }
                let catchups = r.encoder.take_catchups();
                if catchups != 0 {
                    stats.catch_up_sends.fetch_add(catchups, Relaxed);
                }
                let stale = r.encoder.take_stale_dropped();
                if stale != 0 {
                    stats.stale_dropped.fetch_add(stale, Relaxed);
                }
            }
        }
        pending.sort_by_key(|m| m.at);
        for e in pending.iter() {
            let bytes = &e.bytes[..e.len];
            let retry = e.route == 0 && bytes == [craft_midi_sync::sync::STOP];
            if let Err(err) = port.submit(e.at, bytes, retry, stats) {
                if retry {
                    self.cleanup.push_back(bytes.to_vec());
                }
                self.disconnect(name, err, now, stats);
                return Some(self.retry);
            }
        }
        port.flush();
        if port.busy() {
            soon(now + Duration::from_millis(1));
        }
        next
    }
}
fn set_route<C: Destination>(map: &mut BTreeMap<String, Port<C>>, index: usize, name: String, route: Route) {
    let port = map.entry(name).or_insert_with(|| Port {
        link: None,
        retry: Instant::now(),
        routes: [None, None],
        future: Vec::new(),
        cleanup: Default::default(),
        timing_epochs: [0; 2],
        waiting_for_timing: false,
        failure: None,
    });
    port.routes[index] = Some(route);
}
fn clear_route<C>(port: &mut Port<C>, index: usize) {
    port.future.retain(|event| event.route != index);
    if let Some(mut route) = port.routes[index].take() {
        route.encoder.stop(&mut |msg| port.cleanup.push_back(msg.to_vec()));
    }
}
/// With a timed-out transport command the worker can be woken by either a control
/// message or the first changed presentation. The thread park token survives a
/// publication between reading the channel and entering the wait.
fn wait_for_update(rx: &Receiver<Update>, next: Option<Instant>, connected: bool, wait_for_late_block: bool) -> Result<Option<Update>, ()> {
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
        std::thread::sleep(delay.min(Duration::from_millis(10)));
    } else if !wait_for_late_block {
        return rx.recv().map(Some).map_err(|_| ());
    }
    let receive = || match rx.try_recv() {
        Ok(update) => Ok(Some(update)),
        Err(std::sync::mpsc::TryRecvError::Empty) => Ok(None),
        Err(_) => Err(()),
    };
    let update = receive()?;
    if update.is_none() && next.is_none() {
        std::thread::park();
        // A single notification can mean either a published block or a queued
        // command. Consume the command before polling the timeline again.
        return receive();
    }
    Ok(update)
}

fn raise_delivery_priority() -> bool {
    thread_priority::ThreadPriorityValue::try_from(68u8)
        .ok()
        .is_some_and(|value| thread_priority::ThreadPriority::Crossplatform(value).set_for_current().is_ok())
}

fn drive<C: Destination>(
    view: View,
    session: Arc<Session>,
    rx: Receiver<Update>,
    trouble: Arc<Mutex<Option<String>>>,
    stats: Arc<Stats>,
    mut open: impl FnMut(&str) -> Result<(C, Option<String>), String>,
) {
    stats.priority.store(if raise_delivery_priority() { 1 } else { 2 }, Relaxed);
    let mut ports = BTreeMap::<String, Port<C>>::new();
    let mut awake = Instant::now();
    let mut transition = None;
    let mut wait_for_late_block = false;
    let state = || view.lock().unwrap_or_else(std::sync::PoisonError::into_inner).transport_state();
    let mut update = None;
    let mut pending = Vec::<Event>::new();
    loop {
        #[cfg(test)]
        stats.iterations.fetch_add(1, Relaxed);
        let now = Instant::now();
        if let Some(u) = update.take() {
            // Input wakes recheck the presentation once. Local transport and setup
            // commands may precede the callback that publishes their new timeline.
            if !matches!(u, Update::InputWake(_)) {
                awake = now + Duration::from_millis(150);
            }
            match u {
                Update::Session => {}
                Update::Beats(dest, c) => {
                    for (name, p) in &mut ports {
                        if !dest.iter().any(|(n, _)| n == name) {
                            clear_route(p, 0);
                        }
                    }
                    for (n, o) in dest {
                        if ports.get(&n).and_then(|p| p.routes[0].as_ref()).is_some_and(|r| r.offset == o && r.compensate == c) {
                            continue;
                        }
                        if let Some(p) = ports.get_mut(&n) {
                            clear_route(p, 0);
                        }
                        set_route(&mut ports, 0, n, Route { map: None, offset: o, compensate: c, encoder: Box::<Cursor>::default() });
                    }
                }
                Update::Encoder(dest, c, encoder) => {
                    for p in ports.values_mut() {
                        clear_route(p, 1);
                    }
                    if let Some((n, o)) = dest {
                        set_route(&mut ports, 1, n, Route { map: None, offset: o, compensate: c, encoder });
                    }
                }
                Update::Wake(before) => transition = Some((before, now + Duration::from_secs(2))),
                Update::InputWake(pending) => pending.store(false, Relaxed),
                Update::TransportWake(pending, before) => {
                    pending.store(false, Relaxed);
                    transition = Some((before, now + Duration::from_secs(2)));
                }
            }
            ports.retain(|_, p| !p.cleanup.is_empty() || p.routes.iter().any(Option::is_some) || p.link.as_ref().is_some_and(Destination::busy));
            stats.connected().retain(|n| ports.contains_key(n));
        }
        let mut next = None;
        let mut soon = |at: Instant| next = Some(next.map_or(at, |t: Instant| t.min(at)));
        // Sample before polling so a callback racing this iteration cannot cancel
        // the final read of its newly published transport state.
        if wait_for_late_block {
            wait_for_late_block = view.lock().unwrap_or_else(std::sync::PoisonError::into_inner).late_transport_waiter.is_some();
        }
        let observed_state = transition.and_then(|_| state());
        for (name, p) in &mut ports {
            if p.link.is_none() && now >= p.retry {
                match open(name) {
                    Ok((out, warn)) => {
                        stats.connected().insert(name.clone());
                        p.link = Some(out);
                        p.failure = warn;
                        for r in p.routes.iter_mut().flatten() {
                            r.encoder.reset();
                        }
                    }
                    Err(e) => p.failure = Some(e),
                }
                p.retry = now + Duration::from_secs(1);
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
            let err = ports.values().find_map(|p| p.failure.as_deref().or_else(|| p.routes.iter().flatten().find_map(|r| r.encoder.error())));
            let mut old = trouble.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            if old.as_deref() != err {
                *old = err.map(str::to_owned);
            }
        }
        ports.retain(|name, p| {
            let keep = !p.cleanup.is_empty() || p.routes.iter().any(Option::is_some) || p.link.as_ref().is_some_and(Destination::busy);
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
        match wait_for_update(&rx, next, connected, wait_for_late_block || waiting_for_timing) {
            Ok(next_update) => update = next_update,
            Err(()) => break,
        }
    }
    view.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clear_waiters();
    stop_and_flush_ports(&mut ports, &stats);
}

fn stop_and_flush_ports<C: Destination>(ports: &mut BTreeMap<String, Port<C>>, stats: &Stats) {
    for p in ports.values_mut() {
        clear_route(p, 0);
        clear_route(p, 1);
        p.flush_final_cleanup(stats);
    }
    // Let healthy drivers hand off their final Stop before application exit.
    // All destinations share this deadline, including any stalled in the OS.
    let deadline = Instant::now() + Duration::from_millis(25);
    while Instant::now() < deadline {
        let mut outstanding = false;
        for p in ports.values_mut() {
            if let Some(link) = &mut p.link {
                let ready = link.ready(&mut p.cleanup);
                if !matches!(ready, Ok(false)) && !p.cleanup.is_empty() {
                    p.flush_final_cleanup(stats);
                    outstanding = true;
                } else {
                    outstanding |= link.busy();
                }
            }
        }
        if !outstanding {
            break;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
}

#[cfg(test)]
#[path = "sync_output_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "mtc_sync_output_tests.rs"]
mod mtc_on_shared_port;
