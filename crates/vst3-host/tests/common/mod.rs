//! Shared by the integration tests: builds the fixture plugin (a cdylib outside the workspace)
//! and installs it as a `.vst3` bundle in a temp folder.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

/// Builds the fixture once and returns a folder containing `Fixture.vst3`.
pub fn fixture_dir() -> &'static Path {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| {
        let manifest = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixture/Cargo.toml");
        let target = Path::new(env!("CARGO_TARGET_TMPDIR")).join("vst3-fixture-target");
        let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
        let st = Command::new(cargo)
            .args(["build", "--quiet", "--manifest-path"])
            .arg(&manifest)
            .arg("--target-dir")
            .arg(&target)
            .env_remove("RUSTFLAGS")
            .status()
            .expect("run cargo");
        assert!(st.success(), "building the VST3 fixture failed");
        let (lib, folder, exe) = if cfg!(target_os = "macos") {
            ("libvst3_fixture.dylib", "MacOS", "Fixture")
        } else if cfg!(windows) {
            ("vst3_fixture.dll", "x86_64-win", "Fixture.vst3")
        } else {
            ("libvst3_fixture.so", if cfg!(target_arch = "aarch64") { "aarch64-linux" } else { "x86_64-linux" }, "Fixture.so")
        };
        let built = target.join("debug").join(lib);
        let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("vst3-fixture-install-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        // Install as a real bundle to exercise bundle resolution and the module entry.
        let bin = dir.join("vendor/Fixture.vst3/Contents").join(folder);
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::copy(&built, bin.join(exe)).unwrap();
        assert_eq!(soundcraft_vst3_host::add_search_dir(&dir), 6);
        dir
    })
}

/// Path of the installed fixture library (call after `fixture_dir`).
#[allow(dead_code)]
fn fixture_library() -> PathBuf {
    let dir = fixture_dir();
    if cfg!(target_os = "macos") {
        dir.join("vendor/Fixture.vst3/Contents/MacOS/Fixture")
    } else if cfg!(windows) {
        dir.join("vendor/Fixture.vst3/Contents/x86_64-win/Fixture.vst3")
    } else {
        let arch = if cfg!(target_arch = "aarch64") { "aarch64-linux" } else { "x86_64-linux" };
        dir.join("vendor/Fixture.vst3/Contents").join(arch).join("Fixture.so")
    }
}

/// The fixture's `setActive`/`setProcessing` calls made on the calling thread since the last
/// call, oldest first. The host already loaded the library; opening it again shares that copy.
#[allow(unsafe_code, dead_code)] // test-only symbol lookup in the fixture; not every test binary uses it
pub fn take_fixture_calls() -> Vec<String> {
    type TakeLog = unsafe extern "C" fn(*mut u8, usize) -> usize;
    let lib = unsafe { libloading::Library::new(fixture_library()) }.expect("open fixture library");
    let take: libloading::Symbol<TakeLog> = unsafe { lib.get(b"fixture_take_call_log\0") }.expect("fixture_take_call_log symbol");
    let mut buf = vec![0u8; 4096];
    let n = unsafe { take(buf.as_mut_ptr(), buf.len()) };
    let text = String::from_utf8(buf[..n].to_vec()).expect("utf8");
    if text.is_empty() { Vec::new() } else { text.split(',').map(str::to_owned).collect() }
}
