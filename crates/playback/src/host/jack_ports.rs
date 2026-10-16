//! Which hardware the JACK host's streams are wired to.
//!
//! JACK has no devices: cpal makes one client per stream and connects it to `system:capture_*` /
//! `system:playback_*`. With a JACK server that is the interface the server runs on. With
//! PipeWire's JACK library, `system` is whatever the desktop's default source and sink are, which
//! may well be a webcam microphone next to the speakers. So the Playback Engine dialog lists the
//! physical ports grouped by their client (one group per interface) and, when one is chosen, the
//! stream's ports are rewired to it after the stream opens.

use super::{DeviceInfo, Direction};
use jack::{Client, ClientOptions, PortFlags};

/// Prefix of the device ids this module hands out (the rest is the JACK client name).
const ID: &str = "jack-ports:";

fn client() -> Result<Client, String> {
    if jack_sys::library().is_err() {
        return Err("the JACK library (libjack) was not found".into());
    }
    Client::new("soundcraft-patchbay", ClientOptions::NO_START_SERVER).map(|(c, _)| c).map_err(|e| e.to_string())
}

/// The physical audio ports a stream of `direction` can be wired to, in the server's order.
fn hardware_ports(client: &Client, direction: Direction) -> Vec<String> {
    let flags = match direction {
        Direction::Input => PortFlags::IS_PHYSICAL | PortFlags::IS_OUTPUT,
        Direction::Output => PortFlags::IS_PHYSICAL | PortFlags::IS_INPUT,
    };
    // Monitor ports replay an output: recording one into the mix is a feedback loop.
    client.ports(None, Some("audio"), flags).into_iter().filter(|p| !p.contains(":monitor_")).collect()
}

fn group_of(port: &str) -> &str {
    port.rsplit_once(':').map_or(port, |(group, _)| group)
}

/// One entry per interface: the groups of physical ports.
pub(super) fn groups(direction: Direction) -> Vec<DeviceInfo> {
    let Ok(client) = client() else { return Vec::new() };
    let mut out: Vec<(String, usize)> = Vec::new();
    for port in hardware_ports(&client, direction) {
        let group = group_of(&port);
        match out.iter_mut().find(|(g, _)| g == group) {
            Some((_, n)) => *n += 1,
            None => out.push((group.to_string(), 1)),
        }
    }
    out.into_iter().map(|(group, n)| DeviceInfo { id: format!("{ID}{group}"), name: tidy_group(&group), detail: format!("{n} ch") }).collect()
}

/// PipeWire names a group after the device and its profile, often saying the device twice.
fn tidy_group(group: &str) -> String {
    let words: Vec<&str> = group.split_whitespace().collect();
    let mut out: Vec<&str> = Vec::new();
    for w in words {
        if !out.contains(&w) {
            out.push(w);
        }
    }
    if out.is_empty() { group.to_string() } else { out.join(" ") }
}

/// Wire this process's stream of `direction` to the port group `id` names, replacing whatever it
/// was connected to. Channels pair up in order; extra ones on either side stay unconnected.
pub(super) fn patch(direction: Direction, id: &str) -> Result<(), String> {
    let Some(group) = id.strip_prefix(ID) else { return Ok(()) };
    let client = client()?;
    let hardware: Vec<String> = hardware_ports(&client, direction).into_iter().filter(|p| group_of(p) == group).collect();
    if hardware.is_empty() {
        return Err(format!("'{group}' has no ports now"));
    }
    // cpal names its clients after the process (`cpal_client_<pid>_in` / `_out`).
    let (ours, flags) = match direction {
        Direction::Input => (format!("cpal_client_{}_in:", std::process::id()), PortFlags::IS_INPUT),
        Direction::Output => (format!("cpal_client_{}_out:", std::process::id()), PortFlags::IS_OUTPUT),
    };
    let mine: Vec<String> = client.ports(None, Some("audio"), flags).into_iter().filter(|p| p.starts_with(&ours)).collect();
    if mine.is_empty() {
        return Err("the stream's JACK ports were not found".into());
    }
    // Undo what the stream was wired to (asking a port for its connections only works for the
    // client that owns it, so every possible partner is tried; most are not connected).
    let partners = match direction {
        Direction::Input => client.ports(None, Some("audio"), PortFlags::IS_OUTPUT),
        Direction::Output => client.ports(None, Some("audio"), PortFlags::IS_INPUT),
    };
    for port in &mine {
        for other in &partners {
            let _ = match direction {
                Direction::Input => client.disconnect_ports_by_name(other, port),
                Direction::Output => client.disconnect_ports_by_name(port, other),
            };
        }
    }
    for (port, hw) in mine.iter().zip(hardware.iter()) {
        match direction {
            Direction::Input => client.connect_ports_by_name(hw, port),
            Direction::Output => client.connect_ports_by_name(port, hw),
        }
        .map_err(|e| format!("connecting {hw}: {e}"))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn groups_are_named_once_and_ports_split_at_the_last_colon() {
        assert_eq!(tidy_group("Scarlett 2i2 3rd Gen Direct Scarlett 2i2 USB"), "Scarlett 2i2 3rd Gen Direct USB");
        assert_eq!(tidy_group("system"), "system");
        assert_eq!(group_of("Built-in Audio: Analog Stereo:capture_FL"), "Built-in Audio: Analog Stereo");
        assert_eq!(group_of("odd"), "odd");
    }
}
