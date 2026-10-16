//! Builds only the original owned capacity fixture from this host's current source root.
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};
pub fn fixture_dir() -> &'static Path {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| {
        let manifest = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/capacity_fixture/Cargo.toml");
        let target = Path::new(env!("CARGO_TARGET_TMPDIR")).join("clap-capacity-fixture-target");
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
        let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("clap-capacity-install-{}-{nonce}", std::process::id()));
        // Never remove an existing path. This process owns the directory only after creation.
        std::fs::create_dir(&dir).expect("create unique owned fixture directory");
        let lib = if cfg!(target_os = "macos") {
            "libclap_capacity_fixture.dylib"
        } else if cfg!(windows) {
            "clap_capacity_fixture.dll"
        } else {
            "libclap_capacity_fixture.so"
        };
        let built = target.join("debug").join(lib);
        if cfg!(target_os = "macos") {
            let bin = dir.join("Capacity.clap/Contents/MacOS");
            std::fs::create_dir_all(&bin).unwrap();
            std::fs::copy(built, bin.join("Capacity")).unwrap();
        } else {
            std::fs::copy(built, dir.join("Capacity.clap")).unwrap();
        }
        assert_eq!(soundcraft_clap_host::add_search_dir(&dir), 1);
        dir
    })
}
