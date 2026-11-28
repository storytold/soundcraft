use super::callback::{AudioState, Fault, guarded_block};
use super::{Arc, AudioConfig, Cmd, Ordering, Plugin, Receiver, Sender, Shared, devices, mark_audio_thread};

/// Start a freshly built output stream. `false` when the device refuses to start it, so the caller
/// treats it like a stream that failed to build and tries the next configuration.
pub(crate) fn start_stream(stream: &impl cpal::traits::StreamTrait) -> bool {
    match stream.play() {
        Ok(()) => true,
        Err(e) => {
            log::warn!("audio stream failed to start: {e}");
            false
        }
    }
}

/// A built output stream and the channels and fault flag its callback shares with the player.
pub(crate) struct Opened {
    pub(crate) stream: cpal::Stream,
    pub(crate) tx: Sender<Cmd>,
    pub(crate) grx: Receiver<Vec<Box<dyn Plugin>>>,
    pub(crate) fault: Arc<Fault>,
}

/// Build and start the attempts in order; the first that builds and starts wins. A build error or
/// a stream that will not start is logged, and the next attempt is tried. `None` when none works.
pub(crate) fn first_started<A: std::fmt::Debug, S, E: std::fmt::Display>(
    attempts: impl IntoIterator<Item = A>,
    mut build: impl FnMut(&A) -> Result<S, E>,
    start: impl Fn(&S) -> bool,
) -> Option<(A, S)> {
    for attempt in attempts {
        match build(&attempt) {
            Ok(built) if start(&built) => return Some((attempt, built)),
            // `start` has already logged why the stream would not start.
            Ok(_) => {}
            Err(e) => log::warn!("cannot open audio stream ({attempt:?}): {e}"),
        }
    }
    None
}

/// The output stream: the mix renders f32 into a scratch buffer allocated here (so the callback
/// never allocates), converted to the device's sample type.
pub(crate) fn build_output<T>(
    device: &cpal::Device,
    config: cpal::StreamConfig,
    mut state: AudioState,
    channels: usize,
    fault: &Arc<Fault>,
    shared: &Arc<Shared>,
) -> Result<cpal::Stream, cpal::Error>
where
    T: cpal::SizedSample + cpal::FromSample<f32>,
{
    use cpal::traits::DeviceTrait;
    let frame = channels.max(1);
    let mut scratch = vec![0.0f32; 8192 * frame];
    let chunk = scratch.len();
    let (failed, guard, gone) = (Arc::clone(fault), Arc::clone(shared), Arc::clone(fault));
    device.build_output_stream(
        config,
        move |data: &mut [T], _: &cpal::OutputCallbackInfo| {
            mark_audio_thread();
            for out in data.chunks_mut(chunk) {
                let Some(s) = scratch.get_mut(..out.len()) else { return };
                guarded_block(&failed, &guard, s, |d| state.fill(d, channels));
                for (o, x) in out.iter_mut().zip(s.iter()) {
                    *o = T::from_sample(*x);
                }
            }
        },
        // cpal may call this on the audio thread (on ALSA in a loop while the device stays
        // broken): marked, so the logger hands the record off instead of writing it.
        move |e: cpal::Error| {
            mark_audio_thread();
            // cpal 0.18 uses one error type for every operation. A device that went away, or a
            // stream that must be rebuilt (`StreamInvalidated`), stops playback until the user
            // reconnects; a rerouted default device (`DeviceChanged`) keeps playing on the new
            // route, and an xrun is transient, so neither counts as losing the stream.
            if devices::stream_lost(&e) {
                gone.gone.store(true, Ordering::Relaxed);
            }
            log::warn!("audio stream error: {e}");
        },
        None,
    )
}

/// Open the output device `cfg` chooses (else the default) at the chosen rate (else the session
/// rate) if possible, in the best sample format it offers (f32, else 32- or 16-bit integers),
/// with as many channels as it offers up to `want_ch` (at least stereo when available), and the
/// chosen buffer size when the device allows it.
pub(crate) fn open_device(
    want_rate: u32,
    want_ch: u16,
    cfg: &AudioConfig,
) -> Result<(cpal::Device, cpal::StreamConfig, cpal::SampleFormat, String), String> {
    use cpal::traits::DeviceTrait;
    let host = devices::host(cfg);
    let device = devices::device(&host, cfg.output.as_deref(), true).ok_or_else(|| "no output device".to_string())?;
    let name = devices::device_name(&device).unwrap_or_else(|| "Audio device".into());
    // Prefer a config at the chosen (else session) rate; else the device default (we resample).
    let want_ch = want_ch.max(2);
    let rates: Vec<u32> = cfg.sample_rate.into_iter().chain(Some(want_rate)).collect();
    let configs: Vec<cpal::SupportedStreamConfigRange> = device.supported_output_configs().map(|configs| configs.collect()).unwrap_or_default();
    let (config, format) = match pick_output_config(&configs, &rates, want_ch) {
        Some((mut c, buffers, format)) => {
            c.buffer_size = devices::buffer_size(cfg.buffer_size, &buffers);
            (c, format)
        }
        None => {
            let d = device.default_output_config().map_err(|e| e.to_string())?;
            let format = d.sample_format();
            // cpal 0.18's default can name a format these callbacks do not decode (I24, F64);
            // open the same rate and channels in a format they do, rather than lose output.
            let c = if devices::format_rank(format).is_some() {
                d
            } else {
                // Same rate and channels first; else any usable channel count at the default
                // rate; else the session rate (the mix resamples and folds channels). A device
                // whose only decoded format is wider or at another rate would otherwise open no
                // stream at all.
                devices::supported_format_config(&configs, d.sample_rate(), d.channels())
                    .or_else(|| devices::supported_format_config(&configs, want_rate, want_ch))
                    .ok_or_else(|| format!("unsupported device sample format {format:?}"))?
            };
            let mut stream_config = c.config();
            stream_config.buffer_size = devices::buffer_size(cfg.buffer_size, c.buffer_size());
            (stream_config, c.sample_format())
        }
    };
    Ok((device, config, format, name))
}

/// Pick the output configuration for the first requested rate the device offers (the configured
/// rate first, then the session rate), preferring the best sample format and then the most
/// channels from stereo up to `want_ch`. `None` when the device offers none of the requested
/// rates in a usable format; the caller then uses the device's default output configuration,
/// which the mix resamples to the session rate.
pub(crate) fn pick_output_config(
    configs: &[cpal::SupportedStreamConfigRange],
    rates: &[u32],
    want_ch: u16,
) -> Option<(cpal::StreamConfig, cpal::SupportedBufferSize, cpal::SampleFormat)> {
    for &rate in rates {
        let mut best: Option<(cpal::StreamConfig, cpal::SupportedBufferSize, cpal::SampleFormat, usize, u16)> = None;
        for c in configs {
            let Some(rank) = devices::format_rank(c.sample_format()) else { continue };
            if !c.contains_rate(rate) || c.channels() < 2 || c.channels() > want_ch {
                continue;
            }
            let Some(config) = c.try_with_sample_rate(rate) else { continue };
            // Better: a preferred format first, then more channels (ties keep the earlier range).
            let better = (rank, std::cmp::Reverse(c.channels()));
            if best.as_ref().is_none_or(|x| better < (x.3, std::cmp::Reverse(x.4))) {
                best = Some((config.config(), *c.buffer_size(), c.sample_format(), rank, c.channels()));
            }
        }
        if let Some((config, buffers, format, _, _)) = best {
            return Some((config, buffers, format));
        }
    }
    None
}
