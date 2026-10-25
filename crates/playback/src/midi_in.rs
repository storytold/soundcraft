//! MIDI input devices: keyboards, pad and fader controllers connected to the computer.
//!
//! [`MidiInputs`] opens every enabled input port (WinMM on Windows, CoreMIDI on macOS, the ALSA
//! sequencer on Linux, through `midir`) and hands each channel message to the player through a
//! [`MidiSink`]. The audio callback plays them at the next block on the instruments of the tracks
//! taking MIDI input: record-armed or input-monitored instrument and MIDI tracks, or the selected
//! one when none is (MIDI Thru; see `soundcraft_mix::takes_midi_input`).
//!
//! The device callbacks run on threads of the system's MIDI service, never on the audio thread;
//! they lock a mutex only to read where to send. Elsewhere (the browser, FreeBSD) there are no
//! ports and every call is a no-op.

use crate::{MidiSink, Player};
#[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

/// The client name MIDI services show for SoundCraft.
#[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
const CLIENT: &str = "SoundCraft";

/// Where the device callbacks send: the current player, replaced when the audio device reopens.
type Route = Arc<Mutex<Option<MidiSink>>>;

/// The last messages any port delivered (see [`MidiInputs::recent`]).
type Recent = Arc<Mutex<std::collections::VecDeque<[u8; 3]>>>;

/// How many recent messages are kept.
const RECENT: usize = 12;

/// A MIDI input port that failed to open, and why (shown in Setup › MIDI › MIDI Input Devices).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortError {
    pub port: String,
    pub error: String,
}

/// The open MIDI input ports, feeding one player.
pub struct MidiInputs {
    route: Route,
    /// The last messages received, newest last (Setup › MIDI › MIDI Input Devices shows them).
    recent: Recent,
    #[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
    open: Vec<(String, midir::MidiInputConnection<()>, Arc<AtomicU64>)>,
    failed: Vec<PortError>,
}

impl Default for MidiInputs {
    fn default() -> Self {
        Self::new()
    }
}

impl MidiInputs {
    /// No port open, nowhere to send yet.
    pub fn new() -> Self {
        MidiInputs {
            route: Arc::new(Mutex::new(None)),
            recent: Arc::new(Mutex::new(std::collections::VecDeque::with_capacity(RECENT))),
            #[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
            open: Vec::new(),
            failed: Vec::new(),
        }
    }

    /// Send what is played to `player` from now on (after the audio device reopened). Cheap when
    /// it already does: call it as often as convenient.
    pub fn route_to(&mut self, player: &Player) {
        let mut r = self.route.lock().unwrap_or_else(PoisonError::into_inner);
        if r.as_ref().is_none_or(|s| s.player() != player.id()) {
            *r = Some(player.midi_sink());
        }
    }

    /// Stop sending anywhere (messages are dropped until [`MidiInputs::route_to`]).
    pub fn unroute(&mut self) {
        *self.route.lock().unwrap_or_else(PoisonError::into_inner) = None;
    }

    /// Bring the open ports in line with the system and the user's choice: open every present port
    /// not in `disabled`, close the ones that are disabled or went away. A port that failed to open
    /// is tried again only with `retry` (the Refresh button), so a port another program holds is
    /// not reopened every few seconds.
    pub fn sync(&mut self, disabled: &[String], retry: bool) {
        if retry {
            self.failed.clear();
        }
        self.sync_ports(disabled);
    }

    #[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
    fn sync_ports(&mut self, disabled: &[String]) {
        let Ok(probe) = midir::MidiInput::new(CLIENT) else {
            self.failed = vec![PortError { port: "MIDI".into(), error: "the system MIDI service is unavailable".into() }];
            return;
        };
        let ports = named_ports(&probe);
        // Dropping a connection closes the port.
        self.open.retain(|(name, _, _)| !disabled.contains(name) && ports.iter().any(|(n, _)| n == name));
        self.failed.retain(|f| ports.iter().any(|(n, _)| *n == f.port));
        for (name, id) in ports {
            if disabled.contains(&name) || self.open.iter().any(|(n, _, _)| *n == name) || self.failed.iter().any(|f| f.port == name) {
                continue;
            }
            let heard = Arc::new(AtomicU64::new(0));
            match connect(&id, Arc::clone(&self.route), Arc::clone(&heard), Arc::clone(&self.recent)) {
                Ok(conn) => {
                    log::info!("MIDI input open: {name}");
                    self.open.push((name, conn, heard));
                }
                Err(error) => {
                    log::warn!("MIDI input {name}: {error}");
                    self.failed.push(PortError { port: name, error });
                }
            }
        }
    }

    #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
    fn sync_ports(&mut self, _disabled: &[String]) {}

    /// Close every port.
    pub fn close_all(&mut self) {
        #[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
        self.open.clear();
        self.failed.clear();
    }

    /// The ports open now, by name.
    pub fn open_ports(&self) -> Vec<String> {
        #[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
        return self.open.iter().map(|(n, _, _)| n.clone()).collect();
        #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
        Vec::new()
    }

    /// The last channel messages received from any port, oldest first.
    pub fn recent(&self) -> Vec<[u8; 3]> {
        self.recent.lock().unwrap_or_else(PoisonError::into_inner).iter().copied().collect()
    }

    /// Each open port and how many channel messages it has delivered so far (to show activity).
    pub fn activity(&self) -> Vec<(String, u64)> {
        #[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
        return self.open.iter().map(|(n, _, h)| (n.clone(), h.load(Ordering::Relaxed))).collect();
        #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
        Vec::new()
    }

    /// The ports that failed to open, and why.
    pub fn failed_ports(&self) -> &[PortError] {
        &self.failed
    }
}

/// The MIDI input ports present now, by name (a repeated name gets " #2", " #3"…, as two
/// identical keyboards would). Empty where SoundCraft has no MIDI device layer.
pub fn input_ports() -> Vec<String> {
    #[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
    {
        midir::MidiInput::new(CLIENT).map(|m| named_ports(&m).into_iter().map(|(n, _)| n).collect()).unwrap_or_default()
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
    Vec::new()
}

/// Each port's display name (made unique) and its id.
#[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
fn named_ports(m: &midir::MidiInput) -> Vec<(String, String)> {
    let raw: Vec<(String, String)> = m.ports().iter().take(256).map(|p| (m.port_name(p).unwrap_or_else(|_| "MIDI input".into()), p.id())).collect();
    unique_names(raw)
}

#[cfg(any(test, target_os = "windows", target_os = "macos", target_os = "linux"))]
fn unique_names(raw: Vec<(String, String)>) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::with_capacity(raw.len());
    for (name, id) in raw {
        let name = name.trim().to_string();
        let mut shown = name.clone();
        let mut k = 2;
        while out.iter().any(|(n, _)| *n == shown) {
            shown = format!("{name} #{k}");
            k += 1;
        }
        out.push((shown, id));
    }
    out
}

/// Open the port with `id` (found again by id: a port belongs to the MIDI client that listed it).
#[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
fn connect(id: &str, route: Route, heard: Arc<AtomicU64>, recent: Recent) -> Result<midir::MidiInputConnection<()>, String> {
    let mut input = midir::MidiInput::new(CLIENT).map_err(|e| e.to_string())?;
    // System exclusive, MIDI clock and active sensing are not played.
    input.ignore(midir::Ignore::All);
    let port = input.find_port_by_id(id).ok_or("the device was disconnected")?;
    let mut running = 0u8;
    input
        .connect(
            &port,
            "SoundCraft MIDI In",
            move |_stamp, bytes, _| {
                let sink = route.lock().unwrap_or_else(PoisonError::into_inner);
                parse(bytes, &mut running, |m| {
                    heard.fetch_add(1, Ordering::Relaxed);
                    let mut r = recent.lock().unwrap_or_else(PoisonError::into_inner);
                    if r.len() >= RECENT {
                        r.pop_front();
                    }
                    r.push_back(m);
                    if let Some(sink) = sink.as_ref() {
                        sink.send(m);
                    }
                });
            },
            (),
        )
        .map_err(|e| {
            // WinMM lets one program at a time open a port.
            if cfg!(target_os = "windows") { format!("{e} (is another program using it?)") } else { e.to_string() }
        })
}

/// A channel message in words, for people reading what a device sends ("CC 1 Modulation = 64").
pub fn describe(msg: [u8; 3]) -> String {
    const NAMES: [&str; 12] = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];
    let [status, d1, d2] = msg;
    let ch = (status & 0x0F) + 1;
    let note = |n: u8| format!("{}{}", NAMES.get(usize::from(n % 12)).copied().unwrap_or("?"), i32::from(n / 12) - 1);
    let what = match status & 0xF0 {
        0x90 if d2 > 0 => format!("Note on {} velocity {d2}", note(d1)),
        0x80 | 0x90 => format!("Note off {}", note(d1)),
        0xA0 => format!("Key pressure {} = {d2}", note(d1)),
        0xB0 => {
            let name = match d1 {
                0 => " Bank Select",
                1 => " Modulation",
                5 => " Portamento Time",
                6 => " Data Entry",
                7 => " Volume",
                10 => " Pan",
                11 => " Expression",
                32 => " Bank Select LSB",
                64 => " Sustain",
                65 => " Portamento",
                71 => " Resonance",
                72 => " Release",
                73 => " Attack",
                74 => " Brightness",
                91 => " Reverb",
                93 => " Chorus",
                100 | 101 => " RPN",
                120 => " All Sound Off",
                121 => " Reset Controllers",
                123 => " All Notes Off",
                _ => "",
            };
            format!("CC {d1}{name} = {d2}")
        }
        0xC0 => format!("Program {}", u16::from(d1) + 1),
        0xD0 => format!("Pressure {d1}"),
        0xE0 => format!("Pitch bend {:+}", (i32::from(d2) << 7 | i32::from(d1)) - 8192),
        _ => "System message".into(),
    };
    format!("ch {ch} · {what}")
}

/// Split raw MIDI bytes into channel messages (`[status, data1, data2]`, `data2` 0 for program
/// change and channel pressure), with running status; system messages are skipped. `running`
/// carries the running status from one call to the next.
pub fn parse(bytes: &[u8], running: &mut u8, mut out: impl FnMut([u8; 3])) {
    let mut i = 0;
    while let Some(&b) = bytes.get(i) {
        if b >= 0xF8 {
            // Realtime (clock, start, stop…): one byte, may sit anywhere, keeps running status.
            i += 1;
            continue;
        }
        if b >= 0xF0 {
            // System common and exclusive: skip to the next status byte; clears running status.
            *running = 0;
            i += 1;
            while bytes.get(i).is_some_and(|&x| x < 0x80) {
                i += 1;
            }
            if bytes.get(i) == Some(&0xF7) {
                i += 1;
            }
            continue;
        }
        let status = if b >= 0x80 {
            i += 1;
            *running = b;
            b
        } else if *running != 0 {
            *running
        } else {
            // Data without a status: ignore it.
            i += 1;
            continue;
        };
        let two = !matches!(status & 0xF0, 0xC0 | 0xD0);
        let data = |k: usize| bytes.get(k).copied().filter(|x| *x < 0x80);
        // A truncated message is dropped; a status byte that cut it short is read next.
        let Some(d1) = data(i) else { continue };
        i += 1;
        let d2 = if two {
            let Some(d2) = data(i) else { continue };
            i += 1;
            d2
        } else {
            0
        };
        out([status, d1, d2]);
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn all(bytes: &[u8]) -> Vec<[u8; 3]> {
        let mut running = 0;
        let mut v = Vec::new();
        parse(bytes, &mut running, |m| v.push(m));
        v
    }

    #[test]
    fn parses_channel_messages_with_running_status() {
        assert_eq!(all(&[0x90, 60, 100]), vec![[0x90, 60, 100]]);
        // Running status: two more notes without repeating 0x90; a clock byte in between.
        assert_eq!(all(&[0x90, 60, 100, 62, 90, 0xF8, 64, 0]), vec![[0x90, 60, 100], [0x90, 62, 90], [0x90, 64, 0]]);
        // Two-byte messages.
        assert_eq!(all(&[0xC1, 5, 0xD1, 40]), vec![[0xC1, 5, 0], [0xD1, 40, 0]]);
        // Sysex skipped, then a pitch bend.
        assert_eq!(all(&[0xF0, 1, 2, 3, 0xF7, 0xE0, 0, 64]), vec![[0xE0, 0, 64]]);
    }

    #[test]
    fn hostile_bytes_never_panic() {
        assert!(all(&[]).is_empty());
        assert!(all(&[0x90]).is_empty());
        assert!(all(&[0x90, 60]).is_empty());
        assert!(all(&[60, 100, 0xF0, 0x90]).is_empty());
        // Running status persists across calls (one message per callback on some systems).
        let mut running = 0;
        let mut v = Vec::new();
        parse(&[0xB0, 64, 127], &mut running, |m| v.push(m));
        parse(&[64, 0], &mut running, |m| v.push(m));
        assert_eq!(v, vec![[0xB0, 64, 127], [0xB0, 64, 0]]);
    }

    #[test]
    fn messages_read_in_words() {
        assert_eq!(describe([0x90, 60, 100]), "ch 1 · Note on C4 velocity 100");
        assert_eq!(describe([0x91, 61, 0]), "ch 2 · Note off C#4");
        assert_eq!(describe([0xB0, 1, 64]), "ch 1 · CC 1 Modulation = 64");
        assert_eq!(describe([0xB0, 16, 5]), "ch 1 · CC 16 = 5");
        assert_eq!(describe([0xE0, 0, 0x40]), "ch 1 · Pitch bend +0");
        assert_eq!(describe([0xC3, 4, 0]), "ch 4 · Program 5");
    }

    #[test]
    fn repeated_port_names_are_numbered() {
        let named = unique_names(vec![("Keys".into(), "a".into()), ("Pads".into(), "b".into()), ("Keys".into(), "c".into())]);
        let names: Vec<&str> = named.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, vec!["Keys", "Pads", "Keys #2"]);
    }

    #[test]
    fn messages_reach_the_routed_player() {
        let s = Arc::new(soundcraft_model::Session::default());
        let player = Player::new(Arc::clone(&s));
        let mut inputs = MidiInputs::new();
        inputs.route_to(&player);
        let sink = inputs.route.lock().unwrap().clone().unwrap();
        assert_eq!(sink.player(), player.id());
        assert!(sink.send([0x90, 60, 100]));
        // A new player gets a new route.
        let other = Player::new(s);
        inputs.route_to(&other);
        assert_eq!(inputs.route.lock().unwrap().as_ref().unwrap().player(), other.id());
        inputs.close_all();
        assert!(inputs.open_ports().is_empty());
    }
}
