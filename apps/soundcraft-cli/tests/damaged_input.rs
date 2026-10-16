//! `soundcraft-cli convert` refuses a damaged lossless file instead of writing shorter audio (#550).
// Integration tests: unwrapping and panicking on failure is fine here, unlike in shipped code (AGENTS.md).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::process::Command;

use soundcraft_audio_io::{AudioBuffer, BitDepth, EncodeOptions, FileFormat};

#[test]
fn convert_refuses_a_damaged_flac() {
    let dir = std::env::temp_dir().join(format!("sc-cli-damaged-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut buf = AudioBuffer::new(48_000, 1, 48_000);
    for (i, s) in buf.channels[0].iter_mut().enumerate() {
        *s = (i as f32 * 0.05).sin() * 0.25;
    }
    let opts = EncodeOptions { format: FileFormat::Flac, bit_depth: BitDepth::Int16, dither: false, bwf: None };
    let good = soundcraft_audio_io::encode(&buf, &opts).unwrap();
    let mut damaged = good.clone();
    let mid = damaged.len() / 2;
    damaged[mid] ^= 0xFF;
    let cases = [("good", good.clone(), true), ("damaged", damaged, false), ("cut", good[..good.len() / 2].to_vec(), false)];
    for (name, bytes, ok) in cases {
        let (input, output) = (dir.join(format!("{name}.flac")), dir.join(format!("{name}.wav")));
        std::fs::write(&input, bytes).unwrap();
        let o = Command::new(env!("CARGO_BIN_EXE_soundcraft-cli"))
            .env("SOUNDCRAFT_NO_PREFS", "1")
            .arg("convert")
            .arg(&input)
            .arg(&output)
            .output()
            .unwrap();
        let stderr = String::from_utf8_lossy(&o.stderr);
        assert_eq!(o.status.success(), ok, "{name}: {stderr}");
        assert_eq!(output.exists(), ok, "{name}");
        if !ok {
            assert!(stderr.contains("damaged audio, nothing written"), "{name}: {stderr}");
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
}
