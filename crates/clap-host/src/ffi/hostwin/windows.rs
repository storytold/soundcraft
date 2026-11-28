use clap_sys::ext::gui::clap_window_handle;
use std::ffi::c_void;
use std::sync::OnceLock;
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{RDW_ALLCHILDREN, RDW_UPDATENOW, RedrawWindow};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::GetActiveWindow;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    AdjustWindowRectEx, CS_HREDRAW, CS_VREDRAW, CW_USEDEFAULT, CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GWL_EXSTYLE,
    GWL_STYLE, GetWindowLongPtrW, IDC_ARROW, IsWindow, IsWindowVisible, LoadCursorW, MSG, PM_REMOVE, PeekMessageW, PostQuitMessage, RegisterClassExW,
    SW_HIDE, SW_SHOW, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOZORDER, SetForegroundWindow, SetWindowPos, ShowWindow, WM_CLOSE, WM_QUIT, WM_TIMER,
    WNDCLASSEXW, WS_CAPTION, WS_CLIPCHILDREN, WS_OVERLAPPED, WS_SYSMENU,
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

    /// Per-frame service on the main thread (#1447): while the transport plays eframe always keeps a
    /// repaint pending, and Win32 only synthesizes `WM_TIMER` and `WM_PAINT` when the queue runs dry —
    /// so a timer-driven editor is starved until playback pauses. Dispatches the thread's pending
    /// timers (thread-wide on purpose: nothing else on this thread uses Win32 timers, and a timer
    /// with a `TIMERPROC` only runs through dispatch) and flushes the paint they invalidate.
    pub fn service(&self) {
        let Some(hwnd) = self.live() else { return };
        pump_timers();
        // SAFETY: a live window of this thread; ALLCHILDREN reaches the plugin's child windows.
        unsafe { RedrawWindow(hwnd, std::ptr::null(), std::ptr::null_mut(), RDW_UPDATENOW | RDW_ALLCHILDREN) };
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

/// Dispatches up to 32 `WM_TIMER`s pending on this thread (main thread only). The cap keeps a timer
/// that re-arms on every dispatch from starving the frame; leftovers are served next tick.
fn pump_timers() {
    // SAFETY: `PeekMessageW`/`DispatchMessageW`/`PostQuitMessage` on this thread's own queue;
    // `msg` outlives them all.
    unsafe {
        let mut msg: MSG = std::mem::zeroed();
        for _ in 0..32 {
            if PeekMessageW(&mut msg, std::ptr::null_mut(), WM_TIMER, WM_TIMER, PM_REMOVE) == 0 {
                break;
            }
            if msg.message == WM_QUIT {
                // PeekMessageW returns WM_QUIT regardless of the filter — repost it with its
                // exit code so the app's own message loop still sees the quit, and stop here.
                PostQuitMessage(msg.wParam as i32);
                break;
            }
            DispatchMessageW(&msg);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use windows_sys::Win32::Graphics::Gdi::{GetUpdateRect, InvalidateRect};
    use windows_sys::Win32::UI::WindowsAndMessaging::{KillTimer, SetTimer};

    static FIRED: AtomicUsize = AtomicUsize::new(0);

    unsafe extern "system" fn on_timer(hwnd: HWND, _msg: u32, _id: usize, _time: u32) {
        FIRED.fetch_add(1, Ordering::Relaxed);
        // SAFETY: a live window of this thread.
        unsafe { InvalidateRect(hwnd, std::ptr::null(), 1) };
    }

    /// #1447: while the transport plays eframe always has a repaint pending, and Win32 only
    /// synthesizes `WM_TIMER`/`WM_PAINT` when the queue runs dry — an editor that animates from a
    /// timer freezes until Pause. `service` must dispatch the timer and its paint without a
    /// message loop ever waiting.
    #[test]
    fn an_editors_timer_and_paint_run_while_the_thread_never_waits_for_messages() {
        // windows are created and served on the app's UI thread — named "main" here.
        std::thread::Builder::new()
            .name("main".into())
            .spawn(|| {
                let window = HostWindow::new("timer test", 120.0, 80.0).expect("host window");
                let h = window.raw() as HWND;
                FIRED.store(0, Ordering::Relaxed);
                // SAFETY: live window of this thread; proc outlives the timer (killed below).
                unsafe { SetTimer(h, 1, 10, Some(on_timer)) };
                std::thread::sleep(std::time::Duration::from_millis(100));
                assert_eq!(FIRED.load(Ordering::Relaxed), 0, "the timer fired without anyone pumping");
                window.service();
                assert!(FIRED.load(Ordering::Relaxed) > 0, "service() did not dispatch WM_TIMER");
                // SAFETY: `GetUpdateRect` accepts any window; false means the paint was flushed.
                assert_eq!(unsafe { GetUpdateRect(h, std::ptr::null_mut(), 0) }, 0, "the timer's paint is still pending");
                unsafe { KillTimer(h, 1) };
                window.close();
            })
            .expect("spawn")
            .join()
            .expect("join");
    }

    /// `PeekMessageW` returns `WM_QUIT` regardless of the filter range, so the pump can swallow
    /// the quit the app's own loop is waiting for. It must repost it — with its exit code — and
    /// stop, leaving the signal for the owning loop.
    #[test]
    fn the_pump_leaves_a_quit_and_its_exit_code_for_the_owning_loop() {
        std::thread::Builder::new()
            .name("main".into())
            .spawn(|| {
                // SAFETY: this thread owns the queue it posts to and peeks.
                unsafe { PostQuitMessage(7) };
                pump_timers();
                let mut msg: MSG = unsafe { std::mem::zeroed() };
                assert_ne!(unsafe { PeekMessageW(&mut msg, std::ptr::null_mut(), 0, 0, PM_REMOVE) }, 0, "the pump swallowed the quit");
                assert_eq!((msg.message, msg.wParam), (WM_QUIT, 7), "the quit signal or its exit code changed");
            })
            .expect("spawn")
            .join()
            .expect("join");
    }
}
