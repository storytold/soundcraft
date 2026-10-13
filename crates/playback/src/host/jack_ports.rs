//! Which hardware the JACK host's streams are wired to.
//!
//! JACK has no devices: cpal makes one client per stream and connects it to `system:capture_*` /
//! `system:playback_*`. With a JACK server that is the interface the server runs on. With
//! PipeWire's JACK library, `system` is whatever the desktop's default source and sink are, which
//! may well be a webcam microphone next to the speakers. So the Playback Engine dialog lists the
//! physical ports grouped by their client (one group per interface) and, when one is chosen, the
//! stream's ports are rewired to it after the stream opens. A rewiring that fails part-way is
//! undone, so the stream keeps the connections it had.

use super::{Direction, JACK_PORTS, Listed};
use jack::{Client, ClientOptions, PortFlags};

/// A connection: the port audio comes out of, then the port it goes into.
type Wire = (String, String);

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

/// The interfaces `ports` belong to, in the order they first appear, each with its number of
/// ports. A port the server lists twice counts once.
fn grouped(ports: &[String]) -> Vec<(&str, usize)> {
    let mut out: Vec<(&str, usize)> = Vec::new();
    for (n, port) in ports.iter().enumerate() {
        if ports.iter().take(n).any(|earlier| earlier == port) {
            continue;
        }
        let group = group_of(port);
        match out.iter_mut().find(|(g, _)| *g == group) {
            Some((_, count)) => *count += 1,
            None => out.push((group, 1)),
        }
    }
    out
}

/// One entry per interface: the groups of physical ports.
pub(super) fn groups(direction: Direction) -> Vec<Listed> {
    let Ok(client) = client() else { return Vec::new() };
    let ports = hardware_ports(&client, direction);
    grouped(&ports)
        .into_iter()
        .map(|(group, n)| Listed { id: format!("{JACK_PORTS}{group}"), name: tidy_group(group), detail: format!("{n} ch"), ranges: Vec::new() })
        .collect()
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

/// The connections that put the stream's ports (`mine`) on the ports of `group` among
/// `hardware`. Channels pair up in order; extra ones on either side stay unconnected.
fn plan(direction: Direction, mine: &[String], hardware: &[String], group: &str) -> Result<Vec<Wire>, String> {
    let mut of_group: Vec<&String> = Vec::new();
    for port in hardware.iter().filter(|p| group_of(p) == group) {
        if !of_group.contains(&port) {
            of_group.push(port);
        }
    }
    if of_group.is_empty() {
        return Err(format!("'{group}' has no ports now"));
    }
    if mine.is_empty() {
        return Err("the stream's JACK ports were not found".into());
    }
    Ok(mine.iter().zip(of_group).map(|(port, hw)| wire(direction, port, hw)).collect())
}

/// The connection between the stream's `port` and `other`, in the direction the audio flows.
fn wire(direction: Direction, port: &str, other: &str) -> Wire {
    match direction {
        Direction::Input => (other.to_string(), port.to_string()),
        Direction::Output => (port.to_string(), other.to_string()),
    }
}

/// What rewiring needs from the JACK server.
trait Patchbay {
    /// Connect `from` to `to`: `Ok(false)` when they already were connected.
    fn connect(&mut self, from: &str, to: &str) -> Result<bool, String>;
    /// Disconnect `from` from `to`: false when they were not connected (or it failed).
    fn disconnect(&mut self, from: &str, to: &str) -> bool;
}

impl Patchbay for Client {
    fn connect(&mut self, from: &str, to: &str) -> Result<bool, String> {
        match self.connect_ports_by_name(from, to) {
            Ok(()) => Ok(true),
            Err(jack::Error::PortAlreadyConnected(..)) => Ok(false),
            Err(e) => Err(e.to_string()),
        }
    }

    fn disconnect(&mut self, from: &str, to: &str) -> bool {
        self.disconnect_ports_by_name(from, to).is_ok()
    }
}

/// Replace the stream's connections with `wanted`. `possible` is every connection the stream's
/// ports could be in now: asking a port for its connections only works for the client that owns
/// it, so each one is tried, and those that were there are remembered. If a wanted connection
/// cannot be made (a port went away since it was listed), the ones made so far are taken down and
/// the remembered ones put back: the stream is never left without its ports, or half wired.
fn rewire(bay: &mut impl Patchbay, possible: &[Wire], wanted: &[Wire]) -> Result<(), String> {
    let mut removed: Vec<&Wire> = Vec::new();
    for w in possible.iter().filter(|w| !wanted.contains(w)) {
        if bay.disconnect(&w.0, &w.1) {
            removed.push(w);
        }
    }
    let mut made: Vec<&Wire> = Vec::new();
    for w in wanted {
        match bay.connect(&w.0, &w.1) {
            Ok(true) => made.push(w),
            // It was wired like this already: nothing to undo.
            Ok(false) => {}
            Err(e) => {
                for m in made {
                    bay.disconnect(&m.0, &m.1);
                }
                let lost: Vec<&str> = removed.into_iter().filter(|r| bay.connect(&r.0, &r.1).is_err()).map(|r| r.1.as_str()).collect();
                let restored = if lost.is_empty() { String::new() } else { format!(" (and could not reconnect {})", lost.join(", ")) };
                return Err(format!("connecting {} to {}: {e}{restored}", w.0, w.1));
            }
        }
    }
    Ok(())
}

/// Wire this process's stream of `direction` to the port group `id` names, replacing whatever it
/// was connected to; on failure the stream keeps what it was connected to.
pub(super) fn patch(direction: Direction, id: &str) -> Result<(), String> {
    let Some(group) = id.strip_prefix(JACK_PORTS) else { return Ok(()) };
    let mut client = client()?;
    // cpal names its clients after the process (`cpal_client_<pid>_in` / `_out`).
    let (ours, flags) = match direction {
        Direction::Input => (format!("cpal_client_{}_in:", std::process::id()), PortFlags::IS_INPUT),
        Direction::Output => (format!("cpal_client_{}_out:", std::process::id()), PortFlags::IS_OUTPUT),
    };
    let mine: Vec<String> = client.ports(None, Some("audio"), flags).into_iter().filter(|p| p.starts_with(&ours)).collect();
    let wanted = plan(direction, &mine, &hardware_ports(&client, direction), group)?;
    let partners = match direction {
        Direction::Input => client.ports(None, Some("audio"), PortFlags::IS_OUTPUT),
        Direction::Output => client.ports(None, Some("audio"), PortFlags::IS_INPUT),
    };
    let possible: Vec<Wire> = mine.iter().flat_map(|port| partners.iter().map(move |other| wire(direction, port, other))).collect();
    rewire(&mut client, &possible, &wanted)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(v: &[&str]) -> Vec<String> {
        v.iter().map(ToString::to_string).collect()
    }

    fn wires(v: &[(&str, &str)]) -> Vec<Wire> {
        v.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect()
    }

    #[test]
    fn groups_are_named_once_and_ports_split_at_the_last_colon() {
        assert_eq!(tidy_group("Scarlett 2i2 3rd Gen Direct Scarlett 2i2 USB"), "Scarlett 2i2 3rd Gen Direct USB");
        assert_eq!(tidy_group("system"), "system");
        assert_eq!(group_of("Built-in Audio: Analog Stereo:capture_FL"), "Built-in Audio: Analog Stereo");
        assert_eq!(group_of("odd"), "odd");
    }

    /// What PipeWire's JACK library lists on a laptop with a USB interface.
    const CAPTURE: [&str; 5] = [
        "Built-in Audio Analog Stereo:capture_FL",
        "Built-in Audio Analog Stereo:capture_FR",
        "Scarlett 2i2 USB:capture_FL",
        "Scarlett 2i2 USB:capture_FR",
        "Webcam: Mono:capture_MONO",
    ];

    #[test]
    fn physical_ports_are_grouped_by_interface() {
        assert_eq!(grouped(&names(&CAPTURE)), [("Built-in Audio Analog Stereo", 2), ("Scarlett 2i2 USB", 2), ("Webcam: Mono", 1)]);
        // A JACK server: one interface.
        assert_eq!(grouped(&names(&["system:playback_1", "system:playback_2"])), [("system", 2)]);
        // Groups keep the order of their first port; a port listed twice counts once.
        let mixed = names(&["b:1", "a:1", "b:2", "b:1", "a:1"]);
        assert_eq!(grouped(&mixed), [("b", 2), ("a", 1)]);
        assert!(grouped(&[]).is_empty());
    }

    #[test]
    fn channels_pair_up_in_order_and_extra_ones_stay_unconnected() {
        let hardware = names(&CAPTURE);
        let mine = names(&["cpal_client_1_in:in_0", "cpal_client_1_in:in_1"]);
        assert_eq!(
            plan(Direction::Input, &mine, &hardware, "Scarlett 2i2 USB").unwrap(),
            wires(&[("Scarlett 2i2 USB:capture_FL", "cpal_client_1_in:in_0"), ("Scarlett 2i2 USB:capture_FR", "cpal_client_1_in:in_1")])
        );
        // Fewer hardware ports than the stream has: the rest of the stream stays unconnected.
        assert_eq!(
            plan(Direction::Input, &mine, &hardware, "Webcam: Mono").unwrap(),
            wires(&[("Webcam: Mono:capture_MONO", "cpal_client_1_in:in_0")])
        );
        // More hardware ports than the stream has, and the audio flows the other way.
        let playback = names(&["system:playback_1", "system:playback_2", "system:playback_3", "system:playback_1"]);
        let out = names(&["cpal_client_1_out:out_0", "cpal_client_1_out:out_1"]);
        assert_eq!(
            plan(Direction::Output, &out, &playback, "system").unwrap(),
            wires(&[("cpal_client_1_out:out_0", "system:playback_1"), ("cpal_client_1_out:out_1", "system:playback_2")])
        );
        // The interface was unplugged since it was chosen, or the stream is gone.
        assert_eq!(plan(Direction::Input, &mine, &hardware, "Gone").unwrap_err(), "'Gone' has no ports now");
        assert!(plan(Direction::Input, &mine, &[], "Scarlett 2i2 USB").is_err());
        assert_eq!(plan(Direction::Input, &[], &hardware, "Scarlett 2i2 USB").unwrap_err(), "the stream's JACK ports were not found");
    }

    /// A JACK graph: what is connected, and ports that refuse connections (they went away).
    struct Graph {
        connected: Vec<Wire>,
        broken: Vec<&'static str>,
    }

    impl Graph {
        fn new(connected: &[(&str, &str)], broken: &[&'static str]) -> Graph {
            Graph { connected: wires(connected), broken: broken.to_vec() }
        }

        fn sorted(mut self) -> Vec<Wire> {
            self.connected.sort();
            self.connected
        }
    }

    impl Patchbay for Graph {
        fn connect(&mut self, from: &str, to: &str) -> Result<bool, String> {
            if self.broken.contains(&from) || self.broken.contains(&to) {
                return Err("no such port".into());
            }
            let w = (from.to_string(), to.to_string());
            if self.connected.contains(&w) {
                return Ok(false);
            }
            self.connected.push(w);
            Ok(true)
        }

        fn disconnect(&mut self, from: &str, to: &str) -> bool {
            let before = self.connected.len();
            self.connected.retain(|w| !(w.0 == from && w.1 == to));
            self.connected.len() != before
        }
    }

    /// An output stream on the system ports, to be moved to the two ports of `usb`.
    const ON_SYSTEM: [(&str, &str); 2] = [("me:out_0", "system:playback_1"), ("me:out_1", "system:playback_2")];
    const TO_USB: [(&str, &str); 2] = [("me:out_0", "usb:playback_1"), ("me:out_1", "usb:playback_2")];

    /// Every connection the stream's two ports could be in.
    fn possible() -> Vec<Wire> {
        let sinks = ["system:playback_1", "system:playback_2", "usb:playback_1", "usb:playback_2"];
        ["me:out_0", "me:out_1"].iter().flat_map(|port| sinks.iter().map(move |sink| (port.to_string(), sink.to_string()))).collect()
    }

    #[test]
    fn rewiring_replaces_the_streams_connections_and_leaves_the_others() {
        let other = ("browser:out_0", "system:playback_1");
        let mut graph = Graph::new(&[ON_SYSTEM[0], ON_SYSTEM[1], other], &[]);
        rewire(&mut graph, &possible(), &wires(&TO_USB)).unwrap();
        assert_eq!(graph.sorted(), wires(&[other, TO_USB[0], TO_USB[1]]));
        // Already where it should be: nothing changes.
        let mut graph = Graph::new(&TO_USB, &[]);
        rewire(&mut graph, &possible(), &wires(&TO_USB)).unwrap();
        assert_eq!(graph.sorted(), wires(&TO_USB));
        // Half there already (one channel was wired by hand).
        let mut graph = Graph::new(&[TO_USB[0], ON_SYSTEM[1]], &[]);
        rewire(&mut graph, &possible(), &wires(&TO_USB)).unwrap();
        assert_eq!(graph.sorted(), wires(&TO_USB));
    }

    #[test]
    fn a_failure_on_the_first_channel_keeps_the_old_connections() {
        let mut graph = Graph::new(&ON_SYSTEM, &["usb:playback_1"]);
        let e = rewire(&mut graph, &possible(), &wires(&TO_USB)).unwrap_err();
        assert_eq!(e, "connecting me:out_0 to usb:playback_1: no such port");
        assert_eq!(graph.sorted(), wires(&ON_SYSTEM));
    }

    #[test]
    fn a_failure_on_a_later_channel_undoes_the_ones_made_and_keeps_the_old_connections() {
        let mut graph = Graph::new(&ON_SYSTEM, &["usb:playback_2"]);
        let e = rewire(&mut graph, &possible(), &wires(&TO_USB)).unwrap_err();
        assert_eq!(e, "connecting me:out_1 to usb:playback_2: no such port");
        assert_eq!(graph.sorted(), wires(&ON_SYSTEM), "not half on the new interface");
        // A wanted connection that was there before the rewiring stays through the undo.
        let mut graph = Graph::new(&[TO_USB[0], ON_SYSTEM[1]], &["usb:playback_2"]);
        assert!(rewire(&mut graph, &possible(), &wires(&TO_USB)).is_err());
        assert_eq!(graph.sorted(), wires(&[TO_USB[0], ON_SYSTEM[1]]));
    }

    #[test]
    fn an_old_connection_that_cannot_be_put_back_is_named() {
        // Both the new port and one of the old ones went away while rewiring.
        struct Unplugged(Graph);
        impl Patchbay for Unplugged {
            fn connect(&mut self, from: &str, to: &str) -> Result<bool, String> {
                self.0.connect(from, to)
            }
            fn disconnect(&mut self, from: &str, to: &str) -> bool {
                let was = self.0.disconnect(from, to);
                if to == "system:playback_2" {
                    self.0.broken.push("system:playback_2");
                }
                was
            }
        }
        let mut graph = Unplugged(Graph::new(&ON_SYSTEM, &["usb:playback_1"]));
        let e = rewire(&mut graph, &possible(), &wires(&TO_USB)).unwrap_err();
        assert_eq!(e, "connecting me:out_0 to usb:playback_1: no such port (and could not reconnect system:playback_2)");
        assert_eq!(graph.0.sorted(), wires(&[ON_SYSTEM[0]]));
    }
}
