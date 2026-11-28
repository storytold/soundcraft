//! The latency the audio backend reports for each open stream: how long after an output callback
//! its first frame is heard, and how long before an input callback its first frame was captured.
//! Both come from cpal's callback timestamps (ASIO: the driver's output and input latencies;
//! WASAPI: the device position and stream latency; Core Audio: the I/O buffer plus the device
//! latency and safety offset).

use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::time::Duration;

/// A report longer than this is treated as missing: it would come from a mismatched clock.
const PLAUSIBLE: Duration = Duration::from_secs(1);

/// Stored once the stream is lost: nothing is reported from then on.
const LOST: u64 = u64::MAX;

/// The last plausible report of one stream, written by its audio callback and read by the UI.
#[derive(Debug, Default)]
pub struct ReportedLatency(
    /// Nanoseconds + 1; 0 means nothing reported yet, so a zero-latency report stays visible;
    /// [`LOST`] once the stream failed.
    AtomicU64,
);

impl ReportedLatency {
    /// Record one callback's report. A missing or implausible report keeps the last valid one,
    /// and a lost stream keeps reporting nothing.
    pub fn observe(&self, report: Option<Duration>) {
        if let Some(latency) = report.filter(|d| *d <= PLAUSIBLE) {
            let nanos = u64::try_from(latency.as_nanos()).unwrap_or(u64::MAX).min(LOST - 2);
            let mut seen = self.0.load(Relaxed);
            while seen != LOST {
                match self.0.compare_exchange_weak(seen, nanos + 1, Relaxed, Relaxed) {
                    Ok(_) => break,
                    Err(now) => seen = now,
                }
            }
        }
    }

    /// The stream failed (its error callback said it is gone): its figures are no longer the
    /// device's, so nothing is reported until a new stream replaces it.
    pub fn lose(&self) {
        self.0.store(LOST, Relaxed);
    }

    /// The last plausible report, or `None` before the stream reported one or after it was lost.
    pub fn get(&self) -> Option<Duration> {
        match self.0.load(Relaxed) {
            0 | LOST => None,
            v => Some(Duration::from_nanos(v - 1)),
        }
    }
}

/// A report as the window and the commands show it: a zero report is what a backend gives when it
/// does not know (seen with a Windows virtual input), so it is shown as not reported.
/// `Player::output_latency` and `Recorder::input_latency` still return it as given.
pub fn shown(latency: Option<Duration>) -> Option<Duration> {
    latency.filter(|l| !l.is_zero())
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
    fn a_lost_stream_reports_nothing_and_its_late_callbacks_are_ignored() {
        let latency = ReportedLatency::default();
        latency.observe(Some(Duration::from_micros(6750)));
        latency.lose();
        assert_eq!(latency.get(), None, "a dead stream's figures are not shown");
        latency.observe(Some(Duration::from_micros(6750)));
        assert_eq!(latency.get(), None, "a callback after the error does not bring them back");
    }

    #[test]
    fn a_zero_report_is_shown_as_not_reported() {
        // A Windows virtual input reported 0 samples: the backend did not know, it was not instant.
        assert_eq!(shown(Some(Duration::ZERO)), None);
        assert_eq!(shown(Some(Duration::from_micros(6750))), Some(Duration::from_micros(6750)));
        assert_eq!(shown(None), None);
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
