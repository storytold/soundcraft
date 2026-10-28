//! Unknown options and missing option values must be reported, not silently skipped (#549).
// Integration tests: unwrapping and panicking on failure is fine here, unlike in shipped code (AGENTS.md).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn bin() -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_soundcraft-cli"));
    cmd.env("SOUNDCRAFT_NO_PREFS", "1");
    cmd
}

fn run(args: &[&str]) -> Output {
    bin().args(args).output().unwrap()
}

fn tmpdir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("sc-cli-options-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A 0.1 s mono 48 kHz 16-bit WAV, written by hand so the test needs no fixture.
fn tone(dir: &Path) -> PathBuf {
    let frames: u32 = 4800;
    let data: Vec<u8> = (0..frames).flat_map(|_| [0x00, 0x10]).collect();
    let (bytes, rate, ch, bits) = (data.len() as u32, 48_000u32, 1u16, 16u16);
    let mut wav = Vec::new();
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + bytes).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&ch.to_le_bytes());
    wav.extend_from_slice(&rate.to_le_bytes());
    wav.extend_from_slice(&(rate * u32::from(ch) * u32::from(bits) / 8).to_le_bytes());
    wav.extend_from_slice(&(ch * bits / 8).to_le_bytes());
    wav.extend_from_slice(&bits.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&bytes.to_le_bytes());
    wav.extend_from_slice(&data);
    let path = dir.join("tone.wav");
    std::fs::write(&path, wav).unwrap();
    path
}

fn sample_format(cli_info: &str) -> String {
    serde_json::from_str::<serde_json::Value>(cli_info).unwrap()["sample_format"].as_str().unwrap().to_owned()
}

#[test]
fn convert_rejects_an_unknown_option_instead_of_writing_the_file() {
    let dir = tmpdir("convert-unknown");
    let input = tone(&dir);
    let out = dir.join("out.wav");
    for bad in ["--bitdepth", "--normalise", "--bogus"] {
        let o = run(&["convert", input.to_str().unwrap(), out.to_str().unwrap(), bad]);
        assert!(!o.status.success(), "{bad} should fail");
        assert!(
            String::from_utf8_lossy(&o.stderr).contains(&format!("unexpected argument '{bad}' found")),
            "{bad}: {}",
            String::from_utf8_lossy(&o.stderr)
        );
        assert!(!out.exists(), "{bad} should not write the output");
    }
}

#[test]
fn convert_rejects_an_option_whose_value_is_missing() {
    let dir = tmpdir("convert-novalue");
    let input = tone(&dir);
    let out = dir.join("out.wav");
    for bad in ["--rate", "--bit-depth"] {
        let o = run(&["convert", input.to_str().unwrap(), out.to_str().unwrap(), bad]);
        assert!(!o.status.success(), "{bad} should fail");
        assert!(
            String::from_utf8_lossy(&o.stderr).contains(&format!("a value is required for '{bad} ")),
            "{bad}: {}",
            String::from_utf8_lossy(&o.stderr)
        );
        assert!(!out.exists(), "{bad} should not write the output");
    }
}

#[test]
fn convert_accepts_the_inline_value_form() {
    let dir = tmpdir("convert-inline");
    let input = tone(&dir);
    let out = dir.join("out.wav");
    let o = run(&["convert", input.to_str().unwrap(), out.to_str().unwrap(), "--bit-depth=32f"]);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let info = run(&["info", out.to_str().unwrap()]);
    assert_eq!(sample_format(&String::from_utf8_lossy(&info.stdout)), "Float32");
}

#[test]
fn convert_still_accepts_the_documented_space_separated_form() {
    let dir = tmpdir("convert-space");
    let input = tone(&dir);
    let out = dir.join("out.wav");
    let o = run(&["convert", input.to_str().unwrap(), out.to_str().unwrap(), "--bit-depth", "16"]);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let info = run(&["info", out.to_str().unwrap()]);
    assert_eq!(sample_format(&String::from_utf8_lossy(&info.stdout)), "Int16");
}

#[test]
fn run_rejects_a_misspelled_or_value_less_bounce() {
    let dir = tmpdir("run-bounce");
    let out = dir.join("x.wav");
    let o = run(&["run", "--cmd", "session.new", "--bounce"]);
    assert!(!o.status.success());
    assert!(String::from_utf8_lossy(&o.stderr).contains("a value is required for '--bounce <OUT>'"), "{}", String::from_utf8_lossy(&o.stderr));

    let o = run(&["run", "--cmd", "session.new", "--bounces", out.to_str().unwrap()]);
    assert!(!o.status.success());
    assert!(String::from_utf8_lossy(&o.stderr).contains("unexpected argument '--bounces' found"), "{}", String::from_utf8_lossy(&o.stderr));
    assert!(!out.exists(), "nothing should be bounced");
}

#[test]
fn run_bounces_with_the_inline_value_form() {
    let dir = tmpdir("run-inline");
    let out = dir.join("y.wav");
    let o = run(&["run", "--cmd", "session.new", &format!("--bounce={}", out.display())]);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    assert!(out.exists(), "--bounce=PATH should write the file");
}

#[test]
fn run_executes_the_inline_cmd_form() {
    let o = run(&["run", "--cmd=track.new", "--cmd", "track.new"]);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let stdout = String::from_utf8_lossy(&o.stdout);
    assert_eq!(stdout.lines().filter(|l| l.starts_with("track.new: ")).count(), 2, "both --cmd forms should run: {stdout}");
}
