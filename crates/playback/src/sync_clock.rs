//! Presentation timestamps shared by external MIDI synchronization protocols.
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
    pub lead: Duration,
    pub frame_offset: usize,
    pub clamped: bool,
}

impl DeviceTiming {
    pub(crate) fn new(callback: Instant, reported: Option<Duration>, frame_offset: usize) -> Self {
        let valid = reported.filter(|&v| v <= Duration::from_secs(1));
        Self { callback, lead: valid.unwrap_or_default(), frame_offset, clamped: valid.is_none() }
    }
    pub(crate) fn heard_instant(self, device_rate: f64, frame_index: usize) -> Instant {
        let frames = self.frame_offset.saturating_add(frame_index);
        let elapsed = Duration::try_from_secs_f64(frames as f64 / device_rate).unwrap_or_default();
        self.callback.checked_add(self.lead).and_then(|t| t.checked_add(elapsed)).unwrap_or(self.callback)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Presentation {
    pub pass: u64,
    pub origin: i64,
    pub playing: bool,
    pub sample: f64,
    pub heard_at: Instant,
    pub samples_per_second: f64,
    pub limit: f64,
    pub looped: Option<Range>,
    pub end: Option<i64>,
    pub lead: Duration,
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
    fn adjusted(mut self, compensate: bool, offset_seconds: f64) -> Self {
        self.heard_at = shift(self.heard_at, offset_seconds - if compensate { 0.0 } else { self.lead.as_secs_f64() });
        self
    }
    pub(crate) fn current(mut self, now: Instant) -> Self {
        let projected = self.sample_heard_at(now);
        if self.playing {
            if let Some(range) = self.looped {
                if projected >= range.end as f64 {
                    let wraps = 1 + ((projected - range.end as f64) / range.len() as f64).floor() as i64;
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

pub(crate) fn shift(at: Instant, seconds: f64) -> Instant {
    let Ok(dt) = Duration::try_from_secs_f64(seconds.abs()) else { return at };
    if seconds < 0. { at.checked_sub(dt) } else { at.checked_add(dt) }.unwrap_or(at)
}

// Keep scheduled buffers until they can actually be heard. Publishing two future
// callbacks back-to-back must not conceal the first one's start or its final clocks.
const HISTORY: usize = 2048;
pub(crate) struct Timeline {
    blocks: VecDeque<Presentation>,
}
impl Default for Timeline {
    fn default() -> Self {
        Self { blocks: VecDeque::with_capacity(HISTORY) }
    }
}
impl Timeline {
    pub(crate) fn push(&mut self, block: Presentation) {
        if self.blocks.len() == HISTORY {
            self.blocks.pop_front();
        }
        self.blocks.push_back(block);
    }
    /// Most recent audible block and first future transition for this destination.
    pub(crate) fn read(&self, now: Instant, compensate: bool, offset_seconds: f64) -> (Option<Presentation>, Option<Instant>) {
        let adjusted = |block: &Presentation| block.adjusted(compensate, offset_seconds);
        let Some(index) = self.blocks.iter().rposition(|block| adjusted(block).heard_at <= now) else {
            return (None, self.blocks.front().map(|block| adjusted(block).heard_at));
        };
        let block = adjusted(&self.blocks[index]);
        let future = self.blocks.range(index + 1..).find(|b| block.playing || b.playing || block.pass != b.pass || block.sample != b.sample);
        (Some(block.current(now)), future.map(|b| adjusted(b).heard_at))
    }
}

#[derive(Default)]
pub(crate) struct CallbackMetrics {
    pub lead_us: AtomicU64,
    pub max_gap_us: AtomicU64,
    pub late_buffers: AtomicU64,
    pub clamps: AtomicU64,
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
        self.metrics.lead_us.store(timing.lead.as_micros() as u64, Relaxed);
        if timing.clamped {
            self.metrics.clamps.fetch_add(1, Relaxed);
        }
        if let Some((last, size)) = self.last_callback.replace((timing.callback, buffer_seconds)) {
            let gap = timing.callback.saturating_duration_since(last);
            self.metrics.max_gap_us.fetch_max(gap.as_micros() as u64, Relaxed);
            if gap.as_secs_f64() > 2. * size {
                self.metrics.late_buffers.fetch_add(1, Relaxed);
            }
        }
    }
    pub(crate) fn locate(&mut self, at: i64) {
        self.pass = self.pass.wrapping_add(1);
        self.origin = Some(at);
    }
}
/// An output protocol on the shared worker. Return the next required wake time.
/// A disconnected port resets its encoders before the next transport snapshot.
pub trait SyncEncoder: Send {
    fn update(&mut self, block: Presentation, now: Instant, session: &Session, send: &mut dyn FnMut(Instant, &[u8])) -> Option<Instant>;
    fn reset(&mut self);
    fn stop(&mut self, _send: &mut dyn FnMut(&[u8])) {}
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
