//! The MIDI input of a MIDI or instrument track, kept in its input route.

use crate::Route;

/// The MIDI input a MIDI or instrument track listens to. It lives in the track's input route as
/// `Route::Hardware("MIDI:<channel>:<device>")`, with `*` for any channel (else 1–16) and for any
/// device; every other route means all devices, all channels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct MidiInput<'a> {
    /// Device (input port) name; `None`: all enabled devices.
    pub device: Option<&'a str>,
    /// MIDI channel 0..=15; `None`: all channels.
    pub channel: Option<u8>,
}

impl<'a> MidiInput<'a> {
    /// Reads a track's input route. Does not allocate (the audio thread calls it).
    pub fn of(route: &'a Route) -> MidiInput<'a> {
        let Route::Hardware(h) = route else { return MidiInput::default() };
        let Some((ch, dev)) = h.strip_prefix("MIDI:").and_then(|r| r.split_once(':')) else { return MidiInput::default() };
        MidiInput {
            device: Some(dev).filter(|d| *d != "*" && !d.is_empty()),
            channel: ch.parse::<u8>().ok().filter(|c| (1..=16).contains(c)).map(|c| c - 1),
        }
    }

    /// The input route that stores this choice.
    pub fn route(&self) -> Route {
        if self.device.is_none() && self.channel.is_none() {
            return Route::None;
        }
        let ch = self.channel.filter(|c| *c < 16).map_or_else(|| "*".to_string(), |c| (c + 1).to_string());
        Route::Hardware(format!("MIDI:{ch}:{}", self.device.unwrap_or("*")))
    }

    /// Whether an event on `channel` (0..=15) from `device` reaches the track.
    pub fn accepts(&self, device: Option<&str>, channel: u8) -> bool {
        self.channel.is_none_or(|c| c == channel) && self.device.is_none_or(|d| device == Some(d))
    }

    /// Short text for the track's input selector.
    pub fn label(&self) -> String {
        match (self.device, self.channel) {
            (None, None) => "All".to_string(),
            (None, Some(c)) => format!("All ch {}", c + 1),
            (Some(d), None) => d.to_string(),
            (Some(d), Some(c)) => format!("{d} ch {}", c + 1),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn midi_input_round_trips_through_the_input_route() {
        for (device, channel) in [(None, None), (Some("Keys: with:colons"), None), (None, Some(9)), (Some("Pad"), Some(0))] {
            let m = MidiInput { device, channel };
            let r = m.route();
            assert_eq!(MidiInput::of(&r), m, "{r:?}");
        }
        assert_eq!(MidiInput { device: None, channel: None }.route(), Route::None);
        // Audio-style and broken routes mean everything.
        for h in ["In 1-2", "MIDI:", "MIDI:17:*", "MIDI:0:*", "MIDI:x"] {
            assert_eq!(MidiInput::of(&Route::Hardware(h.into())).channel, None, "{h}");
        }
        let m = MidiInput { device: Some("Pad"), channel: Some(2) };
        assert!(m.accepts(Some("Pad"), 2));
        assert!(!m.accepts(Some("Pad"), 3) && !m.accepts(Some("Keys"), 2) && !m.accepts(None, 2));
        assert!(MidiInput::default().accepts(None, 7));
        assert_eq!((m.label(), MidiInput::default().label()), ("Pad ch 3".to_string(), "All".to_string()));
    }
}
