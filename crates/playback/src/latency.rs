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

/// The process-wide origin that [`InstantCell`] counts from: [`PLAUSIBLE`] before it was first
/// needed, so a capture moved back by any accepted input latency is still after it.
fn epoch() -> Instant {
    static EPOCH: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    *EPOCH.get_or_init(|| {
        let now = Instant::now();
        now.checked_sub(PLAUSIBLE).unwrap_or(now)
    })
}

/// An instant an audio callback writes and another thread reads, without locking.
#[derive(Debug, Default)]
pub struct InstantCell(
    /// Nanoseconds after [`epoch`] + 1; 0 means none.
    AtomicU64,
);

impl InstantCell {
    /// A cell whose instants may be as early as [`PLAUSIBLE`] before now (the epoch is fixed no
    /// later than that).
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

/// The stream clock's time of `instant`, in seconds. Within one host the input and output
/// streams share this clock (ASIO: the driver's system time; WASAPI: the performance counter;
/// Core Audio: the host time), and it stamps the device's own moments rather than the time a
/// callback happens to run.
pub fn clock_seconds(instant: cpal::StreamInstant) -> f64 {
    instant.as_nanos() as f64 / 1e9
}

/// A cell for a stream clock time (see [`clock_seconds`]) that the input callback writes once.
#[derive(Debug, Default)]
pub struct ClockCell(
    /// `f64` bits + 1; 0 means none.
    AtomicU64,
);

impl ClockCell {
    /// Set it only when it holds none, so the first writer wins.
    pub fn set_if_empty(&self, seconds: f64) {
        let _ = self.0.compare_exchange(0, seconds.to_bits().wrapping_add(1), Relaxed, Relaxed);
    }
    pub fn clear(&self) {
        self.0.store(0, Relaxed);
    }
    pub fn get(&self) -> Option<f64> {
        match self.0.load(Relaxed) {
            0 => None,
            v => Some(f64::from_bits(v.wrapping_sub(1))),
        }
    }
}

/// A moment told two ways: by when an audio callback ran, moved by the latency its stream
/// reports, and by the stream clock (see [`clock_seconds`]) as the backend stamped it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Moment {
    pub at: Instant,
    pub clock: Option<f64>,
}

impl Moment {
    /// The moment `seconds` later (earlier when negative).
    pub fn shifted(self, seconds: f64) -> Option<Moment> {
        Some(Moment { at: shift(self.at, seconds)?, clock: self.clock.map(|c| c + seconds) })
    }
}

/// A session sample and when it was heard at the output.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Heard {
    /// Session sample position, fractional when the device runs at another rate.
    pub sample: f64,
    pub when: Moment,
}

/// What the output callbacks say about when session samples are heard: the first sample of the
/// pass that the latest Play started, and the first sample of the latest callback. The audio
/// thread only `try_lock`s; a busy lock skips one update of the latest callback, and the pass's
/// stamp waits for the next callback.
#[derive(Debug, Default)]
pub struct HeardStamps {
    pass: Mutex<Option<Heard>>,
    last: Mutex<Option<Heard>>,
}

impl HeardStamps {
    /// Note the latest callback's `heard`, and publish `pass`, the pass's first sample, taking it
    /// when the lock is free (it stays for the next callback when it is not).
    pub(crate) fn record(&self, heard: Heard, pass: &mut Option<Heard>) {
        if pass.is_some()
            && let Ok(mut p) = self.pass.try_lock()
        {
            *p = pass.take();
        }
        if let Ok(mut l) = self.last.try_lock() {
            *l = Some(heard);
        }
    }

    /// When `sample` was (or will be) heard at `rate` session samples per second: from the start
    /// of the pass when that is the sample, else projected from the latest callback.
    pub fn heard_at(&self, sample: f64, rate: f64) -> Option<Moment> {
        let pass = self.pass.lock().ok().and_then(|p| *p);
        if let Some(p) = pass.filter(|p| (p.sample - sample).abs() < 0.5) {
            return Some(p.when);
        }
        let last = self.last.lock().ok().and_then(|l| *l)?;
        last.when.shifted((sample - last.sample) / rate.max(1.0))
    }
}

/// `at` moved by `seconds` (earlier when negative).
pub fn shift(at: Instant, seconds: f64) -> Option<Instant> {
    let d = Duration::try_from_secs_f64(seconds.abs()).ok()?;
    if seconds < 0.0 { at.checked_sub(d) } else { at.checked_add(d) }
}

/// Session samples a take's first sample precedes the record start by: from its capture at
/// `captured` to the moment `heard` that the record start was heard, at `rate`.
///
/// The stream clock decides when both moments have it: a callback can run late (an input
/// callback waits while the mix renders the output in the same device cycle, longer the heavier
/// the session), and timing by the callbacks would place the take late by that wait. The clocks
/// are trusted only within half the reported `round_trip` of the callback timing, the most such
/// a wait can be; past that the backend stamps its moments otherwise than expected, and the
/// callback timing places the take.
pub fn placement_offset(heard: Moment, captured: Moment, rate: f64, round_trip: Option<Duration>) -> i64 {
    let seconds = if heard.at >= captured.at {
        heard.at.duration_since(captured.at).as_secs_f64()
    } else {
        -captured.at.duration_since(heard.at).as_secs_f64()
    };
    let by_callbacks = (seconds * rate).round() as i64;
    let Some(by_clock) = heard.clock.zip(captured.clock).map(|(h, c)| ((h - c) * rate).round() as i64) else {
        return by_callbacks;
    };
    let bound = round_trip.map_or(0.0, |rt| rt.as_secs_f64() * rate / 2.0);
    if (by_clock.abs_diff(by_callbacks) as f64) <= bound {
        by_clock
    } else {
        log::warn!("stream clock places the take {by_clock} samples early, the callbacks {by_callbacks}: using the callbacks");
        by_callbacks
    }
}

/// When the first frame of an input callback was captured, on the stream clock.
///
/// With `device_latency` (Core Audio, see [`coreaudio_device_latency`]) the time is the callback's
/// host time less that latency; otherwise it is cpal's capture time.
#[cfg(not(target_os = "freebsd"))]
pub(crate) fn capture_clock(info: &cpal::InputCallbackInfo, device_latency: Option<Duration>) -> f64 {
    let t = info.timestamp();
    device_latency.map_or_else(|| clock_seconds(t.capture), |l| from_host_time(clock_seconds(t.callback), l, true))
}

/// When the first frame of an output callback will be heard, on the stream clock.
///
/// With `device_latency` (Core Audio, see [`coreaudio_device_latency`]) the time is the callback's
/// host time plus that latency; otherwise it is cpal's playback time.
pub(crate) fn playback_clock(info: &cpal::OutputCallbackInfo, device_latency: Option<Duration>) -> f64 {
    let t = info.timestamp();
    device_latency.map_or_else(|| clock_seconds(t.playback), |l| from_host_time(clock_seconds(t.callback), l, false))
}

/// A Core Audio callback's host time (seconds on the stream clock) moved to the converters: an
/// input frame was captured `device_latency` before the HAL stamped it, an output frame is heard
/// `device_latency` after.
pub(crate) fn from_host_time(host_time: f64, device_latency: Duration, input: bool) -> f64 {
    if input { host_time - device_latency.as_secs_f64() } else { host_time + device_latency.as_secs_f64() }
}

/// The device latency of a Core Audio device for one direction (`kAudioDevicePropertyLatency`:
/// the converters and the driver, not the buffer or the safety offset), at `rate` Hz.
///
/// cpal 0.18's Core Audio stamps cannot place a take: the callback's `AudioTimeStamp.mHostTime`
/// is already the HAL's time for the buffer's first frame (one buffer before the input callback
/// runs and one after the output callback runs, safety offset included), and cpal moves it by the
/// buffer, the safety offset and the device latency again (RustAudio/cpal#1398). Measured on a
/// Fireface UFX through a cable, the host time moved by the device latency alone gives the round
/// trip the converters show; cpal's capture and playback times give twice the buffer and both
/// safety offsets more. `None` when the device or the property cannot be read; the clocks then
/// keep cpal's times (and the callback timing places the take, see [`placement_offset`]).
#[cfg(target_os = "macos")]
pub(crate) fn coreaudio_device_latency(device: &cpal::Device, input: bool, rate: u32) -> Option<Duration> {
    use coreaudio::audio_unit::macos_helpers::{audio_unit_from_device_id_uninitialized, get_device_id_from_name};
    use coreaudio::audio_unit::{Element, Scope};
    /// `kAudioDevicePropertyLatency` ('ltnc').
    const DEVICE_LATENCY: u32 = u32::from_be_bytes(*b"ltnc");
    let name = crate::devices::device_name(device)?;
    let id = get_device_id_from_name(&name, input)?;
    let unit = audio_unit_from_device_id_uninitialized(id, input).ok()?;
    let scope = if input { Scope::Input } else { Scope::Output };
    let frames: u32 = unit.get_property(DEVICE_LATENCY, scope, Element::Output).ok()?;
    Some(Duration::from_secs_f64(f64::from(frames) / f64::from(rate.max(1))))
}

/// Elsewhere the backends' own capture and playback times are used as they are.
#[cfg(not(target_os = "macos"))]
pub(crate) fn coreaudio_device_latency(_device: &cpal::Device, _input: bool, _rate: u32) -> Option<Duration> {
    None
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
    fn a_pass_stamp_skipped_on_a_busy_lock_is_published_by_the_next_callback() {
        let stamps = HeardStamps::default();
        let moment = |s: u64| Moment { at: epoch() + Duration::from_secs(s), clock: Some(s as f64) };
        let mut pending = Some(Heard { sample: 0.0, when: moment(1) });
        stamps.record(Heard { sample: 0.0, when: moment(1) }, &mut pending);
        assert_eq!(pending, None);
        // A second Play from the same sample while the UI reads the pass.
        let start = Heard { sample: 0.0, when: moment(10) };
        let mut pending = Some(start);
        {
            let _reading = stamps.pass.lock().unwrap();
            stamps.record(start, &mut pending);
        }
        assert_eq!(pending, Some(start), "kept for the next callback");
        stamps.record(Heard { sample: 512.0, when: moment(11) }, &mut pending);
        assert_eq!(pending, None);
        assert_eq!(stamps.heard_at(0.0, 48_000.0), Some(start.when), "the new pass's start, not the old one");
    }

    #[test]
    fn an_instant_cell_keeps_a_capture_from_before_it_was_made() {
        let cell = InstantCell::new();
        // The first callback after the recorder opened, its first frame captured 20 ms earlier.
        let captured = Instant::now().checked_sub(Duration::from_millis(20)).unwrap();
        cell.set_if_empty(captured);
        let kept = cell.get().unwrap();
        assert!(kept.max(captured) - kept.min(captured) < Duration::from_micros(1), "{kept:?} vs {captured:?}");
        cell.clear();
        cell.set(captured);
        let kept = cell.get().unwrap();
        assert!(kept.max(captured) - kept.min(captured) < Duration::from_micros(1));
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
        // Play from 1_000; the first callback's first sample is heard at t0, 100 s on the clock.
        let start = Moment { at: t0, clock: Some(100.0) };
        stamps.record(Heard { sample: 1_000.0, when: start }, &mut Some(Heard { sample: 1_000.0, when: start }));
        // Later callbacks move the latest stamp on.
        stamps.record(Heard { sample: 49_000.0, when: start.shifted(1.0).unwrap() }, &mut None);
        assert_eq!(stamps.heard_at(1_000.0, 48_000.0), Some(start), "the pass start is exact");
        // A punch at 25_000 is projected from the latest callback: half a second after the start.
        let punch = stamps.heard_at(25_000.0, 48_000.0).unwrap();
        assert!(punch.at.duration_since(t0).abs_diff(Duration::from_millis(500)) < Duration::from_micros(1), "{punch:?}");
        assert!(punch.clock.is_some_and(|c| (c - 100.5).abs() < 1e-9), "{punch:?}");
    }

    /// Moments `seconds` apart by both the callbacks and the clock, from `at` and 50 s on the clock.
    fn moment(at: Instant, seconds: f64) -> Moment {
        Moment { at: shift(at, seconds).unwrap(), clock: Some(50.0 + seconds) }
    }

    #[test]
    fn the_placement_offset_is_how_long_capture_began_before_the_start_was_heard() {
        let t = Instant::now() + Duration::from_secs(1);
        let rt = Some(Duration::from_micros(12_979)); // 623 samples
        let heard = moment(t, 0.0);
        assert_eq!(placement_offset(heard, moment(t, -607.0 / 48_000.0), 48_000.0, rt), 607);
        assert_eq!(placement_offset(heard, moment(t, 0.001), 48_000.0, rt), -48);
        // Without the clock on either side the callbacks decide.
        let no_clock = |m: Moment| Moment { clock: None, ..m };
        assert_eq!(placement_offset(no_clock(heard), moment(t, -0.01), 48_000.0, rt), 480);
    }

    /// An RME Fireface UFX under Core Audio at 1024 frames and 48 kHz (device latency 22 in and 41
    /// out, safety offset 24 each way): the output callback's host time is 2096 samples after the
    /// input callback's for a 2159-sample round trip through a cable. cpal's capture and playback
    /// times add the buffer and the safety offset again (4255) and fall back to the callbacks; the
    /// host times moved by the device latency alone give the round trip.
    #[test]
    fn core_audio_host_times_moved_by_the_device_latency_give_the_round_trip() {
        let rate = 48_000.0;
        let t = Instant::now() + Duration::from_secs(1);
        let (input_host, output_host) = (50.0, 50.0 + 2_096.0 / rate);
        let rt = Some(Duration::from_secs_f64(2_159.0 / rate));
        let heard = |clock| Moment { at: t, clock: Some(clock) };
        let captured = |clock| Moment { at: shift(t, -2_131.0 / rate).unwrap(), clock: Some(clock) };
        let cpal = placement_offset(heard(output_host + 1_089.0 / rate), captured(input_host - 1_070.0 / rate), rate, rt);
        assert_eq!(cpal, 2_131, "cpal's times are 4255 samples apart: the callbacks place the take");
        let latency = |frames: f64| Duration::from_secs_f64(frames / rate);
        let fixed = placement_offset(
            heard(from_host_time(output_host, latency(41.0), false)),
            captured(from_host_time(input_host, latency(22.0), true)),
            rate,
            rt,
        );
        assert_eq!(fixed, 2_159);
    }

    #[test]
    fn an_input_callback_that_runs_late_does_not_place_the_take_late() {
        // Core Audio, buffer 1024 (Mac mini and Fireface UFX): the input callback of a device
        // cycle runs after the output callback has rendered the mix, here 0.4 ms later. The
        // clock says the take began 2159 samples (input 1070 + output 1089) before the start
        // was heard; the callbacks, 19 fewer, which placed takes 13 to 18 samples late.
        let t = Instant::now() + Duration::from_secs(1);
        let rate = 48_000.0;
        let heard = moment(t, 1_089.0 / rate);
        let waited = 19.0 / rate;
        let captured = Moment { at: shift(t, waited - 1_070.0 / rate).unwrap(), clock: Some(50.0 - 1_070.0 / rate) };
        let rt = Some(Duration::from_secs_f64(2_159.0 / rate));
        assert_eq!(placement_offset(heard, captured, rate, rt), 2_159);
        // A heavier session waits longer, up to the device buffer; the clock still decides.
        let busy = Moment { at: shift(t, 1_000.0 / rate - 1_070.0 / rate).unwrap(), ..captured };
        assert_eq!(placement_offset(heard, busy, rate, rt), 2_159);
    }

    #[test]
    fn a_clock_far_from_the_callbacks_is_not_trusted() {
        // Clock moments a backend stamps otherwise than expected (here two buffers of 1024 off)
        // fall back to the callbacks, as does a stream that reported no latency to bound them.
        let t = Instant::now() + Duration::from_secs(1);
        let rate = 48_000.0;
        let heard = moment(t, 1_089.0 / rate);
        let captured = Moment { at: shift(t, -1_070.0 / rate).unwrap(), clock: Some(50.0 - 3_118.0 / rate) };
        let rt = Some(Duration::from_secs_f64(2_159.0 / rate));
        assert_eq!(placement_offset(heard, captured, rate, rt), 2_159);
        let late = Moment { at: shift(t, 19.0 / rate - 1_070.0 / rate).unwrap(), clock: Some(50.0 - 1_070.0 / rate) };
        assert_eq!(placement_offset(heard, late, rate, None), 2_140);
    }

    #[test]
    fn a_clock_cell_holds_the_first_time_written() {
        let cell = ClockCell::default();
        assert_eq!(cell.get(), None);
        cell.set_if_empty(0.0);
        cell.set_if_empty(7.5);
        assert_eq!(cell.get(), Some(0.0), "zero is a time, and the first writer wins");
        cell.clear();
        cell.set_if_empty(7.5);
        assert_eq!(cell.get(), Some(7.5));
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
