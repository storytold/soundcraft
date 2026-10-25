//! The audio callback's load: how long each device callback takes to render, against how long its
//! buffer lasts. The audio thread is the only writer and the UI reads; both lock-free and
//! allocation-free.
//!
//! A callback that takes longer than its buffer lasts is *late*: the device ran out of audio
//! before the callback returned, which is heard as a dropout. cpal doesn't report the device's
//! own overload notices, so late callbacks are counted here instead.

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering::Relaxed};

/// Audio the average and peak are taken over before they are published, in seconds.
const WINDOW_S: f64 = 0.5;

/// The callback load, for the UI and agents. Loads are render time over buffer duration: 1.0 is
/// all of it.
#[derive(Debug, Clone, Copy, PartialEq, Default, serde::Serialize)]
pub struct AudioLoad {
    /// False until a device callback has been timed (no device, the silent clock or the web).
    pub measured: bool,
    /// Averaged over the last half second of audio.
    pub average: f32,
    /// The heaviest callback of the last half second.
    pub peak: f32,
    /// The heaviest callback since the stream opened, or since the last reset.
    pub worst: f32,
    /// Callbacks that took longer than their buffer lasts (dropouts), since the stream opened or
    /// the last reset.
    pub late: u64,
    /// Callbacks timed since the stream opened, or since the last reset.
    pub callbacks: u64,
    /// How long the last callback's buffer lasts, in milliseconds.
    pub buffer_ms: f32,
}

/// The published load (f32 values as bits).
#[derive(Default)]
pub(crate) struct LoadMeter {
    measured: AtomicBool,
    average: AtomicU32,
    peak: AtomicU32,
    worst: AtomicU32,
    late: AtomicU64,
    callbacks: AtomicU64,
    buffer_ms: AtomicU32,
    /// Asked by the UI; the audio thread clears the counters at its next callback.
    reset: AtomicBool,
}

/// The audio thread's own running window.
#[derive(Debug, Default)]
pub(crate) struct LoadWindow {
    busy: f64,
    audio: f64,
    peak: f64,
}

impl LoadMeter {
    /// One callback: `busy` seconds rendering `audio` seconds of output. Audio thread only.
    pub(crate) fn record(&self, w: &mut LoadWindow, busy: f64, audio: f64) {
        if !(audio > 0.0 && busy.is_finite() && busy >= 0.0) {
            return;
        }
        if self.reset.swap(false, Relaxed) {
            self.worst.store(0, Relaxed);
            self.late.store(0, Relaxed);
            self.callbacks.store(0, Relaxed);
        }
        let load = busy / audio;
        self.callbacks.store(self.callbacks.load(Relaxed).saturating_add(1), Relaxed);
        if load > 1.0 {
            self.late.store(self.late.load(Relaxed).saturating_add(1), Relaxed);
        }
        if load as f32 > f32::from_bits(self.worst.load(Relaxed)) {
            self.worst.store((load as f32).to_bits(), Relaxed);
        }
        self.buffer_ms.store(((audio * 1000.0) as f32).to_bits(), Relaxed);
        self.measured.store(true, Relaxed);
        w.busy += busy;
        w.audio += audio;
        w.peak = w.peak.max(load);
        if w.audio >= WINDOW_S {
            self.average.store(((w.busy / w.audio) as f32).to_bits(), Relaxed);
            self.peak.store((w.peak as f32).to_bits(), Relaxed);
            *w = LoadWindow::default();
        }
    }

    pub(crate) fn snapshot(&self) -> AudioLoad {
        let f = |a: &AtomicU32| f32::from_bits(a.load(Relaxed));
        AudioLoad {
            measured: self.measured.load(Relaxed),
            average: f(&self.average),
            peak: f(&self.peak),
            worst: f(&self.worst),
            late: self.late.load(Relaxed),
            callbacks: self.callbacks.load(Relaxed),
            buffer_ms: f(&self.buffer_ms),
        }
    }

    /// Starts the worst load and the counts again (at the next callback).
    pub(crate) fn reset(&self) {
        self.reset.store(true, Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn averages_and_peaks_are_published_per_window_and_late_callbacks_counted() {
        let m = LoadMeter::default();
        let mut w = LoadWindow::default();
        assert!(!m.snapshot().measured);
        // 125 ms buffers: three at 20% and one late at 120% close a half-second window.
        for busy in [0.025, 0.025, 0.15, 0.025] {
            m.record(&mut w, busy, 0.125);
        }
        let s = m.snapshot();
        assert!(s.measured);
        assert!((s.average - 0.45).abs() < 1e-6, "{s:?}");
        assert!((s.peak - 1.2).abs() < 1e-6, "{s:?}");
        assert!((s.worst - 1.2).abs() < 1e-6, "{s:?}");
        assert_eq!((s.late, s.callbacks), (1, 4));
        assert!((s.buffer_ms - 125.0).abs() < 1e-3);

        // Until the next window closes, the last one stays; then it is replaced. The worst load
        // and the counts carry on.
        for _ in 0..3 {
            m.record(&mut w, 0.0125, 0.125);
        }
        assert!((m.snapshot().average - 0.45).abs() < 1e-6);
        m.record(&mut w, 0.0125, 0.125);
        let s = m.snapshot();
        assert!((s.average - 0.1).abs() < 1e-6 && (s.peak - 0.1).abs() < 1e-6, "{s:?}");
        assert!((s.worst - 1.2).abs() < 1e-6);
        assert_eq!((s.late, s.callbacks), (1, 8));

        m.reset();
        m.record(&mut w, 0.0375, 0.125);
        let s = m.snapshot();
        assert_eq!((s.late, s.callbacks), (0, 1));
        assert!((s.worst - 0.3).abs() < 1e-6);
    }

    #[test]
    fn hostile_timings_are_ignored() {
        let m = LoadMeter::default();
        let mut w = LoadWindow::default();
        for (busy, audio) in [(0.01, 0.0), (0.01, -1.0), (f64::NAN, 0.1), (f64::INFINITY, 0.1), (-0.01, 0.1), (0.01, f64::NAN)] {
            m.record(&mut w, busy, audio);
        }
        assert_eq!(m.snapshot(), AudioLoad::default());
    }
}
