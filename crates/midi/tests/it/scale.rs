use proptest::prelude::*;
use soundcraft_midi::{KeyScale, Scale};

#[test]
fn key_text_reads_with_sharps_flats_case_and_spacing() {
    let k = |s: &str| KeyScale::parse(s);
    assert_eq!(k("Db major"), k("C# major"));
    assert_eq!(k("Db major"), k("D♭ major"));
    assert_eq!(k("  d   MINOR "), k("D minor"));
    assert_eq!(k("D minor"), k("D natural minor"));
    assert_eq!(k("A harmonic_minor"), k("a Harmonic Minor"));
    // Flats and sharps wrap around the octave.
    assert_eq!(k("Cb major"), k("B major"));
    assert_eq!(k("B# minor"), k("C minor"));
    assert_ne!(k("D minor"), k("D major"));
}

#[test]
fn unreadable_key_text_is_no_key() {
    for text in ["", "   ", "C", "major", "H major", "C majorish", "Cbb major", "C# ", "C minor major", "Do majeur"] {
        assert_eq!(KeyScale::parse(text), None, "{text:?}");
    }
}

#[test]
fn the_stored_form_reads_back_for_every_root_and_scale() {
    for root in 0..12 {
        for scale in Scale::ALL {
            let k = KeyScale::new(root, scale);
            assert_eq!(KeyScale::parse(&k.storage()), Some(k));
            assert_eq!(Scale::from_id(scale.id()), Some(scale));
        }
    }
}

#[test]
fn every_scale_contains_its_root_once_per_octave() {
    for scale in Scale::ALL {
        let iv = scale.intervals();
        assert!(iv.windows(2).all(|w| w[0] < w[1]) && iv.iter().all(|i| *i < 12), "{scale:?}");
        for root in 0..12 {
            let k = KeyScale::new(root, scale);
            for pitch in 0..128 {
                if k.is_root(pitch) {
                    assert!(k.contains(pitch), "{scale:?} root {root}: {pitch}");
                }
            }
            assert_eq!((0..12).filter(|p| k.contains(*p)).count(), iv.len());
        }
    }
}

#[test]
fn a_mode_shares_its_notes_with_its_relative_major() {
    // D Dorian, A natural minor and C major are the white keys.
    let white = |p: i32| !matches!(p.rem_euclid(12), 1 | 3 | 6 | 8 | 10);
    for k in ["C major", "D dorian", "A minor", "G mixolydian", "B locrian"] {
        let k = KeyScale::parse(k).unwrap();
        assert!((0..128).all(|p| k.contains(p) == white(p)), "{k:?}");
    }
}

proptest! {
    #[test]
    fn any_text_parses_without_panicking(text in any::<String>()) {
        let _ = KeyScale::parse(&text);
    }
}
