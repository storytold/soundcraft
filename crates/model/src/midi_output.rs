//! The external MIDI output of a MIDI or instrument track, kept in the session's edit flags.

use crate::{EditState, TrackId};

/// Edit flags `midi.out.<track id>=<channel>:<device>` hold each track's MIDI output.
const PREFIX: &str = "midi.out.";

/// Where a MIDI or instrument track plays its clips outside SoundCraft: one output port and one
/// channel. Every event of the track goes out on that channel, whatever channel it was recorded
/// on. A track without one sends no MIDI to external gear.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MidiOutput<'a> {
    /// Output port label, as the MIDI output list shows it.
    pub device: &'a str,
    /// MIDI channel 0..=15.
    pub channel: u8,
}

impl<'a> MidiOutput<'a> {
    /// The output of `track`. Does not allocate (the MIDI output worker calls it).
    pub fn of(edit: &'a EditState, track: TrackId) -> Option<MidiOutput<'a>> {
        Self::all(edit).find_map(|(id, out)| (id == track).then_some(out))
    }

    /// Every track's output. Does not allocate.
    pub fn all(edit: &'a EditState) -> impl Iterator<Item = (TrackId, MidiOutput<'a>)> + 'a {
        edit.flags.range::<str, _>((std::ops::Bound::Included(PREFIX), std::ops::Bound::Unbounded)).take_while(|f| f.starts_with(PREFIX)).filter_map(
            |f| {
                let (id, value) = f.get(PREFIX.len()..)?.split_once('=')?;
                Some((TrackId(id.parse().ok()?), Self::parse(value).ok()?))
            },
        )
    }

    /// Reads `<channel 1-16>:<device>`, the form `track.midi_output` takes after `MIDI:`.
    /// Anything else is an error that names what is wrong.
    pub fn parse(value: &'a str) -> Result<MidiOutput<'a>, String> {
        let (channel, device) = value.split_once(':').ok_or_else(|| format!("`{value}` is not <channel 1-16>:<device>"))?;
        let channel = channel
            .parse::<u8>()
            .ok()
            .filter(|c| (1..=16).contains(c))
            .ok_or_else(|| format!("MIDI output channel `{channel}` is not 1-16 (an output names one channel)"))?;
        if device.trim().is_empty() || device == "*" {
            return Err(format!("MIDI output device `{device}` must name one port"));
        }
        Ok(MidiOutput { device, channel: channel - 1 })
    }

    /// The text `track.midi_output` takes and returns: `MIDI:<channel 1-16>:<device>`.
    pub fn route(&self) -> String {
        format!("MIDI:{}:{}", self.channel + 1, self.device)
    }

    /// Point `track` at `output`, or clear its output with `None`.
    pub fn store(edit: &mut EditState, track: TrackId, output: Option<MidiOutput<'_>>) {
        let key = format!("{PREFIX}{}=", track.0);
        edit.flags.retain(|f| !f.starts_with(&key));
        if let Some(out) = output {
            edit.flags.insert(format!("{key}{}:{}", out.channel + 1, out.device));
        }
    }

    /// Short text for the track's output selector.
    pub fn label(&self) -> String {
        format!("{} ch {}", self.device, self.channel + 1)
    }
}

/// Whether the MIDI input port named `input` (as the MIDI input list names it) and the output
/// port labelled `output` (as the MIDI output list labels it) are one device, so that what is
/// sent to the output can come straight back on the input (an IAC bus, a virtual port, a device
/// that echoes what it receives).
///
/// The names are compared, as nothing else ties an input to an output across systems: CoreMIDI
/// and WinMM give both directions of a port the same name (IAC Driver Bus 1, a teVirtualMIDI
/// port, a USB interface's DIN port). Also told apart as the lists do: ALSA's client prefix and
/// address (`Client:Port 28:0`), WinMM's `MIDIIN2 (Model 12)` / `MIDIOUT2 (Model 12)` for a
/// device's further ports, and the number a list adds to a repeated name (`Keys (2)`), which is
/// not the same on both lists: two devices with one name count as one. Any ALSA "Midi Through"
/// port is the same as any other. Does not allocate.
pub fn same_midi_device(input: &str, output: &str) -> bool {
    let (a, b) = (device_base(input), device_base(output));
    !a.is_empty() && a == b
}

/// The part of a port name that is the same for its input and its output.
fn device_base(name: &str) -> &str {
    let mut s = name.trim();
    // A number the port list added to a repeated name: "Keys (2)".
    if let Some((head, n)) = s.strip_suffix(')').and_then(|r| r.rsplit_once(" ("))
        && n.parse::<usize>().is_ok_and(|n| n >= 2)
    {
        s = head.trim_end();
    }
    // WinMM's further ports of a device: "MIDIIN2 (Model 12)", "MIDIOUT2 (Model 12)".
    for prefix in ["MIDIIN", "MIDIOUT"] {
        if let Some(rest) = s.strip_prefix(prefix)
            && let Some((n, inner)) = rest.split_once(" (")
            && n.bytes().all(|b| b.is_ascii_digit())
            && let Some(inner) = inner.strip_suffix(')')
        {
            s = inner.trim();
        }
    }
    // ALSA: "Client:Port name 28:0" without the address, then without a client name the port
    // repeats.
    if let Some((head, tail)) = s.rsplit_once(' ')
        && let Some((client, port)) = tail.split_once(':')
        && !client.is_empty()
        && !port.is_empty()
        && client.bytes().chain(port.bytes()).all(|b| b.is_ascii_digit())
    {
        s = head.trim_end();
    }
    if let Some((client, port)) = s.split_once(':')
        && !client.trim().is_empty()
        && port.trim_start().starts_with(client.trim())
    {
        s = port.trim();
    }
    if s.starts_with("Midi Through") { "Midi Through" } else { s }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn midi_output_round_trips_through_the_edit_flags_and_names_every_bad_value() {
        let mut edit = EditState::default();
        let keys = MidiOutput { device: "UM-ONE: port 1", channel: 9 };
        MidiOutput::store(&mut edit, TrackId(12), Some(keys));
        MidiOutput::store(&mut edit, TrackId(2), Some(MidiOutput { device: "Pad", channel: 0 }));
        assert_eq!(MidiOutput::of(&edit, TrackId(12)), Some(keys));
        // Track 1's flag is not track 12's, though one key is a prefix of the other.
        assert_eq!(MidiOutput::of(&edit, TrackId(1)), None);
        let mut ids: Vec<_> = MidiOutput::all(&edit).map(|(id, _)| id.0).collect();
        ids.sort_unstable();
        assert_eq!(ids, [2, 12]);
        MidiOutput::store(&mut edit, TrackId(12), None);
        assert_eq!(MidiOutput::of(&edit, TrackId(12)), None);
        assert_eq!(MidiOutput::parse(&keys.route()["MIDI:".len()..]), Ok(keys));
        for bad in ["0:Pad", "17:Pad", "*:Pad", "x:Pad", "1:", "1:*", "Pad", "1: "] {
            assert!(MidiOutput::parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn an_input_and_an_output_are_one_device_when_their_names_say_so() {
        // CoreMIDI, WinMM: one name both ways.
        assert!(same_midi_device("IAC Driver Bus 1", "IAC Driver Bus 1"));
        assert!(same_midi_device("SYN", "SYN"));
        // The numbers the lists add to a repeated name need not match.
        assert!(same_midi_device("Keys (2)", "Keys"));
        assert!(same_midi_device("Keys", "Keys (3)"));
        // WinMM's further ports of a device, and ALSA's client and address.
        assert!(same_midi_device("Model 12", "MIDIOUT2 (Model 12)"));
        assert!(same_midi_device("MIDIIN3 (Model 12)", "Model 12"));
        assert!(same_midi_device("UM-ONE MIDI 1", "UM-ONE:UM-ONE MIDI 1 20:0"));
        assert!(same_midi_device("Midi Through", "Midi Through:Midi Through Port-0 14:0"));
        // Different devices never are, nor the MIDI Keyboard window.
        assert!(!same_midi_device("KB", "SYN"));
        assert!(!same_midi_device("IAC Driver Bus 1", "IAC Driver Bus 2"));
        assert!(!same_midi_device("MIDI Keyboard window", "SYN"));
        assert!(!same_midi_device("", ""));
    }
}
