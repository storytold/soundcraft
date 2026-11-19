//! Mackie Control messages for the shared MIDI control surface.
use super::{Event, Navigation, STRIPS};

const STOP: u8 = 0x5d;
const PLAY: u8 = 0x5e;

pub(super) fn decode(packet: [u8; 3]) -> Option<Event> {
    match packet {
        [0x90, PLAY, 1..=127] => Some(Event::Play),
        [0x90, STOP, 1..=127] => Some(Event::Stop),
        [0x90, 0x2e, 1..=127] => Some(Event::Navigate(Navigation::BankLeft)),
        [0x90, 0x2f, 1..=127] => Some(Event::Navigate(Navigation::BankRight)),
        [channel @ 0xe0..=0xe7, low @ 0..=127, high @ 0..=127] => {
            Some(Event::Fader(usize::from(channel - 0xe0), u16::from(low) | (u16::from(high) << 7)))
        }
        _ => None,
    }
}

pub(super) fn lamp(strip: usize, index: usize, lit: bool) -> [u8; 3] {
    let note = if strip == STRIPS { [STOP, PLAY, 0x5f][index] } else { [0x10, 0x08, 0][index] + strip as u8 };
    [0x90, note, if lit { 0x7f } else { 0 }]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_12_reaper_packets_distinguish_press_and_release() {
        for (note, pressed) in
            [(0x5e, Event::Play), (0x5d, Event::Stop), (0x2e, Event::Navigate(Navigation::BankLeft)), (0x2f, Event::Navigate(Navigation::BankRight))]
        {
            assert_eq!(decode([0x90, note, 0x7f]), Some(pressed));
            assert_eq!(decode([0x90, note, 0]), None);
            assert_eq!(decode([0x80, note, 0x7f]), None);
        }
        assert_eq!(decode([0xe0, 0x18, 0x18]), Some(Event::Fader(0, 3096)));
        for strip in 0..8 {
            assert_eq!(decode([0xe0 + strip, 1, 0x40]), Some(Event::Fader(usize::from(strip), 8193)));
        }
        assert_eq!(decode([0xe8, 0, 0]), None);
        assert_eq!(decode([0xe0, 0x80, 0]), None);
    }

    #[test]
    fn lamps_use_the_transport_notes_and_the_selected_strip() {
        assert_eq!(lamp(STRIPS, 0, true), [0x90, 0x5d, 0x7f]);
        assert_eq!(lamp(STRIPS, 1, false), [0x90, 0x5e, 0]);
        assert_eq!(lamp(STRIPS, 2, true), [0x90, 0x5f, 0x7f]);
        assert_eq!(lamp(0, 0, true), [0x90, 0x10, 0x7f]);
        assert_eq!(lamp(7, 1, false), [0x90, 0x0f, 0]);
        assert_eq!(lamp(7, 2, true), [0x90, 0x07, 0x7f]);
    }
}
