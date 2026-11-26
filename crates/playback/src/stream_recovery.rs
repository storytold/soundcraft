//! Recovery of a shared WASAPI output whose presentation timestamps went stale.
use crate::stream::{RenderGraph, start_stream};
use crate::{AudioState, Fault, Shared, build_format};
use cpal::traits::{DeviceTrait, HostTrait};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, TryLockError};
use std::time::{Duration, Instant};

const CHECK_DELAY: Duration = Duration::from_millis(500);
const PROBE_TIMEOUT: Duration = Duration::from_secs(3);
const MAX_ATTEMPTS: u8 = 3;
const PRESENCE_INTERVAL: Duration = Duration::from_secs(30);

#[derive(PartialEq, Eq, Debug)]
enum Decision {
    Wait,
    Probe,
    Adopt,
    Discard,
}

#[derive(Default)]
struct Retry {
    next: Option<Instant>,
    attempts: u8,
    healthy_since: Option<Instant>,
    last_lookup: Option<Instant>,
}

impl Retry {
    fn lookup<T>(&mut self, now: Instant, find: impl FnOnce() -> Option<T>) -> Option<T> {
        let exhausted = self.attempts >= MAX_ATTEMPTS;
        if exhausted && self.last_lookup.is_some_and(|last| now.saturating_duration_since(last) < PRESENCE_INTERVAL) {
            return None;
        }
        self.last_lookup = Some(now);
        let Some(device) = find() else {
            self.attempts = 0;
            return None;
        };
        // Once the fast budget is spent, each successful slow lookup may retry opening.
        self.attempts = (self.attempts + 1).min(MAX_ATTEMPTS);
        Some(device)
    }

    fn decide(&mut self, now: Instant, safe: bool, unhealthy: bool, probe: Option<ProbeState>) -> Decision {
        if unhealthy {
            self.healthy_since = None;
        } else {
            self.next = None;
            let since = self.healthy_since.get_or_insert(now);
            if now.saturating_duration_since(*since) >= Duration::from_secs(10) {
                self.attempts = 0;
            }
        }
        if !unhealthy || !safe {
            return if probe.is_some() { Decision::Discard } else { Decision::Wait };
        }
        match probe {
            Some(ProbeState::Fresh) => Decision::Adopt,
            Some(ProbeState::Failed) => Decision::Discard,
            Some(ProbeState::Waiting) => Decision::Wait,
            None => {
                let next = self.next.get_or_insert(now + CHECK_DELAY);
                if now < *next {
                    return Decision::Wait;
                }
                *next = now + Duration::from_secs(2u64.pow(u32::from(self.attempts.min(MAX_ATTEMPTS))));
                Decision::Probe
            }
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ProbeState {
    Waiting,
    Fresh,
    Failed,
}

/// Count only consecutive, genuinely reported timestamps from this stream. The audio
/// thread is the sole writer; packing the count and callback time makes them one observation.
struct Reports {
    start: Instant,
    latest: AtomicU64,
}

impl Reports {
    fn new(now: Instant) -> Self {
        Self { start: now, latest: AtomicU64::new(0) }
    }
    fn write(&self, now: Instant, valid: bool) {
        let millis = now.saturating_duration_since(self.start).as_millis() as u64;
        let prev = self.latest.load(Ordering::Relaxed);
        let recent = (millis + 1).saturating_sub(prev >> 8) <= CHECK_DELAY.as_millis() as u64;
        let streak = if !valid {
            0
        } else if recent {
            ((prev & 255) + 1).min(255)
        } else {
            1
        };
        self.latest.store(((millis + 1) << 8) | streak, Ordering::Release);
    }
    fn recent_streak(&self, now: Instant) -> Option<u8> {
        let sample = self.latest.load(Ordering::Acquire);
        let millis = (sample >> 8).checked_sub(1)?;
        (now.saturating_duration_since(self.start + Duration::from_millis(millis)) <= CHECK_DELAY).then_some(sample as u8)
    }
}

/// Only the currently selected callback renders. A candidate writes digital silence.
/// The control thread takes the very same gate only for the stopped handoff.
struct Route {
    selected: AtomicBool,
    reports: Reports,
}

impl Route {
    fn new(selected: bool, now: Instant) -> Arc<Self> {
        Arc::new(Self { selected: AtomicBool::new(selected), reports: Reports::new(now) })
    }
    fn hand_over<T>(&self, next: &Self, graph: &Mutex<T>, playing: &AtomicBool, failed: &AtomicBool, now: Instant) -> bool {
        let Some(_guard) = try_graph(graph) else {
            return false;
        };
        // guarded_block latches a panic under this same graph lock. Recheck it here so
        // an intervening callback cannot silently discard the error along with the old route.
        if failed.load(Ordering::Relaxed) || playing.load(Ordering::Acquire) || next.reports.recent_streak(now).is_none_or(|n| n < 8) {
            return false;
        }
        self.selected.store(false, Ordering::Release);
        next.selected.store(true, Ordering::Release);
        true
    }

    fn render<T>(&self, graph: &Mutex<T>, draw: impl FnOnce(&mut T)) -> bool {
        if !self.selected.load(Ordering::Acquire) {
            return false;
        }
        let Some(mut graph) = try_graph(graph) else { return false };
        if !self.selected.load(Ordering::Acquire) {
            return false;
        }
        draw(&mut graph);
        true
    }
}

// Audio and control threads must never wait for each other. Poisoning signals a previous
// unwind, not contention; an available graph still permits rendering or a stopped handoff.
fn try_graph<T>(graph: &Mutex<T>) -> Option<MutexGuard<'_, T>> {
    match graph.try_lock() {
        Ok(guard) => Some(guard),
        Err(TryLockError::Poisoned(error)) => Some(error.into_inner()),
        Err(TryLockError::WouldBlock) => None,
    }
}

struct SharedGraph {
    graph: Arc<Mutex<AudioState>>,
    route: Arc<Route>,
}

impl RenderGraph for SharedGraph {
    fn render(&mut self, draw: impl FnOnce(&mut AudioState)) -> bool {
        self.route.render(&self.graph, draw)
    }
    fn report(&self, now: Instant, valid: bool) {
        self.route.reports.write(now, valid);
    }
}

struct Candidate {
    stream: cpal::Stream,
    fault: Arc<Fault>,
    route: Arc<Route>,
}

impl Candidate {
    fn state(&self, now: Instant) -> ProbeState {
        if self.fault.gone.load(Ordering::Acquire) {
            return ProbeState::Failed;
        }
        if self.route.reports.recent_streak(now).is_some_and(|n| n >= 8) {
            return ProbeState::Fresh;
        }
        if now.saturating_duration_since(self.route.reports.start) >= PROBE_TIMEOUT {
            return ProbeState::Failed;
        }
        ProbeState::Waiting
    }
}

pub(crate) struct Recovery {
    endpoint: cpal::DeviceId,
    config: cpal::StreamConfig,
    format: cpal::SampleFormat,
    graph: Arc<Mutex<AudioState>>,
    current: Arc<Route>,
    candidate: Option<Candidate>,
    retry: Retry,
}

fn recoverable_endpoint(id: Option<cpal::DeviceId>) -> Option<cpal::DeviceId> {
    id.filter(|id| id.host() == cpal::HostId::Wasapi)
}

pub(crate) fn open(
    device: &cpal::Device,
    config: cpal::StreamConfig,
    format: cpal::SampleFormat,
    state: AudioState,
    fault: &Arc<Fault>,
    shared: &Arc<Shared>,
) -> Result<(cpal::Stream, Option<Recovery>), cpal::Error> {
    let Some(endpoint) = recoverable_endpoint(device.id().ok()) else {
        // ASIO keeps the original single-owner callback; an exclusive driver cannot
        // safely probe a second stream alongside the first.
        return build_format(device, config, format, state, fault, shared).map(|s| (s, None));
    };
    let graph = Arc::new(Mutex::new(state));
    let current = Route::new(true, Instant::now());
    let render = SharedGraph { graph: graph.clone(), route: current.clone() };
    let stream = build_format(device, config, format, render, fault, shared)?;
    let recovery = Recovery { endpoint, config, format, graph, current, candidate: None, retry: Retry::default() };
    Ok((stream, Some(recovery)))
}

impl Recovery {
    pub(crate) fn service(&mut self, safe: bool, shared: &Arc<Shared>, stream: &mut Option<cpal::Stream>, fault: &mut Arc<Fault>) -> bool {
        let now = Instant::now();
        let current = &self.current.reports;
        let unhealthy = fault.gone.load(Ordering::Acquire)
            || (now.saturating_duration_since(current.start) > CHECK_DELAY && current.recent_streak(now).unwrap_or_default() == 0);
        let safe = safe && !shared.playing.load(Ordering::Acquire) && !fault.failed.load(Ordering::Relaxed);
        match self.retry.decide(now, safe, unhealthy, self.candidate.as_ref().map(|c| c.state(now))) {
            Decision::Probe => self.probe(now, shared),
            Decision::Adopt => return self.adopt(shared, stream, fault),
            Decision::Discard => {
                self.candidate.take();
            }
            Decision::Wait => {}
        }
        false
    }

    fn probe(&mut self, now: Instant, shared: &Arc<Shared>) {
        // Enumerate the original endpoint by its opaque host ID. Absence is a lookup,
        // not an open attempt. Reappearance starts a fresh bounded retry budget.
        let Some(device) =
            self.retry.lookup(now, || cpal::host_from_id(self.endpoint.host()).ok().and_then(|host| host.device_by_id(&self.endpoint)))
        else {
            return;
        };
        let route = Route::new(false, Instant::now());
        let render = SharedGraph { graph: self.graph.clone(), route: route.clone() };
        let fault = Arc::new(Fault::default());
        match build_format(&device, self.config, self.format, render, &fault, shared) {
            Ok(stream) if start_stream(&stream) => self.candidate = Some(Candidate { stream, fault, route }),
            Err(error) => log::warn!("Cannot probe the original WASAPI output: {error}"),
            Ok(_) => {}
        }
    }

    fn adopt(&mut self, shared: &Shared, stream: &mut Option<cpal::Stream>, fault: &mut Arc<Fault>) -> bool {
        let Some(probe) = &self.candidate else {
            return false;
        };
        if probe.fault.gone.load(Ordering::Acquire)
            || !self.current.hand_over(&probe.route, &self.graph, &shared.playing, &fault.failed, Instant::now())
        {
            return false;
        }
        let Some(candidate) = self.candidate.take() else {
            return false;
        };
        self.current = candidate.route;
        *fault = candidate.fault;
        *stream = Some(candidate.stream);
        self.retry.next = None;
        log::info!("Restored original WASAPI output from fresh presentation timestamps");
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sync_clock::StreamLead;
    #[test]
    fn only_an_identifiable_wasapi_endpoint_gets_the_handoff_callback() {
        let wasapi = cpal::DeviceId::new(cpal::HostId::Wasapi, "original-endpoint");
        assert_eq!(recoverable_endpoint(Some(wasapi.clone())), Some(wasapi));
        assert_eq!(recoverable_endpoint(None), None);
        #[cfg(feature = "asio")]
        assert_eq!(recoverable_endpoint(Some(cpal::DeviceId::new(cpal::HostId::Asio, "original-endpoint"))), None);
    }

    #[test]
    fn legal_final_lead_then_invalid_requests_a_new_stream_and_requires_its_own_reports() {
        let mut old = StreamLead::default();
        let start = Instant::now();
        for micros in [43_000, 56_395] {
            assert!(!old.observe(start, Some(Duration::from_micros(micros))).clamped);
        }
        let invalid = old.observe(start, None);
        assert!(invalid.clamped);
        assert_eq!(invalid.lead(), Some(Duration::from_micros(56_395)));
        let mut retry = Retry::default();
        assert_eq!(retry.decide(start, true, true, None), Decision::Wait);
        let now = start + Duration::from_secs(1);
        assert_eq!(retry.decide(now, true, true, None), Decision::Probe);
        assert_eq!(retry.decide(now, true, true, Some(ProbeState::Waiting)), Decision::Wait);
        assert_eq!(retry.decide(now, true, true, Some(ProbeState::Fresh)), Decision::Adopt);
    }
    #[test]
    fn repeated_faults_cannot_reopen_storm_and_long_storage_does_not_exhaust_the_budget() {
        let mut retry = Retry::default();
        let now = Instant::now();
        for _ in 0..120 {
            assert!(retry.lookup(now, || None::<()>).is_none());
        }
        for _ in 0..3 {
            assert!(retry.lookup(now, || Some(())).is_some());
            retry.decide(now, true, false, None);
            retry.decide(now + Duration::from_secs(1), true, true, None);
        }
        assert!(retry.lookup(now, || Some(())).is_none());
        let later = now + PRESENCE_INTERVAL;
        assert!(retry.lookup(later, || None::<()>).is_none());
        assert!(retry.lookup(later, || Some(())).is_some());
    }

    #[test]
    fn timing_recovery_bounds_device_enumeration_after_exhaustion() {
        let mut retry = Retry::default();
        let start = Instant::now();
        for _ in 0..MAX_ATTEMPTS {
            assert_eq!(retry.lookup(start, || Some(())), Some(()));
        }
        let mut lookups = 0;
        for second in 0..=60 {
            let now = start + Duration::from_secs(second);
            if retry.decide(now, true, true, None) == Decision::Probe {
                let before = lookups;
                let device = retry.lookup(now, || {
                    lookups += 1;
                    Some(())
                });
                assert_eq!(device.is_some(), lookups > before, "a successful lookup after cooldown is a real reopen attempt");
            }
        }
        assert!((1..=2).contains(&lookups), "{lookups} presence checks in 60 s with the reopen budget exhausted");
        let later = start + Duration::from_secs(120);
        assert!(retry.lookup(later, || None::<()>).is_none());
        assert_eq!(retry.lookup(later + Duration::from_secs(1), || Some(())), Some(()));
    }

    #[test]
    fn callback_panic_latched_after_service_check_prevents_handoff() {
        let now = Instant::now();
        let current = Route::new(true, now);
        let next = Route::new(false, now);
        let graph = Mutex::new(());
        let fault = Fault::default();
        let running = AtomicBool::new(false);
        for _ in 0..8 {
            next.reports.write(now, true);
        }
        // Simulate a callback latching a failure after the control thread's safety check.
        current.render(&graph, |_| fault.failed.store(true, Ordering::Relaxed));
        assert!(!current.hand_over(&next, &graph, &running, &fault.failed, now));
        assert!(!next.selected.load(Ordering::Acquire));
        assert_eq!(fault.trouble(false), Some(crate::OutputTrouble::Bug));
        assert!(current.hand_over(&next, &graph, &running, &AtomicBool::new(false), now));
    }

    #[test]
    fn exhausted_recovery_can_reopen_an_endpoint_that_never_disappears() {
        let mut retry = Retry::default();
        let start = Instant::now();
        for _ in 0..MAX_ATTEMPTS {
            assert_eq!(retry.lookup(start, || Some(())), Some(()));
        }
        for period in 1..=3 {
            let due = start + PRESENCE_INTERVAL * period;
            assert!(retry.lookup(due - Duration::from_millis(1), || -> Option<()> { panic!("early enumeration") }).is_none());
            assert_eq!(retry.lookup(due, || Some(())), Some(()));
        }
    }

    #[test]
    fn timing_recovery_keeps_the_graph_available_after_poisoning() {
        let graph = Mutex::new(0);
        let _ = std::panic::catch_unwind(|| {
            let _guard = graph.lock().unwrap();
            panic!("simulate a poisoned graph");
        });
        let now = Instant::now();
        let old = Route::new(true, now);
        assert!(old.render(&graph, |count| *count += 1), "poisoned graph stayed silent");
        let next = Route::new(false, now);
        for _ in 0..8 {
            next.reports.write(now, true);
        }
        assert!(old.hand_over(&next, &graph, &AtomicBool::new(false), &AtomicBool::new(false), now));
        assert!(next.render(&graph, |count| assert_eq!(*count, 1)));
    }

    #[test]
    fn handoff_keeps_the_graph_and_a_silent_probe_never_consumes_a_command() {
        let now = Instant::now();
        let old = Route::new(true, now);
        let next = Route::new(false, now);
        let commands = Mutex::new(vec![1, 2, 3]);
        let running = AtomicBool::new(false);
        let failed = AtomicBool::new(false);
        assert!(old.render(&commands, |v| assert_eq!(v.pop(), Some(3))));
        assert!(!next.render(&commands, |_| {}));
        for _ in 0..7 {
            next.reports.write(now, true);
        }
        assert!(!old.hand_over(&next, &commands, &running, &failed, now));
        next.reports.write(now, false);
        next.reports.write(now, true);
        assert!(!old.hand_over(&next, &commands, &running, &failed, now));
        for _ in 0..7 {
            next.reports.write(now, true);
        }
        assert!(!old.hand_over(&next, &commands, &running, &failed, now + Duration::from_secs(1)));
        running.store(true, Ordering::Release);
        assert!(!old.hand_over(&next, &commands, &running, &failed, now));
        running.store(false, Ordering::Release);
        {
            let _rendering = commands.lock();
            assert!(!old.hand_over(&next, &commands, &running, &failed, now));
        }
        assert!(old.hand_over(&next, &commands, &running, &failed, now));
        assert!(!old.render(&commands, |_| {}));
        assert!(next.render(&commands, |v| assert_eq!(v.as_slice(), &[1, 2])));
    }

    #[test]
    fn active_transport_cancels_a_prepared_probe_and_device_failure_does_not_swap() {
        let now = Instant::now();
        let mut retry = Retry::default();
        assert_eq!(retry.decide(now, false, true, Some(ProbeState::Fresh)), Decision::Discard);
        assert_eq!(retry.decide(now, true, true, Some(ProbeState::Failed)), Decision::Discard);
        assert_eq!(retry.decide(now, false, true, None), Decision::Wait);
        assert_eq!(retry.decide(now, true, false, Some(ProbeState::Fresh)), Decision::Discard);
    }
}
