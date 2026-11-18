//! Audio device selection: which audio system (cpal host), output and input device, sample rate
//! and buffer size to open. The choice is per machine (the app keeps it with its preferences, not
//! in sessions). Anything that cannot be honoured falls back to the system default: a device that
//! is unplugged, a rate or buffer size the device does not offer. Which audio systems there are
//! and the order they are tried in is in [`crate::host`].

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
    /// Audio system: a host id ([`AudioSystem::id`]: `"pipewire"`, `"jack"`, `"alsa"`, `"wasapi"`,
    /// `"asio"`, `"coreaudio"`, …). `None`: the automatic order (see [`crate::host`]).
    pub system: Option<String>,
    /// Output device id ([`DeviceInfo::id`]).
    pub output: Option<String>,
    /// Input device id ([`DeviceInfo::id`]).
    pub input: Option<String>,
    /// Device sample rate; the mix resamples when it differs from the session's.
    pub sample_rate: Option<u32>,
    /// Device buffer in frames.
    pub buffer_size: Option<u32>,
}

impl AudioConfig {
    /// This choice with the system and the devices as the ids `systems` lists them under.
    /// Preferences written before devices had ids hold names (`system: "ALSA"`,
    /// `output: "Speakers"`): a name that matches a listed system or device becomes its id. What
    /// matches nothing stays as it is, since the device may only be unplugged; opening falls back
    /// to the default for it (see [`device`]). So do the device names of a chosen system that is
    /// not listed (an ASIO choice read by a build without ASIO): a device of another system that
    /// happens to have the same name is not the one that was meant.
    #[must_use]
    pub fn with_ids(&self, systems: &[AudioSystem]) -> AudioConfig {
        // Read the way stream opening reads it: any case, spaces around it, `auto` for no choice.
        let want = self.system.as_deref().and_then(crate::host::host_key);
        let system = want.as_deref().and_then(|want| find_system(systems, want));
        let chosen = want.is_some();
        let id = |want: &Option<String>, automatic: fn(&AudioSystem) -> bool, list: fn(&AudioSystem) -> &Vec<DeviceInfo>| {
            // The devices of the chosen system, else of the one the automatic order opens.
            let devices = if chosen { system } else { systems.iter().find(|s| automatic(s)) };
            let found = want.as_deref().zip(devices).and_then(|(want, s)| {
                let list = list(s);
                list.iter().find(|d| d.id == want).or_else(|| list.iter().find(|d| d.name == want))
            });
            found.map(|d| d.id.clone()).or_else(|| want.clone())
        };
        AudioConfig {
            // A system that is not listed stays as it was written; `auto` and blank are no choice.
            system: system.map(|s| s.id.clone()).or_else(|| self.system.clone().filter(|_| chosen)),
            output: id(&self.output, |s| s.is_default, |s| &s.outputs),
            input: id(&self.input, |s| s.is_default_input, |s| &s.inputs),
            sample_rate: self.sample_rate,
            buffer_size: self.buffer_size,
        }
    }
}

/// The listed system a host id or name (in any case) means.
fn find_system<'a>(systems: &'a [AudioSystem], want: &str) -> Option<&'a AudioSystem> {
    systems.iter().find(|s| s.id.eq_ignore_ascii_case(want) || s.name.eq_ignore_ascii_case(want))
}

/// The system an output (or input) stream opens on with this choice, as [`crate::host::pick`]
/// decides it: the one forced for this run (`--audio-host`, see [`crate::host::forced_host`]),
/// else the chosen one, else the first of the automatic order, in each case one that has
/// something to open for that direction. So a settings window lists the devices of the system
/// the streams really use: not ALSA's while the command line forces JACK, and not those of a
/// chosen JACK whose server is not running. With nothing to open anywhere, the system that was
/// asked for, else the first.
pub fn effective_system<'a>(systems: &'a [AudioSystem], cfg: &AudioConfig, forced: Option<&str>, output: bool) -> Option<&'a AudioSystem> {
    // A system that was asked for is taken when it has a default device at all; the automatic
    // order also wants its output to reach something (see `host::has_default`).
    let has = |s: &&AudioSystem| if output { s.has_output } else { s.has_input };
    let automatic = |s: &&AudioSystem| if output { s.is_default } else { s.is_default_input };
    let configured = cfg.system.as_deref().and_then(crate::host::host_key);
    // The forced one when it is listed at all, else the chosen one (see `host::order`).
    let asked = [forced.and_then(crate::host::host_key), configured].into_iter().flatten().find_map(|want| find_system(systems, &want));
    asked.filter(has).or_else(|| systems.iter().find(automatic)).or(asked).or(systems.first())
}

/// One device as the settings window lists it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct DeviceInfo {
    /// What [`AudioConfig::output`] and [`AudioConfig::input`] hold to choose this device.
    pub id: String,
    pub name: String,
    /// What tells it apart from its neighbours (`"2 ch · USB"`); may be empty.
    pub detail: String,
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
    /// What [`AudioConfig::system`] holds to choose this system.
    pub id: String,
    pub name: String,
    /// The system the automatic order opens the output on: the first that has something to
    /// play to, which need not be the first listed (JACK without a running server has nothing).
    pub is_default: bool,
    /// The system the automatic order opens the input on.
    pub is_default_input: bool,
    /// Whether "Default" has an output to open here. It need not be one of `outputs`: PipeWire
    /// follows the desktop's default sink and JACK connects to the system ports.
    pub has_output: bool,
    /// Whether that output reaches something to play to. PipeWire with a microphone and no
    /// sink, and a JACK server without system playback ports, have a default output that plays
    /// to nothing: chosen, such a system still opens it; the automatic order passes it over.
    pub output_route: bool,
    /// Whether "Default" opens an input here.
    pub has_input: bool,
    pub outputs: Vec<DeviceInfo>,
    pub inputs: Vec<DeviceInfo>,
}

/// The device's user-facing name. cpal 0.18 removed `DeviceTrait::name()` in favour of the
/// structured `description()`; `Display` for a device also formats that name but reports a
/// formatting error (which `to_string` would turn into a panic), so keep the fallible call here.
pub(crate) fn device_name(d: &cpal::Device) -> Option<String> {
    let name = d.description().ok()?.name().trim().to_owned();
    match name.as_str() {
        // PipeWire's names for "whatever the system default is".
        "default_output" | "default_input" | "default_sink" => Some("System default".to_owned()),
        // A node the host has no name for: its id says more.
        "" | "unknown" => d.id().ok().map(|id| id.to_string()),
        _ => Some(name),
    }
}

fn device_id(d: &cpal::Device) -> Option<String> {
    d.id().ok().map(|id| id.to_string())
}

/// What the settings window needs to know about a listed device. Only formats the streams can
/// convert count towards its channels, rates and buffer range. A JACK port group has no
/// configurations (its rate and buffer are the server's) and is listed all the same.
fn describe(listed: crate::host::Listed, default_id: Option<&str>) -> DeviceInfo {
    let ranges: Vec<&cpal::SupportedStreamConfigRange> = listed.ranges.iter().filter(|c| format_rank(c.sample_format()).is_some()).collect();
    let max_channels = ranges.iter().map(|c| c.channels()).max().unwrap_or(0);
    let sample_rates = STANDARD_RATES.iter().copied().filter(|r| ranges.iter().any(|c| c.contains_rate(*r))).collect();
    let (mut buffer_min, mut buffer_max) = (None::<u32>, None::<u32>);
    for c in &ranges {
        if let cpal::SupportedBufferSize::Range { min, max } = c.buffer_size() {
            buffer_min = Some(buffer_min.map_or(*min, |m| m.min(*min)));
            buffer_max = Some(buffer_max.map_or(*max, |m| m.max(*max)));
        }
    }
    DeviceInfo {
        is_default: default_id == Some(listed.id.as_str()),
        id: listed.id,
        name: listed.name,
        detail: listed.detail,
        max_channels,
        sample_rates,
        buffer_min,
        buffer_max,
    }
}

/// Every audio system this build can open on this machine, with its devices, in the automatic
/// order. Enumerating can take a moment (some systems probe each device): call it when the
/// settings window opens, not every frame.
pub fn audio_systems() -> Vec<AudioSystem> {
    use crate::host::Direction;
    let mut systems: Vec<AudioSystem> = crate::host::host_ids()
        .into_iter()
        .filter_map(|id| {
            let host = cpal::host_from_id(id).ok()?;
            let def_out = host.default_output_device().as_ref().and_then(device_id);
            let def_in = host.default_input_device().as_ref().and_then(device_id);
            let list = |direction: Direction, default: Option<&str>| -> Vec<DeviceInfo> {
                crate::host::listed(&host, direction).into_iter().map(|entry| describe(entry, default)).collect()
            };
            let (outputs, inputs) = (list(Direction::Output, def_out.as_deref()), list(Direction::Input, def_in.as_deref()));
            Some(AudioSystem {
                id: id.to_string(),
                name: id.name().to_string(),
                is_default: false,
                is_default_input: false,
                has_output: crate::host::has_default(&host, Direction::Output, false),
                output_route: crate::host::has_default(&host, Direction::Output, true),
                has_input: crate::host::has_default(&host, Direction::Input, true),
                outputs,
                inputs,
            })
        })
        .collect();
    // The same test the automatic order makes when it opens a stream.
    if let Some(system) = systems.iter_mut().find(|s| s.output_route) {
        system.is_default = true;
    }
    if let Some(system) = systems.iter_mut().find(|s| s.has_input) {
        system.is_default_input = true;
    }
    systems
}

/// The configuration for a stream that is wired to a JACK interface afterwards: `channels` wide
/// (one per port of the interface, see [`crate::host::group_ports`]) at the first of `rates` the
/// host runs at, in the best sample format the streams convert.
pub(crate) fn port_group_config(configs: &[cpal::SupportedStreamConfigRange], rates: &[u32], channels: u16) -> Option<cpal::SupportedStreamConfig> {
    rates.iter().find_map(|rate| supported_format_config(configs, *rate, channels.max(1)))
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

/// The audio system to open an output (or input) stream on: the one the command line forced,
/// else the chosen one, else the first of the automatic order, in each case one that has a
/// device for that direction (see [`crate::host`]). With no such system, the platform's own.
pub(crate) fn host(cfg: &AudioConfig, output: bool) -> cpal::Host {
    let direction = if output { crate::host::Direction::Output } else { crate::host::Direction::Input };
    crate::host::pick(cfg, direction).unwrap_or_else(cpal::default_host)
}

/// The chosen output (or input) device of `host`, else its default device. `want` is a device id;
/// a name, as preferences written before devices had ids hold, finds the device too.
pub(crate) fn device(host: &cpal::Host, want: Option<&str>, output: bool) -> Option<cpal::Device> {
    // A JACK port group is no device: the stream opens on the default and is wired afterwards.
    if let Some(want) = want.filter(|w| !w.starts_with(crate::host::JACK_PORTS)) {
        let id = want.parse::<cpal::DeviceId>().ok();
        // An id finds any device of the host: a microphone is no output.
        let by_id = id.as_ref().and_then(|id| host.device_by_id(id)).filter(|d| if output { d.supports_output() } else { d.supports_input() });
        let found = by_id.or_else(|| {
            if output { host.output_devices().ok() } else { host.input_devices().ok() }
                .and_then(|mut ds| ds.find(|d| d.description().is_ok_and(|description| description.name() == want)))
        });
        if found.is_some() {
            return found;
        }
        // A device remembered for another system (the command line or the automatic order chose
        // this one) is not worth a warning.
        if id.is_some_and(|id| id.host() != host.id()) {
            log::debug!("audio device `{want}` belongs to another audio system than {}; using the default", host.id().name());
        } else {
            log::warn!("audio device `{want}` is not available; using the default");
        }
    }
    if output { host.default_output_device() } else { host.default_input_device() }
}

/// The buffer to request: the chosen size brought into the device's range (a size remembered
/// from another device still gets the nearest one this device has), else the default.
pub(crate) fn buffer_size(want: Option<u32>, supported: &cpal::SupportedBufferSize) -> cpal::BufferSize {
    match (want, supported) {
        (Some(n), cpal::SupportedBufferSize::Range { min, max }) if min <= max => cpal::BufferSize::Fixed(n.clamp(*min, *max)),
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
        let sizes = |min, max| cpal::SupportedBufferSize::Range { min, max };
        // (chosen, what the device supports, requested)
        let cases = [
            (Some(256), sizes(64, 1024), cpal::BufferSize::Fixed(256)),
            (Some(64), sizes(64, 1024), cpal::BufferSize::Fixed(64)),
            (Some(1024), sizes(64, 1024), cpal::BufferSize::Fixed(1024)),
            // Outside the range: the nearest size the device has.
            (Some(32), sizes(64, 1024), cpal::BufferSize::Fixed(64)),
            (Some(4096), sizes(64, 1024), cpal::BufferSize::Fixed(1024)),
            // A device with one size only.
            (Some(128), sizes(480, 480), cpal::BufferSize::Fixed(480)),
            (Some(8192), sizes(480, 480), cpal::BufferSize::Fixed(480)),
            // A range that makes no sense is not trusted.
            (Some(256), sizes(1024, 64), cpal::BufferSize::Default),
            (None, sizes(64, 1024), cpal::BufferSize::Default),
            (None, cpal::SupportedBufferSize::Unknown, cpal::BufferSize::Default),
            (Some(128), cpal::SupportedBufferSize::Unknown, cpal::BufferSize::Fixed(128)),
            (Some(1 << 20), cpal::SupportedBufferSize::Unknown, cpal::BufferSize::Default),
            (Some(8), cpal::SupportedBufferSize::Unknown, cpal::BufferSize::Default),
        ];
        for (want, supported, expected) in cases {
            assert_eq!(buffer_size(want, &supported), expected, "{want:?} on {supported:?}");
        }
        let d = info("x", "x", &[range(2, 48_000, 48_000, cpal::SampleFormat::F32)]);
        assert_eq!((d.buffer_min, d.buffer_max), (Some(64), Some(1024)));
        assert_eq!(d.buffer_sizes(), [64, 128, 256, 512, 1024]);
        let open = DeviceInfo { buffer_min: None, buffer_max: None, ..d };
        assert_eq!(open.buffer_sizes().len(), STANDARD_BUFFERS.len());
    }

    /// A listed device with these configurations, described for the settings window.
    fn info(id: &str, name: &str, ranges: &[cpal::SupportedStreamConfigRange]) -> DeviceInfo {
        describe(crate::host::Listed { id: id.into(), name: name.into(), detail: String::new(), ranges: ranges.to_vec() }, Some("default-id"))
    }

    /// An audio system with these devices; `default`: the one the automatic order opens.
    fn system(id: &str, name: &str, default: bool, outputs: Vec<DeviceInfo>, inputs: Vec<DeviceInfo>) -> AudioSystem {
        AudioSystem {
            id: id.into(),
            name: name.into(),
            is_default: default,
            is_default_input: default,
            has_output: !outputs.is_empty(),
            output_route: !outputs.is_empty(),
            has_input: !inputs.is_empty(),
            outputs,
            inputs,
        }
    }

    /// A stream wired to a JACK interface has one channel per port of it: a one-port interface
    /// gets a mono stream (the mix folds into it, so nothing panned right is lost) and a
    /// four-input one a four-channel stream, whatever rate the session asks for.
    #[test]
    fn a_jack_stream_is_as_wide_as_the_interface_it_is_wired_to() {
        // What cpal's JACK host offers: every channel count at the server's rate.
        let jack: Vec<_> = (1..=64).map(|channels| range(channels, 48_000, 48_000, cpal::SampleFormat::F32)).collect();
        let at = |rates: &[u32], ports| port_group_config(&jack, rates, ports).map(|c| (c.sample_rate(), c.channels()));
        assert_eq!(at(&[48_000], 1), Some((48_000, 1)));
        assert_eq!(at(&[48_000], 4), Some((48_000, 4)));
        // A rate the server does not run at is passed over for the one it does.
        assert_eq!(at(&[44_100, 48_000], 4), Some((48_000, 4)));
        assert_eq!(at(&[44_100], 4), None);
        // More ports than the host has channels: as many as it has.
        assert_eq!(at(&[48_000], 200), Some((48_000, 64)));
        assert_eq!(at(&[48_000], 0), Some((48_000, 1)));
    }

    #[test]
    fn a_device_is_described_by_the_formats_the_streams_convert() {
        // I24 is not converted: its wider channel count, rates and buffer range do not count.
        let wide_i24 = cpal::SupportedStreamConfigRange::new(
            8,
            44_100,
            192_000,
            cpal::SupportedBufferSize::Range { min: 16, max: 8192 },
            cpal::SampleFormat::I24,
        );
        let d = info(
            "alsa:hw:CARD=0,DEV=0",
            "Interface",
            &[wide_i24, range(2, 44_100, 48_000, cpal::SampleFormat::I32), range(1, 96_000, 96_000, cpal::SampleFormat::I16)],
        );
        assert_eq!((d.id.as_str(), d.name.as_str(), d.is_default), ("alsa:hw:CARD=0,DEV=0", "Interface", false));
        assert_eq!((d.max_channels, d.sample_rates.as_slice()), (2, &[44_100, 48_000, 96_000][..]));
        assert_eq!((d.buffer_min, d.buffer_max), (Some(64), Some(1024)));
        // A device with no format the streams convert is still listed, with nothing to choose.
        let i24_only = info("alsa:hw:CARD=1,DEV=0", "Odd", &[wide_i24]);
        assert_eq!((i24_only.max_channels, i24_only.sample_rates.len(), i24_only.buffer_min), (0, 0, None));
        assert!(info("default-id", "Speakers", &[]).is_default);
    }

    #[test]
    fn a_jack_port_group_is_listed_without_configurations() {
        // No cpal device stands behind it: the rate and the buffer are the server's.
        let id = format!("{}Scarlett 2i2", crate::host::JACK_PORTS);
        let d = info(&id, "Scarlett 2i2", &[]);
        assert_eq!((d.id.as_str(), d.max_channels, d.sample_rates.len(), d.buffer_min, d.buffer_max), (id.as_str(), 0, 0, None, None));
        // Every standard buffer size stays on offer, and the group can be chosen.
        assert_eq!(d.buffer_sizes().len(), STANDARD_BUFFERS.len());
        let systems = [system("jack", "JACK", true, vec![d], Vec::new())];
        let chosen = AudioConfig { system: Some("jack".into()), output: Some(id.clone()), ..Default::default() };
        assert_eq!(chosen.with_ids(&systems), chosen);
    }

    /// Preferences written before devices had ids hold the system's and the devices' names.
    #[test]
    fn names_from_older_preferences_become_ids() {
        let device = |id: &str, name: &str| info(id, name, &[range(2, 48_000, 48_000, cpal::SampleFormat::F32)]);
        let systems = [
            system(
                "pipewire",
                "PipeWire",
                true,
                vec![device("pipewire:alsa_output.usb-X", "Scarlett 2i2")],
                vec![device("pipewire:alsa_input.usb-X", "Scarlett 2i2")],
            ),
            system(
                "alsa",
                "ALSA",
                false,
                vec![device("alsa:hw:CARD=USB,DEV=0", "Scarlett 2i2 USB, USB Audio"), device("alsa:pipewire", "PipeWire Sound Server")],
                vec![device("alsa:hw:CARD=USB,DEV=0", "Scarlett 2i2 USB, USB Audio")],
            ),
        ];
        let old = AudioConfig {
            system: Some("ALSA".into()),
            output: Some("PipeWire Sound Server".into()),
            input: Some("Scarlett 2i2 USB, USB Audio".into()),
            sample_rate: Some(48_000),
            buffer_size: Some(256),
        };
        let new = old.with_ids(&systems);
        assert_eq!(new.system.as_deref(), Some("alsa"));
        assert_eq!(new.output.as_deref(), Some("alsa:pipewire"));
        assert_eq!(new.input.as_deref(), Some("alsa:hw:CARD=USB,DEV=0"));
        assert_eq!((new.sample_rate, new.buffer_size), (Some(48_000), Some(256)));
        // Ids stay as they are.
        assert_eq!(new.with_ids(&systems), new);
        // No system chosen: the devices of the one the automatic order opens.
        let default_system = AudioConfig { output: Some("Scarlett 2i2".into()), ..Default::default() }.with_ids(&systems);
        assert_eq!((default_system.system, default_system.output.as_deref()), (None, Some("pipewire:alsa_output.usb-X")));
        // A system or device that is not there now (unplugged, server not running) is kept.
        let gone = AudioConfig { system: Some("JACK".into()), output: Some("Speakers".into()), input: Some("Mic".into()), ..Default::default() };
        assert_eq!(gone.with_ids(&systems), gone);
        // Nor does a device of the default system stand in for one of a system that is not there,
        // whatever its name: the choice must still mean that system's device when it is back.
        let other_host = AudioConfig {
            system: Some("ASIO".into()),
            output: Some("Scarlett 2i2".into()),
            input: Some("Scarlett 2i2".into()),
            ..Default::default()
        };
        assert_eq!(other_host.with_ids(&systems), other_host);
        let unplugged = AudioConfig { system: Some("alsa".into()), output: Some("Speakers".into()), ..Default::default() }.with_ids(&systems);
        assert_eq!((unplugged.system.as_deref(), unplugged.output.as_deref()), (Some("alsa"), Some("Speakers")));
        assert_eq!(AudioConfig::default().with_ids(&systems), AudioConfig::default());
        // The system is read the way stream opening reads it: spaces and case do not hide it,
        // and `auto` or blank is no choice (the default system's devices, and no system kept).
        let spaced =
            AudioConfig { system: Some(" ALSA ".into()), output: Some("PipeWire Sound Server".into()), ..Default::default() }.with_ids(&systems);
        assert_eq!((spaced.system.as_deref(), spaced.output.as_deref()), (Some("alsa"), Some("alsa:pipewire")));
        for none in ["auto", " "] {
            let auto = AudioConfig { system: Some(none.into()), output: Some("Scarlett 2i2".into()), ..Default::default() }.with_ids(&systems);
            assert_eq!((auto.system, auto.output.as_deref()), (None, Some("pipewire:alsa_output.usb-X")), "{none:?}");
        }
        // The automatic order opens the output on one system and the input on another (the first
        // has nothing to record from): each name is looked up where its stream opens.
        let mut split = systems.clone();
        split[0].is_default_input = false;
        split[1].is_default_input = true;
        let names = AudioConfig { output: Some("Scarlett 2i2".into()), input: Some("Scarlett 2i2 USB, USB Audio".into()), ..Default::default() };
        let ids = names.with_ids(&split);
        assert_eq!((ids.output.as_deref(), ids.input.as_deref()), (Some("pipewire:alsa_output.usb-X"), Some("alsa:hw:CARD=USB,DEV=0")));
        // The first listed system has nothing to play to (JACK without a server): not the default.
        let mut no_server = systems.to_vec();
        no_server.insert(0, system("jack", "JACK", false, Vec::new(), Vec::new()));
        assert_eq!(
            AudioConfig { output: Some("Scarlett 2i2".into()), ..Default::default() }.with_ids(&no_server).output.as_deref(),
            Some("pipewire:alsa_output.usb-X")
        );
        assert_eq!(old.with_ids(&[]), old);
    }

    /// The settings window lists the devices of the system the streams open on, which is not
    /// always the chosen one: the command line can force another, and a chosen system with
    /// nothing to open gives way to the automatic order.
    #[test]
    fn the_listed_devices_are_those_of_the_system_the_streams_open_on() {
        let d = || vec![info("x", "x", &[])];
        // JACK's server is not running; PipeWire has no microphone.
        let mut systems = [
            system("pipewire", "PipeWire", true, d(), Vec::new()),
            system("jack", "JACK", false, Vec::new(), Vec::new()),
            system("alsa", "ALSA", false, d(), d()),
        ];
        systems[0].is_default_input = false;
        systems[2].is_default_input = true;
        let id = |cfg: &AudioConfig, forced: Option<&str>, output| effective_system(&systems, cfg, forced, output).map(|s| s.id.as_str());
        let chose = |system: &str| AudioConfig { system: Some(system.into()), ..Default::default() };
        let auto = AudioConfig::default();
        assert_eq!((id(&auto, None, true), id(&auto, None, false)), (Some("pipewire"), Some("alsa")));
        // A chosen system holds, by id or by the name older preferences kept.
        assert_eq!((id(&chose("ALSA"), None, true), id(&chose("alsa"), None, false)), (Some("alsa"), Some("alsa")));
        // The command line wins over it; `auto` there lifts nothing but itself.
        assert_eq!(id(&chose("alsa"), Some("pipewire"), true), Some("pipewire"));
        assert_eq!(id(&chose("alsa"), Some("auto"), true), Some("alsa"));
        // A forced host this build does not have: the chosen one.
        assert_eq!(id(&chose("alsa"), Some("asio"), true), Some("alsa"));
        // Chosen or forced, but with nothing to open (no server; no microphone): the streams fall
        // back to the automatic order, and so does the list.
        assert_eq!((id(&chose("jack"), None, true), id(&chose("jack"), None, false)), (Some("pipewire"), Some("alsa")));
        assert_eq!(id(&chose("alsa"), Some("pipewire"), false), Some("alsa"));
        // A chosen or forced JACK whose server has no system playback ports still opens its
        // default output (only the automatic order passes it over): its devices are the ones listed.
        let mut unrouted = systems.clone();
        (unrouted[1].has_output, unrouted[1].output_route) = (true, false);
        let id = |cfg: &AudioConfig, forced: Option<&str>| effective_system(&unrouted, cfg, forced, true).map(|s| s.id.as_str());
        assert_eq!((id(&chose("jack"), None), id(&chose("alsa"), Some("jack"))), (Some("jack"), Some("jack")));
        assert_eq!(id(&auto, None), Some("pipewire"), "not asked for: passed over");
        // Nothing to open anywhere: the system that was asked for, else the first.
        let silent = [system("pipewire", "PipeWire", false, Vec::new(), Vec::new()), system("jack", "JACK", false, Vec::new(), Vec::new())];
        assert_eq!(effective_system(&silent, &chose("jack"), None, true).map(|s| s.id.as_str()), Some("jack"));
        assert_eq!(effective_system(&silent, &auto, None, true).map(|s| s.id.as_str()), Some("pipewire"));
        assert!(effective_system(&[], &auto, None, true).is_none());
    }

    #[test]
    fn unknown_systems_and_devices_fall_back_to_the_defaults() {
        // A system that does not exist: the one the automatic order opens.
        let cfg = AudioConfig { system: Some("No Such System".into()), ..Default::default() };
        assert_eq!(host(&cfg, true).id(), host(&AudioConfig::default(), true).id());
        // No device with this id or name exists: the default (or none, on a machine without audio).
        let h = host(&AudioConfig::default(), true);
        let default = h.default_output_device().as_ref().and_then(device_id);
        for missing in ["No Such Device 1234", "alsa:no-such-device-1234", "jack-ports:No Such Interface"] {
            assert_eq!(device(&h, Some(missing), true).as_ref().and_then(device_id), default, "{missing}");
        }
    }

    #[test]
    fn enumeration_does_not_fail() {
        let systems = audio_systems();
        // Every listed system has a name; on a machine without audio the lists are just empty.
        assert!(systems.iter().all(|s| !s.name.is_empty() && !s.id.is_empty()));
        assert!(systems.iter().filter(|s| s.is_default).count() <= 1 && systems.iter().filter(|s| s.is_default_input).count() <= 1);
        // The default system is one "Default" opens something on.
        assert!(
            systems.iter().all(|s| (!s.is_default || s.output_route) && (!s.is_default_input || s.has_input) && (!s.output_route || s.has_output))
        );
        // Device ids are what the config holds: unique within a list.
        for list in systems.iter().flat_map(|s| [&s.outputs, &s.inputs]) {
            let mut ids: Vec<&str> = list.iter().map(|d| d.id.as_str()).collect();
            ids.sort_unstable();
            ids.dedup();
            assert_eq!(ids.len(), list.len());
        }
    }
}
