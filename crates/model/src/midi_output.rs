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
}
