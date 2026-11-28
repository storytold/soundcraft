//! Presentation timestamps shared by every external MIDI output encoder (track notes and the
//! synchronization protocols).
use crate::{Range, Session};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub(crate) const RELAUNCH_PULSES: i64 = 6;
pub(crate) const BACKWARD_TOLERANCE_PULSES: i64 = 1;
pub(crate) const CATCH_UP_SPACING: Duration = Duration::from_millis(1);

#[derive(Clone, Copy)]
pub(crate) struct DeviceTiming {
    pub callback: Instant,
    lead: Option<Duration>,
    pub frame_offset: usize,
    pub clamped: bool,
}

/// One instance per built output stream, including a fresh instance on reopen.
#[derive(Default)]
pub(crate) struct StreamLead {
    last_valid: Option<Duration>,
}

impl StreamLead {
    pub(crate) fn observe(&mut self, now: Instant, report: Option<Duration>) -> DeviceTiming {
        let valid = report.filter(|&v| v <= Duration::from_secs(1));
        if valid.is_some() {
            self.last_valid = valid;
        }
        DeviceTiming { callback: now, lead: self.last_valid, frame_offset: 0, clamped: valid.is_none() }
    }
}

impl DeviceTiming {
    #[cfg(test)]
    pub(crate) fn new(callback: Instant, reported: Option<Duration>, frame_offset: usize) -> Self {
        Self { frame_offset, ..StreamLead::default().observe(callback, reported) }
    }
    pub(crate) fn lead(self) -> Option<Duration> {
        self.lead
    }
    pub(crate) fn heard_instant(self, device_rate: f64, frame_index: usize) -> Instant {
        let frames = self.frame_offset.saturating_add(frame_index);
        let elapsed = Duration::try_from_secs_f64(frames as f64 / device_rate).unwrap_or_default();
        // Without a report, this is the callback clock for uncompensated routes.
        // The optional lead travels with the block; compensated routes wait for it.
        self.callback.checked_add(self.lead.unwrap_or_default()).and_then(|t| t.checked_add(elapsed)).unwrap_or(self.callback)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Presentation {
    /// Revision of the session actually rendered by this callback.
    pub generation: u64,
    pub sample_rate: soundcraft_time::SampleRate,
    /// Mixer delay in session samples (zero when the session disables delay compensation).
    pub system_delay_samples: usize,
    pub pass: u64,
    pub origin: i64,
    pub playing: bool,
    pub sample: f64,
    pub heard_at: Instant,
    pub samples_per_second: f64,
    pub limit: f64,
    pub looped: Option<Range>,
    pub end: Option<i64>,
    pub lead: Option<Duration>,
}

impl Presentation {
    pub fn sample_heard_at(self, now: Instant) -> f64 {
        if !self.playing {
            return self.sample;
        }
        (self.sample + now.saturating_duration_since(self.heard_at).as_secs_f64() * self.samples_per_second).min(self.limit)
    }
    /// The renderer accumulates fractional steps while the publisher may multiply
    /// a complete block. At an integer boundary the two forms can differ by an ULP.
    pub fn sample_index_at(self, now: Instant) -> i64 {
        let sample = self.sample_heard_at(now);
        let nearest = sample.round();
        if (nearest - sample).abs() <= 2. * f64::EPSILON * sample.abs().max(1.) { nearest as i64 } else { sample.floor() as i64 }
    }
    pub fn instant_for(self, sample: f64) -> Instant {
        shift(self.heard_at, (sample - self.sample) / self.samples_per_second)
    }
    fn adjusted(mut self, compensate: bool, offset_samples: i64) -> Option<Self> {
        let lead = if compensate { self.lead? } else { self.lead.unwrap_or_default() };
        let (total, _) = delivery_offset(lead, self.system_delay_samples, self.samples_per_second, self.sample_rate, compensate, offset_samples);
        self.heard_at = shift(self.heard_at, total - lead.as_secs_f64());
        Some(self)
    }
    pub(crate) fn current(mut self, now: Instant) -> Self {
        let projected = self.sample_heard_at(now);
        if self.playing {
            if let Some(range) = self.looped {
                if projected >= range.end as f64 && self.limit > range.end as f64 {
                    // A wrap exactly at the rendered end belongs to the next callback,
                    // including when one short loop repeats several times per block.
                    let length = range.len() as f64;
                    let reached = 1. + ((projected - range.end as f64) / length).floor();
                    let rendered = ((self.limit - range.end as f64) / length).ceil();
                    let wraps = reached.min(rendered) as i64;
                    let distance = wraps.saturating_mul(range.len()) as f64;
                    self.pass = self.pass.wrapping_add(wraps as u64);
                    self.origin = range.start;
                    self.sample -= distance;
                    self.limit -= distance;
                }
            } else if let Some(end) = self.end
                && projected >= end as f64
            {
                self.playing = false;
                self.sample = end as f64;
            }
        }
        self
    }
}

/// Seconds from callback publication. Keep a millisecond of scheduling margin when advancing
/// a compensated device, but never manufacture lead when the host reported less than that.
/// The mixer delay (`system_samples`) passes at the transport's rate (`system_rate`, session
/// samples per second at the playback speed: half as long at 2x); the port offset is a time
/// calibration at the session rate `sr`.
pub(crate) fn delivery_offset(
    lead: Duration,
    system_samples: usize,
    system_rate: f64,
    sr: soundcraft_time::SampleRate,
    compensate: bool,
    offset: i64,
) -> (f64, bool) {
    let system = if system_rate > 0. { system_samples as f64 / system_rate } else { system_samples as f64 / sr.as_f64() };
    let available = if compensate { lead.as_secs_f64() + system } else { 0. };
    let max = crate::max_sync_offset_samples(sr);
    let requested = available + offset.clamp(-max, max) as f64 / sr.as_f64();
    let floor = available.min(0.001);
    (requested.max(floor), requested < floor)
}

pub(crate) fn shift(at: Instant, seconds: f64) -> Instant {
    let Ok(dt) = Duration::try_from_secs_f64(seconds.abs()) else { return at };
    if seconds < 0. { at.checked_sub(dt) } else { at.checked_add(dt) }.unwrap_or(at)
}

// Keep scheduled buffers until they can actually be heard. Publishing two future
// callbacks back-to-back must not conceal the first one's start or its last events.
const HISTORY: usize = 2048;
#[derive(Clone)]
pub(crate) struct SessionRevision {
    pub generation: u64,
    pub doc: Arc<Session>,
    pub remap: bool,
}
pub(crate) const MAX_REVISIONS: usize = 8;

pub(crate) struct Timeline {
    maps: VecDeque<SessionRevision>,
    blocks: VecDeque<Presentation>,
    // Only the consecutive timed blocks after the latest loss can drive compensated output.
    timed_blocks: usize,
    pub(crate) timing_epoch: u64,
    pub(crate) late_transport_waiter: Option<std::thread::Thread>,
    timing_waiter: Option<std::thread::Thread>,
    revision_waiter: Option<std::thread::Thread>,
}
impl Default for Timeline {
    fn default() -> Self {
        Self {
            blocks: VecDeque::with_capacity(HISTORY),
            timed_blocks: 0,
            timing_epoch: 0,
            maps: VecDeque::with_capacity(MAX_REVISIONS),
            late_transport_waiter: None,
            timing_waiter: None,
            revision_waiter: None,
        }
    }
}
impl Timeline {
    /// Called by the control thread, never by the audio callback.
    pub(crate) fn register(&mut self, revision: SessionRevision, active: bool) -> bool {
        if !active {
            self.maps.clear();
        }
        let full = self.maps.len() == MAX_REVISIONS;
        if full {
            self.maps.pop_front();
        }
        self.maps.push_back(revision);
        full
    }
    pub(crate) fn unversioned(&self) -> bool {
        self.maps.is_empty()
    }
    pub(crate) fn map(&self, generation: u64) -> Option<&SessionRevision> {
        self.maps.iter().find(|r| r.generation == generation)
    }
    pub(crate) fn retire_before(&mut self, generation: u64) {
        while self.maps.len() > 1 && self.maps.front().is_some_and(|r| r.generation < generation) {
            self.maps.pop_front();
        }
    }
    pub(crate) fn push(&mut self, block: Presentation) {
        if block.lead.is_some() {
            self.timed_blocks = (self.timed_blocks + 1).min(HISTORY);
        } else if self.timed_blocks > 0 {
            self.timed_blocks = 0;
            self.timing_epoch = self.timing_epoch.wrapping_add(1);
        }
        if block.lead.is_some()
            && let Some(worker) = self.timing_waiter.take()
        {
            worker.unpark();
        }
        // The worker arms this once, only after its bounded wait has expired.
        // Ordinary callbacks publish under their existing try_lock without signalling.
        if self.revision_waiter.is_some()
            && self.map(block.generation).is_some()
            && let Some(worker) = self.revision_waiter.take()
        {
            worker.unpark();
        }
        if self.late_transport_waiter.is_some()
            && self.transport_state() != Some((block.pass, block.playing))
            && let Some(waiter) = self.late_transport_waiter.take()
        {
            waiter.unpark();
        }
        if self.blocks.len() == HISTORY {
            self.blocks.pop_front();
        }
        self.blocks.push_back(block);
    }
    /// The session rate, mixer delay and transport rate (samples per second at the playback
    /// speed) of the latest block.
    pub(crate) fn mixer_delay(&self) -> Option<(soundcraft_time::SampleRate, usize, f64)> {
        self.blocks.back().map(|block| (block.sample_rate, block.system_delay_samples, block.samples_per_second))
    }
    pub(crate) fn transport_state(&self) -> Option<(u64, bool)> {
        self.blocks.back().map(|block| (block.pass, block.playing))
    }
    /// Called under the same lock as read(), so the first usable timestamp
    /// cannot land between checking the timeline and registering the waiter.
    pub(crate) fn wait_for_timing(&mut self) {
        self.timing_waiter.get_or_insert_with(std::thread::current);
    }
    pub(crate) fn wait_for_revision(&mut self) {
        self.revision_waiter.get_or_insert_with(std::thread::current);
    }
    pub(crate) fn clear_waiters(&mut self) {
        self.revision_waiter = None;
        self.timing_waiter = None;
        self.late_transport_waiter = None;
    }
    /// Most recent audible block and first future transition for this destination.
    pub(crate) fn read(&self, now: Instant, compensate: bool, offset_samples: i64) -> (Option<Presentation>, Option<Instant>) {
        let first = if compensate { self.blocks.len().saturating_sub(self.timed_blocks) } else { 0 };
        let adjusted = |block: &Presentation| block.adjusted(compensate, offset_samples);
        let Some((index, block)) =
            self.blocks.range(first..).enumerate().rev().find_map(|(i, b)| adjusted(b).filter(|b| b.heard_at <= now).map(|b| (i, b)))
        else {
            return (None, self.blocks.range(first..).filter_map(adjusted).map(|b| b.heard_at).min());
        };
        // Each block carries its own output lead, so a later publication can be due first.
        // Wake for the earliest transition; selection above then takes the latest audible block.
        let future = self
            .blocks
            .range(first + index + 1..)
            .filter_map(adjusted)
            .filter(|b| block.playing || b.playing || block.pass != b.pass || block.sample != b.sample || block.generation != b.generation)
            .map(|b| b.heard_at)
            .min();
        (Some(block.current(now)), future)
    }
}

#[derive(Default)]
pub(crate) struct CallbackMetrics {
    // One coherent observation under concurrent status reads. 0 = unobserved,
    // 1 = unknown; otherwise 2 + 2 * microseconds + retained. Valid lead <= 1 s.
    lead_reading: AtomicU64,
    pub(crate) fresh_lead_reports: AtomicU64,
    pub max_gap_us: AtomicU64,
    pub late_buffers: AtomicU64,
    pub clamps: AtomicU64,
}
impl CallbackMetrics {
    pub(crate) fn lead(&self) -> (Option<u64>, crate::OutputLeadSource) {
        use crate::OutputLeadSource::*;
        match self.lead_reading.load(Relaxed) {
            0 => (None, Unobserved),
            1 => (None, Unknown),
            v => (Some((v - 2) / 2), if v % 2 == 0 { Reported } else { Retained }),
        }
    }
}
pub(crate) type View = Arc<Mutex<Timeline>>;
pub(crate) struct Publisher {
    pub(crate) view: View,
    metrics: Arc<CallbackMetrics>,
    last_callback: Option<(Instant, f64)>,
    pub(crate) pass: u64,
    pub(crate) origin: Option<i64>,
}
impl Publisher {
    pub(crate) fn new(view: View, metrics: Arc<CallbackMetrics>) -> Self {
        Self { view, metrics, last_callback: None, pass: 0, origin: None }
    }
    pub(crate) fn observe(&mut self, timing: DeviceTiming, buffer_seconds: f64) {
        if timing.frame_offset != 0 {
            return;
        }
        let reading = timing.lead.map_or(1, |lead| 2 + 2 * lead.as_micros() as u64 + u64::from(timing.clamped));
        self.metrics.lead_reading.store(reading, Relaxed);
        if !timing.clamped {
            self.metrics.fresh_lead_reports.fetch_add(1, Relaxed);
        }
        if timing.clamped {
            self.metrics.clamps.fetch_add(1, Relaxed);
        }
        if let Some((last, size)) = self.last_callback.replace((timing.callback, buffer_seconds)) {
            let gap = timing.callback.saturating_duration_since(last);
            let gap_us = gap.as_micros() as u64;
            if gap_us > self.metrics.max_gap_us.load(Relaxed) {
                self.metrics.max_gap_us.fetch_max(gap_us, Relaxed);
            }
            if gap.as_secs_f64() > 2. * size {
                self.metrics.late_buffers.fetch_add(1, Relaxed);
            }
        }
    }
    pub(crate) fn publish(&self, block: Presentation) {
        if let Ok(mut timeline) = self.view.try_lock() {
            timeline.push(block);
        }
    }
    pub(crate) fn locate(&mut self, at: i64) {
        self.pass = self.pass.wrapping_add(1);
        self.origin = Some(at);
    }
}
/// An output protocol on the shared worker, such as the track note output or Beat Clock. Return the next
/// required wake time. A disconnected port resets its encoders before the next transport
/// snapshot. Future events are queued per port (up to 256) until their requested deadline;
/// equal timestamps retain emission order, and a full queue never evicts a note release.
/// Emit packets of at most 16 bytes.
pub trait SyncEncoder: Send {
    fn update(&mut self, block: Presentation, now: Instant, session: &Session, send: &mut dyn FnMut(Instant, &[u8])) -> Option<Instant>;
    fn reset(&mut self);
    /// Previously queued deadlines on a compensated route are no longer usable.
    fn timing_lost(&mut self) {
        self.reset();
    }
    fn stop(&mut self, _send: &mut dyn FnMut(&[u8])) {}
    /// Everything the last update sent for the present went out to the port: none of it failed.
    fn delivered(&mut self) {}
    /// All MIDI Notes Off: release now whatever this encoder holds on the device. Playback
    /// continues with the next update.
    fn panic(&mut self, _send: &mut dyn FnMut(&[u8])) {}
    fn remap(&mut self, _old: &Session, _new: &Session) {}
    fn error(&self) -> Option<&'static str> {
        None
    }
    fn take_catchups(&mut self) -> u64 {
        0
    }
    fn take_stale_dropped(&mut self) -> u64 {
        0
    }
}
