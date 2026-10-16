//! Audio device selection: which audio system (host), output and input device, sample rate and
//! buffer size to open. The choice is per machine (the app keeps it with its preferences, not in
//! sessions). Anything that cannot be honoured falls back to the system default: a device that is
//! unplugged, a rate the device does not offer. The hosts themselves, their order and the opening
//! of devices are in [`crate::host`].

use cpal::traits::{DeviceTrait, HostTrait};

/// Rates offered when a device supports them (any rate inside a supported range is possible, but
/// these are the ones interfaces actually run at).
pub const STANDARD_RATES: [u32; 6] = [44_100, 48_000, 88_200, 96_000, 176_400, 192_000];
/// Buffer sizes offered when the device reports a range that contains them.
pub const STANDARD_BUFFERS: [u32; 10] = crate::host::BUFFER_SIZES;

/// The user's audio device choice. Every field is optional: `None` means "the system default".
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct AudioConfig {
    /// Audio system: a host id or name (`"pipewire"`, `"jack"`, `"alsa"`, `"wasapi"`, `"asio"`,
    /// `"coreaudio"`, …; see [`crate::host::available_hosts`]). `None`: the automatic order.
    pub system: Option<String>,
    /// Output device id (see [`crate::host::output_devices`] and [`DeviceInfo::id`]).
    pub output: Option<String>,
    /// Input device id (see [`crate::host::input_devices`]).
    pub input: Option<String>,
    /// Device sample rate; the mix resamples when it differs from the session's.
    pub sample_rate: Option<u32>,
    /// Device buffer in frames.
    pub buffer_size: Option<u32>,
}

/// One device as the settings window lists it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct DeviceInfo {
    /// What [`AudioConfig::output`] and [`AudioConfig::input`] hold to choose this device.
    pub id: String,
    pub name: String,
    /// What tells it apart from its neighbours (`"USB · 2 ch"`); may be empty.
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
    /// The first system of the automatic order.
    pub is_default: bool,
    pub outputs: Vec<DeviceInfo>,
    pub inputs: Vec<DeviceInfo>,
}

fn device_id(d: &cpal::Device) -> Option<String> {
    d.id().ok().map(|id| id.to_string())
}

/// What the settings window needs to know about a listed device. A JACK port group has no cpal
/// device behind it: its rate and buffer are the server's.
fn describe(info: crate::host::DeviceInfo, device: Option<&cpal::Device>, default_id: Option<&str>, output: bool) -> DeviceInfo {
    let usable = |c: &cpal::SupportedStreamConfigRange| crate::format_rank(c.sample_format()) != u8::MAX;
    let ranges: Vec<cpal::SupportedStreamConfigRange> = match device {
        Some(d) if output => d.supported_output_configs().map(|c| c.filter(usable).collect()).unwrap_or_default(),
        Some(d) => d.supported_input_configs().map(|c| c.filter(usable).collect()).unwrap_or_default(),
        None => Vec::new(),
    };
    let max_channels = ranges.iter().map(cpal::SupportedStreamConfigRange::channels).max().unwrap_or(0);
    let sample_rates =
        STANDARD_RATES.iter().copied().filter(|r| ranges.iter().any(|c| c.min_sample_rate() <= *r && c.max_sample_rate() >= *r)).collect();
    let (mut buffer_min, mut buffer_max) = (None::<u32>, None::<u32>);
    for c in &ranges {
        if let cpal::SupportedBufferSize::Range { min, max } = c.buffer_size() {
            buffer_min = Some(buffer_min.map_or(*min, |m| m.min(*min)));
            buffer_max = Some(buffer_max.map_or(*max, |m| m.max(*max)));
        }
    }
    DeviceInfo {
        is_default: default_id == Some(info.id.as_str()),
        id: info.id,
        name: info.name,
        detail: info.detail,
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
    crate::host::host_ids()
        .into_iter()
        .enumerate()
        .filter_map(|(n, id)| {
            use crate::host::Direction;
            let host = cpal::host_from_id(id).ok()?;
            let name = id.name().to_string();
            let def_out = host.default_output_device().as_ref().and_then(device_id);
            let def_in = host.default_input_device().as_ref().and_then(device_id);
            let list = |direction: Direction, default: Option<&str>| -> Vec<DeviceInfo> {
                let output = direction == Direction::Output;
                crate::host::listed(&host, &name, direction)
                    .into_iter()
                    .map(|(info, device)| describe(info, device.as_ref(), default, output))
                    .collect()
            };
            let (outputs, inputs) = (list(Direction::Output, def_out.as_deref()), list(Direction::Input, def_in.as_deref()));
            Some(AudioSystem { id: id.to_string(), name, is_default: n == 0, outputs, inputs })
        })
        .collect()
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

    #[test]
    fn buffer_sizes_follow_the_device_range() {
        use crate::host::buffer_size;
        let r = cpal::SupportedBufferSize::Range { min: 64, max: 1024 };
        assert_eq!(buffer_size(Some(256), &r), cpal::BufferSize::Fixed(256));
        // Outside the range: the nearest size the device has.
        assert_eq!(buffer_size(Some(32), &r), cpal::BufferSize::Fixed(64));
        assert_eq!(buffer_size(Some(4096), &r), cpal::BufferSize::Fixed(1024));
        assert_eq!(buffer_size(None, &r), cpal::BufferSize::Default);
        assert_eq!(buffer_size(Some(128), &cpal::SupportedBufferSize::Unknown), cpal::BufferSize::Fixed(128));
        assert_eq!(buffer_size(Some(1 << 20), &cpal::SupportedBufferSize::Unknown), cpal::BufferSize::Default);
        let d = DeviceInfo {
            id: "x".into(),
            name: "x".into(),
            detail: String::new(),
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
    fn enumeration_does_not_fail() {
        let systems = audio_systems();
        // Every listed system has a name; on a machine without audio the lists are just empty.
        assert!(systems.iter().all(|s| !s.name.is_empty() && !s.id.is_empty()));
        assert!(systems.iter().filter(|s| s.is_default).count() <= 1);
    }
}
