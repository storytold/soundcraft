//! `soundcraft-cli convert` dithers only when the output loses resolution (#548), run as the real binary.
// Integration tests: unwrapping and panicking on failure is fine here, unlike in shipped code (AGENTS.md).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::{Path, PathBuf};
use std::process::Command;

use soundcraft_audio_io::{AudioBuffer, BitDepth, EncodeOptions, FileFormat};

fn tmpdir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("sc-cli-convert-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A 16-bit mono WAV of `samples`, written without dither so the file holds exactly these values.
fn write_wav16(path: &Path, samples: &[i16]) {
    let mut buf = AudioBuffer::new(48_000, 1, samples.len());
    for (d, s) in buf.channels[0].iter_mut().zip(samples) {
        *d = f32::from(*s) / 32768.0;
    }
    let opts = EncodeOptions { format: FileFormat::Wav, bit_depth: BitDepth::Int16, dither: false, bwf: None };
    std::fs::write(path, soundcraft_audio_io::encode(&buf, &opts).unwrap()).unwrap();
}

fn convert(input: &Path, output: &Path, extra: &[&str]) {
    let o = Command::new(env!("CARGO_BIN_EXE_soundcraft-cli"))
        .env("SOUNDCRAFT_NO_PREFS", "1")
        .arg("convert")
        .arg(input)
        .arg(output)
        .args(extra)
        .output()
        .unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
}

/// The decoded samples of `path`, scaled to 16-bit steps.
fn steps16(path: &Path) -> Vec<f64> {
    let (_, buf) = soundcraft_audio_io::decode(&std::fs::read(path).unwrap(), None).unwrap();
    buf.channels[0].iter().map(|s| f64::from(*s) * 32768.0).collect()
}

#[test]
fn convert_copies_samples_when_nothing_is_lost() {
    let dir = tmpdir("exact");
    let (silence, tone) = (dir.join("silence.wav"), dir.join("tone.wav"));
    let tone_samples: Vec<i16> = (0..4800).map(|i| (8000.0 * (f64::from(i) * 0.05).sin()) as i16).collect();
    write_wav16(&silence, &[0; 4800]);
    write_wav16(&tone, &tone_samples);
    let want: Vec<f64> = tone_samples.iter().map(|s| f64::from(*s)).collect();

    // Same depth, wider and float: every sample comes back unchanged, silence stays silent.
    for (depth, name) in [("16", "s16.wav"), ("24", "s24.wav"), ("32f", "s32.wav")] {
        let out = dir.join(name);
        convert(&silence, &out, &["--bit-depth", depth]);
        assert!(steps16(&out).iter().all(|s| *s == 0.0), "silence at {depth} is not silent");
        let out = dir.join(format!("t{name}"));
        convert(&tone, &out, &["--bit-depth", depth]);
        assert_eq!(steps16(&out), want, "tone at {depth} changed");
    }
    // The default output depth (24-bit) too.
    let out = dir.join("default.wav");
    convert(&tone, &out, &[]);
    assert_eq!(steps16(&out), want);

    // Asked for, dither is still applied.
    let out = dir.join("forced.wav");
    convert(&silence, &out, &["--bit-depth", "16", "--dither"]);
    assert!(steps16(&out).iter().any(|s| *s != 0.0), "--dither did not dither");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn convert_dithers_when_reducing_depth_unless_told_not_to() {
    let dir = tmpdir("reduce");
    let src = dir.join("src24.wav");
    // A 24-bit file with detail below one 16-bit step.
    let mut buf = AudioBuffer::new(48_000, 1, 4800);
    for (i, s) in buf.channels[0].iter_mut().enumerate() {
        *s = 0.3 / 32768.0 * (i as f32 * 0.01).sin();
    }
    let opts = EncodeOptions { format: FileFormat::Wav, bit_depth: BitDepth::Int24, dither: false, bwf: None };
    std::fs::write(&src, soundcraft_audio_io::encode(&buf, &opts).unwrap()).unwrap();

    let dithered = dir.join("d16.wav");
    convert(&src, &dithered, &["--bit-depth", "16"]);
    assert!(steps16(&dithered).iter().any(|s| *s != 0.0), "24 to 16-bit was not dithered");
    // Plain rounding: everything is below half a step, so it all rounds to zero.
    let plain = dir.join("p16.wav");
    convert(&src, &plain, &["--bit-depth", "16", "--no-dither"]);
    assert!(steps16(&plain).iter().all(|s| *s == 0.0), "--no-dither still dithered");
    let _ = std::fs::remove_dir_all(&dir);
}
