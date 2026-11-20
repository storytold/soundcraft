use std::ffi::{c_char, c_void};

pub const PLATFORM_TYPE: Option<*const c_char> = None;

pub fn on_main_thread() -> bool {
    true
}

pub struct HostWindow;

impl HostWindow {
    pub fn new(_title: &str, _w: f64, _h: f64) -> Result<HostWindow, String> {
        Err("VST3 editors are not hosted on this platform yet".into())
    }
    pub fn raw(&self) -> *mut c_void {
        std::ptr::null_mut()
    }
    pub fn parent(&self) -> *mut c_void {
        std::ptr::null_mut()
    }
    pub fn raise(&self) {}
    pub fn is_visible(&self) -> bool {
        false
    }
    pub fn close(self) {}
}

pub fn resize(_window: *mut c_void, _w: f64, _h: f64) {}
