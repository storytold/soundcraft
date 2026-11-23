//! Incoming MIDI Beat Clock. The input thread timestamps and interprets the wire;
//! the host drains only transport transitions. Ordinary clock ticks take no queue space.
use crate::sync_input::PulseFit;

#[derive(Debug, Copy, Clone, Eq, PartialEq, Default)]
pub enum State {
    #[default]
    Waiting,
    Locked,
    Coasting,
    Stopped,
}
#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub enum Action {
    Locate(i64),
    Play {
        pulse: i64,
        at: i64,
    },
    /// The input thread already submitted the prepared playback command.
    Started {
        pulse: i64,
        at: i64,
        render_from: i64,
    },
    Stop,
}
#[derive(Default, Clone, Copy, Debug)]
pub struct Snapshot {
    pub bpm: Option<f64>,
    /// A confirmed tempo step or brake needs a fast grid correction during playback.
    pub recovering_tempo_step: bool,
    /// A fast brake has reached long pulse intervals. Follow the arriving pulses
    /// themselves while the new tempo and phase settle.
    pub braking: bool,
    /// Grid speed estimated for the next confirmed pulse during acceleration out of a brake.
    /// Status and loss detection continue to use the measured source tempo.
    pub brake_grid_bpm: Option<f64>,
    /// Short-term slope for upcoming score events while tracking a measured phase excursion.
    pub phase_grid_bpm: Option<f64>,
    /// The source has a sustained tempo trend above its measured timestamp noise.
    pub tempo_ramp: bool,
    /// Number of intervals used if the latest F8 confirmed a new tempo.
    pub step_intervals: u8,
    /// The input is delivering groups of F8s with shared timing.
    pub packed_delivery: bool,
    /// An isolated late arrival may be the start of a batch. Its timestamp cannot
    /// position the score until another pulse distinguishes timing from delivery.
    pub wire_phase_pending: bool,
    pub jitter_us: f64,
    pub phase_error_us: i64,
    pub state: State,
    pub dropouts: u64,
    /// Most recently played musical clock number, separate from the tempo estimator counter.
    pub pulse: i64,
    pub stamp_us: i64,
    pub now_us: i64,
}

/// Musical start and input calibration captured when a stopped position is armed.
pub struct LocatedCue {
    pub pulse: i64,
    pub origin: i64,
    pub offset: i64,
    pub rate: soundcraft_time::SampleRate,
    pub revision: std::sync::Weak<std::sync::Mutex<bool>>,
}

/// One-shot transport handoff. Device callbacks never prepare plugins or retain a player.
pub(crate) struct PreparedResume {
    pub(crate) revision: std::sync::Weak<std::sync::Mutex<bool>>,
    pub(crate) pulse: i64,
    pub(crate) origin: i64,
    pub(crate) offset: i64,
    pub(crate) sr: soundcraft_time::SampleRate,
    pub(crate) shared: std::sync::Weak<super::Shared>,
    pub(crate) tx: std::sync::Weak<super::Sender<super::Cmd>>,
    #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
    pub(crate) output: std::sync::Weak<super::Mutex<super::sync_output::Output>>,
    #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
    pub(crate) silent: bool,
}
impl PreparedResume {
    /// Queue the start, or say why the host has to start it instead.
    fn dispatch(&self, at: i64, age_us: impl FnOnce() -> i64) -> Result<i64, &'static str> {
        use super::{Cmd, Ordering};
        const CLOSED: &str = "the player was closed";
        let revision = self.revision.upgrade().ok_or("the prepared session was replaced")?;
        let shared = self.shared.upgrade().ok_or(CLOSED)?;
        let tx = self.tx.upgrade().ok_or(CLOSED)?;
        if shared.playing.load(Ordering::Acquire) {
            return Err("playback is already running");
        }
        #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
        let output = self.output.upgrade().ok_or(CLOSED)?;
        #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
        let output = output.try_lock().map_err(|_| "the Clock output was busy")?;
        #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
        let lead = if self.silent { 0 } else { output.reported_lead_us().ok_or("the audio device has not reported its output latency yet")? };
        #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
        let lead = 0;
        let render_from = self
            .origin
            .saturating_add(self.sr.samples(age_us().max(0) as f64 / 1e6))
            .saturating_add(self.sr.samples(lead as f64 / 1e6))
            .saturating_sub(self.offset)
            .max(0);
        // The host revokes this gate before an edit can mutate the prepared document.
        // Hold it through queueing and publishing the start. An edit that won this race
        // leaves the original Play action for the host to prepare against its fresh session.
        let current = revision.try_lock().map_err(|_| "an edit is changing the session")?;
        if !*current {
            return Err("an edit changed the session");
        }
        // The renderer starts at its next callback, up to one device buffer later. Without
        // the arrival it would present `render_from` that much behind the source.
        let cmd = Cmd::PlayClock { origin: self.origin, render_from, arrival: Some((at, self.offset)) };
        #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
        let queued = output.send_transport(&tx, cmd);
        #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
        let queued = tx.send(cmd).is_ok();
        if !queued {
            return Err("the playback command could not be queued");
        }
        shared.position.store(render_from, Ordering::Relaxed);
        shared.playing.store(true, Ordering::Release);
        Ok(render_from)
    }
}

mod wire_step;

mod dll;

/// Pulses a phase excursion holds the score at the wire before the usual one-pulse lookahead returns.
const PHASE_HOLD_PULSES: u16 = 12;

pub(crate) type BoundaryListener = std::sync::Arc<dyn Fn(Option<i64>) + Send + Sync>;

pub(crate) struct Follower {
    pub(crate) boundary_listener: Option<BoundaryListener>,
    edge: wire_step::WireStep,
    pub(crate) cue: Option<PreparedResume>,
    pub(crate) source: Option<u8>,
    pub(crate) coast: bool,
    pub(crate) waiting_for_output: bool,
    state: State,
    awaiting_clock: Option<i64>,
    pub(crate) next_pulse: i64,
    fit_index: i64,
    // Three F8s within one millisecond cannot be individual beats in the
    // supported tempo range. Keep each pulse and fit completed groups by their final ordinal.
    packet_head: Option<(i64, i64)>,
    packed_delivery: bool,
    singleton_groups: u8,
    previous_batch_noise: [f64; 4],
    // Two beats and their shared endpoint. Let measured BPM settle after a temporary nudge.
    settling: PulseFit<49>,
    // Variance of wire jitter independent of the long fit's response to a tempo bend.
    wire_noise_variance: f64,
    readout_pulses_left: u8,
    phase_pulses_left: u8,
    /// Consecutive pulses received while phase tracking.
    phase_tracked_pulses: u16,
    /// Filtered pulse time and period for projecting the score past the wire.
    dll: dll::PulseDll,
    short: PulseFit<12>,
    steady: PulseFit<192>,
    changes: u8,
    step_pulses_left: u8,
    step_intervals: u8,
    latest: Option<i64>,
    // Wire history survives Coast. A fresh pair in the new acquisition epoch
    // replaces the old period; the silence between epochs is never a beat.
    wire_stamps: [i64; 13],
    wire_points: usize,
    last_wire_interval: Option<i64>,
    previous_wire_interval: Option<i64>,
    brake_pulses_left: u8,
    last_sensing_byte: Option<i64>,
    coast_period_us: Option<f64>,
    played_stamp: i64,
    played_pulse: i64,
    phase_error_us: i64,
    dropouts: u64,
    events: [Option<Action>; 32],
    head: usize,
    queued_action: bool,
    count: usize,
    spp: Option<Option<u8>>,
}
impl Default for Follower {
    fn default() -> Self {
        Self {
            boundary_listener: None,
            edge: Default::default(),
            cue: None,
            source: None,
            coast: true,
            waiting_for_output: false,
            state: State::Waiting,
            awaiting_clock: None,
            next_pulse: 0,
            fit_index: 0,
            packet_head: None,
            packed_delivery: false,
            singleton_groups: 0,
            previous_batch_noise: [0.; 4],
            settling: Default::default(),
            wire_noise_variance: 0.,
            readout_pulses_left: 0,
            phase_pulses_left: 0,
            phase_tracked_pulses: 0,
            dll: Default::default(),
            short: Default::default(),
            steady: Default::default(),
            changes: 0,
            step_pulses_left: 0,
            step_intervals: 0,
            latest: None,
            wire_stamps: [0; 13],
            wire_points: 0,
            last_wire_interval: None,
            previous_wire_interval: None,
            brake_pulses_left: 0,
            last_sensing_byte: None,
            played_stamp: 0,
            played_pulse: 0,
            coast_period_us: None,
            phase_error_us: 0,
            dropouts: 0,
            events: [None; 32],
            head: 0,
            queued_action: false,
            count: 0,
            spp: None,
        }
    }
}
impl Follower {
    pub(crate) fn arm(&mut self, port: u8, cue: PreparedResume) -> bool {
        if self.source == Some(port) && self.count == 0 && self.next_pulse == cue.pulse && matches!(self.state, State::Waiting | State::Stopped) {
            self.cue = Some(cue);
            return true;
        }
        false
    }

    pub(crate) fn dispatch_prepared(&mut self, now: impl FnOnce() -> i64) {
        let newest = self.count.checked_sub(1).and_then(|i| self.events[(self.head + i) % self.events.len()]);
        let Some(Action::Play { pulse, at }) = newest else { return };
        let started = if self.count != 1 {
            Err("an earlier transport action is still waiting for the host")
        } else {
            match self.cue.take() {
                None => Err("no stopped position was prepared"),
                Some(cue) if cue.pulse != pulse => Err("a different position was prepared"),
                Some(cue) => cue.dispatch(at, || now().saturating_sub(at)),
            }
        };
        match started {
            Ok(render_from) => {
                self.waiting_for_output = false;
                self.events[self.head] = Some(Action::Started { pulse, at, render_from });
            }
            // The host starts it at its next frame, which can be a whole UI frame later.
            Err(reason) => log::warn!("external Clock start at pulse {pulse} is left to the host: {reason}"),
        }
    }

    fn push(&mut self, action: Action) {
        if !matches!(action, Action::Play { .. }) {
            self.cue = None;
            self.step_pulses_left = 0;
            self.phase_pulses_left = 0;
        }
        self.queued_action = true;
        if self.count == self.events.len() {
            // A paused host cannot replay an arbitrarily long history safely.
            // Resynchronise on a fresh Start/Continue after delivering one Stop.
            self.events.fill(None);
            self.head = 0;
            self.count = 1;
            self.events[0] = Some(Action::Stop);
            self.state = State::Stopped;
            self.awaiting_clock = None;
            return;
        }
        self.events[(self.head + self.count) % self.events.len()] = Some(action);
        self.count += 1;
    }
    pub(crate) fn take(&mut self) -> Option<Action> {
        if self.count == 0 {
            return None;
        }
        let a = self.events[self.head].take();
        self.head = (self.head + 1) % self.events.len();
        self.count -= 1;
        a
    }
    pub(crate) fn start_pending(&self) -> bool {
        self.awaiting_clock.is_some() || self.events.iter().flatten().any(|action| matches!(action, Action::Play { .. } | Action::Started { .. }))
    }

    pub(crate) fn coalesced_render(&self) -> bool {
        self.state == State::Locked && self.packed_delivery
    }

    pub(crate) fn render_through_pulse(&self) -> Option<i64> {
        match self.state {
            // While braking or consuming one USB batch, the wire does not yet
            // establish the next pulse's time.
            // A phase excursion first holds the score at the wire, since the source may be
            // slowing. One beat later the excursion is the long fit catching up with a held
            // nudge, and holding the wire would only delay every note by up to a buffer.
            State::Locked
                if self.brake_pulses_left > 0 || self.packed_delivery || (self.phase_tracking() && self.phase_tracked_pulses < PHASE_HOLD_PULSES) =>
            {
                Some(self.played_pulse)
            }
            State::Locked => Some(self.played_pulse.saturating_add(1)),
            State::Stopped => Some(self.played_pulse),
            State::Waiting | State::Coasting => None,
        }
    }

    /// Filtered latest pulse time and period for projecting the score past the wire.
    /// Abrupt steps, brakes and packed delivery keep the wire limit until the loop settles.
    pub(crate) fn render_prediction(&self) -> Option<(i64, f64)> {
        if self.state != State::Locked || self.step_pulses_left != 0 || self.brake_pulses_left != 0 || self.packed_delivery {
            return None;
        }
        self.dll.prediction()
    }

    /// At a steady or gently changing rate, independent timestamp noise is better
    /// rejected by the long fit. An abrupt or sustained rate excursion takes priority.
    fn prefer_steady_phase(&self) -> bool {
        self.wire_noise_variance > 850. * 850.
            && self.readout_pulses_left == 0
            && self.short.period_us().zip(self.steady.period_us()).is_some_and(|(short, long)| (short - long).abs() < 0.025 * long)
    }
    fn phase_tracking(&self) -> bool {
        self.state == State::Locked
            && self.phase_pulses_left > 0
            && !self.prefer_steady_phase()
            && self.step_pulses_left == 0
            && self.brake_pulses_left == 0
            && !self.packed_delivery
    }
    fn phase_period(&self) -> Option<f64> {
        let last = self.last_wire_interval?;
        let prior = self.previous_wire_interval?;
        if last.abs_diff(prior) <= 2 && last >= 2_500 {
            // An ideal source may round two successive intervals in opposite directions.
            return Some((last + prior) as f64 / 2.);
        }
        let recent = self.wire_stamps[12].saturating_sub(self.wire_stamps[8]) as f64 / 4.;
        if self.steady.period_us().is_some_and(|period| (recent - period).abs() > 0.05 * period) { Some(recent) } else { self.short.period_us() }
    }

    fn observe_phase_arrival(&mut self, at: i64) {
        self.phase_pulses_left = self.phase_pulses_left.saturating_sub(1);
        if self.packed_delivery || self.wire_points != self.wire_stamps.len() || self.step_pulses_left != 0 {
            return;
        }
        let (Some(last), Some(period)) = (self.latest, self.steady.period_us()) else { return };
        let noise = self.steady.jitter_us();
        let interval_error = (at.saturating_sub(last) as f64 - period).abs();
        let interval_changed = interval_error > (5. * noise).max(0.012 * period).max(30.);
        let phase_changed = noise < 1_000.
            && self.steady.predict_us(self.fit_index.saturating_add(1)).is_some_and(|expected| at.abs_diff(expected) as f64 > (3. * noise).max(350.));
        if interval_changed || phase_changed {
            // A deck can bend its speed below the eight-percent step threshold.
            // Stop committing ahead of the wire while the latest phase is moving.
            self.phase_pulses_left = 48;
        }
    }
    fn bpm(&self) -> Option<f64> {
        let fit = self.steady.period_us().or_else(|| self.short.period_us());
        let fit = if self.state == State::Locked && self.readout_pulses_left > 0 && self.settling.jitter_us() < 950. {
            self.settling.period_us().or(fit)
        } else {
            fit
        };
        let period = match self.brake_pulses_left {
            0 => fit,
            1..=36 => self.short.period_us().or(fit),
            _ => self.last_wire_interval.map(|p| p as f64).or(fit),
        }?;
        // At 5 BPM, two timestamps with ±1 ms jitter can enclose a 502 ms interval.
        (2_500. ..=502_000.).contains(&period).then(|| 2_500_000. / period.clamp(2_500., 500_000.))
    }
    fn brake_grid_bpm(&self) -> Option<f64> {
        if self.brake_pulses_left <= 36 || self.wire_points != self.wire_stamps.len() {
            return None;
        }
        let last = self.last_wire_interval? as f64;
        let w = &self.wire_stamps;
        let older = w[8].saturating_sub(w[4]) as f64 / 4.;
        let recent = w[12].saturating_sub(w[8]) as f64 / 4.;
        let shortening = (older - recent) / 4.;
        if recent <= 0. || shortening <= 0.012 * recent {
            return None;
        }
        // Match the advancing wire while the render cursor can already be several
        // pulses ahead. The confirmed-pulse horizon still gates every note.
        let period = (recent - 4. * shortening).clamp(last / 2., last);
        Some((2_500_000. / period).clamp(5., 1000.))
    }

    fn wire_phase_pending(&self) -> bool {
        if self.packed_delivery {
            // Only completed batches give both the edge time and its musical ordinal.
            return self.steady.period_us().is_none();
        }
        let stamps = &self.wire_stamps;
        let previous = stamps[11].saturating_sub(stamps[10]);
        let interval = stamps[12].saturating_sub(stamps[11]);
        // At least half a pulse overdue, or two arrivals inside the one-ms batch
        // window. The next independent interval can confirm a genuine slowdown.
        self.wire_points >= 3 && (interval <= 1_000 || (previous >= 2_500 && interval > previous.saturating_add(previous / 2)))
    }

    pub(crate) fn snapshot(&self, now_us: i64) -> Snapshot {
        Snapshot {
            // The gap before the first USB batch can resemble a much slower clock.
            // Three intervals allow the follower to distinguish that startup burst.
            bpm: (self.fit_index >= 4).then(|| self.bpm()).flatten().or_else(|| self.coast_period_us.map(|p| 2_500_000. / p)),
            recovering_tempo_step: self.step_pulses_left > 0 || self.brake_pulses_left > 0,
            braking: self.brake_pulses_left > 0,
            brake_grid_bpm: self.brake_grid_bpm(),
            phase_grid_bpm: self.phase_tracking().then(|| self.phase_period()).flatten().map(|p| 2_500_000. / p),
            tempo_ramp: self.state == State::Locked && !self.packed_delivery && self.step_pulses_left == 0 && self.steady.is_tempo_ramp(),
            step_intervals: self.step_intervals,
            packed_delivery: self.packed_delivery,
            wire_phase_pending: self.wire_phase_pending(),
            jitter_us: self.steady.jitter_us(),
            phase_error_us: self.phase_error_us,
            state: self.state,
            dropouts: self.dropouts,
            pulse: self.played_pulse,
            stamp_us: self.played_stamp,
            now_us,
        }
    }
    /// Returns true when losing the clock invalidates the published render horizon.
    pub(crate) fn expire(&mut self, now: i64) -> bool {
        let period = self.bpm().map(|bpm| 2_500_000. / bpm);
        // The first pulse of an acquisition has no interval yet. Give even a
        // 5 BPM source time to provide a second pulse, also when returning from Coast.
        let timeout = match (period, self.last_wire_interval) {
            (None, _) if self.short.period_us().is_none() && self.steady.period_us().is_none() => 1_500_000,
            (fitted, interval) => {
                let last = interval.unwrap_or(0);
                let growth = if self.brake_pulses_left > 0 { last.saturating_sub(self.previous_wire_interval.unwrap_or(last)).max(0) } else { 0 };
                let expected = fitted.unwrap_or(0.).max(last.saturating_add(growth) as f64);
                // Timestamped groups of up to five pulses can be followed by
                // a whole group's silence even when the preceding group was short.
                let group_gap = if self.packed_delivery { 3. * 5. * fitted.unwrap_or(0.) + 1_000. } else { 0. };
                (3. * expected).max(group_gap).ceil().clamp(150_000., 1_500_000.) as i64
            }
        };
        let cable_lost = self.last_sensing_byte.is_some_and(|last| now.saturating_sub(last) > 300_000);
        if cable_lost {
            self.last_sensing_byte = None;
        }
        if self.latest.is_some_and(|last| cable_lost || now.saturating_sub(last) > timeout) {
            // Idle pulses can predate a fresh transport request by an entire timeout.
            let request = self.awaiting_clock.filter(|at| now.saturating_sub(*at) <= timeout);
            let cue = self.cue.take();
            self.lost();
            if request.is_some() {
                self.awaiting_clock = request;
                self.cue = cue;
            }
            return true;
        }
        false
    }
    pub(crate) fn lost(&mut self) {
        if let Some(listener) = &self.boundary_listener {
            listener(None);
        }
        self.cue = None;
        self.coast_period_us = self.bpm().map(|bpm| 2_500_000. / bpm).or(self.coast_period_us);
        self.short.clear();
        self.steady.clear();
        self.settling.clear();
        self.dll.clear();
        self.readout_pulses_left = 0;
        self.wire_noise_variance = 0.;
        self.phase_pulses_left = 0;
        self.latest = None;
        self.edge = Default::default();
        self.wire_points = 0;
        self.packet_head = None;
        self.packed_delivery = false;
        self.singleton_groups = 0;
        self.previous_batch_noise = [0.; 4];
        self.step_intervals = 0;
        self.brake_pulses_left = 0;
        self.changes = 0;
        self.step_pulses_left = 0;
        if self.state == State::Locked {
            self.dropouts = self.dropouts.saturating_add(1);
            if self.coast {
                self.state = State::Coasting;
            } else {
                self.state = State::Stopped;
                self.push(Action::Stop);
            }
        }
        self.awaiting_clock = None;
    }
    /// Three consecutive four-pulse spans isolate a sustained brake from a single
    /// tempo step. A smooth period increase produces a quadratic time curve; compare
    /// the growth with residual timing noise before leaving the long jitter filter.
    fn braking_trend(&self) -> bool {
        let w = &self.wire_stamps;
        if self.wire_points < w.len() {
            return false;
        }
        let first = w[4].saturating_sub(w[0]) as f64;
        let middle = w[8].saturating_sub(w[4]) as f64;
        let last = w[12].saturating_sub(w[8]) as f64;
        let growth = (middle - first).min(last - middle);
        if growth <= 2_000. || middle <= 1.05 * first || last <= 1.05 * middle {
            return false;
        }
        // A step in the middle of the window increases both four-interval spans,
        // but the last six intervals settle onto the new tempo. A continuing brake
        // must also grow between the two halves of that most recent span.
        if w[12].saturating_sub(w[9]) <= w[9].saturating_sub(w[6]) {
            return false;
        }
        // A single big interval change is a tempo step, even when a four-pulse
        // span straddles the transition. A brake widens over multiple intervals.
        let largest_jump = w.windows(3).map(|t| t[2].saturating_sub(t[1]) as f64 - t[1].saturating_sub(t[0]) as f64).fold(0., f64::max);
        // The outer spans each contain four intervals. A single jump accounting
        // for 80% of their mean-period difference signifies a step.
        if largest_jump >= 0.8 * ((last - first) / 4.) {
            return false;
        }
        let slope = w[12].saturating_sub(w[0]) as f64 / 12.;
        let curvature = (w[12].saturating_sub(w[6]) as f64 - w[6].saturating_sub(w[0]) as f64) / 72.;
        w.iter().enumerate().all(|(index, stamp)| {
            let x = index as f64 - 6.;
            let residual = stamp.saturating_sub(w[6]) as f64 - x * (slope + x * curvature);
            2. * residual.abs() < growth
        })
    }

    fn observe_wire_interval(&mut self, at: i64) {
        if self.brake_pulses_left == 1 && self.wire_points == self.wire_stamps.len() {
            // Seed the normal fit with the settled end of the ramp; its older
            // history describes a different speed and phase.
            self.steady.clear();
            for (index, stamp) in self.wire_stamps.iter().enumerate() {
                self.steady.push(self.fit_index - 12 + index as i64, *stamp);
            }
        }
        self.brake_pulses_left = self.brake_pulses_left.saturating_sub(1);
        self.wire_stamps.rotate_left(1);
        self.wire_stamps[12] = at;
        self.wire_points = (self.wire_points + 1).min(self.wire_stamps.len());
        let Some(previous) = self.latest else { return };
        let gap = at.saturating_sub(previous).min(1_500_000);
        if self.packed_delivery || gap <= 1_000 {
            self.brake_pulses_left = 0;
            return;
        }
        let changing = self.last_wire_interval.is_some_and(|last| gap.abs_diff(last) > (last / 16).max(4_000) as u64);
        if self.braking_trend() || (self.brake_pulses_left > 0 && changing) {
            // Hold the score at the last confirmed wire pulse through the brake
            // and the subsequent acceleration, then settle back over two beats.
            self.brake_pulses_left = 48;
        }
        if let Some(previous_gap) = self.last_wire_interval {
            // A second difference cancels constant period and phase. With independent
            // timestamp errors its variance is (1² + 2² + 1²) times their variance.
            // Bound isolated transport or driver discontinuities in the running estimate.
            let change = (gap - previous_gap).clamp(-7_000, 7_000) as f64;
            self.wire_noise_variance += 0.025 * (change * change / 6. - self.wire_noise_variance);
        }
        self.previous_wire_interval = self.last_wire_interval;
        self.last_wire_interval = Some(gap);
    }

    fn clock(&mut self, at: i64) {
        let at = self.latest.map_or(at, |prev| prev.max(at));
        self.observe_phase_arrival(at);
        self.observe_wire_interval(at);
        self.latest = Some(at);
        self.fit_index = self.fit_index.saturating_add(1);
        self.step_intervals = 0;
        self.step_pulses_left = self.step_pulses_left.saturating_sub(1);
        if self.packet_head.is_none_or(|(_, stamp)| at.saturating_sub(stamp) > 1000) {
            if self.packed_delivery
                && let Some((head, stamp)) = self.packet_head
            {
                // A group can only reach the driver once its last pulse exists.
                // The first arrival timestamp is therefore an observation of the
                // previous group's last ordinal once the next group starts.
                self.observe_period(self.fit_index - 1, stamp);
                let width = self.fit_index - head;
                self.singleton_groups = if width == 1 { self.singleton_groups.saturating_add(1) } else { 0 };
                if self.singleton_groups >= 4 {
                    self.packed_delivery = false;
                    self.previous_batch_noise = [0.; 4];
                }
            }
            self.packet_head = Some((self.fit_index, at));
        }
        let (head, _) = self.packet_head.unwrap_or((self.fit_index, at));
        if !self.packed_delivery && self.fit_index - head >= 2 {
            self.packed_delivery = true;
            self.singleton_groups = 0;
            self.settling.clear();
            self.readout_pulses_left = 0;
            self.short.clear();
            self.steady.clear();
            self.dll.clear();
            self.changes = 0;
        }
        if !self.packed_delivery {
            self.observe_period(self.fit_index, at);
        }
        let start_requested = self.awaiting_clock.take().is_some();
        if self.state == State::Coasting && (start_requested || self.bpm().is_some()) {
            // F8 has no absolute position. Match the returning train to the nearest
            // pulse on the coasting grid. A lone tick after a timeout has no fitted tempo.
            if let Some(period) = self.coast_period_us {
                let coasted = (at.saturating_sub(self.played_stamp).max(0) as f64 / period).round() as i64;
                self.next_pulse = self.next_pulse.max(self.played_pulse.saturating_add(coasted));
            }
            self.state = State::Locked;
        }
        if start_requested {
            self.state = State::Locked;
            self.push(Action::Play { pulse: self.next_pulse, at });
        }
        if self.state == State::Locked {
            self.played_stamp = match self.brake_pulses_left {
                0 if self.phase_tracking() => at,
                0 => self.steady.predict_us(self.fit_index).unwrap_or(at),
                1..=36 => self.short.predict_us(self.fit_index).unwrap_or(at),
                _ => at,
            };
            self.played_pulse = self.next_pulse;
            self.next_pulse = self.next_pulse.saturating_add(1);
        }
        self.phase_tracked_pulses = if self.phase_tracking() { self.phase_tracked_pulses.saturating_add(1) } else { 0 };
    }
    fn observe_period(&mut self, ordinal: i64, at: i64) {
        self.readout_pulses_left = self.readout_pulses_left.saturating_sub(1);
        if let Some((recent, long)) = self.settling.period_us().zip(self.steady.period_us())
            && (recent - long).abs() > 0.007 * long
        {
            // A meaningful rate change can settle in two beats. Give the long fit
            // its full window to forget the old rate before switching the display back.
            self.readout_pulses_left = 192;
        }
        self.settling.push(ordinal, at);
        self.short.push(ordinal, at);
        // Long regression suppresses USB quantisation and timestamp jitter.
        // A sustained slope change outside the short fit's noise band starts a fresh epoch.
        if let (Some(fast), Some(slow)) = (self.short.period_us(), self.steady.period_us()) {
            let threshold = (0.013 * slow).max(0.35 * self.short.jitter_us());
            if (fast - slow).abs() > threshold {
                self.changes = self.changes.saturating_add(1);
            } else {
                self.changes = 0;
            }
            // Four batch heads can span twenty pulses. Use the noise floor from
            // before that candidate, so the step itself cannot mask the change.
            let mut noise = self.steady.jitter_us();
            if self.packed_delivery {
                self.previous_batch_noise.rotate_left(1);
                std::mem::swap(&mut noise, &mut self.previous_batch_noise[3]);
            }
            let proposed = self.short.confirmed_step(slow, noise);
            let step = if self.packed_delivery { proposed } else { self.edge.observe(ordinal, at, slow, noise, proposed) };
            if !self.packed_delivery
                && self.step_pulses_left == 0
                && let (Some(short_phase), Some(steady_phase)) = (self.short.predict_us(ordinal), self.steady.predict_us(ordinal))
                && (short_phase.abs_diff(steady_phase) as f64 > (3. * self.short.jitter_us()).max(950.)
                    || self.short.jitter_us() < 5. && (fast - slow).abs() > 0.01 * slow)
            {
                self.phase_pulses_left = 48;
            }
            if let Some(((index, stamp), intervals)) = step {
                self.dll.step(ordinal, at, Some(fast));
                self.steady.clear();
                self.steady.push(index, stamp);
                self.changes = 0;
                if self.state == State::Locked {
                    self.step_intervals = intervals;
                    self.phase_pulses_left = 0;
                    self.step_pulses_left = 24;
                }
            } else if self.changes >= 6 {
                self.steady.clear();
                self.changes = 0;
            }
        }
        self.phase_error_us = self.steady.predict_us(ordinal).map_or(0, |pred| at.saturating_sub(pred));
        self.steady.push(ordinal, at);
        self.dll.observe(ordinal, at, self.short.period_us());
    }
    /// System common bytes may be interrupted by real-time messages, even inside SysEx.
    /// Channel data and incomplete or malformed system messages never trigger a transport action.
    pub(crate) fn receive(&mut self, bytes: &[u8], at: i64) -> bool {
        self.queued_action = false;
        self.expire(at);
        if !bytes.is_empty() && (self.last_sensing_byte.is_some() || bytes.contains(&0xfe)) {
            self.last_sensing_byte = Some(self.last_sensing_byte.map_or(at, |last| last.max(at)));
        }
        for &byte in bytes {
            match byte {
                0xf8 => self.clock(at),
                0xfa => {
                    if let Some(listener) = &self.boundary_listener {
                        listener(None);
                    }
                    self.state = State::Waiting;
                    self.next_pulse = 0;
                    self.awaiting_clock = Some(at);
                    self.push(Action::Locate(0));
                }
                0xfb => {
                    if self.state != State::Locked {
                        if let Some(listener) = &self.boundary_listener {
                            listener(None);
                        }
                        self.awaiting_clock = Some(at);
                    }
                }
                0xfc => {
                    if self.state == State::Locked && let Some(listener) = &self.boundary_listener {
                        listener(Some(self.played_pulse));
                    }
                    self.awaiting_clock = None;
                    self.state = State::Stopped;
                    self.push(Action::Stop);
                }
                0xf9..=0xff => {}
                0xf2 => self.spp = Some(None),
                0..=0x7f => match self.spp {
                    Some(None) => self.spp = Some(Some(byte)),
                    Some(Some(low)) => {
                        self.spp = None;
                        if self.waiting_for_output || (matches!(self.state, State::Waiting | State::Stopped) && self.awaiting_clock.is_none()) {
                            // The clock can be locked before a device has ever presented sound.
                            // A new stopped insertion replaces that unrendered Continue.
                            if self.waiting_for_output {
                                self.state = State::Stopped;
                                self.awaiting_clock = None;
                            }
                            self.next_pulse = (i64::from(low) | i64::from(byte) << 7) * 6;
                            self.push(Action::Locate(self.next_pulse));
                        }
                    }
                    _ => {}
                },
                _ => self.spp = None,
            }
        }
        self.queued_action
    }
}
#[cfg(test)]
mod tests;

#[cfg(all(test, any(target_os = "windows", target_os = "macos", target_os = "linux")))]
mod render_lead;

#[cfg(all(test, any(target_os = "windows", target_os = "macos", target_os = "linux")))]
mod transport_tests;

#[cfg(all(test, any(target_os = "windows", target_os = "macos", target_os = "linux")))]
mod start_phase;
