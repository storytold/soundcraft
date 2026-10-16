//! Audio host (driver API) selection.
//!
//! Every platform has a built-in host (CoreAudio, WASAPI, ALSA, WebAudio); cargo features add the
//! others: `jack` and `pipewire` on Linux, `asio` and `jack` on Windows, `jack` on macOS.
//! Without a preference the first host that has a default device wins. On Linux the order is
//! PipeWire, JACK, ALSA: the sound server the desktop runs is preferred over the raw device it
//! holds, and a running JACK server usually owns the interface exclusively. On macOS and Windows
//! the system host (CoreAudio, WASAPI) comes first, so ASIO and JACK are used only when chosen.
//!
//! The preference comes from [`set_preferred_host`] (the Playback Engine dialog, `--audio-host`)
//! or else the `SOUNDCRAFT_AUDIO_HOST` environment variable, and holds for both the output
//! ([`crate::Player`]) and the input ([`crate::record::Recorder`]). A preferred host that cannot
//! be opened is not an error: the automatic order is used and a warning is logged.

use std::sync::{Mutex, PoisonError};

#[cfg(all(feature = "jack", not(target_arch = "wasm32")))]
mod jack_ports;

/// Environment variable naming the preferred host id (`pipewire`, `jack`, `alsa`, …).
pub const HOST_ENV: &str = "SOUNDCRAFT_AUDIO_HOST";

static PREFERRED: Mutex<Option<String>> = Mutex::new(None);
static OUTPUT_DEVICE: Mutex<Option<String>> = Mutex::new(None);
static INPUT_DEVICE: Mutex<Option<String>> = Mutex::new(None);
static BUFFER_SIZE: Mutex<Option<u32>> = Mutex::new(None);
/// A host that wins over the configured one for this run (`--audio-host`).
static FORCED: Mutex<Option<String>> = Mutex::new(None);

/// Device buffer sizes (frames) offered in the Playback Engine dialog.
pub const BUFFER_SIZES: [u32; 10] = [16, 32, 64, 128, 256, 512, 1024, 2048, 4096, 8192];

/// Ask the devices for this buffer size (frames per callback) from now on; `None` leaves it to
/// the host. Smaller is less delay between playing and hearing, larger is safer against
/// crackles. The host may give another size (PipeWire keeps to its own quantum limits).
pub fn set_preferred_buffer_size(frames: Option<u32>) {
    *BUFFER_SIZE.lock().unwrap_or_else(PoisonError::into_inner) = frames.filter(|n| (8..=65_536).contains(n));
}

pub fn preferred_buffer_size() -> Option<u32> {
    *BUFFER_SIZE.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The buffer size to open a stream with: `want` brought into what the device supports.
pub(crate) fn buffer_size(want: Option<u32>, supported: &cpal::SupportedBufferSize) -> cpal::BufferSize {
    match (want, supported) {
        (None, _) => cpal::BufferSize::Default,
        (Some(n), cpal::SupportedBufferSize::Range { min, max }) if min <= max => cpal::BufferSize::Fixed(n.clamp(*min, *max)),
        // The device does not say what it takes: ask, within reason (a refusal falls back to the
        // device's own size).
        (Some(n), _) if (16..=8192).contains(&n) => cpal::BufferSize::Fixed(n),
        _ => cpal::BufferSize::Default,
    }
}

/// A host this build can open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostInfo {
    /// Stable lowercase id (`"pipewire"`, `"jack"`, `"alsa"`, `"coreaudio"`, …).
    pub id: String,
    /// Display name (`"PipeWire"`, `"JACK"`, `"ALSA"`, …).
    pub name: String,
}

/// A device of the host in use.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceInfo {
    /// Stable id, for [`set_preferred_output_device`] / [`set_preferred_input_device`].
    pub id: String,
    /// Display name.
    pub name: String,
    /// What tells it apart from its neighbours (`"USB · 2 ch"`, `"direct · hw:CARD=0,DEV=0"`); may
    /// be empty.
    pub detail: String,
}

/// Use this host for the rest of the run, whatever [`configure`] is given (the command line's
/// `--audio-host`); `None` lifts it.
pub fn force_host(id: Option<&str>) {
    *FORCED.lock().unwrap_or_else(PoisonError::into_inner) = id.map(|s| s.trim().to_ascii_lowercase()).filter(|s| !s.is_empty() && s != "auto");
}

/// Take the host, devices and buffer size to open from `cfg`: what [`crate::Player::with_config`]
/// and [`crate::record::Recorder::open_with`] do first. `system` is a host id or name, `output`
/// and `input` are ids from [`output_devices`] and [`input_devices`]. A forced host
/// ([`force_host`]) wins, and then the devices of another host are not used.
pub fn configure(cfg: &crate::AudioConfig) {
    let system = cfg.system.as_deref().map(|s| s.trim().to_ascii_lowercase()).filter(|s| !s.is_empty() && s != "auto");
    let forced = FORCED.lock().unwrap_or_else(PoisonError::into_inner).clone();
    let own_devices = forced.is_none() || forced == system;
    set_preferred_host(forced.as_deref().or(system.as_deref()));
    set_preferred_output_device(cfg.output.as_deref().filter(|_| own_devices));
    set_preferred_input_device(cfg.input.as_deref().filter(|_| own_devices));
    set_preferred_buffer_size(cfg.buffer_size);
}

/// Prefer a host by id for the devices opened from now on; `None` (or an empty id) returns to the
/// automatic order. Changing the host forgets the preferred devices. Streams that are already open
/// keep their host until they are reopened.
pub fn set_preferred_host(id: Option<&str>) {
    let id = id.map(|s| s.trim().to_ascii_lowercase()).filter(|s| !s.is_empty() && s != "auto");
    let mut slot = PREFERRED.lock().unwrap_or_else(PoisonError::into_inner);
    if *slot != id {
        // Device ids belong to a host.
        set_preferred_output_device(None);
        set_preferred_input_device(None);
    }
    *slot = id;
}

/// The hosts this platform knows that cannot be chosen right now: display name and why (not in
/// this build, library or server missing). For showing them greyed out instead of not at all.
pub fn unavailable_hosts() -> Vec<(String, String)> {
    let known: &[(&str, &str, bool)] = if cfg!(target_os = "linux") {
        &[("pipewire", "PipeWire", cfg!(feature = "pipewire")), ("jack", "JACK", cfg!(feature = "jack"))]
    } else if cfg!(target_os = "windows") {
        &[("asio", "ASIO", cfg!(feature = "asio")), ("jack", "JACK", cfg!(feature = "jack"))]
    } else if cfg!(target_os = "macos") {
        &[("jack", "JACK", cfg!(feature = "jack"))]
    } else {
        &[]
    };
    let have = available_hosts();
    known
        .iter()
        .filter(|(id, _, _)| !have.iter().any(|h| h.id == *id))
        .map(|(id, name, built)| {
            let why = match (*built, *id) {
                (false, _) => format!("not in this build: build with --features {id}"),
                (true, "jack") => "the JACK library (libjack) was not found".to_string(),
                (true, "pipewire") => "PipeWire is not running".to_string(),
                (true, _) => "not available on this machine".to_string(),
            };
            (name.to_string(), why)
        })
        .collect()
}

/// The hosts this platform can have besides its built-in one, and the cargo features that add
/// them: `("JACK and PipeWire", "jack,pipewire")`. For telling the user why a host is not offered.
pub fn optional_hosts() -> Option<(&'static str, &'static str)> {
    if cfg!(target_os = "linux") {
        Some(("JACK and PipeWire", "jack,pipewire"))
    } else if cfg!(target_os = "windows") {
        Some(("ASIO and JACK", "asio,jack"))
    } else if cfg!(target_os = "macos") {
        Some(("JACK", "jack"))
    } else {
        None
    }
}

/// Open this output device (an id from [`output_devices`]) from now on; `None` is the host's
/// default. A device that is gone falls back to the default with a warning.
pub fn set_preferred_output_device(id: Option<&str>) {
    *OUTPUT_DEVICE.lock().unwrap_or_else(PoisonError::into_inner) = id.filter(|s| !s.is_empty()).map(str::to_string);
}

/// Open this input device (an id from [`input_devices`]) from now on; `None` is the host's default.
pub fn set_preferred_input_device(id: Option<&str>) {
    *INPUT_DEVICE.lock().unwrap_or_else(PoisonError::into_inner) = id.filter(|s| !s.is_empty()).map(str::to_string);
}

fn preferred_device(direction: Direction) -> Option<String> {
    let slot = match direction {
        Direction::Output => &OUTPUT_DEVICE,
        Direction::Input => &INPUT_DEVICE,
    };
    slot.lock().unwrap_or_else(PoisonError::into_inner).clone()
}

/// The output devices of the host in use (the preferred one, else the first with a default
/// device). Probing every device takes a moment on ALSA: call it on demand, not every frame.
pub fn output_devices() -> Vec<DeviceInfo> {
    devices(Direction::Output)
}

/// The input devices of the host in use; see [`output_devices`].
pub fn input_devices() -> Vec<DeviceInfo> {
    devices(Direction::Input)
}

fn devices(direction: Direction) -> Vec<DeviceInfo> {
    let Ok((host, _, host_name)) = pick_host(direction) else { return Vec::new() };
    listed(&host, &host_name, direction).into_iter().map(|(info, _)| info).collect()
}

/// The devices of `host` worth offering for `direction`, tidied (see [`tidy`]), each with the
/// cpal device behind it: none for a JACK port group, which is wired after the stream opens.
pub(crate) fn listed(host: &cpal::Host, host_name: &str, direction: Direction) -> Vec<(DeviceInfo, Option<cpal::Device>)> {
    use cpal::traits::{DeviceTrait, HostTrait};
    // JACK has no devices of its own: offer the interfaces its ports belong to.
    #[cfg(all(feature = "jack", not(target_arch = "wasm32")))]
    if host_name == "JACK" {
        return jack_ports::groups(direction).into_iter().map(|d| (d, None)).collect();
    }
    let _ = host_name;
    let Ok(all) = host.devices() else { return Vec::new() };
    // (monitor of an output, entry, device): real inputs are listed before monitors.
    let mut out: Vec<(bool, DeviceInfo, cpal::Device)> = Vec::new();
    for d in all {
        let fits = match direction {
            Direction::Output => d.supports_output(),
            Direction::Input => d.supports_input(),
        };
        let (Ok(id), true) = (d.id(), fits) else { continue };
        let id = id.to_string();
        if out.iter().any(|(_, x, _)| x.id == id) {
            continue;
        }
        let desc = d.description().ok();
        let raw = Raw {
            id: &id,
            name: desc.as_ref().map_or("", |x| x.name()),
            note: desc.as_ref().and_then(|x| x.extended().nth(1)),
            interface: desc
                .as_ref()
                .map(|x| x.interface_type().to_string())
                .filter(|_| desc.as_ref().is_some_and(|x| x.interface_type() != cpal::InterfaceType::Unknown)),
            input_only: desc.as_ref().is_some_and(|x| x.direction() == cpal::DeviceDirection::Input),
        };
        let Some((name, mut detail, monitor)) = tidy(&raw, direction) else { continue };
        // Only now, for the entries that are shown: asking for the configs opens the device.
        let channels = match direction {
            Direction::Output => d.supported_output_configs().ok().and_then(|c| c.map(|c| c.channels()).max()),
            Direction::Input => d.supported_input_configs().ok().and_then(|c| c.map(|c| c.channels()).max()),
        };
        // 64 is what ALSA plugins and JACK answer for "anything".
        if let Some(n) = channels.filter(|n| *n < 64) {
            detail.insert(0, format!("{n} ch"));
        }
        out.push((monitor, DeviceInfo { id, name, detail: detail.join(" · ") }, d));
    }
    out.sort_by_key(|(monitor, ..)| *monitor);
    out.into_iter().map(|(_, info, d)| (info, Some(d))).collect()
}

/// What a host says about a device, for [`tidy`].
struct Raw<'a> {
    /// `host:device`.
    id: &'a str,
    name: &'a str,
    /// Second description line (ALSA: what the PCM is for).
    note: Option<&'a str>,
    /// USB, Bluetooth, …; `None` when the host does not know.
    interface: Option<String>,
    /// A capture-only device (a source), as opposed to an output that can also be recorded.
    input_only: bool,
}

/// The name and detail parts to show for a device, and whether it is the monitor of an output;
/// `None` for entries that only clutter the list: duplicates of "Default", ALSA's channel-layout
/// aliases of the same card, other applications' streams.
fn tidy(raw: &Raw, direction: Direction) -> Option<(String, Vec<String>, bool)> {
    let (host, device) = raw.id.split_once(':').unwrap_or(("", raw.id));
    let mut name = raw.name.trim().to_string();
    let mut detail: Vec<String> = raw.interface.iter().cloned().collect();
    let mut monitor = false;
    match host {
        "pipewire" => {
            // The host's default nodes are the "Default" entry; nameless nodes are other
            // applications' streams.
            if device.ends_with("_default") || name.is_empty() || name == "unknown" {
                return None;
            }
            // PipeWire lets every output be recorded.
            if direction == Direction::Input && !raw.input_only {
                name = format!("Monitor of {name}");
                monitor = true;
            }
        }
        "alsa" => {
            let kind = device.split(':').next().unwrap_or(device);
            match kind {
                // "Default" covers these; the rest are views of a card that `hw` / `plughw` open too.
                "default" | "sysdefault" | "null" | "front" | "rear" | "side" | "center_lfe" | "iec958" | "dmix" | "dsnoop" => return None,
                k if k.starts_with("surround") => return None,
                "hw" => detail.push("direct".into()),
                "plughw" => detail.push("with conversion".into()),
                "hdmi" => detail.push("HDMI".into()),
                _ => detail.extend(raw.note.map(str::to_string)),
            }
            detail.push(device.to_string());
        }
        _ => {}
    }
    if name.is_empty() || name == "unknown" {
        name = device.to_string();
    }
    Some((name, detail, monitor))
}

/// The name to show for an opened device.
pub(crate) fn device_label(device: &cpal::Device, fallback: &str) -> String {
    use cpal::traits::DeviceTrait;
    let name = device.description().map(|d| d.name().trim().to_string()).unwrap_or_default();
    match name.as_str() {
        "" => fallback.to_string(),
        // PipeWire's names for "whatever the system default is".
        "default_output" | "default_input" | "default_sink" => "System default".to_string(),
        "unknown" => device.id().map_or_else(|_| fallback.to_string(), |id| id.to_string()),
        _ => name,
    }
}

/// The preferred host id: the one set with [`set_preferred_host`], else [`HOST_ENV`].
pub fn preferred_host() -> Option<String> {
    let set = PREFERRED.lock().unwrap_or_else(PoisonError::into_inner).clone();
    set.or_else(|| std::env::var(HOST_ENV).ok().map(|s| s.trim().to_ascii_lowercase()).filter(|s| !s.is_empty() && s != "auto"))
}

/// The hosts this build can open on this machine, in the automatic order.
pub fn available_hosts() -> Vec<HostInfo> {
    host_ids().into_iter().map(|id| HostInfo { id: id.to_string(), name: id.name().to_string() }).collect()
}

pub(crate) fn host_ids() -> Vec<cpal::HostId> {
    let mut ids: Vec<cpal::HostId> = cpal::available_hosts().into_iter().filter(|id| usable(*id)).collect();
    ids.sort_by_key(|id| rank(&id.to_string()));
    ids
}

/// Place of a host in the automatic order (lower first). The system's own host leads everywhere;
/// ASIO and, off Linux, JACK are for people who ask for them: ASIO takes the interface away from
/// other applications and JACK needs a server the user started.
fn rank(id: &str) -> u8 {
    match id {
        "pipewire" | "coreaudio" | "wasapi" | "webaudio" => 0,
        "pulseaudio" => 1,
        // On Linux and the BSDs a running JACK server usually owns the device ALSA would open.
        "jack" if cfg!(any(target_os = "linux", target_os = "dragonfly", target_os = "freebsd", target_os = "netbsd")) => 2,
        "alsa" => 3,
        "asio" => 4,
        "jack" => 5,
        _ => 6,
    }
}

/// False for a host that would panic rather than fail when opened.
fn usable(id: cpal::HostId) -> bool {
    // The JACK bindings load libjack at runtime and panic on every call when it is missing, and
    // cpal reports the host as available regardless: ask for the library first.
    #[cfg(all(feature = "jack", not(target_arch = "wasm32")))]
    if id == cpal::HostId::Jack && jack_sys::library().is_err() {
        return false;
    }
    let _ = id;
    true
}

/// `ids` with the preferred host (when present) moved to the front.
fn ordered(mut ids: Vec<String>, preferred: Option<&str>) -> Vec<String> {
    if let Some(p) = preferred {
        match ids.iter().position(|id| id == p) {
            Some(i) => {
                let id = ids.remove(i);
                ids.insert(0, id);
            }
            None => log::warn!("audio host '{p}' is not available here (have: {}); choosing automatically", ids.join(", ")),
        }
    }
    ids
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Direction {
    Output,
    Input,
}

/// After a stream of `direction` opened on `host_name`: on JACK, wire it to the interface chosen
/// as the preferred device (see `jack_ports`). Elsewhere the device was already the right one.
pub(crate) fn wire_stream(direction: Direction, host_name: &str) {
    #[cfg(all(feature = "jack", not(target_arch = "wasm32")))]
    if host_name == "JACK"
        && let Some(id) = preferred_device(direction)
        && let Err(e) = jack_ports::patch(direction, &id)
    {
        log::warn!("JACK: {e}; the stream stays on the system ports");
    }
    let _ = (direction, host_name);
}

/// The device to open: the preferred device of the host in use when it is still there, else that
/// host's default. Returns the device and its host's display name.
pub(crate) fn default_device(direction: Direction) -> Result<(cpal::Device, String), String> {
    use cpal::traits::HostTrait;
    let (host, default, host_name) = pick_host(direction)?;
    if let Some(want) = preferred_device(direction) {
        let id = want.parse::<cpal::DeviceId>().ok();
        // A device remembered for another host (the automatic choice moved on) is not an error.
        let same_host = id.as_ref().is_some_and(|id| id.host().name() == host_name);
        match id.and_then(|id| host.device_by_id(&id)) {
            Some(device) => return Ok((device, host_name)),
            None if same_host => log::warn!("audio device '{want}' is not available on {host_name}; using the default"),
            None => log::debug!("audio device '{want}' belongs to another host than {host_name}; using the default"),
        }
    }
    Ok((default, host_name))
}

/// The preferred host, else the first host that has a default device for `direction`: the host,
/// that default device and the host's display name.
fn pick_host(direction: Direction) -> Result<(cpal::Host, cpal::Device, String), String> {
    use cpal::traits::HostTrait;
    let ids = host_ids();
    let preferred = preferred_host();
    let order = ordered(ids.iter().map(ToString::to_string).collect(), preferred.as_deref());
    let what = match direction {
        Direction::Output => "output",
        Direction::Input => "input",
    };
    for (n, want) in order.iter().enumerate() {
        let Some(id) = ids.iter().find(|id| &id.to_string() == want) else { continue };
        let host = match cpal::host_from_id(*id) {
            Ok(host) => host,
            Err(e) => {
                log::debug!("audio host {}: {e}", id.name());
                continue;
            }
        };
        let device = match direction {
            Direction::Output => host.default_output_device(),
            Direction::Input => host.default_input_device(),
        };
        match device {
            Some(device) => return Ok((host, device, id.name().to_string())),
            // Only the host the user asked for is worth a warning; the rest are just probed.
            None if n == 0 && preferred.as_deref() == Some(want.as_str()) => {
                log::warn!("audio host {} has no {what} device (is its server running?); choosing automatically", id.name());
            }
            None => log::debug!("audio host {}: no default {what} device", id.name()),
        }
    }
    Err(format!("no default {what} device"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(v: &[&str]) -> Vec<String> {
        v.iter().map(ToString::to_string).collect()
    }

    #[test]
    fn preferred_host_goes_first_and_unknown_ones_are_ignored() {
        let all = ids(&["pipewire", "jack", "alsa"]);
        assert_eq!(ordered(all.clone(), None), all);
        assert_eq!(ordered(all.clone(), Some("alsa")), ids(&["alsa", "pipewire", "jack"]));
        assert_eq!(ordered(all.clone(), Some("jack")), ids(&["jack", "pipewire", "alsa"]));
        assert_eq!(ordered(all.clone(), Some("asio")), all);
    }

    fn raw<'a>(id: &'a str, name: &'a str, note: Option<&'a str>, input_only: bool) -> Raw<'a> {
        Raw { id, name, note, interface: None, input_only }
    }

    #[test]
    fn device_lists_drop_clutter_and_explain_the_rest() {
        use Direction::{Input, Output};
        // PipeWire: defaults and other applications' streams go; outputs are monitors as inputs.
        assert!(tidy(&raw("pipewire:output_default", "default_output", None, false), Output).is_none());
        assert!(tidy(&raw("pipewire:karere", "unknown", None, false), Output).is_none());
        let sink = raw("pipewire:alsa_output.usb-X", "Scarlett 2i2", None, false);
        assert_eq!(tidy(&sink, Output), Some(("Scarlett 2i2".into(), vec![], false)));
        assert_eq!(tidy(&sink, Input), Some(("Monitor of Scarlett 2i2".into(), vec![], true)));
        let source = raw("pipewire:alsa_input.usb-X", "Scarlett 2i2", None, true);
        assert_eq!(tidy(&source, Input), Some(("Scarlett 2i2".into(), vec![], false)));
        // ALSA: one direct and one converting entry per card, not eleven aliases.
        for hidden in [
            "alsa:default",
            "alsa:null",
            "alsa:sysdefault:CARD=USB",
            "alsa:front:CARD=USB,DEV=0",
            "alsa:surround51:CARD=USB,DEV=0",
            "alsa:iec958:CARD=USB,DEV=0",
        ] {
            assert!(tidy(&raw(hidden, "Scarlett 2i2 USB, USB Audio", None, false), Output).is_none(), "{hidden}");
        }
        let hw = raw("alsa:hw:CARD=0,DEV=0", "Scarlett 2i2 USB, USB Audio", Some("Direct hardware device without any conversions"), false);
        assert_eq!(tidy(&hw, Output), Some(("Scarlett 2i2 USB, USB Audio".into(), vec!["direct".into(), "hw:CARD=0,DEV=0".into()], false)));
        let pw = raw("alsa:pipewire", "PipeWire Sound Server", None, false);
        assert_eq!(tidy(&pw, Output), Some(("PipeWire Sound Server".into(), vec!["pipewire".into()], false)));
        // Other hosts: as they come.
        assert_eq!(tidy(&raw("coreaudio:42", "MacBook Pro Speakers", None, false), Output), Some(("MacBook Pro Speakers".into(), vec![], false)));
    }

    #[test]
    fn the_system_host_leads_the_automatic_order() {
        let mut win = ids(&["asio", "wasapi", "jack"]);
        win.sort_by_key(|id| rank(id));
        // (ASIO against JACK depends on the platform the test runs on.)
        assert_eq!(win.first().map(String::as_str), Some("wasapi"));
        let mut mac = ids(&["jack", "coreaudio"]);
        mac.sort_by_key(|id| rank(id));
        assert_eq!(mac, ids(&["coreaudio", "jack"]));
        if cfg!(target_os = "linux") {
            let mut linux = ids(&["alsa", "jack", "pipewire"]);
            linux.sort_by_key(|id| rank(id));
            assert_eq!(linux, ids(&["pipewire", "jack", "alsa"]));
        }
    }

    #[test]
    fn the_platform_host_is_always_listed() {
        // Whatever the features, the built-in host (ALSA, CoreAudio, WASAPI) can be opened.
        let hosts = available_hosts();
        assert!(!hosts.is_empty());
        assert!(hosts.iter().all(|h| !h.id.is_empty() && h.id == h.id.to_ascii_lowercase() && !h.name.is_empty()));
    }
}
