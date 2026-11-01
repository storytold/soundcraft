use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let sequence = NEXT.fetch_add(1, Ordering::Relaxed);
        let stamp = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        let path = std::env::temp_dir().join(format!("soundcraft-script-status-{}-{stamp}-{sequence}", std::process::id()));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn invoke(dir: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_soundcraft-cli")).current_dir(dir).env("SOUNDCRAFT_NO_PREFS", "1").args(args).output().unwrap()
}

#[test]
fn keep_going_runs_later_commands_and_finishes_but_reports_failure() {
    let dir = Scratch::new();
    std::fs::write(
        dir.0.join("audit.txt"),
        "audit.deliberately_invalid {}\ntrack.new {\"name\":\"After failure\",\"count\":1,\"format\":\"stereo\"}\n",
    )
    .unwrap();
    let stop = invoke(&dir.0, &["script", "audit.txt", "--save", "stopped.scraft"]);
    assert_eq!(stop.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&stop.stderr).contains("audit.deliberately_invalid"));
    assert!(!String::from_utf8_lossy(&stop.stdout).contains("track.new:"));
    assert!(!dir.0.join("stopped.scraft").exists());

    let keep = invoke(
        &dir.0,
        &["script", "audit.txt", "--keep-going", "--bounce", "after.wav", "--start", "0", "--end", "0.01", "--save", "after.scraft", "--inspect"],
    );
    assert_eq!(keep.status.code(), Some(1), "{}", String::from_utf8_lossy(&keep.stderr));
    assert!(String::from_utf8_lossy(&keep.stderr).contains("audit.deliberately_invalid"));
    let stdout = String::from_utf8_lossy(&keep.stdout);
    assert!(stdout.contains("track.new:") && stdout.contains("After failure"), "{stdout}");
    assert!(dir.0.join("after.wav").is_file());
    let info = invoke(&dir.0, &["info", "after.scraft"]);
    assert!(info.status.success(), "{}", String::from_utf8_lossy(&info.stderr));
    assert!(String::from_utf8_lossy(&info.stdout).contains("After failure"));
}

#[test]
fn successful_script_and_parsing_or_finish_failures_keep_their_status() {
    let dir = Scratch::new();
    std::fs::write(dir.0.join("good.txt"), "track.new {\"name\":\"Good\",\"count\":1,\"format\":\"stereo\"}\n").unwrap();
    let success = invoke(&dir.0, &["script", "good.txt", "--keep-going"]);
    assert!(success.status.success(), "{}", String::from_utf8_lossy(&success.stderr));

    std::fs::write(dir.0.join("bad.txt"), "track.new {bad\ntrack.new {\"name\":\"Too late\"}\n").unwrap();
    let malformed = invoke(&dir.0, &["script", "bad.txt", "--keep-going", "--save", "malformed.scraft"]);
    assert_eq!(malformed.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&malformed.stderr).contains("bad JSON"));
    assert!(!String::from_utf8_lossy(&malformed.stdout).contains("track.new:"));
    assert!(!dir.0.join("malformed.scraft").exists());

    std::fs::write(dir.0.join("parent"), "not a directory").unwrap();
    for (option, path) in [("--save", "parent/out.scraft"), ("--bounce", "parent/out.wav")] {
        let fail = invoke(&dir.0, &["script", "good.txt", "--keep-going", option, path]);
        assert_eq!(fail.status.code(), Some(1), "{option}: {}", String::from_utf8_lossy(&fail.stderr));
        assert!(String::from_utf8_lossy(&fail.stderr).contains("parent"));
    }

    std::fs::write(dir.0.join("engine.txt"), "session.save {}\ntrack.new {\"name\":\"After engine failure\",\"count\":1}\n").unwrap();
    let fail = invoke(&dir.0, &["script", "engine.txt", "--keep-going", "--save", "engine.scraft"]);
    assert_eq!(fail.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&fail.stderr).contains("session.save"));
    assert!(String::from_utf8_lossy(&fail.stdout).contains("track.new:"));
    assert!(dir.0.join("engine.scraft").is_file());

    let both_fail = invoke(&dir.0, &["script", "engine.txt", "--keep-going", "--save", "parent/out.scraft"]);
    assert_eq!(both_fail.status.code(), Some(1));
    let errors = String::from_utf8_lossy(&both_fail.stderr);
    assert!(errors.contains("session.save") && errors.contains("parent"), "{errors}");
    assert!(!errors.contains("script finished with command errors"), "{errors}");
}
