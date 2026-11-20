//! One native worker owns all sync protocols and one connection per exact port name.
use crate::{
    Cmd, MidiSyncStatus, Session,
    beat_clock::Cursor,
    sync_clock::{CallbackMetrics, Publisher, SyncEncoder, View},
    sync_status::PortTiming,
};
use midir::{MidiOutput, MidiOutputConnection};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicU64, Ordering::Relaxed},
    mpsc::{Receiver, Sender, channel},
};
use std::time::{Duration, Instant};

pub(crate) fn output_names() -> Result<Vec<String>, String> {
    std::panic::catch_unwind(|| {
        let midi = MidiOutput::new("SoundCraft MIDI sync").map_err(|e| e.to_string())?;
        Ok(midi.ports().into_iter().filter_map(|p| midi.port_name(&p).ok()).collect::<BTreeSet<_>>().into_iter().collect())
    })
    .map_err(|_| "MIDI output enumeration failed".to_owned())?
}

/// Extension point for OS-timestamped MIDI backends. The native midir backend
/// waits with Rust's high-resolution sleep and records the immediate send time.
trait SyncPort: Send {
    fn send_at(&mut self, at: Instant, bytes: &[u8]) -> Result<Instant, String>;
}
impl SyncPort for MidiOutputConnection {
    fn send_at(&mut self, at: Instant, bytes: &[u8]) -> Result<Instant, String> {
        if let Some(wait) = at.checked_duration_since(Instant::now()) {
            std::thread::sleep(wait);
        }
        let handed = Instant::now();
        self.send(bytes).map_err(|e| e.to_string())?;
        Ok(handed)
    }
}
fn open_unique(name: &str) -> Result<(MidiOutputConnection, Option<String>), String> {
    let midi = MidiOutput::new("SoundCraft MIDI sync").map_err(|e| e.to_string())?;
    let mut matches = midi.ports().into_iter().filter(|p| midi.port_name(p).is_ok_and(|n| n == name));
    let port = matches.next().ok_or_else(|| format!("MIDI output {name} unavailable"))?;
    let warning = matches.next().map(|_| format!("Multiple MIDI outputs named {name}; using the first"));
    let connection = midi.connect(&port, "SoundCraft MIDI sync").map_err(|e| e.to_string())?;
    Ok((connection, warning))
}
#[derive(Default)]
struct Stats {
    connected: Mutex<BTreeSet<String>>,
    priority: AtomicU64,
    sent: AtomicU64,
    relaunches: AtomicU64,
    catch_up_sends: AtomicU64,
    stale_dropped: AtomicU64,
    buckets: [AtomicU64; 7],
    max: AtomicU64,
}
impl Stats {
    fn connected(&self) -> std::sync::MutexGuard<'_, BTreeSet<String>> {
        self.connected.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }
    fn record(&self, at: Instant, handed: Instant) {
        let us = handed.saturating_duration_since(at).as_micros() as u64;
        let bucket = BOUNDS.partition_point(|&v| v < us);
        self.buckets[bucket].fetch_add(1, Relaxed);
        self.sent.fetch_add(1, Relaxed);
        self.max.fetch_max(us, Relaxed);
    }
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
#[derive(Default)]
pub(crate) struct Output {
    view: View,
    metrics: Arc<CallbackMetrics>,
    session: Option<Arc<Session>>,
    beat: Vec<(String, i64)>,
    compensated: bool,
    additional: Option<(String, i64, bool)>,
    stats: Arc<Stats>,
    trouble: Arc<Mutex<Option<String>>>,
    wake_tx: Arc<Mutex<Option<Sender<Update>>>>,
    pending_wake: Arc<AtomicBool>,
    worker: Option<(Sender<Update>, std::thread::JoinHandle<()>)>,
}
enum Update {
    Session(Arc<Session>),
    Beats(Vec<(String, i64)>, bool),
    Encoder(Option<(String, i64)>, bool, Box<dyn SyncEncoder>),
    Wake,
    InputWake(Arc<AtomicBool>),
}
impl Output {
    pub(crate) fn configure(&mut self, s: &Arc<Session>, audio: &Sender<Cmd>) {
        if self.session.is_none() {
            let _ = audio.send(Cmd::Clock(Some(Publisher::new(self.view.clone(), self.metrics.clone()))));
        }
        self.session = Some(s.clone());
        if let Some((tx, _)) = &self.worker {
            let _ = tx.send(Update::Session(s.clone()));
        }
    }
    fn send(&mut self, u: Update) {
        if self.worker.is_none() {
            let Some(s) = self.session.clone() else { return };
            let (tx, rx) = channel();
            let view = self.view.clone();
            let trouble = self.trouble.clone();
            let stats = self.stats.clone();
            match std::thread::Builder::new().name("MIDI sync output".into()).spawn(move || drive(view, s, rx, trouble, stats, open_unique)) {
                Ok(thread) => {
                    *self.wake_tx.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(tx.clone());
                    self.worker = Some((tx, thread));
                }
                Err(e) => {
                    *self.trouble.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(e.to_string());
                    return;
                }
            }
        }
        if let Some((tx, _)) = &self.worker {
            let _ = tx.send(u);
        }
    }
    pub(crate) fn beats(&mut self, ports: Vec<(String, i64)>, compensate: bool) {
        if self.beat == ports && self.compensated == compensate {
            return;
        }
        self.beat = ports.clone();
        self.compensated = compensate;
        if !ports.is_empty() || self.worker.is_some() {
            self.send(Update::Beats(ports, compensate));
        }
    }
    pub(crate) fn encoder(&mut self, port: Option<(String, i64)>, compensate: bool, encoder: Box<dyn SyncEncoder>) {
        self.additional = port.clone().map(|(n, o)| (n, o, compensate));
        self.send(Update::Encoder(port, compensate, encoder));
    }
    pub(crate) fn waker(&self) -> Arc<dyn Fn() + Send + Sync> {
        let dest = self.wake_tx.clone();
        let pending = self.pending_wake.clone();
        Arc::new(move || {
            if pending.load(Relaxed) {
                return;
            }
            if let Some(tx) = dest.lock().unwrap_or_else(std::sync::PoisonError::into_inner).as_ref()
                && !pending.swap(true, Relaxed)
                && tx.send(Update::InputWake(pending.clone())).is_err()
            {
                pending.store(false, Relaxed);
            }
        })
    }
    pub(crate) fn wake(&self) {
        if let Some((tx, _)) = &self.worker {
            let _ = tx.send(Update::Wake);
        }
    }
    pub(crate) fn trouble(&self) -> Option<String> {
        self.trouble.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone()
    }
    pub(crate) fn presented(&self) -> Option<i64> {
        let now = Instant::now();
        self.view.lock().unwrap_or_else(std::sync::PoisonError::into_inner).read(now, true, 0.).0.map(|b| b.sample_index_at(now))
    }
    pub(crate) fn status(&self) -> MidiSyncStatus {
        let buckets = self.stats.buckets.each_ref().map(|v| v.load(Relaxed));
        let max = self.stats.max.load(Relaxed);
        let lead = self.metrics.lead_us.load(Relaxed);
        let sr = self.session.as_ref().map_or(48_000., |s| s.sample_rate.as_f64());
        let total =
            |offset: i64, compensate: bool| (if compensate { lead as i64 } else { 0 }).saturating_add((offset as f64 * 1e6 / sr).round() as i64);
        let connected = self.stats.connected();
        let mut ports = BTreeMap::new();
        for (name, offset) in &self.beat {
            ports.insert(
                name.clone(),
                PortTiming {
                    connected: connected.contains(name),
                    offset_samples: *offset,
                    beat_clock_total_us: Some(total(*offset, self.compensated)),
                    mtc_total_us: None,
                },
            );
        }
        if let Some((name, offset, compensate)) = &self.additional {
            ports
                .entry(name.clone())
                .or_insert(PortTiming { connected: connected.contains(name), offset_samples: *offset, beat_clock_total_us: None, mtc_total_us: None })
                .mtc_total_us = Some(total(*offset, *compensate));
        }
        MidiSyncStatus {
            sent: self.stats.sent.load(Relaxed),
            relaunches: self.stats.relaunches.load(Relaxed),
            catch_up_sends: self.stats.catch_up_sends.load(Relaxed),
            stale_dropped: self.stats.stale_dropped.load(Relaxed),
            lateness_buckets: buckets,
            lateness_p50_us: percentile(buckets, 50, max),
            lateness_p95_us: percentile(buckets, 95, max),
            lateness_max_us: max,
            reported_lead_us: lead,
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
            let _ = worker.join();
        }
    }
}

struct Route {
    offset: i64,
    compensate: bool,
    encoder: Box<dyn SyncEncoder>,
}
struct Port<C> {
    link: Option<C>,
    retry: Instant,
    routes: [Option<Route>; 2],
    failure: Option<String>,
}
struct Event {
    at: Instant,
    len: usize,
    bytes: [u8; 10],
}
impl Event {
    fn new(at: Instant, msg: &[u8]) -> Option<Self> {
        let mut bytes = [0; 10];
        bytes.get_mut(..msg.len())?.copy_from_slice(msg);
        Some(Self { at, len: msg.len(), bytes })
    }
}
impl<C: SyncPort> Port<C> {
    fn poll(&mut self, name: &str, now: Instant, session: &Session, view: &View, stats: &Stats, pending: &mut Vec<Event>) -> Option<Instant> {
        let Some(port) = &mut self.link else { return Some(self.retry) };
        let mut next = None;
        let mut soon = |at: Instant| next = Some(next.map_or(at, |t: Instant| t.min(at)));
        pending.clear();
        for r in self.routes.iter_mut().flatten() {
            let (block, future) = view.lock().unwrap_or_else(std::sync::PoisonError::into_inner).read(
                now,
                r.compensate,
                r.offset as f64 / session.sample_rate.as_f64(),
            );
            if let Some(at) = future {
                soon(at);
            }
            if let Some(b) = block {
                if let Some(at) = r.encoder.update(b, now, session, &mut |at, m| {
                    if let Some(e) = Event::new(at, m) {
                        pending.push(e);
                    }
                }) {
                    soon(at);
                }
                stats.catch_up_sends.fetch_add(r.encoder.take_catchups(), Relaxed);
                stats.stale_dropped.fetch_add(r.encoder.take_stale_dropped(), Relaxed);
            }
        }
        pending.sort_by_key(|m| m.at);
        for e in pending.iter() {
            match port.send_at(e.at, &e.bytes[..e.len]) {
                Ok(handed) => {
                    stats.record(e.at, handed);
                    if matches!(e.bytes[0], 0xf0 | 0xf2) {
                        stats.relaunches.fetch_add(1, Relaxed);
                    }
                }
                Err(err) => {
                    self.failure = Some(format!("{name}: {err}"));
                    stats.connected().remove(name);
                    self.link = None;
                    self.retry = now + Duration::from_secs(1);
                    soon(self.retry);
                    break;
                }
            }
        }
        next
    }
}
fn set_route<C: SyncPort>(map: &mut BTreeMap<String, Port<C>>, index: usize, name: String, route: Route) {
    let port = map.entry(name).or_insert_with(|| Port { link: None, retry: Instant::now(), routes: [None, None], failure: None });
    port.routes[index] = Some(route);
}
fn clear_route<C: SyncPort>(port: &mut Port<C>, index: usize) {
    if let Some(mut old) = port.routes[index].take()
        && let Some(out) = &mut port.link
    {
        old.encoder.stop(&mut |m| {
            let _ = out.send_at(Instant::now(), m);
        });
    }
}
fn drive<C: SyncPort>(
    view: View,
    mut session: Arc<Session>,
    rx: Receiver<Update>,
    trouble: Arc<Mutex<Option<String>>>,
    stats: Arc<Stats>,
    mut open: impl FnMut(&str) -> Result<(C, Option<String>), String>,
) {
    // Use a safe best-effort API. Hosts that deny a negative nice level keep
    // their normal scheduling policy and expose that in the diagnostics.
    let priority = thread_priority::ThreadPriorityValue::try_from(68u8)
        .ok()
        .is_some_and(|value| thread_priority::ThreadPriority::Crossplatform(value).set_for_current().is_ok());
    stats.priority.store(if priority { 1 } else { 2 }, Relaxed);
    let mut ports = BTreeMap::<String, Port<C>>::new();
    let mut awake = Instant::now();
    let mut update = None;
    let mut pending = Vec::<Event>::new();
    loop {
        let now = Instant::now();
        if let Some(u) = update.take() {
            awake = now + Duration::from_millis(150);
            match u {
                Update::Session(s) => {
                    for r in ports.values_mut().flat_map(|p| p.routes.iter_mut().flatten()) {
                        r.encoder.remap(&session, &s);
                    }
                    session = s;
                }
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
                        set_route(&mut ports, 0, n, Route { offset: o, compensate: c, encoder: Box::<Cursor>::default() });
                    }
                }
                Update::Encoder(dest, c, encoder) => {
                    for p in ports.values_mut() {
                        clear_route(p, 1);
                    }
                    if let Some((n, o)) = dest {
                        set_route(&mut ports, 1, n, Route { offset: o, compensate: c, encoder });
                    }
                }
                Update::Wake => {}
                Update::InputWake(pending) => pending.store(false, Relaxed),
            }
            ports.retain(|_, p| p.routes.iter().any(Option::is_some));
            stats.connected().retain(|n| ports.contains_key(n));
        }
        let mut next = None;
        let mut soon = |at: Instant| next = Some(next.map_or(at, |t: Instant| t.min(at)));
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
        {
            let err = ports.values().find_map(|p| p.failure.as_deref().or_else(|| p.routes.iter().flatten().find_map(|r| r.encoder.error())));
            let mut old = trouble.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            if old.as_deref() != err {
                *old = err.map(str::to_owned);
            }
        }
        if now < awake {
            soon(now + Duration::from_millis(1));
        }
        if let Some(at) = next {
            let delay = at.saturating_duration_since(Instant::now());
            // Only sleep is used for protocol deadlines. A long reconnect wait is
            // interruptible by control changes. Recheck every 10 ms during playback.
            if ports.values().any(|p| p.link.is_some()) || delay <= Duration::from_millis(100) {
                std::thread::sleep(delay.min(Duration::from_millis(10)));
                match rx.try_recv() {
                    Ok(u) => update = Some(u),
                    Err(std::sync::mpsc::TryRecvError::Empty) => {}
                    Err(_) => break,
                }
            } else {
                match rx.recv_timeout((delay - Duration::from_millis(30)).min(Duration::from_millis(250))) {
                    Ok(u) => update = Some(u),
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                    Err(_) => break,
                }
            }
        } else {
            match rx.recv() {
                Ok(u) => update = Some(u),
                Err(_) => break,
            }
        }
    }
    for p in ports.values_mut() {
        clear_route(p, 0);
        clear_route(p, 1);
    }
}

#[cfg(test)]
#[path = "sync_output_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "sync_timing_test.rs"]
mod timing;

#[cfg(test)]
#[path = "mtc_sync_output_tests.rs"]
mod mtc_on_shared_port;
