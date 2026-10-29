//! Every extension the format table reads decodes with the compiled-in `symphonia` features.

use soundcraft_audio_io::{AudioBuffer, BitDepth, EncodeOptions, FileFormat, decode, encode, probe};

fn fixture(name: &str) -> Vec<u8> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(name);
    std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn generated(format: FileFormat) -> Vec<u8> {
    let tone: Vec<f32> = (0..800).map(|i| (i as f32 * 0.1).sin() * 0.5).collect();
    let buf = AudioBuffer { sample_rate: 8000, channels: vec![tone] };
    encode(&buf, &EncodeOptions { format, bit_depth: BitDepth::Int16, dither: false, bwf: None }).unwrap()
}

fn bytes_for(ext: &str) -> Vec<u8> {
    match ext {
        "wav" | "wave" | "bwf" => generated(FileFormat::Wav),
        "rf64" => {
            let tone = AudioBuffer { sample_rate: 8000, channels: vec![vec![0.25; 800]] };
            let opts = EncodeOptions { format: FileFormat::Wav, bit_depth: BitDepth::Int16, dither: false, bwf: None };
            soundcraft_audio_io::encode_wav_rf64(&tone, &opts).unwrap()
        }
        "aif" | "aiff" => generated(FileFormat::Aiff),
        "aifc" => fixture("tone.aifc"),
        "flac" => generated(FileFormat::Flac),
        "mp3" => fixture("tone.mp3"),
        "mp2" => fixture("tone.mp2"),
        "mp1" => fixture("silence.mp1"),
        "ogg" | "oga" => fixture("tone.ogg"),
        "m4a" | "m4b" | "mp4" => fixture("tone.m4a"),
        "aac" | "adts" => fixture("tone.aac"),
        "caf" => fixture("tone.caf"),
        "mkv" => fixture("tone.mkv"),
        "webm" => fixture("tone.webm"),
        other => panic!("no fixture for the READ extension {other:?}: add one"),
    }
}

#[test]
fn every_read_extension_probes_and_decodes() {
    for &format in FileFormat::READ {
        for ext in format.extensions() {
            let bytes = bytes_for(ext);
            let info = probe(&bytes, Some(ext)).unwrap_or_else(|e| panic!("probe .{ext}: {e}"));
            let (_, buf) = decode(&bytes, Some(ext)).unwrap_or_else(|e| panic!("decode .{ext}: {e}"));
            assert!(info.channels >= 1, ".{ext}");
            assert!(buf.frames() > 0, ".{ext}: no frames");
        }
    }
}
