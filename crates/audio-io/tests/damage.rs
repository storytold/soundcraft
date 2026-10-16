//! Damage in a lossless stream is reported and keeps the timing (#550).

use soundcraft_audio_io::{AudioBuffer, BitDepth, EncodeOptions, FileFormat, decode, encode};

const FRAMES: usize = 48_000;

fn tone() -> AudioBuffer {
    let mut b = AudioBuffer::new(48_000, 1, FRAMES);
    for (i, s) in b.channels[0].iter_mut().enumerate() {
        *s = ((8000.0 * (i as f64 * 0.05).sin()).round() / 32768.0) as f32;
    }
    b
}

fn flac() -> Vec<u8> {
    encode(&tone(), &EncodeOptions { format: FileFormat::Flac, bit_depth: BitDepth::Int16, dither: false, bwf: None }).unwrap()
}

#[test]
fn an_intact_flac_reports_no_damage() {
    let (info, buf) = decode(&flac(), Some("flac")).unwrap();
    assert_eq!(info.damage, None);
    assert_eq!(buf.frames(), FRAMES);
}

#[test]
fn a_corrupt_flac_frame_becomes_silence_and_is_reported() {
    let mut bytes = flac();
    let mid = bytes.len() / 2;
    bytes[mid] ^= 0xFF;
    let (info, buf) = decode(&bytes, Some("flac")).unwrap();
    let damage = info.damage.clone().unwrap_or_else(|| panic!("no damage: {} frames", buf.frames()));
    assert!(damage.contains("corrupt frame"), "{damage}");
    // Nothing is lost or shifted: the length holds, and every sample is either the original one
    // (same position) or silence where the frame could not be decoded.
    assert_eq!(buf.frames(), FRAMES);
    assert_eq!(info.frames, FRAMES as u64);
    let src = tone();
    let (mut same, mut silent) = (0, 0);
    for (a, b) in src.channels[0].iter().zip(&buf.channels[0]) {
        if a == b {
            same += 1;
        } else {
            assert_eq!(*b, 0.0);
            silent += 1;
        }
    }
    assert!(silent > 0 && same > FRAMES / 2, "same {same} silent {silent}");
    // The audio after the damage is in place (it used to come out one block early).
    assert_eq!(buf.channels[0][FRAMES - 1], src.channels[0][FRAMES - 1]);
}

#[test]
fn a_truncated_flac_is_reported() {
    let bytes = flac();
    let (info, buf) = decode(&bytes[..bytes.len() / 2], Some("flac")).unwrap();
    let damage = info.damage.expect("damage reported");
    assert!(damage.contains("ends early") && damage.contains(&format!("{FRAMES}")), "{damage}");
    assert!(buf.frames() < FRAMES);
}

#[test]
fn lossy_and_native_pcm_files_report_no_damage() {
    for format in [FileFormat::Wav, FileFormat::Aiff] {
        let bytes = encode(&tone(), &EncodeOptions { format, bit_depth: BitDepth::Int16, dither: false, bwf: None }).unwrap();
        assert_eq!(decode(&bytes, None).unwrap().0.damage, None);
    }
}
