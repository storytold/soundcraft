//! Pure MIDI messages for the desktop control surfaces.

#[path = "hui/mcu.rs"]
pub(crate) mod mcu;

pub(crate) const STRIPS: usize = 8;
pub(crate) const TRANSPORT: u8 = 0x0e;
pub(crate) const NAVIGATION: u8 = 0x0a;
pub(crate) const LOOP: u8 = 0x0f;
pub(crate) const MARKERS: u8 = 0x14;
pub(crate) const PONG: [u8; 3] = [0x90, 0, 0x7f];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Event {
    Pong,
    Stop,
    Play,
    Record,
    Rewind,
    FastForward,
    Cycle,
    SetMarker,
    Jog(i8),
    Navigate(Navigation),
    Select(usize),
    Mute(usize),
    Solo(usize),
    Arm(usize),
    Pan(usize, i8),
    Fader(usize, u16),
    Touch(usize, bool),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Navigation {
    BankLeft,
    BankRight,
    ChannelLeft,
    ChannelRight,
}

impl Navigation {
    pub(crate) fn target(self, first: usize, count: usize) -> usize {
        let last_bank = (count.saturating_sub(1) / STRIPS) * STRIPS;
        match self {
            Self::BankLeft => first.saturating_sub(STRIPS),
            Self::BankRight => first.saturating_add(STRIPS).min(last_bank),
            Self::ChannelLeft => first.saturating_sub(1),
            Self::ChannelRight => first.saturating_add(1).min(last_bank),
        }
    }
}

#[derive(Default)]
pub(crate) struct Decode {
    zone: Option<u8>,
    pub(crate) coarse: [Option<u8>; STRIPS],
}

impl Decode {
    pub(crate) fn packet(&mut self, packet: [u8; 3]) -> Option<Event> {
        match packet {
            PONG => Some(Event::Pong),
            [0xb0, 0x0f, zone @ 0..=127] => {
                self.zone = Some(zone);
                None
            }
            [0xb0, 0x2f, value] => match (self.zone.take()?, value) {
                (TRANSPORT, 0x41) => Some(Event::Rewind),
                (TRANSPORT, 0x42) => Some(Event::FastForward),
                (TRANSPORT, 0x43) => Some(Event::Stop),
                (TRANSPORT, 0x44) => Some(Event::Play),
                (TRANSPORT, 0x45) => Some(Event::Record),
                (LOOP, 0x43) => Some(Event::Cycle),
                (MARKERS, 0x40) => Some(Event::SetMarker),
                (NAVIGATION, 0x40) => Some(Event::Navigate(Navigation::ChannelLeft)),
                (NAVIGATION, 0x41) => Some(Event::Navigate(Navigation::BankLeft)),
                (NAVIGATION, 0x42) => Some(Event::Navigate(Navigation::ChannelRight)),
                (NAVIGATION, 0x43) => Some(Event::Navigate(Navigation::BankRight)),
                (strip @ 0..=7, 0x41) => Some(Event::Select(usize::from(strip))),
                (strip @ 0..=7, 0x42) => Some(Event::Mute(usize::from(strip))),
                (strip @ 0..=7, 0x43) => Some(Event::Solo(usize::from(strip))),
                (strip @ 0..=7, 0x47) => Some(Event::Arm(usize::from(strip))),
                (strip @ 0..=7, 0x00 | 0x40) => Some(Event::Touch(usize::from(strip), value == 0x40)),
                _ => None,
            },
            [0xb0, 0x0d, value] => relative_steps(value).map(Event::Jog),
            [0xb0, knob @ 0x40..=0x47, value] => relative_steps(value).map(|steps| Event::Pan(usize::from(knob - 0x40), steps)),
            [0xb0, strip @ 0..=7, coarse @ 0..=127] => {
                *self.coarse.get_mut(usize::from(strip))? = Some(coarse);
                None
            }
            [0xb0, fine @ 0x20..=0x27, value @ 0..=127] => {
                let strip = usize::from(fine - 0x20);
                let coarse = self.coarse.get_mut(strip)?.take()?;
                Some(Event::Fader(strip, (u16::from(coarse) << 7) | u16::from(value)))
            }
            _ => None,
        }
    }
}

pub(crate) fn hui_fader(strip: usize, position: u16) -> Option<[[u8; 3]; 2]> {
    let strip = u8::try_from(strip).ok().filter(|strip| *strip < STRIPS as u8)?;
    let [low, high] = fader_bytes(position);
    Some([[0xb0, strip, high], [0xb0, 0x20 + strip, low]])
}

fn fader_bytes(value: u16) -> [u8; 2] {
    [(value & 0x7f) as u8, ((value >> 7) & 0x7f) as u8]
}

fn relative_steps(value: u8) -> Option<i8> {
    let magnitude = (value & 0x3f) as i8;
    match value {
        0 | 0x40 | 0x80..=0xff => None,
        1..=0x3f => Some(-magnitude),
        _ => Some(magnitude),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hui_touch_and_faders_round_trip() {
        let mut decode = Decode::default();
        for (value, held) in [(0x40, true), (0, false)] {
            assert_eq!(decode.packet([0xb0, 0x0f, 7]), None);
            assert_eq!(decode.packet([0xb0, 0x2f, value]), Some(Event::Touch(7, held)));
        }
        let [high, low] = hui_fader(7, 8_193).unwrap();
        assert_eq!(high, [0xb0, 7, 64]);
        assert_eq!(low, [0xb0, 0x27, 1]);
        assert_eq!(decode.packet(high), None);
        assert_eq!(decode.packet(low), Some(Event::Fader(7, 8_193)));
    }
    #[test]
    fn model_12_mixing_and_locate_packets_act_only_on_button_press() {
        let mut decoder = Decode::default();
        for (zone, port, event) in [
            (0x0f, 3, Event::Cycle),
            (0, 2, Event::Mute(0)),
            (7, 3, Event::Solo(7)),
            (0x14, 0, Event::SetMarker),
            (0x0e, 1, Event::Rewind),
            (0x0e, 2, Event::FastForward),
        ] {
            assert_eq!(decoder.packet([0xb0, 0x0f, zone]), None);
            assert_eq!(decoder.packet([0xb0, 0x2f, port | 0x40]), Some(event));
            assert_eq!(decoder.packet([0xb0, 0x0f, zone]), None);
            assert_eq!(decoder.packet([0xb0, 0x2f, port]), None);
        }
        assert_eq!(decoder.packet([0xb0, 0x40, 0x3f]), Some(Event::Pan(0, -63)));
        assert_eq!(decoder.packet([0xb0, 0x47, 0x7f]), Some(Event::Pan(7, 63)));
        assert_eq!(decoder.packet([0xb0, 0x0d, 0x41]), Some(Event::Jog(1)));
        assert_eq!(decoder.packet([0xb0, 0x0d, 0x01]), Some(Event::Jog(-1)));
        assert_eq!(decoder.packet([0xb0, 0x0d, 0x40]), None);
    }

    #[test]
    fn select_buttons_cover_all_strips_only_on_press() {
        let mut hui = Decode::default();
        for strip in 0..8 {
            let selected = Some(Event::Select(usize::from(strip)));
            assert_eq!(hui.packet([0xb0, 0x0f, strip]), None);
            assert_eq!(hui.packet([0xb0, 0x2f, 0x41]), selected);
            assert_eq!(hui.packet([0xb0, 0x0f, strip]), None);
            assert_eq!(hui.packet([0xb0, 0x2f, 0x01]), None);
            assert_eq!(mcu::decode([0x90, 0x18 + strip, 127]), selected);
            assert_eq!(mcu::decode([0x90, 0x18 + strip, 0]), None);
            assert_eq!(mcu::decode([0x80, 0x18 + strip, 64]), None);
        }
    }

    #[test]
    fn model_12_packets_select_transport_strip_buttons_and_faders() {
        let mut decoder = Decode::default();
        assert_eq!(decoder.packet([0x90, 0, 127]), Some(Event::Pong));
        for (port, action) in [(3, Event::Stop), (4, Event::Play), (5, Event::Record)] {
            assert_eq!(decoder.packet([0xb0, 0x0f, TRANSPORT]), None);
            assert_eq!(decoder.packet([0xb0, 0x2f, port | 0x40]), Some(action));
            assert_eq!(decoder.packet([0xb0, 0x0f, TRANSPORT]), None);
            assert_eq!(decoder.packet([0xb0, 0x2f, port]), None);
        }
        for (port, direction) in [(0, Navigation::ChannelLeft), (1, Navigation::BankLeft), (2, Navigation::ChannelRight), (3, Navigation::BankRight)]
        {
            assert_eq!(decoder.packet([0xb0, 0x0f, NAVIGATION]), None);
            assert_eq!(decoder.packet([0xb0, 0x2f, port | 0x40]), Some(Event::Navigate(direction)));
            assert_eq!(decoder.packet([0xb0, 0x0f, NAVIGATION]), None);
            assert_eq!(decoder.packet([0xb0, 0x2f, port]), None);
        }
        for strip in 0..8 {
            assert_eq!(decoder.packet([0xb0, 0x0f, strip]), None);
            assert_eq!(decoder.packet([0xb0, 0x2f, 0x47]), Some(Event::Arm(usize::from(strip))));
            assert_eq!(decoder.packet([0xb0, strip, 96]), None);
            assert_eq!(decoder.packet([0xb0, 0x20 + strip, 16]), Some(Event::Fader(usize::from(strip), 12_304)));
            assert_eq!(decoder.packet([0xb0, 0x20 + strip, 16]), None);
        }
    }
    #[test]
    fn hui_navigation_steps_eight_or_one_and_stops_at_the_last_bank() {
        use Navigation::*;
        assert_eq!(BankRight.target(0, 16), 8);
        assert_eq!(BankRight.target(8, 16), 8);
        assert_eq!(ChannelRight.target(8, 16), 8);
        assert_eq!(ChannelLeft.target(8, 16), 7);
        assert_eq!(ChannelRight.target(7, 16), 8);
        assert_eq!(BankLeft.target(7, 16), 0);
        assert_eq!(BankRight.target(8, 17), 16);
        assert_eq!(BankRight.target(0, 1), 0);
        assert_eq!(ChannelRight.target(0, 0), 0);
    }
}
