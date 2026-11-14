//! Test helpers shared by the `soundcraft-cli` integration test binaries.

use std::ffi::OsString;

/// Nextest archives relocate binaries; plain Cargo keeps the compile-time path valid.
pub fn soundcraft_cli_binary() -> OsString {
    std::env::var_os("NEXTEST_BIN_EXE_soundcraft_cli")
        .or_else(|| std::env::var_os("NEXTEST_BIN_EXE_soundcraft-cli"))
        .unwrap_or_else(|| env!("CARGO_BIN_EXE_soundcraft-cli").into())
}
