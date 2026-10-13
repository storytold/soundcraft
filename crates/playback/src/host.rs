//! Audio host (driver API) selection.
//!
//! Every platform has a built-in host (CoreAudio, WASAPI, ALSA, WebAudio); cargo features add the
//! others: `jack` and `pipewire` on Linux, `asio` and `jack` on Windows, `jack` on macOS.
//! Without a preference the first host that has a default device wins. On Linux the order is
//! PipeWire, JACK, ALSA: the sound server the desktop runs is preferred over the raw device it
//! holds, and a running JACK server usually owns the interface exclusively. On macOS and Windows
//! the system host (CoreAudio, WASAPI) comes first, so ASIO and JACK are used only when chosen.
//!
//! The preference is [`force_host`] (the command line's `--audio-host`) and then
//! [`AudioConfig::system`] (the Playback Engine dialog); it holds for both the output
//! ([`crate::Player`]) and the input ([`crate::record::Recorder`]). A preferred host that cannot
//! be opened is not an error: the automatic order is used and a warning is logged.

use crate::AudioConfig;
use std::sync::{Mutex, PoisonError};

#[cfg(all(feature = "jack", not(target_arch = "wasm32")))]
mod jack_ports;

/// A host that wins over the configured one for this run (`--audio-host`).
static FORCED: Mutex<Option<String>> = Mutex::new(None);

/// Prefix of the device ids that name a group of JACK ports (the rest is the JACK client name).
/// JACK has no devices: such a "device" is wired after the stream opens ([`wire_stream`]).
pub(crate) const JACK_PORTS: &str = "jack-ports:";

/// Use this host for the rest of the run, whatever the [`AudioConfig`] says (the command line's
/// `--audio-host`); `None`, an empty id or `auto` lifts it.
pub fn force_host(id: Option<&str>) {
    *FORCED.lock().unwrap_or_else(PoisonError::into_inner) = id.and_then(host_key);
}

fn forced() -> Option<String> {
    FORCED.lock().unwrap_or_else(PoisonError::into_inner).clone()
}

/// A host id as the user may write it (`"ALSA"`, `" pipewire "`), in the form cpal's ids have.
/// `None` for no choice (empty, `auto`). Host names differ from their ids only in case, so a
/// name remembered by an older version reads as its id.
fn host_key(s: &str) -> Option<String> {
    let s = s.trim().to_ascii_lowercase();
    (!s.is_empty() && s != "auto").then_some(s)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Direction {
    Output,
    Input,
}

/// A device as a host lists it, tidied (see [`tidy`]).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Listed {
    /// What [`AudioConfig::output`] and [`AudioConfig::input`] hold to choose it.
    pub(crate) id: String,
    pub(crate) name: String,
    /// What tells it apart from its neighbours (`"2 ch · USB"`, `"direct · hw:CARD=0,DEV=0"`);
    /// may be empty.
    pub(crate) detail: String,
    /// Every configuration the device offers; none for a JACK port group, whose rate and buffer
    /// are the server's.
    pub(crate) ranges: Vec<cpal::SupportedStreamConfigRange>,
}

/// The hosts this build can open on this machine, in the automatic order.
pub(crate) fn host_ids() -> Vec<cpal::HostId> {
    let mut ids: Vec<cpal::HostId> = cpal::available_hosts().into_iter().filter(|id| usable(*id)).collect();
    ids.sort_by_key(|id| rank(&id.to_string()));
    ids
}

/// Place of a host in the automatic order (lower first). The system's own host leads everywhere;
/// ASIO and, off Linux, JACK are for people who ask for them: ASIO takes the interface away from
/// other applications and JACK needs a server the user started.
fn rank(id: &str) -> u8 {
    rank_on(id, cfg!(any(target_os = "linux", target_os = "dragonfly", target_os = "freebsd", target_os = "netbsd")))
}

/// [`rank`], on a platform where a JACK server is (`jack_owns_device`) or is not the usual owner
/// of the audio interface.
fn rank_on(id: &str, jack_owns_device: bool) -> u8 {
    match id {
        "pipewire" | "coreaudio" | "wasapi" | "webaudio" => 0,
        "pulseaudio" => 1,
        // On Linux and the BSDs a running JACK server usually owns the device ALSA would open.
        "jack" if jack_owns_device => 2,
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

/// The order to try the hosts in. `available` is what this build can open here, in the automatic
/// order (see [`rank`]); the forced host (`--audio-host`) goes first when it is among them, else
/// the configured one (the Playback Engine dialog), else nothing moves. A host that is not
/// available is never tried: with `jack` built in but no libjack, JACK is not in `available`.
fn order(available: &[String], forced: Option<&str>, configured: Option<&str>) -> Vec<String> {
    let mut ids = available.to_vec();
    let wanted = [forced, configured].into_iter().flatten().find_map(|want| ids.iter().position(|id| id == want));
    if let Some(i) = wanted {
        let id = ids.remove(i);
        ids.insert(0, id);
    }
    ids
}

/// The first host of `order` that `open` succeeds on (it has a device), with what `open` made.
fn first_open<T>(order: &[String], mut open: impl FnMut(&str) -> Option<T>) -> Option<(&str, T)> {
    order.iter().find_map(|id| open(id).map(|opened| (id.as_str(), opened)))
}

/// The host to open a `direction` stream on: the forced one, else the one `cfg` names, else the
/// automatic order; in each case the first that has a default device for `direction`. `None` when
/// no host has one.
pub(crate) fn pick(cfg: &AudioConfig, direction: Direction) -> Option<cpal::Host> {
    use cpal::traits::HostTrait;
    let ids = host_ids();
    let available: Vec<String> = ids.iter().map(ToString::to_string).collect();
    let (forced, configured) = (forced(), cfg.system.as_deref().and_then(host_key));
    for want in [&forced, &configured].into_iter().flatten() {
        if !available.contains(want) {
            log::warn!("audio host '{want}' is not available here (have: {}); choosing automatically", available.join(", "));
        }
    }
    let order = order(&available, forced.as_deref(), configured.as_deref());
    let asked = [&forced, &configured].into_iter().flatten().find(|want| available.contains(want));
    let what = match direction {
        Direction::Output => "output",
        Direction::Input => "input",
    };
    let picked = first_open(&order, |want| {
        let id = ids.iter().find(|id| id.to_string() == want)?;
        let host = match cpal::host_from_id(*id) {
            Ok(host) => host,
            Err(e) => {
                log::debug!("audio host {}: {e}", id.name());
                return None;
            }
        };
        let has_device = match direction {
            Direction::Output => host.default_output_device().is_some(),
            Direction::Input => host.default_input_device().is_some(),
        };
        if !has_device {
            // Only the host the user asked for is worth a warning; the rest are just probed.
            if asked.is_some_and(|a| a == want) {
                log::warn!("audio host {} has no {what} device (is its server running?); choosing automatically", id.name());
            } else {
                log::debug!("audio host {}: no default {what} device", id.name());
            }
        }
        has_device.then_some(host)
    });
    picked.map(|(_, host)| host)
}

/// The devices of `host` worth offering for `direction`, tidied (see [`tidy`]). On JACK, which
/// has no devices of its own, the interfaces its physical ports belong to.
pub(crate) fn listed(host: &cpal::Host, direction: Direction) -> Vec<Listed> {
    use cpal::traits::{DeviceTrait, HostTrait};
    #[cfg(all(feature = "jack", not(target_arch = "wasm32")))]
    if host.id() == cpal::HostId::Jack {
        return jack_ports::groups(direction);
    }
    let Ok(all) = host.devices() else { return Vec::new() };
    // (monitor of an output, entry): real inputs are listed before monitors.
    let mut out: Vec<(bool, Listed)> = Vec::new();
    for d in all {
        let fits = match direction {
            Direction::Output => d.supports_output(),
            Direction::Input => d.supports_input(),
        };
        let (Ok(id), true) = (d.id(), fits) else { continue };
        let id = id.to_string();
        if out.iter().any(|(_, x)| x.id == id) {
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
        let ranges: Vec<cpal::SupportedStreamConfigRange> = match direction {
            Direction::Output => d.supported_output_configs().map(Iterator::collect).unwrap_or_default(),
            Direction::Input => d.supported_input_configs().map(Iterator::collect).unwrap_or_default(),
        };
        // 64 is what ALSA plugins and JACK answer for "anything".
        if let Some(n) = ranges.iter().map(cpal::SupportedStreamConfigRange::channels).max().filter(|n| *n < 64) {
            detail.insert(0, format!("{n} ch"));
        }
        out.push((monitor, Listed { id, name, detail: detail.join(" · "), ranges }));
    }
    out.sort_by_key(|(monitor, _)| *monitor);
    out.into_iter().map(|(_, entry)| entry).collect()
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

/// After a stream of `direction` opened on `host`: on JACK, wire it to the interface `device`
/// names (see `jack_ports`). Elsewhere the device was already the right one.
pub(crate) fn wire_stream(direction: Direction, host: &cpal::Host, device: Option<&str>) {
    #[cfg(all(feature = "jack", not(target_arch = "wasm32")))]
    if host.id() == cpal::HostId::Jack
        && let Some(id) = device
        && let Err(e) = jack_ports::patch(direction, id)
    {
        log::warn!("JACK: {e}; the stream keeps the connections it had");
    }
    let _ = (direction, host, device);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(v: &[&str]) -> Vec<String> {
        v.iter().map(ToString::to_string).collect()
    }

    /// `--audio-host` (forced) wins over the Playback Engine dialog (configured), and a host that
    /// cannot be opened here never moves to the front.
    #[test]
    fn the_command_line_wins_over_the_dialog_and_unavailable_hosts_are_skipped() {
        let linux = ids(&["pipewire", "jack", "alsa"]);
        // (available, forced, configured, expected order)
        let cases: &[(&[String], Option<&str>, Option<&str>, &[&str])] = &[
            (&linux, None, None, &["pipewire", "jack", "alsa"]),
            (&linux, None, Some("alsa"), &["alsa", "pipewire", "jack"]),
            (&linux, None, Some("jack"), &["jack", "pipewire", "alsa"]),
            (&linux, Some("alsa"), None, &["alsa", "pipewire", "jack"]),
            (&linux, Some("alsa"), Some("jack"), &["alsa", "pipewire", "jack"]),
            (&linux, Some("jack"), Some("jack"), &["jack", "pipewire", "alsa"]),
            // Not in this build or not on this machine: the next preference, then the automatic order.
            (&linux, Some("asio"), None, &["pipewire", "jack", "alsa"]),
            (&linux, Some("asio"), Some("alsa"), &["alsa", "pipewire", "jack"]),
            (&linux, None, Some("coreaudio"), &["pipewire", "jack", "alsa"]),
        ];
        for (available, forced, configured, want) in cases {
            assert_eq!(order(available, *forced, *configured), ids(want), "forced {forced:?}, configured {configured:?}");
        }
        // `jack` built in but libjack missing: JACK is not available, so asking for it changes nothing.
        let no_libjack = ids(&["pipewire", "alsa"]);
        assert_eq!(order(&no_libjack, Some("jack"), None), no_libjack);
        assert_eq!(order(&no_libjack, None, Some("jack")), no_libjack);
        assert_eq!(order(&[], Some("jack"), Some("alsa")), ids(&[]));
    }

    #[test]
    fn a_host_without_a_device_gives_way_to_the_next_one() {
        let order = ids(&["jack", "pipewire", "alsa"]);
        // The preferred host's server is not running: the next host with a device.
        assert_eq!(first_open(&order, |id| (id != "jack").then_some(id.len())), Some(("pipewire", 8)));
        assert_eq!(first_open(&order, |id| (id == "alsa").then_some(())), Some(("alsa", ())));
        assert_eq!(first_open(&order, |id| (id == "jack").then_some(())), Some(("jack", ())));
        assert_eq!(first_open(&order, |_| None::<()>), None);
        assert_eq!(first_open(&[], |_| Some(())), None);
    }

    #[test]
    fn host_choices_are_read_as_ids() {
        assert_eq!(host_key(" PipeWire "), Some("pipewire".into()));
        // What versions before the host ids kept: the host's display name.
        for (name, id) in [("ALSA", "alsa"), ("JACK", "jack"), ("WASAPI", "wasapi"), ("ASIO", "asio"), ("CoreAudio", "coreaudio")] {
            assert_eq!(host_key(name).as_deref(), Some(id));
        }
        for none in ["", "  ", "auto", "AUTO"] {
            assert_eq!(host_key(none), None, "{none:?}");
        }
    }

    #[test]
    fn forcing_a_host_can_be_lifted() {
        force_host(Some(" Auto "));
        assert_eq!(forced(), None);
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
        let sorted = |hosts: &[&str], jack_owns_device: bool| {
            let mut hosts = ids(hosts);
            hosts.sort_by_key(|id| rank_on(id, jack_owns_device));
            hosts
        };
        // Windows and macOS: ASIO and JACK only when asked for.
        assert_eq!(sorted(&["asio", "wasapi", "jack"], false), ids(&["wasapi", "asio", "jack"]));
        assert_eq!(sorted(&["jack", "coreaudio"], false), ids(&["coreaudio", "jack"]));
        // Linux: the sound server, then JACK (it owns the device when it runs), then the raw device.
        assert_eq!(sorted(&["alsa", "jack", "pipewire"], true), ids(&["pipewire", "jack", "alsa"]));
        assert_eq!(sorted(&["alsa", "jack"], true), ids(&["jack", "alsa"]));
        assert_eq!(sorted(&["alsa"], true), ids(&["alsa"]));
    }

    #[test]
    fn the_platform_host_is_always_listed() {
        // Whatever the features, the built-in host (ALSA, CoreAudio, WASAPI) can be opened.
        let hosts = host_ids();
        assert!(!hosts.is_empty());
        assert!(hosts.iter().all(|h| !h.to_string().is_empty() && h.to_string() == h.to_string().to_ascii_lowercase() && !h.name().is_empty()));
    }
}
