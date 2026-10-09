//! Plugin scanning through real `soundcraft-cli` scan children, one of which dies while loading.

use std::path::Path;
use std::process::Command;

#[test]
fn plugins_are_read_by_child_processes_that_may_die() {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../crates/clap-host/tests/fixture/Cargo.toml");
    let target = Path::new(env!("CARGO_TARGET_TMPDIR")).join("clap-fixture-target");
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let st = Command::new(cargo)
        .args(["build", "--quiet", "--manifest-path"])
        .arg(&manifest)
        .arg("--target-dir")
        .arg(&target)
        .env_remove("RUSTFLAGS")
        .status()
        .unwrap();
    assert!(st.success(), "building the CLAP fixture failed");
    let lib = if cfg!(target_os = "macos") {
        "libclap_fixture.dylib"
    } else if cfg!(windows) {
        "clap_fixture.dll"
    } else {
        "libclap_fixture.so"
    };
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("cli-plugin-scan-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    for name in ["Fixture", "CrashOnLoad"] {
        std::fs::copy(target.join("debug").join(lib), dir.join(format!("{name}.clap"))).unwrap();
    }

    let cache = dir.join("cache");
    soundcraft_plugin_scan::in_children(env!("CARGO_BIN_EXE_soundcraft-cli").into(), Some(cache.clone()));
    assert_eq!(soundcraft_clap_host::add_search_dir(&dir), 2);
    let failures = soundcraft_plugin_scan::failures();
    assert_eq!(failures.len(), 1, "{failures:?}");
    assert!(failures[0].1.ends_with("CrashOnLoad.clap"));
    assert!(failures[0].2.starts_with("ended without a result") && failures[0].2.contains("86"), "{failures:?}");
    assert!(cache.join("plugin-scan-clap.json").exists());
    assert!(soundcraft_clap_host::instantiate("clap:org.soundcraft.test.gain").is_ok());
    let _ = std::fs::remove_dir_all(&dir);
}
