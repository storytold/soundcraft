//! The one ASIO SDK function asio-sys does not bind. The SDK is C++ compiled by asio-sys's build
//! script into its static `asio.lib`, so the symbol carries MSVC C++ mangling (the same scheme
//! asio-sys's generated bindings use, e.g. `?ASIOStart@@YAJXZ` for `ASIOError ASIOStart(void)`).
#![allow(unsafe_code)]

// Links asio-sys (and with it the SDK's static library) into the final binary.
use asio_sys as _;

/// `ASE_NotPresent`: no driver loaded, or the hardware is gone.
pub const ASE_NOT_PRESENT: i32 = -1000;

unsafe extern "C" {
    /// `ASIOError ASIOControlPanel(void)` from the SDK's `asio.cpp`.
    #[link_name = "\u{1}?ASIOControlPanel@@YAJXZ"]
    fn ASIOControlPanel() -> i32;
}

pub fn control_panel() -> i32 {
    // SAFETY: no arguments; the SDK checks its loaded-driver pointer and returns ASE_NotPresent
    // when there is none. Like every ASIO host, this runs on the UI thread while the driver runs.
    unsafe { ASIOControlPanel() }
}
