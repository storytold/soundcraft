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
    let base = option_env!("CARGO_TARGET_TMPDIR").map_or_else(
        || {
            // `<workspace>/crates/<crate>` → `<workspace>`, by walking up the tree; an external
            // `CARGO_TARGET_DIR` (relative ones are taken from the workspace) wins over `target`.
            let krate = Path::new(env!("CARGO_MANIFEST_DIR"));
            let ws = krate.parent().and_then(Path::parent).unwrap_or(krate);
            let target = std::env::var_os("CARGO_TARGET_DIR").map_or_else(|| ws.join("target"), |t| ws.join(t));
            target.join("tmp")
        },
        PathBuf::from,
    );
    let _ = std::fs::create_dir_all(&base);
    std::fs::canonicalize(&base).unwrap_or(base)
}
