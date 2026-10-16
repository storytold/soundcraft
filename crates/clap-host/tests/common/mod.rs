//! Shared by the integration tests: builds the fixture plugin (a cdylib outside the workspace)
//! and installs it in a temp folder.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

/// Builds the fixture once and returns a folder containing `Fixture.clap`.
pub fn fixture_dir() -> &'static Path {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| {
        let manifest = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixture/Cargo.toml");
        let target = Path::new(env!("CARGO_TARGET_TMPDIR")).join("clap-fixture-target");
        let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
        let st = Command::new(cargo)
            .args(["build", "--quiet", "--manifest-path"])
            .arg(&manifest)
            .arg("--target-dir")
            .arg(&target)
            .env_remove("RUSTFLAGS")
            .status()
            .expect("run cargo");
        assert!(st.success(), "building the CLAP fixture failed");
        let lib = if cfg!(target_os = "macos") {
            "libclap_fixture.dylib"
        } else if cfg!(windows) {
            "clap_fixture.dll"
        } else {
            "libclap_fixture.so"
        };
        let built = target.join("debug").join(lib);
        let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("clap-fixture-install-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        if cfg!(target_os = "macos") {
            // Install as a real macOS bundle to exercise bundle resolution.
            let exe = dir.join("vendor/Fixture.clap/Contents/MacOS");
            std::fs::create_dir_all(&exe).unwrap();
            std::fs::copy(&built, exe.join("Fixture")).unwrap();
        } else {
            std::fs::create_dir_all(dir.join("vendor")).unwrap();
            std::fs::copy(&built, dir.join("vendor/Fixture.clap")).unwrap();
        }
        soundcraft_plugin_scan::in_process();
        assert_eq!(soundcraft_clap_host::add_search_dir(&dir), 2);
        dir
    })
}
