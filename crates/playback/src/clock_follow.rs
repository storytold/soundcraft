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
}
impl PreparedResume {
    fn dispatch(&self, age_us: impl FnOnce() -> i64) -> Option<i64> {
        use super::{Cmd, Ordering};
        let revision = self.revision.upgrade()?;
        let shared = self.shared.upgrade()?;
        let tx = self.tx.upgrade()?;
        if shared.playing.load(Ordering::Acquire) {
            return None;
        }
        #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
        let output = self.output.upgrade()?;
        #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
        let output = output.try_lock().ok()?;
        #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
        let lead = output.reported_lead_us();
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
        let current = revision.try_lock().ok()?;
        if !*current {
            return None;
        }
        let cmd = Cmd::PlayClock { origin: self.origin, render_from };
        #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
        let queued = output.send_transport(&tx, cmd);
        #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
        let queued = tx.send(cmd).is_ok();
        if !queued {
            return None;
        }
        shared.position.store(render_from, Ordering::Relaxed);
        shared.playing.store(true, Ordering::Release);
        Some(render_from)
    }
}

pub(crate) struct Follower {
    pub(crate) cue: Option<PreparedResume>,
    pub(crate) source: Option<u8>,
    pub(crate) coast: bool,
    state: State,
    awaiting_clock: Option<i64>,
    pub(crate) next_pulse: i64,
    fit_index: i64,
    short: PulseFit<12>,
    steady: PulseFit<192>,
    changes: u8,
    latest: Option<i64>,
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
            cue: None,
            source: None,
            coast: true,
            state: State::Waiting,
            awaiting_clock: None,
            next_pulse: 0,
            fit_index: 0,
            short: Default::default(),
            steady: Default::default(),
            changes: 0,
            latest: None,
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
        if self.count == 1
            && let Some(Action::Play { pulse, at }) = self.events[self.head]
            && let Some(cue) = self.cue.take()
            && cue.pulse == pulse
            && let Some(render_from) = cue.dispatch(|| now().saturating_sub(at))
        {
            self.events[self.head] = Some(Action::Started { pulse, at, render_from });
        }
    }

    fn push(&mut self, action: Action) {
        if !matches!(action, Action::Play { .. }) {
            self.cue = None;
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
    fn bpm(&self) -> Option<f64> {
        self.steady.period_us().or_else(|| self.short.period_us()).filter(|p| (2500. ..=500_000.).contains(p)).map(|p| 2_500_000. / p)
    }
    pub(crate) fn snapshot(&self, now_us: i64) -> Snapshot {
        Snapshot {
            bpm: self.bpm().or_else(|| self.coast_period_us.map(|p| 2_500_000. / p)),
            jitter_us: self.steady.jitter_us(),
            phase_error_us: self.phase_error_us,
            state: self.state,
            dropouts: self.dropouts,
            pulse: self.played_pulse,
            stamp_us: self.played_stamp,
            now_us,
        }
    }
    pub(crate) fn expire(&mut self, now: i64) {
        let timeout = self.bpm().map_or(150_000, |bpm| (7_500_000. / bpm).ceil() as i64).max(150_000);
        if self.latest.is_some_and(|last| now.saturating_sub(last) > timeout) {
            // Idle pulses can predate a fresh transport request by an entire timeout.
            let request = self.awaiting_clock.filter(|at| now.saturating_sub(*at) <= timeout);
            let cue = self.cue.take();
            self.lost();
            if request.is_some() {
                self.awaiting_clock = request;
                self.cue = cue;
            }
        }
    }
    pub(crate) fn lost(&mut self) {
        self.cue = None;
        self.coast_period_us = self.steady.period_us().or(self.coast_period_us);
        self.short.clear();
        self.steady.clear();
        self.latest = None;
        self.changes = 0;
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
    fn clock(&mut self, at: i64) {
        let at = self.latest.map_or(at, |prev| prev.max(at));
        self.latest = Some(at);
        self.fit_index = self.fit_index.saturating_add(1);
        self.short.push(self.fit_index, at);
        // Long regression suppresses USB quantisation and timestamp jitter.
        // A sustained slope change outside the short fit's noise band starts a fresh epoch.
        if let (Some(fast), Some(slow)) = (self.short.period_us(), self.steady.period_us()) {
            let threshold = (0.013 * slow).max(0.35 * self.short.jitter_us());
            if (fast - slow).abs() > threshold {
                self.changes = self.changes.saturating_add(1);
            } else {
                self.changes = 0;
            }
            if self.changes >= 6 {
                self.steady.clear();
                self.changes = 0;
            }
        }
        self.phase_error_us = self.steady.predict_us(self.fit_index).map_or(0, |pred| at.saturating_sub(pred));
        self.steady.push(self.fit_index, at);
        if self.awaiting_clock.is_some() {
            if self.state == State::Coasting
                && let Some(period) = self.coast_period_us
            {
                let coasted = (at.saturating_sub(self.played_stamp).max(0) as f64 / period).round() as i64;
                self.next_pulse = self.next_pulse.max(self.played_pulse.saturating_add(coasted));
            }
            self.awaiting_clock = None;
            self.state = State::Locked;
            self.push(Action::Play { pulse: self.next_pulse, at });
        }
        if self.state == State::Locked {
            self.played_stamp = self.steady.predict_us(self.fit_index).unwrap_or(at);
            self.played_pulse = self.next_pulse;
            self.next_pulse = self.next_pulse.saturating_add(1);
        }
    }
    /// System common bytes may be interrupted by real-time messages, even inside SysEx.
    /// Channel data and incomplete or malformed system messages never trigger a transport action.
    pub(crate) fn receive(&mut self, bytes: &[u8], at: i64) -> bool {
        self.queued_action = false;
        self.expire(at);
        for &byte in bytes {
            match byte {
                0xf8 => self.clock(at),
                0xfa => {
                    self.state = State::Waiting;
                    self.next_pulse = 0;
                    self.awaiting_clock = Some(at);
                    self.push(Action::Locate(0));
                }
                0xfb => {
                    if self.state != State::Locked {
                        self.awaiting_clock = Some(at);
                    }
                }
                0xfc => {
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
                        if matches!(self.state, State::Waiting | State::Stopped) && self.awaiting_clock.is_none() {
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
