use clap_sys::ext::gui::clap_window_handle;
use std::ffi::c_void;
use std::sync::OnceLock;
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromPoint, MonitorFromWindow};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::GetActiveWindow;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    AdjustWindowRectEx, CS_HREDRAW, CS_VREDRAW, CreateWindowExW, DefWindowProcW, DestroyWindow, GWL_EXSTYLE, GWL_STYLE, GetClientRect, GetCursorPos,
    GetWindowLongPtrW, GetWindowRect, IDC_ARROW, IsWindow, IsWindowVisible, LoadCursorW, RegisterClassExW, SW_HIDE, SW_SHOW, SWP_NOACTIVATE,
    SWP_NOZORDER, SetForegroundWindow, SetWindowPos, ShowWindow, WM_CLOSE, WNDCLASSEXW, WS_CAPTION, WS_CLIPCHILDREN, WS_OVERLAPPED, WS_SYSMENU,
};

pub const SUPPORTED: bool = true;

/// Rust names the process's first thread "main"; the app runs its UI (and message loop) there.
fn on_main_thread() -> bool {
    std::thread::current().name() == Some("main")
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().filter(|&c| c != 0).chain(Some(0)).collect()
}

const STYLE: u32 = WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_CLIPCHILDREN;

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if msg == WM_CLOSE {
        // SAFETY: `hwnd` is the window this procedure was called for.
        unsafe { ShowWindow(hwnd, SW_HIDE) };
        return 0;
    }
    // SAFETY: default handling of a message for the window it was sent to.
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

/// Registers the window class once; the name lives for the whole process.
fn class_name() -> Option<&'static [u16]> {
    static CLASS: OnceLock<Option<Vec<u16>>> = OnceLock::new();
    CLASS
        .get_or_init(|| {
            let name = wide("SoundCraftClapEditor");
            // SAFETY: null asks for this process's module handle.
            let instance = unsafe { GetModuleHandleW(std::ptr::null()) };
            let wc = WNDCLASSEXW {
                cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
                style: CS_HREDRAW | CS_VREDRAW,
                lpfnWndProc: Some(wndproc),
                cbClsExtra: 0,
                cbWndExtra: 0,
                hInstance: instance,
                hIcon: std::ptr::null_mut(),
                // SAFETY: a system cursor (null instance + predefined id).
                hCursor: unsafe { LoadCursorW(std::ptr::null_mut(), IDC_ARROW) },
                hbrBackground: std::ptr::null_mut(),
                lpszMenuName: std::ptr::null(),
                lpszClassName: name.as_ptr(),
                hIconSm: std::ptr::null_mut(),
            };
            // SAFETY: a fully initialised class description whose strings outlive the call
            // (the name is kept for the process lifetime in this `OnceLock`).
            (unsafe { RegisterClassExW(&wc) } != 0).then_some(name)
        })
        .as_deref()
}

/// Outer window size for a client area of `w`×`h`.
fn outer_size(style: u32, ex_style: u32, w: f64, h: f64) -> (i32, i32) {
    let mut r = RECT { left: 0, top: 0, right: w as i32, bottom: h as i32 };
    // SAFETY: `r` is a valid, writable rect.
    unsafe { AdjustWindowRectEx(&mut r, style, 0, ex_style) };
    (r.right.saturating_sub(r.left), r.bottom.saturating_sub(r.top))
}

/// Owns the window handle; destroyed in `close` (main thread only).
pub struct HostWindow {
    hwnd: HWND,
}

// SAFETY: the handle is only used on the main thread (every method checks); the struct
// merely carries it between threads' locks.
unsafe impl Send for HostWindow {}

impl HostWindow {
    fn live(&self) -> Option<HWND> {
        // SAFETY: `IsWindow` accepts any value and only reports whether it names a window.
        (on_main_thread() && unsafe { IsWindow(self.hwnd) } != 0).then_some(self.hwnd)
    }

    pub fn new(title: &str, w: f64, h: f64) -> Result<HostWindow, String> {
        if !on_main_thread() {
            return Err("plugin editors must be opened on the main thread".into());
        }
        let class = class_name().ok_or_else(|| "cannot register the editor window class".to_string())?;
        let title = wide(title);
        let (ow, oh) = outer_size(STYLE, 0, w, h);
        // #1448: land inside the work area of the monitor under the pointer — the slot that was
        // clicked — rather than wherever Windows happens to cascade the window. The size stays
        // what the caller negotiated with the GUI; shrinking here would clip its controls.
        let (x, y) = opening_pos(ow, oh);
        // SAFETY: main thread; the active window (or null) of this thread's queue.
        let owner = unsafe { GetActiveWindow() };
        // SAFETY: registered class and NUL-terminated strings that outlive the call; the
        // owner is null or a window of this thread.
        let hwnd = unsafe {
            CreateWindowExW(
                0,
                class.as_ptr(),
                title.as_ptr(),
                STYLE,
                x,
                y,
                ow,
                oh,
                owner,
                std::ptr::null_mut(),
                GetModuleHandleW(std::ptr::null()),
                std::ptr::null(),
            )
        };
        if hwnd.is_null() {
            return Err("cannot create the editor window".into());
        }
        Ok(HostWindow { hwnd })
    }

    /// The window handle (for resizes).
    pub fn raw(&self) -> *mut c_void {
        self.hwnd
    }

    /// The `HWND` the plugin creates its child window in.
    pub fn parent(&self) -> *mut c_void {
        self.live().unwrap_or(std::ptr::null_mut())
    }

    pub fn handle(&self) -> clap_window_handle {
        clap_window_handle { win32: self.parent() }
    }

    pub fn raise(&self) {
        if let Some(h) = self.live() {
            // SAFETY: a live window of this thread.
            unsafe {
                ShowWindow(h, SW_SHOW);
                SetForegroundWindow(h);
            }
        }
    }

    pub fn is_visible(&self) -> bool {
        // SAFETY: a live window of this thread.
        self.live().is_some_and(|h| unsafe { IsWindowVisible(h) } != 0)
    }

    pub fn close(self) {
        if let Some(h) = self.live() {
            // SAFETY: our own live window; the plugin's GUI was destroyed first.
            unsafe { DestroyWindow(h) };
        }
    }
}

/// Resizes a host window's client area (`window` from [`HostWindow::raw`]); main thread only.
/// The largest client area that fits the work area of the monitor under the pointer, so a caller
/// can negotiate a smaller GUI size before the window exists (#1448). The window chrome is ours,
/// so it is subtracted from the work area rather than clamped.
pub fn fit_client(w: f64, h: f64) -> (f64, f64) {
    let mut pt = POINT::default();
    // SAFETY: `pt` is valid, writable memory; a failed read still yields a valid work area.
    let _ = unsafe { GetCursorPos(&mut pt) };
    let work = work_area_at(pt);
    let (cw, ch) = outer_size(STYLE, 0, 0.0, 0.0);
    let fw = (work.right - work.left - cw).max(1);
    let fh = (work.bottom - work.top - ch).max(1);
    (w.min(f64::from(fw)), h.min(f64::from(fh)))
}

/// A host window's current client size (`window` from [`HostWindow::raw`]); main thread only.
pub fn client_size(window: *mut c_void) -> Option<(f64, f64)> {
    if !on_main_thread() || window.is_null() {
        return None;
    }
    let mut r = RECT::default();
    // SAFETY: `IsWindow` accepts any value; `r` is valid, writable memory.
    (unsafe { IsWindow(window) } != 0 && unsafe { GetClientRect(window, &mut r) } != 0)
        .then(|| (f64::from(r.right - r.left), f64::from(r.bottom - r.top)))
}

/// Resizes a host window's client area to the negotiated `w`×`h` (`window` from
/// [`HostWindow::raw`]); main thread only. The size the caller negotiated with the plugin is
/// kept — even when it is bigger than the work area, where the window is moved so its title bar
/// stays reachable and its clipped side can be dragged into view — and the client size actually
/// applied comes back so the caller can tell the plugin the agreed size (#1448).
pub fn resize(window: *mut c_void, w: f64, h: f64) -> (f64, f64) {
    // SAFETY: `IsWindow` accepts any value.
    if !on_main_thread() || window.is_null() || unsafe { IsWindow(window) } == 0 {
        return (w, h);
    }
    // SAFETY: a live window of this thread; style values are read, not changed.
    let (style, ex) = unsafe { (GetWindowLongPtrW(window, GWL_STYLE) as u32, GetWindowLongPtrW(window, GWL_EXSTYLE) as u32) };
    let (ow, oh) = outer_size(style, ex, w, h);
    let mut rect = RECT::default();
    // SAFETY: a live window of this thread; `rect` is valid, writable memory.
    if unsafe { GetWindowRect(window, &mut rect) } == 0 {
        return (w, h);
    }
    let (x, y) = place_inside(rect.left, rect.top, ow, oh, work_area_of(window));
    // SAFETY: as above; the window keeps its negotiated size and stays reachable.
    unsafe { SetWindowPos(window, std::ptr::null_mut(), x, y, ow, oh, SWP_NOZORDER | SWP_NOACTIVATE) };
    // Windows can cap a window's extent: report the client size it actually applied.
    client_size(window).unwrap_or((w, h))
}

/// The work area (screen minus taskbar and docked bars) of the monitor nearest `pt`.
fn work_area_at(pt: POINT) -> RECT {
    let unclamped = RECT { left: i32::MIN / 2, top: i32::MIN / 2, right: i32::MAX / 2, bottom: i32::MAX / 2 };
    // SAFETY: `monitor` is null or a monitor handle; `info` is valid, writable memory.
    unsafe {
        let monitor = MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST);
        let mut info: MONITORINFO = std::mem::zeroed();
        info.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        if monitor.is_null() || GetMonitorInfoW(monitor, &mut info) == 0 { unclamped } else { info.rcWork }
    }
}

/// The work area of the monitor `window` is mostly on.
fn work_area_of(window: HWND) -> RECT {
    let unclamped = RECT { left: i32::MIN / 2, top: i32::MIN / 2, right: i32::MAX / 2, bottom: i32::MAX / 2 };
    // SAFETY: as above.
    unsafe {
        let monitor = MonitorFromWindow(window, MONITOR_DEFAULTTONEAREST);
        let mut info: MONITORINFO = std::mem::zeroed();
        info.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        if monitor.is_null() || GetMonitorInfoW(monitor, &mut info) == 0 { unclamped } else { info.rcWork }
    }
}

/// Where a `w`×`h` window opens: at the pointer inside that monitor's work area; centered on the
/// primary monitor's work area when the pointer cannot be read (#1448).
fn opening_pos(w: i32, h: i32) -> (i32, i32) {
    let mut pt = POINT { x: 0, y: 0 };
    // SAFETY: `pt` is valid, writable memory.
    if unsafe { GetCursorPos(&mut pt) } != 0 {
        return place_inside(pt.x, pt.y, w, h, work_area_at(pt));
    }
    let work = work_area_at(pt);
    let x = work.left + ((work.right - work.left - w) / 2).max(0);
    let y = work.top + ((work.bottom - work.top - h) / 2).max(0);
    place_inside(x, y, w, h, work)
}

/// A `w`×`h` window's top-left inside `work` (#1448): fully inside when it fits; at the work
/// area's own top-left when it is bigger than the monitor — the title bar stays reachable either
/// way, so an oversized editor's controls can be dragged into view rather than clipped away.
fn place_inside(x: i32, y: i32, w: i32, h: i32, work: RECT) -> (i32, i32) {
    let x = x.clamp(work.left, work.left.max(work.right - w));
    let y = y.clamp(work.top, work.top.max(work.bottom - h));
    (x, y)
}

#[cfg(test)]
mod tests {
    use super::*;

    const WORK: RECT = RECT { left: -1920, top: 0, right: 0, bottom: 1040 };

    #[test]
    fn place_inside_keeps_oversized_windows_draggable() {
        // A window that cannot shrink keeps its size and puts its title bar at the work area's
        // top-left, so its clipped-side controls are reached by dragging, not lost (#1448).
        assert_eq!(place_inside(500, 500, 4000, 2000, WORK), (-1920, 0));
        // A fitting window still lands fully inside near the pointer.
        assert_eq!(place_inside(-50, 1000, 400, 300, WORK), (-400, 740));
    }

    #[test]
    fn new_window_lands_inside_the_work_area() {
        std::thread::Builder::new()
            .name("main".into())
            .spawn(|| {
                let host = HostWindow::new("editor", 400.0, 300.0).unwrap();
                let mut r = RECT::default();
                // SAFETY: `host` is a live window on this thread; `r` is valid, writable memory.
                assert!(unsafe { GetWindowRect(host.raw() as HWND, &mut r) } != 0);
                let work = work_area_of(host.raw() as HWND);
                assert!(r.left >= work.left && r.top >= work.top && r.right <= work.right && r.bottom <= work.bottom);
                host.close();
            })
            .unwrap()
            .join()
            .unwrap();
    }
}
