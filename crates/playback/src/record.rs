//! Audio input capture for recording.
//!
//! A [`Recorder`] opens the default input device and appends interleaved samples to a shared
//! buffer while armed. The callback only `try_lock`s; if the UI thread holds the lock the block is
//! kept in a small local backlog and appended next time, so nothing blocks the audio thread.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

/// Captured audio: planar f32 at `sample_rate`.
#[derive(Debug, Clone, Default)]
pub struct Take {
    pub sample_rate: u32,
    pub channels: Vec<Vec<f32>>,
}

struct Shared {
    armed: AtomicBool,
    data: Mutex<Vec<f32>>,
}

pub struct Recorder {
    shared: Arc<Shared>,
    /// Live input for monitoring (always fed while the input stream runs).
    pub monitor: Arc<InputRing>,
    #[cfg(not(target_os = "freebsd"))]
    _stream: Option<cpal::Stream>,
    pub device_name: String,
    pub sample_rate: u32,
    pub channels: usize,
}

/// Cap: one hour of 8-channel 192 kHz audio; 64 Mi samples (256 MB of f32) on 32-bit targets
/// (wasm32, i686), where the 64-bit product doesn't fit in `usize`.
const MAX_SAMPLES: usize = if usize::BITS >= 64 { (3600u64 * 192_000 * 8) as usize } else { 64 << 20 };

impl Recorder {
    /// Open the default input device. Errors when there is none (the caller shows a message).
    #[cfg(not(target_os = "freebsd"))]
    pub fn open() -> Result<Recorder, String> {
        use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
        let host = cpal::default_host();
        let device = host.default_input_device().ok_or("no audio input device")?;
        #[allow(deprecated)]
        let name = device.name().unwrap_or_else(|_| "Input".into());
        let cfg = device.default_input_config().map_err(|e| e.to_string())?;
        if cfg.sample_format() != cpal::SampleFormat::F32 {
            return Err(format!("unsupported input sample format {:?}", cfg.sample_format()));
        }
        let config = cfg.config();
        let shared = Arc::new(Shared { armed: AtomicBool::new(false), data: Mutex::new(Vec::new()) });
        let sh = Arc::clone(&shared);
        let monitor = InputRing::new(usize::from(config.channels), config.sample_rate.0);
        let mon = Arc::clone(&monitor);
        let mut backlog: Vec<f32> = Vec::new();
        let stream = device
            .build_input_stream(
                &config,
                move |data: &[f32], _: &cpal::InputCallbackInfo| {
                    crate::mark_audio_thread();
                    mon.push(data);
                    if !sh.armed.load(Ordering::Relaxed) {
                        backlog.clear();
                        return;
                    }
                    if let Ok(mut d) = sh.data.try_lock() {
                        if d.len() < MAX_SAMPLES {
                            d.extend_from_slice(&backlog);
                            d.extend_from_slice(data);
                        }
                        backlog.clear();
                    } else if backlog.len() < 1 << 20 {
                        backlog.extend_from_slice(data);
                    }
                },
                |e| {
                    // Possibly the audio thread (see `crate::mark_audio_thread`).
                    crate::mark_audio_thread();
                    log::warn!("input stream error: {e}");
                },
                None,
            )
            .map_err(|e| e.to_string())?;
        stream.play().map_err(|e| e.to_string())?;
        Ok(Recorder {
            monitor,
            shared,
            _stream: Some(stream),
            device_name: name,
            sample_rate: config.sample_rate.0,
            channels: usize::from(config.channels).max(1),
        })
    }

    #[cfg(target_os = "freebsd")]
    pub fn open() -> Result<Recorder, String> {
        Err("recording is not supported on this platform yet".into())
    }

    /// Start capturing (clears any previous take).
    pub fn arm(&self) {
        self.shared.data.lock().unwrap_or_else(PoisonError::into_inner).clear();
        self.shared.armed.store(true, Ordering::Relaxed);
    }

    pub fn is_armed(&self) -> bool {
        self.shared.armed.load(Ordering::Relaxed)
    }

    /// Stop capturing and return the take.
    pub fn take(&self) -> Take {
        self.shared.armed.store(false, Ordering::Relaxed);
        let data = std::mem::take(&mut *self.shared.data.lock().unwrap_or_else(PoisonError::into_inner));
        deinterleave(&data, self.channels, self.sample_rate)
    }

    /// Samples captured so far (per channel).
    pub fn captured_frames(&self) -> usize {
        self.shared.data.try_lock().map_or(0, |d| d.len() / self.channels.max(1))
    }
}

pub fn deinterleave(data: &[f32], channels: usize, sample_rate: u32) -> Take {
    let ch = channels.max(1);
    let frames = data.len() / ch;
    let mut out = vec![Vec::with_capacity(frames); ch];
    for frame in data.chunks_exact(ch) {
        for (c, v) in frame.iter().enumerate() {
            if let Some(o) = out.get_mut(c) {
                o.push(*v);
            }
        }
    }
    Take { sample_rate, channels: out }
}

#[cfg(test)]
mod tests {
    #[test]
    fn deinterleaves() {
        let t = super::deinterleave(&[1.0, 2.0, 3.0, 4.0, 5.0], 2, 48_000);
        assert_eq!(t.channels, vec![vec![1.0, 3.0], vec![2.0, 4.0]]);
        let t = super::deinterleave(&[], 0, 48_000);
        assert_eq!(t.channels.len(), 1);
    }
}

/// Ring state behind the lock: interleaved input frames at the input device's rate, plus the
/// fractional read position (in input frames) of the next output sample.
struct RingState {
    data: std::collections::VecDeque<f32>,
    phase: f64,
}

/// A small interleaved FIFO from the input callback to the output callback, for input
/// monitoring. Both sides only `try_lock`; it holds at most ~0.5 s and drops the oldest frames.
/// The input device may run at a different rate than the session, so the reader converts
/// rates with linear interpolation (cheap, allocation-free, fine for monitoring).
pub struct InputRing {
    state: Mutex<RingState>,
    pub channels: std::sync::atomic::AtomicUsize,
    /// Input device sample rate in Hz (0 when unknown, which reads as "same as the session").
    rate: u32,
    /// Most samples kept: half a second of input, or 24 000 frames when the rate is unknown.
    cap: usize,
}

impl InputRing {
    pub fn new(channels: usize, rate: u32) -> Arc<InputRing> {
        let ch = channels.max(1);
        let frames = if rate == 0 { 24_000 } else { rate as usize / 2 };
        Arc::new(InputRing {
            state: Mutex::new(RingState { data: std::collections::VecDeque::with_capacity(rate.max(48_000) as usize * ch), phase: 0.0 }),
            channels: std::sync::atomic::AtomicUsize::new(ch),
            rate,
            cap: frames * ch,
        })
    }

    pub fn push(&self, samples: &[f32]) {
        if let Ok(mut s) = self.state.try_lock() {
            s.data.extend(samples.iter().copied());
            while s.data.len() > self.cap {
                s.data.pop_front();
            }
        }
    }

    /// Fill `out` (planar, at `session_rate`) with up to `frames` frames, converting from the
    /// input rate. Missing frames are silence.
    pub fn pop_into(&self, out: &mut [Vec<f32>], frames: usize, session_rate: u32) {
        let ch = self.channels.load(Ordering::Relaxed).max(1);
        for c in out.iter_mut() {
            c.iter_mut().take(frames).for_each(|x| *x = 0.0);
        }
        let Ok(mut s) = self.state.try_lock() else { return };
        // Input frames consumed per output frame.
        let step = if self.rate == 0 || session_rate == 0 { 1.0 } else { f64::from(self.rate) / f64::from(session_rate) };
        for f in 0..frames {
            // Drop input frames we have moved past; keep the one at `phase` and the next one.
            while s.phase >= 1.0 && s.data.len() >= 2 * ch {
                s.data.drain(..ch);
                s.phase -= 1.0;
            }
            if s.phase >= 1.0 || s.data.len() < 2 * ch {
                break;
            }
            let fr = s.phase as f32;
            for c in 0..ch {
                let a = s.data.get(c).copied().unwrap_or(0.0);
                let b = s.data.get(ch + c).copied().unwrap_or(a);
                if let Some(x) = out.get_mut(c).and_then(|o| o.get_mut(f)) {
                    *x = a + (b - a) * fr;
                }
            }
            s.phase += step;
        }
    }
}

#[cfg(test)]
mod ring_tests {
    use super::InputRing;

    /// A 1 kHz sine recorded at 16 kHz must come out as a 1 kHz sine at 48 kHz, across calls.
    #[test]
    fn monitor_converts_input_rate_to_session_rate() {
        let ring = InputRing::new(1, 16_000);
        let input: Vec<f32> = (0..4_000).map(|i| (2.0 * std::f64::consts::PI * 1_000.0 * f64::from(i) / 16_000.0).sin() as f32).collect();
        ring.push(&input);
        let mut out = vec![vec![0.0f32; 48]];
        let mut k = 0usize;
        for _ in 0..10 {
            ring.pop_into(&mut out, 48, 48_000);
            for v in &out[0] {
                let want = (2.0 * std::f64::consts::PI * 1_000.0 * k as f64 / 48_000.0).sin() as f32;
                assert!((v - want).abs() < 0.05, "sample {k}: got {v}, want {want}");
                k += 1;
            }
        }
    }

    /// Input faster than the session (96 kHz into 48 kHz): every other frame is read, in order.
    #[test]
    fn monitor_decimates_when_input_is_faster() {
        let ring = InputRing::new(1, 96_000);
        let input: Vec<f32> = (0..1_000).map(|i| i as f32).collect();
        ring.push(&input);
        let mut out = vec![vec![0.0f32; 100]];
        ring.pop_into(&mut out, 100, 48_000);
        for (k, v) in out[0].iter().enumerate() {
            assert_eq!(*v, (2 * k) as f32, "output {k}");
        }
    }

    /// Same rate: samples pass through unchanged.
    #[test]
    fn monitor_passes_through_at_equal_rates() {
        let ring = InputRing::new(2, 48_000);
        let input: Vec<f32> = (0..200).map(|i| i as f32).collect();
        ring.push(&input);
        let mut out = vec![vec![0.0f32; 50], vec![0.0f32; 50]];
        ring.pop_into(&mut out, 50, 48_000);
        assert_eq!(out[0][0], 0.0);
        assert_eq!(out[1][0], 1.0);
        assert_eq!(out[0][49], 98.0);
    }

    /// Nothing buffered: silence, not stale data.
    #[test]
    fn monitor_underrun_is_silence() {
        let ring = InputRing::new(1, 16_000);
        let mut out = vec![vec![1.0f32; 16]];
        ring.pop_into(&mut out, 16, 48_000);
        assert!(out[0].iter().all(|v| *v == 0.0));
    }
}
