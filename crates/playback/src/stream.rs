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

/// How long [`settle_rate_switch`] waits, at most, for the device to report its own rate switch.
/// The reports seen came 0.2–0.8 s after the stream opened (Fireface UFX, BlackHole, built-in
/// speakers on macOS 26).
pub(crate) const RATE_SETTLE: std::time::Duration = std::time::Duration::from_millis(1500);

/// Let a device that runs at another rate switch to `target` before the stream that plays is
/// built, on Core Audio only (`os` is a `std::env::consts::OS` value; see the #1749 review). cpal
/// switches the rate while it builds a Core Audio stream, and the device can report the switch only
/// once that stream listens for rate changes; the stream then ends ("Device sample rate changed")
/// right after it opened. So `build` opens a throwaway stream at `target` whose error callback
/// signals on the sender it is given, and this waits for that late report (or `wait`) before
/// dropping it. WASAPI shared mode and ALSA send no such report, so on any `os` but `"macos"` this
/// returns at once without reading the device's rate (`current`) or building anything: a settle
/// there would only wait out `wait` on the UI thread. `build(device_buffer, report)` builds the
/// throwaway with the chosen buffer size, or the device's own when `device_buffer`: cpal sets the
/// rate before the buffer size, so a throwaway refused for the chosen buffer can still have
/// switched the device, and one at the device's buffer then takes the report (#1749 review).
/// Returns whether a throwaway switched the device. Nothing is built when the device already runs
/// at `target` or its rate cannot be read.
pub(crate) fn settle_rate_switch<S>(
    os: &str,
    current: impl FnOnce() -> Option<u32>,
    target: u32,
    mut build: impl FnMut(bool, std::sync::mpsc::Sender<()>) -> Option<S>,
    wait: std::time::Duration,
) -> bool {
    if os != "macos" || current().is_none_or(|rate| rate == target) {
        return false;
    }
    let (report, reported) = std::sync::mpsc::channel();
    let Some(stream) = build(false, report.clone()).or_else(|| build(true, report)) else { return false };
    let _ = reported.recv_timeout(wait);
    drop(stream);
    true
}

/// An output stream that is built and never started: it only switches the device to `config`'s
/// rate, and signals `report` on any stream error (the device's report of that switch).
pub(crate) fn throwaway_output(
    device: &cpal::Device,
    config: cpal::StreamConfig,
    format: cpal::SampleFormat,
    report: std::sync::mpsc::Sender<()>,
) -> Option<cpal::Stream> {
    use cpal::traits::DeviceTrait;
    device
        .build_output_stream_raw(
            config,
            format,
            |_: &mut cpal::Data, _: &cpal::OutputCallbackInfo| {},
            move |_| {
                let _ = report.send(());
            },
            None,
        )
        .ok()
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

/// One way to open the output: a stream configuration, the sample format it carries, and whether
/// it is the device's default configuration rather than one picked for the chosen rate.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct OutputAttempt {
    pub(crate) config: cpal::StreamConfig,
    pub(crate) format: cpal::SampleFormat,
    pub(crate) default: bool,
}

/// The configurations to try, in order: the one picked for the chosen rate, with the chosen
/// buffer size and then the device's buffer size; then the device's default configuration the
/// same two ways. The default comes last so a device that refuses the picked rate (or does not
/// settle at it in time) still plays, resampled, instead of leaving the player on its silent
/// clock. A configuration already in the list is not tried twice.
pub(crate) fn output_attempts(
    picked: Option<(cpal::StreamConfig, cpal::SampleFormat)>,
    default: Option<(cpal::StreamConfig, cpal::SampleFormat)>,
) -> Vec<OutputAttempt> {
    let mut attempts: Vec<OutputAttempt> = Vec::new();
    let candidates = picked.map(|p| (p, false)).into_iter().chain(default.map(|d| (d, true)));
    for ((config, format), default) in candidates {
        for config in [config, cpal::StreamConfig { buffer_size: cpal::BufferSize::Default, ..config }] {
            if !attempts.iter().any(|a| a.config == config && a.format == format) {
                attempts.push(OutputAttempt { config, format, default });
            }
        }
    }
    attempts
}

/// Open the output device `cfg` chooses (else the default) and list the configurations to try
/// ([`output_attempts`]). The first is at the chosen rate (else the session rate) if the device
/// offers it, in the best sample format it offers (f32, else 32- or 16-bit integers), with as
/// many channels as it offers up to `want_ch` (at least stereo), or else the fewest channels
/// above it (the extra channels stay silent), and the chosen buffer size when the device allows
/// it. The device's default configuration (the mix resamples to it) follows, and is the only
/// one when the device offers none of the requested rates.
pub(crate) fn open_device(want_rate: u32, want_ch: u16, cfg: &AudioConfig) -> Result<(cpal::Device, Vec<OutputAttempt>, String), String> {
    use cpal::traits::DeviceTrait;
    let host = devices::host(cfg);
    let device = devices::device(&host, cfg.output.as_deref(), true).ok_or_else(|| "no output device".to_string())?;
    let name = devices::device_name(&device).unwrap_or_else(|| "Audio device".into());
    // Prefer a config at the chosen (else session) rate: up to the mix's width, else wider; then
    // the device default (we resample).
    let want_ch = want_ch.max(2);
    let rates: Vec<u32> = cfg.sample_rate.into_iter().chain(Some(want_rate)).collect();
    let configs: Vec<cpal::SupportedStreamConfigRange> = device.supported_output_configs().map(|configs| configs.collect()).unwrap_or_default();
    let picked =
        pick_output_config(&configs, &rates, want_ch).or_else(|| pick_wider_output_config(&configs, &rates)).map(|(mut c, buffers, format)| {
            c.buffer_size = devices::buffer_size(cfg.buffer_size, &buffers);
            (c, format)
        });
    let default = match default_output_config(&device, &configs, want_rate, want_ch, cfg) {
        Ok(d) => Some(d),
        // Without a picked config the default is all there is, so its error is the device's.
        Err(e) if picked.is_none() => return Err(e),
        Err(e) => {
            log::debug!("no default output configuration to fall back to: {e}");
            None
        }
    };
    Ok((device, output_attempts(picked, default), name))
}

/// The device's default output configuration with the chosen buffer size, in a sample format the
/// callbacks decode.
fn default_output_config(
    device: &cpal::Device,
    configs: &[cpal::SupportedStreamConfigRange],
    want_rate: u32,
    want_ch: u16,
    cfg: &AudioConfig,
) -> Result<(cpal::StreamConfig, cpal::SampleFormat), String> {
    use cpal::traits::DeviceTrait;
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
        devices::supported_format_config(configs, d.sample_rate(), d.channels())
            .or_else(|| devices::supported_format_config(configs, want_rate, want_ch))
            .ok_or_else(|| format!("unsupported device sample format {format:?}"))?
    };
    let mut stream_config = c.config();
    stream_config.buffer_size = devices::buffer_size(cfg.buffer_size, c.buffer_size());
    Ok((stream_config, c.sample_format()))
}

/// Pick the output configuration for the first requested rate the device offers (the configured
/// rate first, then the session rate), preferring the best sample format and then the most
/// channels from stereo up to `want_ch`. `None` when the device offers none of the requested
/// rates in a usable format within that width; the caller then tries a wider configuration
/// ([`pick_wider_output_config`]), and else the device's default output configuration, which
/// the mix resamples to the session rate.
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

/// The output configuration for the first requested rate the device offers with more channels
/// than the mix, when [`pick_output_config`] finds none within the mix's width: the fewest such
/// channels, then the best sample format. Some multichannel interfaces offer only their full
/// width (an RME Fireface UFX on Core Audio lists one 30-channel configuration). Their default
/// configuration is the rate they happen to run at, so falling back to it ignores the chosen
/// rate, and an input opened at the chosen rate then switches the device and ends the output.
pub(crate) fn pick_wider_output_config(
    configs: &[cpal::SupportedStreamConfigRange],
    rates: &[u32],
) -> Option<(cpal::StreamConfig, cpal::SupportedBufferSize, cpal::SampleFormat)> {
    rates.iter().find_map(|&rate| {
        configs
            .iter()
            .filter(|c| c.channels() >= 2 && c.contains_rate(rate))
            .filter_map(|c| Some((devices::format_rank(c.sample_format())?, c)))
            .min_by_key(|(rank, c)| (c.channels(), *rank))
            .and_then(|(_, c)| c.try_with_sample_rate(rate).map(|config| (config.config(), *c.buffer_size(), c.sample_format())))
    })
}
