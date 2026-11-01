//! Musical conductor data, independent of memory locations.
use crate::ChordId;
use soundcraft_time::Samples;
/// A chord spelling, including an optional slash bass.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ChordSymbol {
    pub root: String,
    pub quality: String,
    pub bass: Option<String>,
}
impl ChordSymbol {
    pub fn valid(&self) -> bool {
        let note = |n: &str| {
            ["C", "C#", "Db", "D", "D#", "Eb", "E", "Fb", "E#", "F", "F#", "Gb", "G", "G#", "Ab", "A", "A#", "Bb", "B", "Cb", "B#"].contains(&n)
        };
        note(&self.root) && self.quality.chars().count() <= 64 && !self.quality.chars().any(char::is_control) && self.bass.as_deref().is_none_or(note)
    }
    pub fn label(&self) -> String {
        let mut label = format!("{}{}", self.root, self.quality);
        if let Some(bass) = &self.bass {
            label.push('/');
            label.push_str(bass);
        }
        label
    }
}
/// One chord symbol on the session timeline.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ChordEvent {
    pub id: ChordId,
    pub at: Samples,
    pub symbol: ChordSymbol,
}
/// Keys in circle-of-fifths order, seven flats through seven sharps.
pub fn key_names(minor: bool) -> &'static [&'static str; 15] {
    if minor {
        &[
            "Ab minor", "Eb minor", "Bb minor", "F minor", "C minor", "G minor", "D minor", "A minor", "E minor", "B minor", "F# minor", "C# minor",
            "G# minor", "D# minor", "A# minor",
        ]
    } else {
        &[
            "Cb major", "Gb major", "Db major", "Ab major", "Eb major", "Bb major", "F major", "C major", "G major", "D major", "A major", "E major",
            "B major", "F# major", "C# major",
        ]
    }
}
/// Accidentals for a recognized major or natural-minor key; legacy text remains readable.
pub fn key_fifths(key: &str) -> Option<i8> {
    [false, true].into_iter().find_map(|minor| key_names(minor).iter().position(|n| *n == key).map(|i| i as i8 - 7))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn key_spelling_covers_every_staff_accidental_count() {
        for minor in [false, true] {
            for (i, key) in key_names(minor).iter().enumerate() {
                assert_eq!(key_fifths(key), Some(i as i8 - 7));
            }
        }
        assert_eq!(key_fifths("old custom key"), None);
    }
    #[test]
    fn legacy_sessions_keep_markers_and_secondary_rate() {
        let mut s = crate::Session { frame_rate: soundcraft_time::FrameRate::Fps25, ..Default::default() };
        s.add_marker("Dm7", crate::MarkerKind::Marker, 100, 100);
        let mut json = serde_json::to_value(&s).unwrap();
        json.as_object_mut().unwrap().remove("chords");
        json.as_object_mut().unwrap().remove("secondary_frame_rate");
        let loaded = crate::Session::from_json(&json.to_string()).unwrap();
        assert_eq!(loaded.markers, s.markers);
        assert!(loaded.chords.is_empty());
        assert_eq!(loaded.secondary_frame_rate, Some(soundcraft_time::FrameRate::Fps30));
    }
}
