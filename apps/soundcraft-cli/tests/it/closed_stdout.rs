//! A closed stdout must not abort `run` or `script` before `--bounce` and `--save`.

use std::process::{Command, Stdio};

fn bin() -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_soundcraft-cli"));
    cmd.env("SOUNDCRAFT_NO_PREFS", "1");
    cmd
}

/// Drop the read end so the child's stdout writes fail with `EPIPE`.
fn closed_stdout() -> std::process::Stdio {
    let (reader, writer) = std::io::pipe().expect("pipe");
    drop(reader);
    Stdio::from(writer)
}

fn temp_dir(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("sc-cli-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn run_with_closed_stdout_still_bounces_and_saves() {
    let dir = temp_dir("run");
    let wav = dir.join("mix.wav");
    let session = dir.join("Synthetic.scraft");
    let output = bin()
        .arg("run")
        .arg("--cmd")
        .arg(r#"track.new={"name":"Synthetic","count":1,"format":"stereo"}"#)
        .arg("--bounce")
        .arg(&wav)
        .arg("--save")
        .arg(&session)
        .arg("--start")
        .arg("0")
        .arg("--end")
        .arg("0.01")
        .stdout(closed_stdout())
        .stderr(Stdio::piped())
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    assert!(wav.is_file(), "bounce was skipped");
    assert!(session.is_file(), "save was skipped");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn script_with_closed_stdout_still_bounces_and_saves() {
    let dir = temp_dir("script");
    let script = dir.join("edit.scraft.txt");
    std::fs::write(&script, "track.new {\"name\":\"Synthetic\",\"count\":1,\"format\":\"stereo\"}\n").unwrap();
    let wav = dir.join("mix.wav");
    let session = dir.join("Synthetic.scraft");
    let output = bin()
        .arg("script")
        .arg(&script)
        .arg("--bounce")
        .arg(&wav)
        .arg("--save")
        .arg(&session)
        .arg("--start")
        .arg("0")
        .arg("--end")
        .arg("0.01")
        .stdout(closed_stdout())
        .stderr(Stdio::piped())
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    assert!(wav.is_file());
    assert!(session.is_file());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn bounce_then_save_with_closed_stdout_writes_the_wav() {
    let dir = temp_dir("bounce");
    let wav = dir.join("mix.wav");
    let session = dir.join("Empty.scraft");
    let output = bin()
        .arg("run")
        .arg("--bounce")
        .arg(&wav)
        .arg("--save")
        .arg(&session)
        .arg("--start")
        .arg("0")
        .arg("--end")
        .arg("0.01")
        .stdout(closed_stdout())
        .stderr(Stdio::piped())
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    assert!(wav.is_file());
    assert!(session.is_file());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn invalid_bounce_destination_still_fails() {
    let dir = temp_dir("bad-bounce");
    let missing = dir.join("no-such-dir").join("mix.wav");
    let output = bin()
        .arg("run")
        .arg("--bounce")
        .arg(&missing)
        .arg("--start")
        .arg("0")
        .arg("--end")
        .arg("0.01")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .unwrap();
    assert!(!output.status.success(), "a bounce that cannot be written must fail");
    assert!(!missing.exists());
    let _ = std::fs::remove_dir_all(&dir);
}
