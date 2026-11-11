//! ASIO driver helpers cpal lacks: [`open_control_panel`] opens the control panel of the driver
//! SoundCraft has loaded (where a driver's own settings live: ASIO4ALL's choice of devices, an
//! interface's buffer size, clock source...), and [`driver_names`] lists the installed drivers
//! without loading them.
//!
//! cpal plays through ASIO via `asio-sys`, which compiles the Steinberg ASIO SDK but does not bind
//! its `ASIOControlPanel()`. This crate declares that one SDK function and calls it; the SDK acts
//! on its single loaded driver, i.e. the one cpal opened. `unsafe` is denied crate-wide and allowed
//! only in the private `ffi` module; the public API is safe and never panics.
//!
//! Without the `asio` feature, or off Windows, [`open_control_panel`] reports that ASIO is not
//! available and [`driver_names`] is empty.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

#[cfg(all(windows, feature = "asio"))]
mod ffi;

/// Whether this build can open an ASIO control panel.
pub const AVAILABLE: bool = cfg!(all(windows, feature = "asio"));

/// Opens the loaded ASIO driver's control panel. Some drivers show it modally (this returns when
/// it closes), others as their own window (this returns at once). Drivers do not reliably tell
/// the host what changed: reopen the device afterwards to pick up a new buffer size.
pub fn open_control_panel() -> Result<(), String> {
    #[cfg(all(windows, feature = "asio"))]
    {
        match ffi::control_panel() {
            0 => Ok(()),
            ffi::ASE_NOT_PRESENT => Err("no ASIO driver is open".into()),
            code => Err(format!("the ASIO driver has no control panel (error {code})")),
        }
    }
    #[cfg(not(all(windows, feature = "asio")))]
    {
        Err("this build has no ASIO support".into())
    }
}

/// The installed ASIO drivers, read from the registry without loading any (empty without the
/// `asio` feature or off Windows). Listing devices through cpal loads each driver, which the SDK
/// cannot do while another driver is open, so those drivers would be missing from the list.
pub fn driver_names() -> Vec<String> {
    #[cfg(all(windows, feature = "asio"))]
    {
        asio_sys::Asio::new().driver_names()
    }
    #[cfg(not(all(windows, feature = "asio")))]
    {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn without_an_open_driver_it_is_an_error_not_a_crash() {
        let e = super::open_control_panel().unwrap_err();
        assert!(e.contains("no ASIO"), "{e}");
    }
}
