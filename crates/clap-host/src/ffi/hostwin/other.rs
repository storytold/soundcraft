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
