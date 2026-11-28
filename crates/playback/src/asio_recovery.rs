//! A driver may keep issuing callbacks after a USB mode change, at a fraction of its
//! configured rate. Detect that separately from a device which stops calling altogether.
use std::time::{Duration, Instant};

#[derive(Default)]
pub(crate) struct Cadence {
    previous: Option<(Instant, Duration)>,
    slow_intervals: u8,
}

impl Cadence {
    pub(crate) fn observe(&mut self, now: Instant, frames: usize, rate: u32) -> bool {
        let period = Duration::from_secs_f64(frames as f64 / f64::from(rate));
        if let Some((last, expected)) = self.previous.replace((now, period)) {
            let threshold = Duration::from_millis(100).max(expected.saturating_mul(8));
            if now.duration_since(last) >= threshold {
                self.slow_intervals = self.slow_intervals.saturating_add(1);
            } else {
                self.slow_intervals = 0;
            }
        }
        self.slow_intervals >= 4
    }
}

pub(crate) fn recovery_due(slow: bool, playing: bool, recording: bool, pending_transport: bool, retry_at: Option<Instant>, now: Instant) -> bool {
    slow && !playing && !recording && !pending_transport && retry_at.is_none_or(|at| now >= at)
}

#[cfg(all(windows, feature = "asio"))]
pub(crate) mod native {
    use super::{Cadence, Duration, Instant, recovery_due};
    use crate::{Arc, AudioState, Fault, IdleRecovery, Mutex, Ordering, Player, Shared, stream};
    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
    use std::sync::atomic::AtomicBool;

    pub(crate) struct Recovery {
        device_id: cpal::DeviceId,
        config: cpal::StreamConfig,
        format: cpal::SampleFormat,
        graph: Arc<Mutex<AudioState>>,
        slow: Arc<AtomicBool>,
        retry_at: Option<Instant>,
    }

    impl Recovery {
        pub(crate) fn new(device_id: cpal::DeviceId, config: cpal::StreamConfig, format: cpal::SampleFormat, state: AudioState) -> Self {
            Self { device_id, config, format, graph: Arc::new(Mutex::new(state)), slow: Arc::default(), retry_at: None }
        }

        pub(crate) fn build(&self, device: &cpal::Device, fault: &Arc<Fault>, shared: &Arc<Shared>) -> Result<cpal::Stream, cpal::Error> {
            let graph = Arc::clone(&self.graph);
            let slow = Arc::clone(&self.slow);
            let rate = self.config.sample_rate;
            let mut cadence = Cadence::default();
            stream::build_format(
                device,
                self.config,
                self.format,
                fault,
                shared,
                move |frames| slow.store(cadence.observe(Instant::now(), frames, rate), Ordering::Relaxed),
                move |data, channels| {
                    // Only the live stream renders. Never wait on a retiring callback.
                    if let Ok(mut state) = graph.try_lock() {
                        state.fill(data, channels);
                    } else {
                        data.fill(0.0);
                    }
                },
            )
        }

        fn reopen(&self, fault: &Arc<Fault>, shared: &Arc<Shared>) -> Result<(cpal::Device, cpal::Stream), String> {
            let host = cpal::host_from_id(self.device_id.host()).map_err(|e| e.to_string())?;
            let device = host.device_by_id(&self.device_id).ok_or_else(|| format!("{} is unavailable", self.device_id))?;
            let stream = self.build(&device, fault, shared).map_err(|e| e.to_string())?;
            stream.play().map_err(|e| e.to_string())?;
            Ok((device, stream))
        }
    }

    pub(crate) fn asio_id(device: &cpal::Device) -> Option<cpal::DeviceId> {
        device.id().ok().filter(|id| id.host() == cpal::HostId::Asio)
    }

    impl Player {
        pub(crate) fn recover_idle_stream(&mut self, playing: bool, recording: bool, pending: bool, release_input: impl FnOnce()) -> IdleRecovery {
            let playing = playing || self.is_playing();
            let Some(recovery) = &mut self.asio_recovery else { return IdleRecovery::None };
            let now = Instant::now();
            if self.fault.failed.load(Ordering::Relaxed) {
                return IdleRecovery::None;
            }
            let slow = self._stream.is_none() || recovery.slow.load(Ordering::Relaxed);
            if !recovery_due(slow, playing, recording, pending, recovery.retry_at, now) {
                return IdleRecovery::None;
            }
            release_input();
            drop(self._stream.take());
            recovery.slow.store(false, Ordering::Relaxed);
            recovery.retry_at = Some(now + Duration::from_secs(3));
            let fault = Arc::new(Fault::default());
            match recovery.reopen(&fault, &self.shared) {
                Ok((device, stream)) => {
                    self._stream = Some(stream);
                    self.fault = fault;
                    // Shared capture opens again on the reopened device.
                    self.asio_device = Some(device);
                    *self.watch.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = None;
                    *self.asio_watch.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = None;
                    log::info!("Reopened idle ASIO stream on {} after sustained slow callbacks", self.device_name);
                    IdleRecovery::Reopened
                }
                Err(e) => {
                    log::warn!("Cannot yet reopen idle ASIO stream on {}: {e}", self.device_name);
                    IdleRecovery::Waiting
                }
            }
        }
    }
}
