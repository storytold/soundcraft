use std::ffi::c_void;
use std::sync::OnceLock;
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::GetActiveWindow;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    AdjustWindowRectEx, CS_HREDRAW, CS_VREDRAW, CW_USEDEFAULT, CreateWindowExW, DefWindowProcW, DestroyWindow, GWL_EXSTYLE, GWL_STYLE,
    GetWindowLongPtrW, IDC_ARROW, IsWindow, IsWindowVisible, LoadCursorW, RegisterClassExW, SW_HIDE, SW_SHOW, SWP_NOACTIVATE, SWP_NOMOVE,
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
                CW_USEDEFAULT,
                CW_USEDEFAULT,
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
pub fn resize(window: *mut c_void, w: f64, h: f64) {
    // SAFETY: `IsWindow` accepts any value.
    if !on_main_thread() || window.is_null() || unsafe { IsWindow(window) } == 0 {
        return;
    }
    // SAFETY: a live window of this thread; style values are read, not changed.
    let (style, ex) = unsafe { (GetWindowLongPtrW(window, GWL_STYLE) as u32, GetWindowLongPtrW(window, GWL_EXSTYLE) as u32) };
    let (ow, oh) = outer_size(style, ex, w, h);
    // SAFETY: as above; only the size changes.
    unsafe { SetWindowPos(window, std::ptr::null_mut(), 0, 0, ow, oh, SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE) };
}
