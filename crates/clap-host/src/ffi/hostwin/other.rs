use clap_sys::ext::gui::clap_window_handle;
use std::ffi::c_void;

pub const SUPPORTED: bool = false;

pub struct HostWindow;

impl HostWindow {
    pub fn new(_title: &str, _w: f64, _h: f64) -> Result<HostWindow, String> {
        Err("embedded plugin editors are not hosted on this platform".into())
    }
    pub fn raw(&self) -> *mut c_void {
        std::ptr::null_mut()
    }
    pub fn handle(&self) -> clap_window_handle {
        clap_window_handle { ptr: std::ptr::null_mut() }
    }
    pub fn raise(&self) {}
    pub fn is_visible(&self) -> bool {
        false
    }
    pub fn close(self) {}
}

/// The largest client area a new window should get (no host windows on this platform).
pub fn fit_client(w: f64, h: f64) -> (f64, f64) {
    (w, h)
}

pub fn resize(_window: *mut c_void, w: f64, h: f64) -> (f64, f64) {
    (w, h)
}
