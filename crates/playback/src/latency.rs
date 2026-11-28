//! Latency and timing for placing takes. The latency the audio backend reports for each open
//! stream: how long after an output callback its first frame is heard, and how long before an
//! input callback its first frame was captured. Both come from cpal's callback timestamps (ASIO:
//! the driver's output and input latencies; WASAPI and Core Audio: the device position and stream
//! latency). Also the stamps of when session samples were heard ([`HeardStamps`]) and captured,
//! told on the stream clock ([`clock_seconds`]; on Core Audio the host time moved by the device
//! latency), and from them a take's [`placement_offset`], with [`nearest_round`] for loop
//! recording.

use std::sync::Mutex;
use std::sync::atomic::{
    AtomicU64,
    Ordering::{Acquire, Relaxed, Release},
};
use std::time::{Duration, Instant};

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

/// The stream clock's time of `instant`, in seconds. It stamps the device's own moments rather
/// than the time a callback happens to run. On ASIO, WASAPI and Core Audio the input and output
/// streams share it; on ALSA they do not (see [`shares_stream_clock`]).
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
/// reports, and by the stream clock (see [`clock_seconds`]) as the backend stamped it (`None`
/// where the input and output do not share one, see [`shares_stream_clock`]).
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
    /// The transport speed the callback rendered at, 1.0 at full speed: session samples went by
    /// at the session rate times this.
    pub speed: f64,
}

/// What the output callbacks say about when session samples are heard: the first sample of the
/// pass that the latest Play started, and the first sample of the latest callback. The audio
/// thread only `try_lock`s; a busy lock skips one update of the latest callback, and the pass's
/// stamp waits for the next callback.
#[derive(Debug, Default)]
pub struct HeardStamps {
    /// The pass's first sample, with the number of the pass it belongs to.
    pass: Mutex<Option<(u64, Heard)>>,
    last: Mutex<Option<Heard>>,
    /// Passes started so far, counted without a lock: a pass stamp answers only while its pass is
    /// the latest, so an older one is not taken for a pass whose stamp still waits.
    passes: AtomicU64,
    /// The first sample heard once the latest record pass began, with the number of that record
    /// pass (see [`HeardStamps::record_start_heard`]).
    record: Mutex<Option<(u64, Heard)>>,
    /// Record passes the UI has begun ([`HeardStamps::begin_record`]): a record stamp answers only
    /// for the latest.
    records: AtomicU64,
}

impl HeardStamps {
    /// Note the latest callback's `heard`, which is the first sample of a new pass when `started`;
    /// and publish `pass`, the pass's first sample still to publish, taking it when the lock is
    /// free (it stays for the next callback when it is not).
    pub(crate) fn record(&self, heard: Heard, started: bool, pass: &mut Option<Heard>) {
        if started {
            self.passes.fetch_add(1, Release);
            *pass = Some(heard);
        }
        if pass.is_some()
            && let Ok(mut p) = self.pass.try_lock()
        {
            // Only the audio thread counts passes: the count is this pass's.
            *p = pass.take().map(|h| (self.passes.load(Relaxed), h));
        }
        if let Ok(mut l) = self.last.try_lock() {
            *l = Some(heard);
        }
    }

    /// The UI begins a record pass, before it tells the audio thread: the stamp the audio thread
    /// latches for it carries this number.
    pub(crate) fn begin_record(&self) {
        self.records.fetch_add(1, Release);
    }

    /// The number of the record pass the UI began last, read by the audio thread when the command
    /// that begins it arrives.
    pub(crate) fn latest_record(&self) -> u64 {
        self.records.load(Acquire)
    }

    /// Publish `pending`, the first sample heard since a record pass began, with that pass's
    /// number, when the lock is free (it stays for the next callback when it is not).
    pub(crate) fn record_begun(&self, pending: &mut Option<(u64, Heard)>) {
        if pending.is_some()
            && let Ok(mut r) = self.record.try_lock()
        {
            *r = pending.take();
        }
    }

    /// When `sample`, the record start, was heard in the loop round the latest record pass began
    /// in, and the speed then: projected from the first sample heard once the pass began (latched
    /// by the audio thread, so later loop rounds do not move it). The record start lies within
    /// about a callback of that sample; in a loop of `round` samples the record start nearest it
    /// counts, which takes a wrap between the two into account. `None` while the latest record
    /// pass has no stamp yet (it began while nothing was heard, or a busy lock holds it back).
    pub fn record_start_heard(&self, sample: f64, rate: f64, round: Option<f64>) -> Option<(Moment, f64)> {
        let latest = self.records.load(Acquire);
        let (n, begun) = self.record.lock().ok().and_then(|r| *r)?;
        if n != latest {
            return None;
        }
        let mut ahead = sample - begun.sample;
        if let Some(len) = round.filter(|l| *l > 0.0) {
            ahead -= (ahead / len).round() * len;
        }
        Some((begun.when.shifted(ahead / (rate * begun.speed).max(1.0))?, begun.speed))
    }

    /// When `sample` was (or will be) heard at the session `rate`, and the transport speed of the
    /// stamp that tells it (see [`Heard::speed`]): from the start of the latest pass when that is
    /// the sample, else projected from the latest callback at the speed it rendered.
    pub fn heard_at(&self, sample: f64, rate: f64) -> Option<(Moment, f64)> {
        let latest = self.passes.load(Acquire);
        let pass = self.pass.lock().ok().and_then(|p| *p);
        if let Some((_, p)) = pass.filter(|(n, p)| *n == latest && (p.sample - sample).abs() < 0.5) {
            return Some((p.when, p.speed));
        }
        let last = self.last.lock().ok().and_then(|l| *l)?;
        Some((last.when.shifted((sample - last.sample) / (rate * last.speed).max(1.0))?, last.speed))
    }
}

/// `heard`, when a sample of a loop of `period` seconds is heard, moved by whole rounds to the
/// round nearest `captured`. A sample after the pass start is projected from the latest callback
/// (see [`HeardStamps::heard_at`]), which names the round that callback is in, not the round a
/// take began in. Right while the take's real offset is under half a round: only a fallback for
/// a record pass that has no stamp latched when it began (see
/// [`HeardStamps::record_start_heard`]), as a capture-to-playback gap of half a round or more
/// (a short loop, a long round trip) lands it a whole round off.
pub fn nearest_round(heard: Moment, captured: Moment, period: f64) -> Moment {
    if period.is_nan() || period <= 0.0 {
        return heard;
    }
    let gap = if captured.at >= heard.at {
        captured.at.duration_since(heard.at).as_secs_f64()
    } else {
        -heard.at.duration_since(captured.at).as_secs_f64()
    };
    let rounds = (gap / period).round();
    if rounds == 0.0 { heard } else { heard.shifted(rounds * period).unwrap_or(heard) }
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

/// Whether the input and output streams of the audio system named `host` (cpal's host name) stamp
/// their times on one clock from one origin, so that a capture time of the input can be compared
/// with a playback time of the output: Core Audio (the host time), WASAPI (the performance
/// counter) and ASIO (the driver's system time). cpal 0.18's ALSA counts each stream's times from
/// that stream's own start, so its input and output times differ by however far apart the two
/// streams were opened. There, and on systems not listed, the callback timing places takes (see
/// [`placement_offset`]).
pub fn shares_stream_clock(host: &str) -> bool {
    matches!(host, "CoreAudio" | "WASAPI" | "ASIO")
}

/// [`shares_stream_clock`] for the audio system `device` belongs to.
pub(crate) fn device_shares_stream_clock(device: &cpal::Device) -> bool {
    use cpal::traits::DeviceTrait;
    device.id().is_ok_and(|id| shares_stream_clock(id.host().name()))
}

/// When the first frame of an input callback was captured, on the stream clock; `None` when the
/// stream's system does not share its clock with the output (`shared`, see
/// [`shares_stream_clock`]).
///
/// With `device_latency` (Core Audio, see [`coreaudio_device_latency`]) the time is the callback's
/// host time less that latency; otherwise it is cpal's capture time.
#[cfg(not(target_os = "freebsd"))]
pub(crate) fn capture_clock(info: &cpal::InputCallbackInfo, device_latency: Option<Duration>, shared: bool) -> Option<f64> {
    let t = info.timestamp();
    shared.then(|| device_latency.map_or_else(|| clock_seconds(t.capture), |l| from_host_time(clock_seconds(t.callback), l, true)))
}

/// When the first frame of an output callback will be heard, on the stream clock; `None` when the
/// stream's system does not share its clock with the input (`shared`, see
/// [`shares_stream_clock`]).
///
/// With `device_latency` (Core Audio, see [`coreaudio_device_latency`]) the time is the callback's
/// host time plus that latency; otherwise it is cpal's playback time.
pub(crate) fn playback_clock(info: &cpal::OutputCallbackInfo, device_latency: Option<Duration>, shared: bool) -> Option<f64> {
    let t = info.timestamp();
    shared.then(|| device_latency.map_or_else(|| clock_seconds(t.playback), |l| from_host_time(clock_seconds(t.callback), l, false)))
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

/// A report as the window and the commands show it: a zero report is what a backend gives when it
/// does not know (seen with a Windows virtual input), so it is shown as not reported. Take
/// placement still uses it as given.
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
    fn a_take_begun_in_an_earlier_loop_round_is_timed_by_that_round() {
        let t0 = Instant::now();
        let moment = |s: f64| Moment { at: t0 + Duration::from_secs_f64(s), clock: Some(100.0 + s) };
        // A 0.25 s loop; the take began 3 ms after its start sample was heard, and the latest
        // callback, two rounds on, projects that sample to its own round.
        let captured = moment(10.003);
        let projected = moment(10.5);
        let heard = nearest_round(projected, captured, 0.25);
        assert!(heard.at.duration_since(t0).abs_diff(Duration::from_secs(10)) < Duration::from_micros(1), "{heard:?}");
        assert!(heard.clock.is_some_and(|c| (c - 110.0).abs() < 1e-9), "{heard:?}");
        assert_eq!(placement_offset(heard, captured, 48_000.0, None), -144);
        // The same round, or no loop, stays as it is.
        assert_eq!(nearest_round(moment(10.0), captured, 0.25), moment(10.0));
        assert_eq!(nearest_round(projected, captured, 0.0), projected);
    }

    #[test]
    fn a_take_whose_gap_is_over_half_a_loop_round_is_placed_by_the_round_its_pass_began_in() {
        // A 4800-sample loop (0.1 s at 48 kHz) and 3840 samples (0.08 s) from the capture of the
        // take's first sample to when the record start is heard: over half a round.
        let rate = 48_000.0;
        let t0 = Instant::now() + Duration::from_secs(1);
        let moment = |s: f64| Moment { at: shift(t0, s).unwrap(), clock: Some(100.0 + s) };
        let stamps = HeardStamps::default();
        stamps.begin_record();
        // The punch at sample 1_100: the first callback after it outputs sample 1_000 at 10 s.
        let mut pending = Some((stamps.latest_record(), Heard { sample: 1_000.0, when: moment(10.0), speed: 1.0 }));
        stamps.record_begun(&mut pending);
        assert_eq!(pending, None);
        // Rounds later the latest callback outputs sample 1_000 again, at 10.3 s.
        stamps.record(Heard { sample: 1_000.0, when: moment(10.3), speed: 1.0 }, false, &mut None);
        let start = 1_100.0;
        let heard_then = 10.0 + 100.0 / rate;
        let captured = moment(heard_then - 0.08);
        let (heard, speed) = stamps.record_start_heard(start, rate, Some(4_800.0)).unwrap();
        assert_eq!(speed, 1.0);
        assert_eq!(placement_offset(heard, captured, rate, None), 3_840);
        assert_eq!(placement_offset(heard, captured, rate, Some(Duration::from_millis(80))), 3_840, "by the clock as well");
        // The round nearest the capture, from the latest callback, is a whole round off.
        let (projected, _) = stamps.heard_at(start, rate).unwrap();
        assert_eq!(placement_offset(nearest_round(projected, captured, 0.1), captured, rate, None), -960);
        // A record start just after a wrap from the latched sample is heard just after it.
        let mut pending = Some((stamps.latest_record(), Heard { sample: 4_790.0, when: moment(20.0), speed: 1.0 }));
        stamps.record_begun(&mut pending);
        let (heard, _) = stamps.record_start_heard(10.0, rate, Some(4_800.0)).unwrap();
        assert!(heard.clock.is_some_and(|c| (c - (120.0 + 20.0 / rate)).abs() < 1e-9), "{heard:?}");
        // A later record pass whose stamp is not published yet has none: the earlier one's does not answer.
        stamps.begin_record();
        assert_eq!(stamps.record_start_heard(start, rate, Some(4_800.0)), None);
    }

    #[test]
    fn a_pass_stamp_skipped_on_a_busy_lock_is_published_by_the_next_callback() {
        let stamps = HeardStamps::default();
        let moment = |s: u64| Moment { at: epoch() + Duration::from_secs(s), clock: Some(s as f64) };
        let mut pending = None;
        stamps.record(Heard { sample: 0.0, when: moment(1), speed: 1.0 }, true, &mut pending);
        assert_eq!(pending, None);
        // A second Play from the same sample, at half speed, while the UI reads the pass; the
        // latest callback after it also renders on from elsewhere before the pass is published.
        let start = Heard { sample: 0.0, when: moment(10), speed: 0.5 };
        {
            let _reading = stamps.pass.lock().unwrap();
            stamps.record(start, true, &mut pending);
        }
        assert_eq!(pending, Some(start), "kept for the next callback");
        let later = Heard { sample: 48_000.0, when: moment(12), speed: 0.5 };
        *stamps.last.lock().unwrap() = Some(later);
        // Until then the old pass's stamp does not answer for the new pass: the latest callback
        // tells it, two seconds before at half speed.
        assert_eq!(stamps.heard_at(0.0, 48_000.0), Some((moment(10), 0.5)), "not the old pass's start");
        stamps.record(Heard { sample: 48_512.0, when: moment(13), speed: 0.5 }, false, &mut pending);
        assert_eq!(pending, None);
        assert_eq!(stamps.heard_at(0.0, 48_000.0), Some((start.when, 0.5)), "the new pass's start and its speed");
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
        stamps.record(Heard { sample: 1_000.0, when: start, speed: 1.0 }, true, &mut None);
        // Later callbacks move the latest stamp on.
        stamps.record(Heard { sample: 49_000.0, when: start.shifted(1.0).unwrap(), speed: 1.0 }, false, &mut None);
        assert_eq!(stamps.heard_at(1_000.0, 48_000.0), Some((start, 1.0)), "the pass start is exact");
        // A punch at 25_000 is projected from the latest callback: half a second after the start.
        let (punch, _) = stamps.heard_at(25_000.0, 48_000.0).unwrap();
        assert!(punch.at.duration_since(t0).abs_diff(Duration::from_millis(500)) < Duration::from_micros(1), "{punch:?}");
        assert!(punch.clock.is_some_and(|c| (c - 100.5).abs() < 1e-9), "{punch:?}");
    }

    #[test]
    fn a_sample_is_projected_at_the_speed_the_latest_callback_played() {
        let stamps = HeardStamps::default();
        let t0 = Instant::now();
        let start = Moment { at: t0, clock: Some(100.0) };
        // At half speed 48_000 session samples go by in two seconds.
        stamps.record(Heard { sample: 49_000.0, when: start.shifted(2.0).unwrap(), speed: 0.5 }, false, &mut None);
        let (punch, speed) = stamps.heard_at(25_000.0, 48_000.0).unwrap();
        assert_eq!(speed, 0.5, "with the speed of the stamp it was projected from");
        assert!(punch.at.duration_since(t0).abs_diff(Duration::from_secs(1)) < Duration::from_micros(1), "{punch:?}");
        assert!(punch.clock.is_some_and(|c| (c - 101.0).abs() < 1e-9), "{punch:?}");
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

    /// cpal 0.18's ALSA counts each stream's times from that stream's own start, so an input opened
    /// 5 ms after the output stamps its captures 5 ms behind the output's playback times. Within half
    /// the round trip of the callbacks, that clock would place the take 240 samples late; the
    /// callbacks place it there instead. Where one clock serves both streams it still decides.
    #[test]
    #[cfg(not(target_os = "freebsd"))]
    fn streams_that_count_from_their_own_start_are_placed_by_the_callbacks() {
        let rate = 48_000.0;
        let t = Instant::now() + Duration::from_secs(1);
        let rt = Some(Duration::from_secs_f64(623.0 / rate));
        let secs = |s: f64| StreamInstant::from_nanos((s * 1e9).round() as u64);
        // The start is heard at 10 s and the take's first sample was captured 607 samples before;
        // the input callback ran 19 samples late, so the callbacks alone say 588.
        let (heard_at, captured_at) = (10.0, 10.0 - 607.0 / rate);
        let place = |host: &str, input_opened_after: f64| {
            let shared = shares_stream_clock(host);
            let output = OutputCallbackInfo::new(OutputStreamTimestamp { callback: secs(heard_at - 0.002), playback: secs(heard_at) });
            let capture = captured_at - input_opened_after;
            let input = InputCallbackInfo::new(InputStreamTimestamp { callback: secs(capture + 0.002), capture: secs(capture) });
            let heard = Moment { at: t, clock: playback_clock(&output, None, shared) };
            let captured = Moment { at: shift(t, (19.0 - 607.0) / rate).unwrap(), clock: capture_clock(&input, None, shared) };
            placement_offset(heard, captured, rate, rt)
        };
        assert_eq!(place("ALSA", 0.005), 588, "the input's own start is not the output's");
        for host in ["CoreAudio", "WASAPI", "ASIO"] {
            assert_eq!(place(host, 0.0), 607, "{host}: one clock for both streams decides");
        }
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
