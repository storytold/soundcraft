//! The latency the audio backend reports for each open stream: how long after an output callback
//! its first frame is heard, and how long before an input callback its first frame was captured.
//! Both come from cpal's callback timestamps (ASIO: the driver's output and input latencies;
//! WASAPI and Core Audio: the device position and stream latency).

use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::time::{Duration, Instant};

/// A report longer than this is treated as missing: it would come from a mismatched clock.
const PLAUSIBLE: Duration = Duration::from_secs(1);

/// The last plausible report of one stream, written by its audio callback and read by the UI.
#[derive(Debug, Default)]
pub struct ReportedLatency(
    /// Nanoseconds + 1; 0 means nothing reported yet, so a zero-latency report stays visible.
    AtomicU64,
);

impl ReportedLatency {
    /// Record one callback's report. A missing or implausible report keeps the last valid one.
    pub fn observe(&self, report: Option<Duration>) {
        if let Some(latency) = report.filter(|d| *d <= PLAUSIBLE) {
            let nanos = u64::try_from(latency.as_nanos()).unwrap_or(u64::MAX);
            self.0.store(nanos.saturating_add(1), Relaxed);
        }
    }

    /// The last plausible report, or `None` before the stream reported one.
    pub fn get(&self) -> Option<Duration> {
        match self.0.load(Relaxed) {
            0 => None,
            v => Some(Duration::from_nanos(v - 1)),
        }
    }
}

/// The process-wide origin that [`InstantCell`] counts from.
fn epoch() -> Instant {
    static EPOCH: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    *EPOCH.get_or_init(Instant::now)
}

/// An instant an audio callback writes and another thread reads, without locking.
#[derive(Debug, Default)]
pub struct InstantCell(
    /// Nanoseconds after [`epoch`] + 1; 0 means none.
    AtomicU64,
);

impl InstantCell {
    /// A cell whose instants may be as early as now (the epoch is fixed no later than this).
    pub fn new() -> Self {
        epoch();
        Self::default()
    }
    pub fn set(&self, at: Instant) {
        let nanos = u64::try_from(at.saturating_duration_since(epoch()).as_nanos()).unwrap_or(u64::MAX - 1);
        self.0.store(nanos + 1, Relaxed);
    }
    /// Set it only when it holds none, so the first writer wins.
    pub fn set_if_empty(&self, at: Instant) {
        let nanos = u64::try_from(at.saturating_duration_since(epoch()).as_nanos()).unwrap_or(u64::MAX - 1);
        let _ = self.0.compare_exchange(0, nanos + 1, Relaxed, Relaxed);
    }
    pub fn clear(&self) {
        self.0.store(0, Relaxed);
    }
    pub fn get(&self) -> Option<Instant> {
        match self.0.load(Relaxed) {
            0 => None,
            v => epoch().checked_add(Duration::from_nanos(v - 1)),
        }
    }
}

/// A session sample and when it was heard at the output.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Heard {
    /// Session sample position, fractional when the device runs at another rate.
    pub sample: f64,
    pub at: Instant,
}

/// What the output callbacks say about when session samples are heard: the first sample of the
/// pass that the latest Play started, and the first sample of the latest callback. The audio
/// thread only `try_lock`s; a busy lock skips one update.
#[derive(Debug, Default)]
pub struct HeardStamps {
    pass: Mutex<Option<Heard>>,
    last: Mutex<Option<Heard>>,
}

impl HeardStamps {
    pub(crate) fn record(&self, heard: Heard, pass_started: bool) {
        if pass_started && let Ok(mut p) = self.pass.try_lock() {
            *p = Some(heard);
        }
        if let Ok(mut l) = self.last.try_lock() {
            *l = Some(heard);
        }
    }

    /// When `sample` was (or will be) heard at `rate` session samples per second: from the start
    /// of the pass when that is the sample, else projected from the latest callback.
    pub fn heard_at(&self, sample: f64, rate: f64) -> Option<Instant> {
        let pass = self.pass.lock().ok().and_then(|p| *p);
        if let Some(p) = pass.filter(|p| (p.sample - sample).abs() < 0.5) {
            return Some(p.at);
        }
        let last = self.last.lock().ok().and_then(|l| *l)?;
        shift(last.at, (sample - last.sample) / rate.max(1.0))
    }
}

/// `at` moved by `seconds` (earlier when negative).
pub fn shift(at: Instant, seconds: f64) -> Option<Instant> {
    let d = Duration::try_from_secs_f64(seconds.abs()).ok()?;
    if seconds < 0.0 { at.checked_sub(d) } else { at.checked_add(d) }
}

/// Session samples a take's first sample precedes the record start by: from its capture at
/// `captured` to the moment `heard` that the record start was heard, at `rate`.
pub fn placement_offset(heard: Instant, captured: Instant, rate: f64) -> i64 {
    let seconds = if heard >= captured { heard.duration_since(captured).as_secs_f64() } else { -captured.duration_since(heard).as_secs_f64() };
    (seconds * rate).round() as i64
}

/// Output latency of one callback: from the callback to when its first frame is played.
pub(crate) fn output_report(info: &cpal::OutputCallbackInfo) -> Option<Duration> {
    let t = info.timestamp();
    t.playback.checked_duration_since(t.callback)
}

/// Input latency of one callback: from when its first frame was captured to the callback.
#[cfg(not(target_os = "freebsd"))]
pub(crate) fn input_report(info: &cpal::InputCallbackInfo) -> Option<Duration> {
    let t = info.timestamp();
    t.callback.checked_duration_since(t.capture)
}

/// `latency` in frames at `rate` Hz, rounded to the nearest frame.
pub fn frames(latency: Duration, rate: u32) -> u64 {
    (latency.as_secs_f64() * f64::from(rate)).round() as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use cpal::{InputCallbackInfo, InputStreamTimestamp, OutputCallbackInfo, OutputStreamTimestamp, StreamInstant};

    fn at(micros: u64) -> StreamInstant {
        StreamInstant::from_micros(micros)
    }

    #[test]
    fn a_stream_reports_nothing_until_its_first_plausible_callback() {
        let latency = ReportedLatency::default();
        assert_eq!(latency.get(), None);
        latency.observe(None);
        latency.observe(Some(Duration::from_secs(2)));
        assert_eq!(latency.get(), None, "a missing or implausible report is not a measurement");
        latency.observe(Some(Duration::ZERO));
        assert_eq!(latency.get(), Some(Duration::ZERO), "zero is a valid report");
    }

    #[test]
    fn a_missing_or_implausible_report_keeps_the_last_valid_latency() {
        let latency = ReportedLatency::default();
        latency.observe(Some(Duration::from_micros(6750)));
        latency.observe(None);
        latency.observe(Some(Duration::from_secs(5)));
        assert_eq!(latency.get(), Some(Duration::from_micros(6750)));
        latency.observe(Some(Duration::from_micros(5896)));
        assert_eq!(latency.get(), Some(Duration::from_micros(5896)));
    }

    #[test]
    fn an_instant_cell_holds_one_instant_and_the_first_writer_wins() {
        let cell = InstantCell::new();
        assert_eq!(cell.get(), None);
        let t = Instant::now();
        cell.set_if_empty(t);
        cell.set_if_empty(t + Duration::from_millis(5));
        let got = cell.get().map(|g| g.saturating_duration_since(t).max(t.saturating_duration_since(g)));
        assert!(got.is_some_and(|d| d < Duration::from_micros(1)), "{got:?}");
        cell.clear();
        assert_eq!(cell.get(), None);
    }

    #[test]
    fn the_pass_start_or_the_latest_callback_tells_when_a_sample_is_heard() {
        let stamps = HeardStamps::default();
        let t0 = Instant::now();
        assert_eq!(stamps.heard_at(1_000.0, 48_000.0), None);
        // Play from 1_000; the first callback's first sample is heard at t0.
        stamps.record(Heard { sample: 1_000.0, at: t0 }, true);
        // Later callbacks move the latest stamp on.
        stamps.record(Heard { sample: 49_000.0, at: t0 + Duration::from_secs(1) }, false);
        assert_eq!(stamps.heard_at(1_000.0, 48_000.0), Some(t0), "the pass start is exact");
        // A punch at 25_000 is projected from the latest callback: half a second after t0.
        let punch = stamps.heard_at(25_000.0, 48_000.0).map(|at| at.duration_since(t0));
        assert!(punch.is_some_and(|d| d.abs_diff(Duration::from_millis(500)) < Duration::from_micros(1)), "{punch:?}");
    }

    #[test]
    fn the_placement_offset_is_how_long_capture_began_before_the_start_was_heard() {
        let heard = Instant::now();
        let early = heard - Duration::from_micros(12_646); // 607 samples at 48 kHz
        assert_eq!(placement_offset(heard, early, 48_000.0), 607);
        let late = heard + Duration::from_millis(1);
        assert_eq!(placement_offset(heard, late, 48_000.0), -48);
    }

    #[test]
    fn output_latency_is_from_the_callback_to_playback() {
        // ASIO: playback = callback + the driver's output latency (324 frames at 48 kHz here).
        let info = OutputCallbackInfo::new(OutputStreamTimestamp { callback: at(1_000_000), playback: at(1_006_750) });
        let latency = output_report(&info);
        assert_eq!(latency, Some(Duration::from_micros(6750)));
        assert_eq!(latency.map(|l| frames(l, 48_000)), Some(324));
        // A playback instant before the callback is not a latency.
        let info = OutputCallbackInfo::new(OutputStreamTimestamp { callback: at(1_000_000), playback: at(999_000) });
        assert_eq!(output_report(&info), None);
    }

    #[test]
    #[cfg(not(target_os = "freebsd"))]
    fn input_latency_is_from_capture_to_the_callback() {
        // ASIO: capture = callback - the driver's input latency (283 frames at 48 kHz here).
        let info = InputCallbackInfo::new(InputStreamTimestamp { callback: at(2_000_000), capture: at(1_994_104) });
        let latency = input_report(&info);
        assert_eq!(latency.map(|l| frames(l, 48_000)), Some(283));
        let info = InputCallbackInfo::new(InputStreamTimestamp { callback: at(2_000_000), capture: at(2_000_500) });
        assert_eq!(input_report(&info), None, "a capture after the callback is not a latency");
    }
}
