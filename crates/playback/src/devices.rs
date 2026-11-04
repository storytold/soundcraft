//! Audio device selection: which audio system (cpal host), output and input device, sample rate
//! and buffer size to open. The choice is per machine (the app keeps it with its preferences, not
//! in sessions). Anything that cannot be honoured falls back to the system default: a device that
//! is unplugged, a rate or buffer size the device does not offer.

use cpal::traits::{DeviceTrait, HostTrait};

/// Rates offered when a device supports them (any rate inside a supported range is possible, but
/// these are the ones interfaces actually run at).
pub const STANDARD_RATES: [u32; 6] = [44_100, 48_000, 88_200, 96_000, 176_400, 192_000];
/// Buffer sizes offered when the device reports a range that contains them.
pub const STANDARD_BUFFERS: [u32; 9] = [32, 64, 128, 256, 512, 1024, 2048, 4096, 8192];

/// Sample formats the streams convert to and from, best first (ASIO drivers usually run in
/// 32-bit integers, WASAPI and Core Audio in float).
pub(crate) const FORMATS: [cpal::SampleFormat; 3] = [cpal::SampleFormat::F32, cpal::SampleFormat::I32, cpal::SampleFormat::I16];

/// Preference of a sample format (lower is better); `None` when it is not supported.
pub(crate) fn format_rank(f: cpal::SampleFormat) -> Option<usize> {
    FORMATS.iter().position(|x| *x == f)
}

/// The user's audio device choice. Every field is optional: `None` means "the system default".
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct AudioConfig {
    /// Audio system (cpal host name: "WASAPI", "ASIO", "CoreAudio", "ALSA", "JACK", ...).
    pub system: Option<String>,
    pub output: Option<String>,
    pub input: Option<String>,
    /// Device sample rate; the mix resamples when it differs from the session's.
    pub sample_rate: Option<u32>,
    /// Device buffer in frames.
    pub buffer_size: Option<u32>,
}

/// One device as the settings window lists it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct DeviceInfo {
    pub name: String,
    pub is_default: bool,
    /// Most channels any usable configuration offers.
    pub max_channels: u16,
    /// Standard rates the device accepts (in a format the streams convert).
    pub sample_rates: Vec<u32>,
    /// Buffer range the device reports (`None` when it does not say).
    pub buffer_min: Option<u32>,
    pub buffer_max: Option<u32>,
}

impl DeviceInfo {
    /// Standard buffer sizes inside the device's range (all of them when the range is unknown).
    pub fn buffer_sizes(&self) -> Vec<u32> {
        STANDARD_BUFFERS.iter().copied().filter(|b| self.buffer_min.is_none_or(|m| *b >= m) && self.buffer_max.is_none_or(|m| *b <= m)).collect()
    }
}

/// An audio system and its devices.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct AudioSystem {
    pub name: String,
    pub is_default: bool,
    pub outputs: Vec<DeviceInfo>,
    pub inputs: Vec<DeviceInfo>,
}

/// The device's user-facing name. cpal 0.18 removed `DeviceTrait::name()` in favour of the
/// structured `description()`; `Display` for a device also formats that name but reports a
/// formatting error (which `to_string` would turn into a panic), so keep the fallible call here.
pub(crate) fn device_name(d: &cpal::Device) -> Option<String> {
    d.description().ok().map(|description| description.name().to_owned())
}

/// Describe one cpal device as [`DeviceInfo`], or return `None` when its name or supported configurations cannot be
/// read. Only formats the playback path can convert are considered.
fn describe(d: &cpal::Device, default_name: Option<&str>, output: bool) -> Option<DeviceInfo> {
    let name = device_name(d)?;
    let ranges: Vec<cpal::SupportedStreamConfigRange> = if output {
        d.supported_output_configs().ok()?.filter(|c| format_rank(c.sample_format()).is_some()).collect()
    } else {
        d.supported_input_configs().ok()?.filter(|c| format_rank(c.sample_format()).is_some()).collect()
    };
    let max_channels = ranges.iter().map(cpal::SupportedStreamConfigRange::channels).max().unwrap_or(0);
    let sample_rates = STANDARD_RATES.iter().copied().filter(|r| ranges.iter().any(|c| c.contains_rate(*r))).collect();
    let (mut buffer_min, mut buffer_max) = (None::<u32>, None::<u32>);
    for c in &ranges {
        if let cpal::SupportedBufferSize::Range { min, max } = c.buffer_size() {
            buffer_min = Some(buffer_min.map_or(*min, |m| m.min(*min)));
            buffer_max = Some(buffer_max.map_or(*max, |m| m.max(*max)));
        }
    }
    Some(DeviceInfo { is_default: default_name == Some(name.as_str()), name, max_channels, sample_rates, buffer_min, buffer_max })
}

/// Every audio system on this machine with its devices. Enumerating can take a moment (some
/// systems probe each device): call it when the settings window opens, not every frame.
pub fn audio_systems() -> Vec<AudioSystem> {
    let default_id = cpal::default_host().id();
    cpal::available_hosts()
        .into_iter()
        .filter_map(|id| {
            let host = cpal::host_from_id(id).ok()?;
            let def_out = host.default_output_device().as_ref().and_then(device_name);
            let def_in = host.default_input_device().as_ref().and_then(device_name);
            let outputs = host.output_devices().map(|ds| ds.filter_map(|d| describe(&d, def_out.as_deref(), true)).collect()).unwrap_or_default();
            let inputs = host.input_devices().map(|ds| ds.filter_map(|d| describe(&d, def_in.as_deref(), false)).collect()).unwrap_or_default();
            Some(AudioSystem { name: id.name().to_string(), is_default: id == default_id, outputs, inputs })
        })
        .collect()
}

/// True when a cpal error means the stream is dead and only a rebuild can restore audio. cpal 0.18
/// reports every operation through one [`cpal::Error`]: `DeviceNotAvailable` means the device went
/// away, and `StreamInvalidated` explicitly requires the stream to be rebuilt. SoundCraft has no
/// in-place rebuild, so both map to the existing actionable "reconnect audio device" trouble
/// instead of leaving a dead stream silently logging. A rerouted default device
/// (`DeviceChanged`) keeps playing, and an xrun is transient, so neither is a lost stream.
/// `HostUnavailable`, `DeviceBusy` and `BackendError` stay log-only until their retry/rebuild
/// policies exist.
pub(crate) fn stream_lost(e: &cpal::Error) -> bool {
    // ErrorKind is non-exhaustive; an equality test avoids a wildcard match arm.
    let kind = e.kind();
    kind == cpal::ErrorKind::DeviceNotAvailable || kind == cpal::ErrorKind::StreamInvalidated
}

/// The device's configuration at `sample_rate` in the best sample format these streams convert.
/// cpal 0.18 ranks `I24` (and `F64`) above `I16` when it picks a default configuration, so a
/// default alone can name a format the callbacks do not decode; this looks for the same rate in a
/// format [`FORMATS`] holds. The exact channel count is preferred; when the device offers none in
/// a usable format at that rate (for example only an 8-channel F32 stream), the closest available
/// channel count is used instead. Both callbacks fold or copy channels, so a wider or narrower
/// device stream is better than falling back to the silent clock. Callers pass a requested count
/// of at least 2 for output; input may ask for mono.
pub(crate) fn supported_format_config(
    configs: &[cpal::SupportedStreamConfigRange],
    sample_rate: u32,
    channels: u16,
) -> Option<cpal::SupportedStreamConfig> {
    let usable = || configs.iter().filter(|c| c.contains_rate(sample_rate)).filter_map(|c| format_rank(c.sample_format()).map(|rank| (rank, c)));
    // Exact count first, then the nearest count (more channels breaks the tie).
    let exact = usable().filter(|(_, c)| c.channels() == channels).min_by_key(|(rank, _)| *rank);
    let best = exact.or_else(|| {
        usable().min_by_key(|(rank, c)| {
            let distance = c.channels().abs_diff(channels);
            (*rank, distance, std::cmp::Reverse(c.channels()))
        })
    });
    best.and_then(|(_, c)| c.try_with_sample_rate(sample_rate))
}

/// The chosen audio system, else the default one.
pub(crate) fn host(cfg: &AudioConfig) -> cpal::Host {
    cfg.system
        .as_deref()
        .and_then(|want| cpal::available_hosts().into_iter().find(|id| id.name().eq_ignore_ascii_case(want)))
        .and_then(|id| cpal::host_from_id(id).ok())
        .unwrap_or_else(|| {
            if let Some(s) = &cfg.system {
                log::warn!("audio system `{s}` is not available; using the default");
            }
            cpal::default_host()
        })
}

/// The named output (or input) device of `host`, else its default device.
pub(crate) fn device(host: &cpal::Host, name: Option<&str>, output: bool) -> Option<cpal::Device> {
    if let Some(want) = name {
        let found = if output { host.output_devices().ok() } else { host.input_devices().ok() }
            .and_then(|mut ds| ds.find(|d| device_name(d).as_deref() == Some(want)));
        if found.is_some() {
            return found;
        }
        log::warn!("audio device `{want}` is not available; using the default");
    }
    if output { host.default_output_device() } else { host.default_input_device() }
}

/// The buffer to request: the chosen size when the device's range allows it, else the default.
pub(crate) fn buffer_size(want: Option<u32>, supported: &cpal::SupportedBufferSize) -> cpal::BufferSize {
    match (want, supported) {
        (Some(n), cpal::SupportedBufferSize::Range { min, max }) if (*min..=*max).contains(&n) => cpal::BufferSize::Fixed(n),
        // The device does not say: ask anyway (cpal reports an error if it cannot, and the caller
        // falls back to the default).
        (Some(n), cpal::SupportedBufferSize::Unknown) if (16..=8192).contains(&n) => cpal::BufferSize::Fixed(n),
        _ => cpal::BufferSize::Default,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_round_trips_and_tolerates_missing_fields() {
        let c = AudioConfig {
            system: Some("WASAPI".into()),
            output: Some("Speakers".into()),
            input: None,
            sample_rate: Some(48_000),
            buffer_size: Some(256),
        };
        let s = serde_json::to_string(&c).unwrap();
        assert_eq!(serde_json::from_str::<AudioConfig>(&s).unwrap(), c);
        assert_eq!(serde_json::from_str::<AudioConfig>("{}").unwrap(), AudioConfig::default());
        assert_eq!(serde_json::from_str::<AudioConfig>(r#"{"output":"X","unknown":1}"#).unwrap().output.as_deref(), Some("X"));
    }

    /// Build a cpal configuration range for tests with a 64..1024-frame buffer.
    fn range(channels: u16, min: u32, max: u32, format: cpal::SampleFormat) -> cpal::SupportedStreamConfigRange {
        cpal::SupportedStreamConfigRange::new(channels, min, max, cpal::SupportedBufferSize::Range { min: 64, max: 1024 }, format)
    }

    /// When the default sample format has no usable conversion, the same rate opens with a supported format; the
    /// nearest channel count keeps the device open, and unsupported rates fail.
    #[test]
    fn a_default_format_the_streams_cannot_convert_uses_a_supported_one_at_the_same_rate() {
        let configs = [
            range(2, 44_100, 48_000, cpal::SampleFormat::I24),
            range(2, 44_100, 48_000, cpal::SampleFormat::I16),
            range(2, 44_100, 48_000, cpal::SampleFormat::F32),
            range(2, 96_000, 96_000, cpal::SampleFormat::F32),
        ];
        let c = supported_format_config(&configs, 48_000, 2).unwrap();
        assert_eq!((c.sample_rate(), c.channels(), c.sample_format()), (48_000, 2, cpal::SampleFormat::F32));
        assert_eq!(supported_format_config(&configs, 96_000, 2).map(|c| c.sample_format()), Some(cpal::SampleFormat::F32));
        assert_eq!(
            supported_format_config(&configs, 48_000, 1).map(|c| (c.channels(), c.sample_format())),
            Some((2, cpal::SampleFormat::F32)),
            "with no mono stream, the nearest usable channel count keeps the device open"
        );
        let i24_only = [range(2, 44_100, 48_000, cpal::SampleFormat::I24)];
        assert!(supported_format_config(&i24_only, 48_000, 2).is_none(), "no supported format at this rate");
    }

    /// cpal 0.18 may name an I24 stereo default while the only format the callbacks decode is a
    /// wider F32 stream at another supported rate. The default-rate search finds nothing usable,
    /// so the output fallback retries at the session rate over any usable channel count; the mix
    /// already folds or copies channels and resamples.
    #[test]
    fn a_default_format_with_no_usable_same_rate_stream_falls_back_to_the_session_rate() {
        let configs = [range(2, 44_100, 44_100, cpal::SampleFormat::I24), range(8, 48_000, 48_000, cpal::SampleFormat::F32)];
        assert!(supported_format_config(&configs, 44_100, 2).is_none(), "the default rate has no usable format");
        let c = supported_format_config(&configs, 48_000, 2).expect("the wider F32 stream at the session rate");
        assert_eq!((c.sample_rate(), c.channels(), c.sample_format()), (48_000, 8, cpal::SampleFormat::F32));
    }

    /// Only device-not-available and invalidated-stream errors stop playback; other cpal errors are treated as
    /// recoverable.
    #[test]
    fn only_a_dead_stream_is_reported() {
        let error = |kind| cpal::Error::with_message(kind, "test");
        assert!(stream_lost(&error(cpal::ErrorKind::DeviceNotAvailable)));
        assert!(stream_lost(&error(cpal::ErrorKind::StreamInvalidated)), "the stream must be rebuilt");
        for kind in [
            cpal::ErrorKind::DeviceChanged,
            cpal::ErrorKind::DeviceBusy,
            cpal::ErrorKind::HostUnavailable,
            cpal::ErrorKind::Xrun,
            cpal::ErrorKind::BackendError,
        ] {
            assert!(!stream_lost(&error(kind)), "{kind:?} must not stop the stream");
        }
    }

    #[test]
    fn buffer_sizes_follow_the_device_range() {
        let r = cpal::SupportedBufferSize::Range { min: 64, max: 1024 };
        assert_eq!(buffer_size(Some(256), &r), cpal::BufferSize::Fixed(256));
        assert_eq!(buffer_size(Some(32), &r), cpal::BufferSize::Default);
        assert_eq!(buffer_size(Some(4096), &r), cpal::BufferSize::Default);
        assert_eq!(buffer_size(None, &r), cpal::BufferSize::Default);
        assert_eq!(buffer_size(Some(128), &cpal::SupportedBufferSize::Unknown), cpal::BufferSize::Fixed(128));
        assert_eq!(buffer_size(Some(1 << 20), &cpal::SupportedBufferSize::Unknown), cpal::BufferSize::Default);
        let d = DeviceInfo {
            name: "x".into(),
            is_default: true,
            max_channels: 2,
            sample_rates: vec![48_000],
            buffer_min: Some(64),
            buffer_max: Some(1024),
        };
        assert_eq!(d.buffer_sizes(), [64, 128, 256, 512, 1024]);
        let open = DeviceInfo { buffer_min: None, buffer_max: None, ..d };
        assert_eq!(open.buffer_sizes().len(), STANDARD_BUFFERS.len());
    }

    #[test]
    fn unknown_systems_and_devices_fall_back_to_the_defaults() {
        let cfg = AudioConfig { system: Some("No Such System".into()), ..Default::default() };
        assert_eq!(host(&cfg).id(), cpal::default_host().id());
        // No device with this name exists: the default (or none, on a machine without audio).
        let h = cpal::default_host();
        let fallback = device(&h, Some("No Such Device 1234"), true).as_ref().and_then(device_name);
        assert_eq!(fallback, h.default_output_device().as_ref().and_then(device_name));
    }

    #[test]
    fn enumeration_does_not_fail() {
        let systems = audio_systems();
        // Every listed system has a name; on a machine without audio the lists are just empty.
        assert!(systems.iter().all(|s| !s.name.is_empty()));
        assert!(systems.iter().filter(|s| s.is_default).count() <= 1);
    }
}
