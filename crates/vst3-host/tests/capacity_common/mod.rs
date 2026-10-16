//! Builds only the original owned capacity fixture from this host's current source root.
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};
pub fn fixture_dir() -> &'static Path {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| {
        let manifest = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/capacity_fixture/Cargo.toml");
        let target = Path::new(env!("CARGO_TARGET_TMPDIR")).join("vst3-capacity-fixture-target");
        let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
        let status = Command::new(cargo)
            .args(["build", "--quiet", "--offline", "--locked", "--jobs", "1", "--manifest-path"])
            .arg(&manifest)
            .arg("--target-dir")
            .arg(&target)
            .env_remove("RUSTFLAGS")
            .status()
            .expect("build controlled fixture");
        assert!(status.success(), "capacity fixture build failed");
        let nonce = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("vst3-capacity-install-{}-{nonce}", std::process::id()));
        // Never remove an existing path. This process owns the directory only after creation.
        std::fs::create_dir(&dir).expect("create unique owned fixture directory");
        let (lib, folder, executable) = if cfg!(target_os = "macos") {
            ("libvst3_capacity_fixture.dylib", "MacOS", "Capacity")
        } else if cfg!(windows) {
            ("vst3_capacity_fixture.dll", "x86_64-win", "Capacity.vst3")
        } else {
            ("libvst3_capacity_fixture.so", if cfg!(target_arch = "aarch64") { "aarch64-linux" } else { "x86_64-linux" }, "Capacity.so")
        };
        let bin = dir.join("Capacity.vst3/Contents").join(folder);
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::copy(target.join("debug").join(lib), bin.join(executable)).unwrap();
        assert_eq!(soundcraft_vst3_host::add_search_dir(&dir), 1);
        dir
    })
}
