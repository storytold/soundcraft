//! MIDI Thru to external gear: live input goes out on the MIDI output of the tracks that play it.
//!
//! A message from an input port reaches every active MIDI or instrument track that plays live
//! input ([`soundcraft_mix::midi_thru`]: Options › MIDI Thru on, record-enabled or
//! input-monitored), whose MIDI input selector takes its port and channel, that has a MIDI output
//! and is not muted there ([`midi_output_muted`]: a MIDI track's mute and solo; an instrument
//! track's mute is for its audio). It goes out on the track's output port and channel, whatever
//! channel it came in on (Pro Tools Reference Guide p.810: "routes MIDI from your controllers to
//! the device and channels assigned to the currently record-enabled MIDI track"). No MIDI Track
//! Offset and no delay compensation apply (p.810). One message reaches a port and channel once,
//! however many tracks route it there.
//!
//! [`Thru`] keeps the notes it started on each destination, with the input port, channel and key
//! that hold them (the *owner*), and the sustain pedals it put down. A key's release ends its
//! note where its note-on went. A destination hears a Note-Off when the last owner of the key
//! there lets go, and not while the clips also sound that key there (the port's clip encoder ends
//! it then; the MIDI output worker keeps the two apart, see `sync_output.rs`). Notes and pedals are let go of by Thru itself when their owner no longer
//! reaches the destination ([`Thru::follow`]: the track was taken out of record, muted, routed
//! elsewhere, its input changed, MIDI Thru switched off), when their input port lost events
//! ([`Thru::release_ports`]), when their output port loses its last route and closes, or is
//! closed as no longer listed or bound to another port now ([`Thru::release_dest`]), and on Stop and All MIDI Notes Off ([`Thru::release_all`]); a key
//! released after that sends nothing more.
//!
//! Nothing here allocates per message once the tables have grown; it runs on the MIDI output
//! worker, which owns the ports.
use crate::Session;
use soundcraft_mix::{LiveMidi, MidiPorts, midi_output_muted, midi_port_bit, midi_thru};
use soundcraft_model::{MidiInput, MidiOutput};

/// A note Thru started on a destination.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Held {
    /// Input port index ([`MidiPorts`]) and channel the key came from.
    port: u8,
    channel: u8,
    key: u8,
    /// Output port label and channel it went to.
    dest: String,
    out: u8,
}

/// A sustain pedal Thru put down on a destination, and who put it there.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Pedal {
    port: u8,
    channel: u8,
    dest: String,
    out: u8,
}

#[derive(Default)]
pub(crate) struct Thru {
    held: Vec<Held>,
    pedals: Vec<Pedal>,
    /// The channels (bit `n` for channel `n`) each port got a note or a pedal on since it was
    /// last opened, released or not: a release whose send failed (a Windows output handle
    /// broken by a device being added or removed) never reached the device.
    written: Vec<(String, u16)>,
    /// Scratch: the destinations of the message being routed.
    targets: Vec<(String, u8)>,
}

/// Every destination (port label and channel) that input from `port` on `channel` reaches now.
fn destinations(s: &Session, names: &MidiPorts, at: i64, port: u8, channel: u8, out: &mut Vec<(String, u8)>) {
    out.clear();
    let name = names.name(port);
    for (id, o) in MidiOutput::all(&s.edit) {
        let Some(t) = s.track(id).filter(|t| t.kind.is_midi() && midi_thru(s, t)) else { continue };
        if midi_output_muted(s, t, at) || !MidiInput::of(&t.mixer.input).accepts(name, channel) {
            continue;
        }
        if !out.iter().any(|(d, c)| d == o.device && *c == o.channel) {
            out.push((o.device.to_owned(), o.channel));
        }
    }
}

/// Whether input from `port` on `channel` reaches `dest` on `out` now.
fn reaches(s: &Session, names: &MidiPorts, at: i64, port: u8, channel: u8, dest: &str, out: u8) -> bool {
    let name = names.name(port);
    MidiOutput::all(&s.edit).any(|(id, o)| {
        o.device == dest
            && o.channel == out
            && s.track(id).is_some_and(|t| {
                t.kind.is_midi() && midi_thru(s, t) && !midi_output_muted(s, t, at) && MidiInput::of(&t.mixer.input).accepts(name, channel)
            })
    })
}

impl Thru {
    /// Send `e`, arrived from an input, to its destinations: `send(port label, bytes)`. `at` is
    /// the play position (for mute automation).
    pub(crate) fn route(&mut self, s: &Session, names: &MidiPorts, at: i64, e: &LiveMidi, send: &mut dyn FnMut(&str, &[u8])) {
        let (kind, channel) = (e.status & 0xF0, e.status & 0x0F);
        let (data1, data2) = (e.data1 & 0x7F, e.data2 & 0x7F);
        if kind == 0x80 || (kind == 0x90 && data2 == 0) {
            // A release ends the note where it was started, wherever the track points now.
            self.release_where(|h| h.port == e.port && h.channel == channel && h.key == data1, data2, send);
            return;
        }
        let mut targets = std::mem::take(&mut self.targets);
        destinations(s, names, at, e.port, channel, &mut targets);
        for (dest, out) in &targets {
            let status = kind | out;
            if kind == 0x90 || (kind == 0xB0 && data1 == 64) {
                self.wrote(dest, *out);
            }
            match kind {
                0x90 => {
                    let owner = Held { port: e.port, channel, key: data1, dest: dest.clone(), out: *out };
                    if !self.held.contains(&owner) {
                        self.held.push(owner);
                    }
                    send(dest, &[status, data1, data2]);
                }
                0xB0 if data1 == 64 => {
                    let pedal = Pedal { port: e.port, channel, dest: dest.clone(), out: *out };
                    if data2 >= 64 {
                        if !self.pedals.contains(&pedal) {
                            self.pedals.push(pedal);
                        }
                    } else {
                        self.pedals.retain(|p| *p != pedal);
                        // Another input still holds the pedal there: it goes up with the last.
                        if self.pedals.iter().any(|p| p.dest == *dest && p.out == *out) {
                            continue;
                        }
                    }
                    send(dest, &[status, 64, data2]);
                }
                0xC0 | 0xD0 => send(dest, &[status, data1]),
                _ => send(dest, &[status, data1, data2]),
            }
        }
        self.targets = targets;
    }

    /// Let go of the notes and pedals whose owner no longer reaches their destination: after a
    /// change of the session (record-enable, input monitor, input, output, mute, Options › MIDI
    /// Thru) or of the play position (mute automation).
    pub(crate) fn follow(&mut self, s: &Session, names: &MidiPorts, at: i64, send: &mut dyn FnMut(&str, &[u8])) {
        if self.held.is_empty() && self.pedals.is_empty() {
            return;
        }
        self.release_where(|h| !reaches(s, names, at, h.port, h.channel, &h.dest, h.out), 0, send);
        self.lift_where(|p| !reaches(s, names, at, p.port, p.channel, &p.dest, p.out), send);
    }

    /// The input ports in `lost` ([`midi_port_bit`]) lost events or closed: their keys' releases
    /// may never come.
    pub(crate) fn release_ports(&mut self, lost: u64, send: &mut dyn FnMut(&str, &[u8])) {
        self.release_where(|h| lost & midi_port_bit(h.port) != 0, 0, send);
        self.lift_where(|p| lost & midi_port_bit(p.port) != 0, send);
    }

    /// Every note off, then every pedal up (Stop, All MIDI Notes Off).
    pub(crate) fn release_all(&mut self, send: &mut dyn FnMut(&str, &[u8])) {
        self.release_where(|_| true, 0, send);
        self.lift_where(|_| true, send);
    }

    /// Every note off and pedal up on `dest`, whose port closes: it lost its last route, is no
    /// longer listed, or its label is bound to another port now.
    pub(crate) fn release_dest(&mut self, dest: &str, send: &mut dyn FnMut(&str, &[u8])) {
        self.release_where(|h| h.dest == dest, 0, send);
        self.lift_where(|p| p.dest == dest, send);
    }

    /// Note that a note or pedal went to `dest` on channel `out`.
    fn wrote(&mut self, dest: &str, out: u8) {
        match self.written.iter_mut().find(|(d, _)| d == dest) {
            Some((_, channels)) => *channels |= 1 << out,
            None => self.written.push((dest.to_owned(), 1 << out)),
        }
    }

    /// The quieting of `channels` on `dest` ([`Self::reopened`]) did not all go out: the port
    /// failed again. They are quieted when it next reopens.
    pub(crate) fn unquieted(&mut self, dest: &str, channels: u16) {
        match self.written.iter_mut().find(|(d, _)| d == dest) {
            Some((_, written)) => *written |= channels,
            None if channels != 0 => self.written.push((dest.to_owned(), channels)),
            None => {}
        }
    }

    /// The port `dest` was reopened: the device may hold notes and a pedal Thru can no longer
    /// end one by one, and releases sent while the port was failing may never have arrived.
    /// Forget the notes, and return the channels (bit `n` for channel `n`) Thru played on since
    /// the port was last opened, to quiet with the pedal up and All Notes Off.
    pub(crate) fn reopened(&mut self, dest: &str) -> u16 {
        let mut channels = 0u16;
        if let Some(i) = self.written.iter().position(|(d, _)| d == dest) {
            channels |= self.written.swap_remove(i).1;
        }
        self.held.retain(|h| {
            let here = h.dest == dest;
            channels |= u16::from(here) << h.out;
            !here
        });
        self.pedals.retain(|p| {
            let here = p.dest == dest;
            channels |= u16::from(here) << p.out;
            !here
        });
        channels
    }

    /// Whether Thru holds a note or a pedal anywhere.
    pub(crate) fn holds(&self) -> bool {
        !self.held.is_empty() || !self.pedals.is_empty()
    }

    /// End the notes `gone` picks. A destination hears the Note-Off (with `velocity`) when no
    /// other owner holds the key there.
    fn release_where(&mut self, gone: impl Fn(&Held) -> bool, velocity: u8, send: &mut dyn FnMut(&str, &[u8])) {
        let mut i = 0;
        while let Some(h) = self.held.get(i) {
            if !gone(h) {
                i += 1;
                continue;
            }
            let h = self.held.swap_remove(i);
            if !self.held.iter().any(|o| o.dest == h.dest && o.out == h.out && o.key == h.key) {
                send(&h.dest, &[0x80 | h.out, h.key, velocity]);
            }
        }
    }

    /// Lift the pedals `gone` picks, where no other owner holds the pedal down.
    fn lift_where(&mut self, gone: impl Fn(&Pedal) -> bool, send: &mut dyn FnMut(&str, &[u8])) {
        let mut i = 0;
        while let Some(p) = self.pedals.get(i) {
            if !gone(p) {
                i += 1;
                continue;
            }
            let p = self.pedals.swap_remove(i);
            if !self.pedals.iter().any(|o| o.dest == p.dest && o.out == p.out) {
                send(&p.dest, &[0xB0 | p.out, 64, 0]);
            }
        }
    }
}

#[cfg(test)]
#[path = "thru_tests.rs"]
mod tests;
