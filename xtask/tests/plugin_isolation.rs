#![cfg(unix)]
#![allow(clippy::unwrap_used)]

use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

struct Probe {
    directory: PathBuf,
    cargo: PathBuf,
    log: PathBuf,
}

impl Probe {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let directory = std::env::temp_dir().join(format!("soundcraft-ci-policy-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        std::fs::create_dir(&directory).unwrap();
        let cargo = directory.join("cargo");
        let log = directory.join("calls");
        // This owned shim records the real xtask's child environment without loading any plugins.
        std::fs::write(
            &cargo,
            r#"#!/bin/sh
{
printf 'args=%s\n' "$*"
printf 'clap=%s\n' "${SOUNDCRAFT_CLAP_SEARCH_PATHS-unset}"
printf 'vst3=%s\n' "${SOUNDCRAFT_VST3_SEARCH_PATHS-unset}"
printf 'au=%s\n' "${SOUNDCRAFT_AU_MANUFACTURERS-unset}"
printf 'clap_smoke=%s\n' "${SOUNDCRAFT_CLAP_SMOKE-unset}"
printf 'vst3_smoke=%s\n' "${SOUNDCRAFT_VST3_SMOKE-unset}"
printf 'au_smoke=%s\n' "${SOUNDCRAFT_AU_SMOKE-unset}"
printf 'legacy_clap=%s\n' "${CLAP_PATH-unset}"
printf 'legacy_vst3=%s\n' "${VST3_PATH-unset}"
printf 'end\n'
} >> "$SOUNDCRAFT_CI_PROBE_LOG"
if [ "${SOUNDCRAFT_CI_PROBE_FAIL-}" = "$1" ]; then exit 23; fi
if [ "$1" = metadata ]; then printf '{"packages":[]}\n'; fi
"#,
        )
        .unwrap();
        std::fs::set_permissions(&cargo, std::fs::Permissions::from_mode(0o700)).unwrap();
        Self { directory, cargo, log }
    }

    fn command(&self, task: &str) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_xtask"));
        command
            .arg(task)
            .env("CARGO", &self.cargo)
            .env("SOUNDCRAFT_CI_PROBE_LOG", &self.log)
            .env("SOUNDCRAFT_CLAP_SEARCH_PATHS", "/caller/clap")
            .env("SOUNDCRAFT_VST3_SEARCH_PATHS", "/caller/vst3")
            .env("SOUNDCRAFT_AU_MANUFACTURERS", "test")
            .env("SOUNDCRAFT_CLAP_SMOKE", "1")
            .env("SOUNDCRAFT_VST3_SMOKE", "1")
            .env("SOUNDCRAFT_AU_SMOKE", "1")
            .env("CLAP_PATH", "/legacy/clap")
            .env("VST3_PATH", "/legacy/vst3")
            .env_remove("SOUNDCRAFT_CI_PROBE_FAIL");
        command
    }

    fn run(&self, task: &str) -> (String, String) {
        let output = self.command(task).output().unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        (String::from_utf8(output.stdout).unwrap(), std::fs::read_to_string(&self.log).unwrap())
    }
}

impl Drop for Probe {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}

#[test]
fn ci_overrides_discovery_and_removes_smoke_for_every_cargo_child() {
    let probe = Probe::new();
    let (stdout, calls) = probe.run("ci");
    let calls: Vec<&str> = calls.split("end\n").filter(|s| !s.is_empty()).collect();
    assert_eq!(calls.len(), 8);
    for (call, args) in calls.iter().zip([
        "fmt --all -- --check",
        "clippy --workspace --all-targets --keep-going -- -D warnings",
        "nextest --version",
        "metadata --format-version 1 --no-deps",
        "nextest run --workspace --lib --bins --tests",
        "test --workspace --doc",
        "metadata --format-version 1 --no-deps",
        "check --target wasm32-unknown-unknown",
    ]) {
        assert!(call.starts_with(&format!("args={args}")), "{call}");
        let lines: Vec<&str> = call.lines().collect();
        for expected in [
            "clap=",
            "vst3=",
            "au=appl",
            "clap_smoke=unset",
            "vst3_smoke=unset",
            "au_smoke=unset",
            "legacy_clap=/legacy/clap",
            "legacy_vst3=/legacy/vst3",
        ] {
            assert!(lines.contains(&expected), "missing {expected}: {call}");
        }
    }
    for gate in ["fmt", "clippy", "test", "doctest", "assets", "layers", "wasm"] {
        assert!(stdout.lines().any(|line| line == format!("ok    {gate}")), "{stdout}");
    }
}

#[test]
fn standalone_wasm_preserves_caller_discovery_and_smoke_settings() {
    let probe = Probe::new();
    let (_, calls) = probe.run("wasm");
    assert_eq!(calls.matches("end\n").count(), 1);
    let lines: Vec<&str> = calls.lines().collect();
    for expected in [
        "clap=/caller/clap",
        "vst3=/caller/vst3",
        "au=test",
        "clap_smoke=1",
        "vst3_smoke=1",
        "au_smoke=1",
        "legacy_clap=/legacy/clap",
        "legacy_vst3=/legacy/vst3",
    ] {
        assert!(lines.contains(&expected), "missing {expected}: {calls}");
    }
}

#[test]
fn ci_stops_at_a_failed_gate_without_reporting_success() {
    let probe = Probe::new();
    let output = probe.command("ci").env("SOUNDCRAFT_CI_PROBE_FAIL", "clippy").output().unwrap();
    assert!(!output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert_eq!(stdout.lines().collect::<Vec<_>>(), ["ok    fmt", "FAIL  clippy"]);
    assert!(String::from_utf8_lossy(&output.stderr).contains("cargo clippy --workspace --all-targets --keep-going -- -D warnings failed"));
    let calls = std::fs::read_to_string(&probe.log).unwrap();
    assert_eq!(calls.matches("end\n").count(), 2);
    assert!(!calls.lines().any(|line| line.starts_with("args=test") || line.starts_with("args=check")));
}
