//! Where tests write their scratch files: beside the build, not in the system temp directory.
//!
//! Some systems mount `/tmp` on a small tmpfs (10 MB on a stock Deepin); a full test run writes
//! more than that, and the `ENOSPC` that follows looks like a bug in whatever was being tested —
//! autosaves silently stop landing, renders come out empty. `target/tmp` sits on the same disk as
//! the build and has room.

use std::path::{Path, PathBuf};

/// The scratch root for this test run; it exists once this returns.
///
/// The path is shaped like the ones the engine stores in sessions — absolute, no `..`, symlinks
/// resolved — because tests compare the two by string. Building it with `join("../../target/tmp")`
/// would leave a `crates/engine/../..` in it and fail every such comparison.
pub(crate) fn root() -> PathBuf {
    let base = option_env!("CARGO_TARGET_TMPDIR").map(PathBuf::from).unwrap_or_else(|| {
        // Unit tests run from `<target>/<profile>/deps/`, so three parents give the target
        // directory: this honours `CARGO_TARGET_DIR` and works from a read-only checkout.
        if let Some(target) = std::env::current_exe().ok().and_then(|exe| exe.parent()?.parent()?.parent().map(Path::to_path_buf)) {
            return target.join("tmp");
        }
        // Last resort: `<workspace>/target/tmp`, by walking up the tree.
        let krate = Path::new(env!("CARGO_MANIFEST_DIR"));
        krate.parent().and_then(Path::parent).map_or_else(|| krate.join("target/tmp"), |ws| ws.join("target/tmp"))
    });
    let _ = std::fs::create_dir_all(&base);
    std::fs::canonicalize(&base).unwrap_or(base)
}
