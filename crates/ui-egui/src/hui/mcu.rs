//! Mackie Control messages for the shared MIDI control surface.
use super::{Event, Navigation, STRIPS, fader_bytes};

const STOP: u8 = 0x5d;
const PLAY: u8 = 0x5e;
pub(crate) const FADERS: usize = STRIPS + 1;

pub(crate) fn decode(packet: [u8; 3]) -> Option<Event> {
    match packet {
        [0x90, PLAY, 1..=127] => Some(Event::Play),
        [0x90, STOP, 1..=127] => Some(Event::Stop),
        [0x90, 0x2e, 1..=127] => Some(Event::Navigate(Navigation::BankLeft)),
        [0x90, 0x2f, 1..=127] => Some(Event::Navigate(Navigation::BankRight)),
        [0x90, 0x30, 1..=127] => Some(Event::Navigate(Navigation::ChannelLeft)),
        [0x90, 0x31, 1..=127] => Some(Event::Navigate(Navigation::ChannelRight)),
        [0x90, note @ 0x00..=0x07, 1..=127] => Some(Event::Arm(usize::from(note))),
        [0x90, note @ 0x08..=0x0f, 1..=127] => Some(Event::Solo(usize::from(note - 0x08))),
        [0x90, note @ 0x10..=0x17, 1..=127] => Some(Event::Mute(usize::from(note - 0x10))),
        [0x90, note @ 0x18..=0x1f, 1..=127] => Some(Event::Select(usize::from(note - 0x18))),
        [0x90, 0x54, 1..=127] => Some(Event::SetMarker),
        [0x90, 0x56, 1..=127] => Some(Event::Cycle),
        [0x90, 0x5b, 1..=127] => Some(Event::Rewind),
        [0x90, 0x5c, 1..=127] => Some(Event::FastForward),
        [0x90, 0x5f, 1..=127] => Some(Event::Record),
        [0x90, note @ 0x68..=0x70, value @ 0..=127] => Some(Event::Touch(usize::from(note - 0x68), value != 0)),
        [0x80, note @ 0x68..=0x70, 0..=127] => Some(Event::Touch(usize::from(note - 0x68), false)),
        [0xb0, cc @ 0x10..=0x17, value] => relative(value).map(|steps| Event::Pan(usize::from(cc - 0x10), steps)),
        [0xb0, 0x3c, value] => relative(value).map(Event::Jog),
        [channel @ 0xe0..=0xe8, low @ 0..=127, high @ 0..=127] => {
            Some(Event::Fader(usize::from(channel - 0xe0), u16::from(low) | (u16::from(high) << 7)))
        }
        _ => None,
    }
}

fn relative(value: u8) -> Option<i8> {
    match value {
        1..=0x3f => Some(value as i8),
        0x41..=0x7f => Some(-((value & 0x3f) as i8)),
        _ => None,
    }
}

pub(crate) fn lamp(strip: usize, index: usize, lit: bool) -> Option<[u8; 3]> {
    let note = match strip {
        STRIPS => *[STOP, PLAY, 0x5f].get(index)?,
        0..STRIPS => *[0x10, 0x08, 0, 0x18].get(index)? + strip as u8,
        _ => return None,
    };
    Some([0x90, note, if lit { 0x7f } else { 0 }])
}

pub(crate) fn fader(strip: usize, position: u16) -> Option<[u8; 3]> {
    let strip = u8::try_from(strip).ok().filter(|strip| usize::from(*strip) < FADERS)?;
    let [low, high] = fader_bytes(position);
    Some([0xe0 + strip, low, high])
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
        assert_eq!(decode([0xe8, 127, 127]), Some(Event::Fader(8, 16_383)));
        assert_eq!(fader(8, 16_383), Some([0xe8, 127, 127]));
        assert_eq!(decode([0x90, 0x70, 127]), Some(Event::Touch(8, true)));
        assert_eq!(decode([0x80, 0x70, 0]), Some(Event::Touch(8, false)));
        assert_eq!(decode([0xe9, 0, 0]), None);
        assert_eq!(fader(9, 0), None);
        assert_eq!(decode([0xe0, 0x80, 0]), None);
    }

    #[test]
    fn lamps_use_the_transport_notes_and_the_selected_strip() {
        assert_eq!(lamp(STRIPS, 0, true), Some([0x90, 0x5d, 0x7f]));
        assert_eq!(lamp(STRIPS, 1, false), Some([0x90, 0x5e, 0]));
        assert_eq!(lamp(STRIPS, 2, true), Some([0x90, 0x5f, 0x7f]));
        assert_eq!(lamp(0, 0, true), Some([0x90, 0x10, 0x7f]));
        assert_eq!(lamp(7, 1, false), Some([0x90, 0x0f, 0]));
        assert_eq!(lamp(7, 2, true), Some([0x90, 0x07, 0x7f]));
    }

    #[test]
    fn mixer_and_navigation_buttons_relative_controls_and_touch_decode() {
        for (note, event) in [
            (0x00, Event::Arm(0)),
            (0x0f, Event::Solo(7)),
            (0x10, Event::Mute(0)),
            (0x30, Event::Navigate(Navigation::ChannelLeft)),
            (0x31, Event::Navigate(Navigation::ChannelRight)),
            (0x54, Event::SetMarker),
            (0x56, Event::Cycle),
            (0x5b, Event::Rewind),
            (0x5c, Event::FastForward),
            (0x5f, Event::Record),
        ] {
            assert_eq!(decode([0x90, note, 127]), Some(event));
            assert_eq!(decode([0x90, note, 0]), None);
            assert_eq!(decode([0x80, note, 127]), None);
        }
        assert_eq!(decode([0xb0, 0x10, 1]), Some(Event::Pan(0, 1)));
        assert_eq!(decode([0xb0, 0x17, 0x47]), Some(Event::Pan(7, -7)));
        assert_eq!(decode([0xb0, 0x3c, 0x41]), Some(Event::Jog(-1)));
        assert_eq!(decode([0xb0, 0x3c, 0x40]), None);
        assert_eq!(decode([0x90, 0x6f, 127]), Some(Event::Touch(7, true)));
        assert_eq!(decode([0x90, 0x6f, 0]), Some(Event::Touch(7, false)));
        assert_eq!(decode([0x80, 0x6f, 64]), Some(Event::Touch(7, false)));
        assert_eq!(fader(1, 8_193), Some([0xe1, 1, 0x40]));
    }
}
