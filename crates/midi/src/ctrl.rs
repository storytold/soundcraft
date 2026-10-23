//! Controller kinds as the editor and the commands name them: ids, value ranges and display names.

use crate::{CtrlKind, note_name};

/// Standard names of the MIDI 1.0 controller numbers people draw most.
pub fn cc_name(n: u8) -> Option<&'static str> {
    Some(match n {
        0 => "Bank Select",
        1 => "Mod Wheel",
        2 => "Breath",
        4 => "Foot Pedal",
        5 => "Portamento Time",
        6 => "Data Entry",
        7 => "Volume",
        8 => "Balance",
        10 => "Pan",
        11 => "Expression",
        12 => "Effect 1",
        13 => "Effect 2",
        64 => "Sustain",
        65 => "Portamento",
        66 => "Sostenuto",
        67 => "Soft Pedal",
        68 => "Legato",
        69 => "Hold 2",
        71 => "Resonance",
        72 => "Release Time",
        73 => "Attack Time",
        74 => "Cutoff",
        84 => "Portamento Control",
        91 => "Reverb",
        92 => "Tremolo",
        93 => "Chorus",
        94 => "Detune",
        95 => "Phaser",
        120 => "All Sound Off",
        121 => "Reset Controllers",
        123 => "All Notes Off",
        _ => return None,
    })
}

impl CtrlKind {
    /// The kinds offered first when a controller lane is chosen.
    pub const COMMON: [CtrlKind; 9] = [
        CtrlKind::Cc(1),
        CtrlKind::Cc(2),
        CtrlKind::Cc(7),
        CtrlKind::Cc(10),
        CtrlKind::Cc(11),
        CtrlKind::Cc(64),
        CtrlKind::PitchBend,
        CtrlKind::ChannelPressure,
        CtrlKind::Program,
    ];

    /// A short id for commands and saved lanes: `cc7`, `pitchbend`, `aftertouch`, `program`, `polyat60`.
    pub fn id(self) -> String {
        match self {
            CtrlKind::Cc(n) => format!("cc{n}"),
            CtrlKind::PitchBend => "pitchbend".into(),
            CtrlKind::ChannelPressure => "aftertouch".into(),
            CtrlKind::Program => "program".into(),
            CtrlKind::PolyPressure(p) => format!("polyat{p}"),
        }
    }

    /// Parses [`CtrlKind::id`] and a few spellings people type (`cc:7`, `CC 7`, `pitch_bend`, `pb`).
    pub fn from_id(s: &str) -> Option<CtrlKind> {
        let s = s.trim().to_ascii_lowercase();
        let number = |rest: &str| rest.trim_start_matches([':', ' ', '_']).parse::<u8>().ok().filter(|n| *n <= 127);
        match s.as_str() {
            "pitchbend" | "pitch_bend" | "pitch bend" | "bend" | "pb" => Some(CtrlKind::PitchBend),
            "aftertouch" | "channel_pressure" | "pressure" | "mono_aftertouch" | "at" => Some(CtrlKind::ChannelPressure),
            "program" | "program_change" | "pc" => Some(CtrlKind::Program),
            _ => {
                if let Some(rest) = s.strip_prefix("polyat").or_else(|| s.strip_prefix("poly_pressure")) {
                    number(rest).map(CtrlKind::PolyPressure)
                } else {
                    s.strip_prefix("cc").and_then(number).map(CtrlKind::Cc)
                }
            }
        }
    }

    /// Smallest and largest value.
    pub fn range(self) -> (i32, i32) {
        match self {
            CtrlKind::PitchBend => (-8192, 8191),
            _ => (0, 127),
        }
    }

    /// The value an untouched controller sits at: centre for pitch bend and pan, else 0.
    pub fn rest_value(self) -> i32 {
        match self {
            CtrlKind::PitchBend => 0,
            CtrlKind::Cc(10) | CtrlKind::Cc(8) => 64,
            _ => 0,
        }
    }

    /// The full name, e.g. "Mod Wheel (CC 1)", "Pitch Bend".
    pub fn label(self) -> String {
        match self {
            CtrlKind::Cc(n) => match cc_name(n) {
                Some(name) => format!("{name} (CC {n})"),
                None => format!("CC {n}"),
            },
            CtrlKind::PitchBend => "Pitch Bend".into(),
            CtrlKind::ChannelPressure => "Mono Aftertouch".into(),
            CtrlKind::Program => "Program Change".into(),
            CtrlKind::PolyPressure(p) => format!("Poly Aftertouch {}", note_name(p)),
        }
    }

    /// A name that fits a narrow lane header.
    pub fn short(self) -> String {
        match self {
            CtrlKind::Cc(1) => "Mod".into(),
            CtrlKind::Cc(2) => "Breath".into(),
            CtrlKind::Cc(7) => "Volume".into(),
            CtrlKind::Cc(10) => "Pan".into(),
            CtrlKind::Cc(11) => "Expr".into(),
            CtrlKind::Cc(64) => "Sustain".into(),
            CtrlKind::Cc(n) => format!("CC {n}"),
            CtrlKind::PitchBend => "Bend".into(),
            CtrlKind::ChannelPressure => "Aftert.".into(),
            CtrlKind::Program => "Program".into(),
            CtrlKind::PolyPressure(p) => format!("PolyAT {}", note_name(p)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_round_trip_and_common_spellings_parse() {
        for k in [CtrlKind::Cc(0), CtrlKind::Cc(127), CtrlKind::PitchBend, CtrlKind::ChannelPressure, CtrlKind::Program, CtrlKind::PolyPressure(60)] {
            assert_eq!(CtrlKind::from_id(&k.id()), Some(k), "{k:?}");
        }
        assert_eq!(CtrlKind::from_id("CC 7"), Some(CtrlKind::Cc(7)));
        assert_eq!(CtrlKind::from_id("cc:64"), Some(CtrlKind::Cc(64)));
        assert_eq!(CtrlKind::from_id("pitch_bend"), Some(CtrlKind::PitchBend));
        assert_eq!(CtrlKind::from_id("cc128"), None);
        assert_eq!(CtrlKind::from_id("cc"), None);
        assert_eq!(CtrlKind::from_id("nonsense"), None);
    }

    #[test]
    fn names_and_ranges() {
        assert_eq!(CtrlKind::Cc(1).label(), "Mod Wheel (CC 1)");
        assert_eq!(CtrlKind::Cc(3).label(), "CC 3");
        assert_eq!(CtrlKind::PitchBend.range(), (-8192, 8191));
        assert_eq!(CtrlKind::Cc(10).rest_value(), 64);
    }
}
