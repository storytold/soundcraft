//! Musical scales and key names: which pitch classes belong to a key such as "D minor".

use crate::names::PITCH_CLASSES;

/// A scale: a fixed set of semitone intervals above its root.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Scale {
    Major,
    NaturalMinor,
    Dorian,
    Phrygian,
    Lydian,
    Mixolydian,
    Locrian,
    HarmonicMinor,
    MelodicMinor,
    MajorPentatonic,
    MinorPentatonic,
    Blues,
}

impl Scale {
    pub const ALL: [Scale; 12] = [
        Scale::Major,
        Scale::NaturalMinor,
        Scale::Dorian,
        Scale::Phrygian,
        Scale::Lydian,
        Scale::Mixolydian,
        Scale::Locrian,
        Scale::HarmonicMinor,
        Scale::MelodicMinor,
        Scale::MajorPentatonic,
        Scale::MinorPentatonic,
        Scale::Blues,
    ];

    /// Semitones above the root, ascending, starting at 0.
    pub fn intervals(self) -> &'static [u8] {
        match self {
            Scale::Major => &[0, 2, 4, 5, 7, 9, 11],
            Scale::NaturalMinor => &[0, 2, 3, 5, 7, 8, 10],
            Scale::Dorian => &[0, 2, 3, 5, 7, 9, 10],
            Scale::Phrygian => &[0, 1, 3, 5, 7, 8, 10],
            Scale::Lydian => &[0, 2, 4, 6, 7, 9, 11],
            Scale::Mixolydian => &[0, 2, 4, 5, 7, 9, 10],
            Scale::Locrian => &[0, 1, 3, 5, 6, 8, 10],
            Scale::HarmonicMinor => &[0, 2, 3, 5, 7, 8, 11],
            // The ascending form.
            Scale::MelodicMinor => &[0, 2, 3, 5, 7, 9, 11],
            Scale::MajorPentatonic => &[0, 2, 4, 7, 9],
            Scale::MinorPentatonic => &[0, 3, 5, 7, 10],
            Scale::Blues => &[0, 3, 5, 6, 7, 10],
        }
    }

    /// Stable identifier for commands and saved sessions, e.g. `natural_minor`.
    pub fn id(self) -> &'static str {
        match self {
            Scale::Major => "major",
            Scale::NaturalMinor => "natural_minor",
            Scale::Dorian => "dorian",
            Scale::Phrygian => "phrygian",
            Scale::Lydian => "lydian",
            Scale::Mixolydian => "mixolydian",
            Scale::Locrian => "locrian",
            Scale::HarmonicMinor => "harmonic_minor",
            Scale::MelodicMinor => "melodic_minor",
            Scale::MajorPentatonic => "major_pentatonic",
            Scale::MinorPentatonic => "minor_pentatonic",
            Scale::Blues => "blues",
        }
    }

    /// English display name (a translation key for the UI), e.g. `Natural Minor`.
    pub fn label(self) -> &'static str {
        match self {
            Scale::Major => "Major",
            Scale::NaturalMinor => "Natural Minor",
            Scale::Dorian => "Dorian",
            Scale::Phrygian => "Phrygian",
            Scale::Lydian => "Lydian",
            Scale::Mixolydian => "Mixolydian",
            Scale::Locrian => "Locrian",
            Scale::HarmonicMinor => "Harmonic Minor",
            Scale::MelodicMinor => "Melodic Minor",
            Scale::MajorPentatonic => "Major Pentatonic",
            Scale::MinorPentatonic => "Minor Pentatonic",
            Scale::Blues => "Blues",
        }
    }

    pub fn from_id(id: &str) -> Option<Scale> {
        Scale::ALL.into_iter().find(|s| s.id() == id)
    }

    /// A scale written the way people write keys: `major`, `minor` (natural minor), an id such as
    /// `harmonic_minor`, or a label such as `Harmonic Minor`, ignoring case.
    fn from_words(words: &str) -> Option<Scale> {
        let w = words.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase();
        if w == "minor" {
            return Some(Scale::NaturalMinor);
        }
        Scale::ALL.into_iter().find(|s| s.id() == w || s.label().to_lowercase() == w)
    }
}

/// A pitch class 0–11 (C = 0) from a note name such as `C`, `F#`, `Bb`, `E♭` or `B#`, ignoring case.
pub fn parse_pitch_class(name: &str) -> Option<u8> {
    let mut chars = name.trim().chars();
    let natural: i32 = match chars.next()?.to_ascii_uppercase() {
        'C' => 0,
        'D' => 2,
        'E' => 4,
        'F' => 5,
        'G' => 7,
        'A' => 9,
        'B' => 11,
        _ => return None,
    };
    let shift = match chars.next() {
        None => 0,
        Some('#' | '♯') => 1,
        Some('b' | 'B' | '♭') => -1,
        Some(_) => return None,
    };
    if chars.next().is_some() {
        return None;
    }
    u8::try_from((natural + shift).rem_euclid(12)).ok()
}

/// The name of pitch class `pc` (sharps), e.g. `C#`.
pub fn pitch_class_name(pc: u8) -> &'static str {
    PITCH_CLASSES.get(usize::from(pc % 12)).copied().unwrap_or("C")
}

/// A key: a root pitch class and a scale, e.g. D Dorian.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct KeyScale {
    /// Pitch class of the root, 0–11 (C = 0).
    pub root: u8,
    pub scale: Scale,
}

impl KeyScale {
    pub fn new(root: u8, scale: Scale) -> Self {
        Self { root: root % 12, scale }
    }

    /// Whether MIDI pitch `pitch` belongs to the key.
    pub fn contains(&self, pitch: i32) -> bool {
        let step = (pitch - i32::from(self.root)).rem_euclid(12);
        self.scale.intervals().iter().any(|i| i32::from(*i) == step)
    }

    /// Whether MIDI pitch `pitch` is the key's root, in any octave.
    pub fn is_root(&self, pitch: i32) -> bool {
        pitch.rem_euclid(12) == i32::from(self.root)
    }

    /// Read key text such as `D minor`, `Db major`, `f# Dorian` or `A harmonic_minor`. Text that is
    /// not a tonic followed by a scale gives `None`.
    pub fn parse(text: &str) -> Option<KeyScale> {
        let text = text.trim();
        let (tonic, rest) = text.split_once(char::is_whitespace)?;
        Some(KeyScale::new(parse_pitch_class(tonic)?, Scale::from_words(rest)?))
    }

    /// The form [`KeyScale::parse`] reads back, e.g. `D dorian`; for saved sessions and commands.
    pub fn storage(&self) -> String {
        format!("{} {}", pitch_class_name(self.root), self.scale.id())
    }
}
