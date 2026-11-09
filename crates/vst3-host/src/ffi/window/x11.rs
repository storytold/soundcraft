use std::ffi::{CString, c_char, c_int, c_void};
use std::sync::{Mutex, PoisonError};
use std::thread::ThreadId;
use x11_dl::xlib::{self, Display, Window as XWindow, XErrorEvent, XEvent};

/// The VST3 platform type for X11 embedding.
pub const PLATFORM_TYPE: Option<*const c_char> = Some(vst3::Steinberg::kPlatformTypeX11EmbedWindowID);

/// The thread that owns the X display. Xlib is only thread-safe when it is initialized for
/// threads, and the app's own X connection (winit) may already be up, so every X call happens
/// on the thread that opened the display: the one that opened the editor.
static OWNER: Mutex<Option<ThreadId>> = Mutex::new(None);

/// True on the thread that may talk to X.
pub fn on_main_thread() -> bool {
    match *OWNER.lock().unwrap_or_else(PoisonError::into_inner) {
        Some(owner) => owner == std::thread::current().id(),
        // Nobody has opened a display yet: the next one to do so becomes its owner.
        None => true,
    }
}

/// Remembers the calling thread as the display's owner.
fn claim() {
    let mut owner = OWNER.lock().unwrap_or_else(PoisonError::into_inner);
    if owner.is_none() {
        *owner = Some(std::thread::current().id());
    }
}

/// An Xlib error handler, as `XSetErrorHandler` takes and returns it.
type ErrorHandler = Option<unsafe extern "C" fn(*mut Display, *mut XErrorEvent) -> c_int>;

/// While `HostWindow::tolerating_stale_windows` runs: the display whose stale-window errors
/// are swallowed (as an address) and the handler that was installed before, which gets every
/// other error. `(0, None)` otherwise.
static TRAP: Mutex<(usize, ErrorHandler)> = Mutex::new((0, None));

/// Swallows `BadWindow`/`BadMatch` on our display: the plugin (or a bridge such as yabridge)
/// can unmap or destroy its child window between our query and our request on it. Xlib's
/// default handler would `exit()` the process on such an error, which happens whenever
/// winit's X11 backend (whose handler only logs) is not the one running, e.g. on Wayland.
/// Everything else goes to the previous handler, so its behaviour is kept.
unsafe extern "C" fn trap_stale_window(display: *mut Display, event: *mut XErrorEvent) -> c_int {
    let (ours, previous) = *TRAP.lock().unwrap_or_else(PoisonError::into_inner);
    let code = if event.is_null() {
        0
    } else {
        // SAFETY: Xlib passes a valid error event that lives for the duration of the call.
        unsafe { (*event).error_code }
    };
    if ours != 0 && display as usize == ours && (code == xlib::BadWindow || code == xlib::BadMatch) {
        return 0;
    }
    match previous {
        // SAFETY: Xlib's own arguments, handed to the handler that was installed before ours.
        Some(handler) => unsafe { handler(display, event) },
        None => 0,
    }
}

/// A top-level X11 window the plugin's view embeds itself into. Holds the X connection;
/// owner thread only.
pub struct HostWindow {
    xlib: x11_dl::xlib::Xlib,
    display: *mut Display,
    window: XWindow,
    width: u32,
    height: u32,
    /// When the keyboard focus was last handed to the plugin, so `pump` does not ask on
    /// every frame.
    focus_checked: std::time::Instant,
    /// The `WM_DELETE_WINDOW` atom, so a close request through it is recognized.
    delete: x11_dl::xlib::Atom,
    /// Set when the window manager asked the window to close.
    closing: bool,
}

// SAFETY: the display is only used on the thread that opened it (see `OWNER`); the struct
// carries the pointer between the locks of the editor link.
unsafe impl Send for HostWindow {}

impl HostWindow {
    pub fn new(title: &str, w: f64, h: f64) -> Result<HostWindow, String> {
        let xlib = x11_dl::xlib::Xlib::open().map_err(|e| format!("cannot load libX11: {e}"))?;
        // SAFETY: a null display name opens the display named by $DISPLAY.
        let display = unsafe { (xlib.XOpenDisplay)(std::ptr::null()) };
        if display.is_null() {
            return Err("cannot open the X display (is $DISPLAY set?)".into());
        }
        claim();
        // SAFETY: a valid display.
        let screen = unsafe { (xlib.XDefaultScreen)(display) };
        // SAFETY: a valid display and screen.
        let root = unsafe { (xlib.XRootWindow)(display, screen) };
        // SAFETY: a valid display and screen.
        let black = unsafe { (xlib.XBlackPixel)(display, screen) };
        let width = (w.max(160.0) as u32).min(8192);
        let height = (h.max(120.0) as u32).min(8192);
        // A frame-less window at the screen origin. A bridge that runs the plugin in another
        // process (yabridge through Wine) translates pointer events against the origin of the
        // Win32 screen, which Wine pins to its own window position; a decorated or moved
        // window leaves the plugin's clicks shifted by the window's screen position. A window
        // manager also keeps managed windows off the panel, so only an override-redirect
        // window keeps the client area on the origin, where screen and window coordinates
        // coincide and clicks land where the plugin drew.
        // SAFETY: a valid display and screen.
        let depth = unsafe { (xlib.XDefaultDepth)(display, screen) };
        // SAFETY: a valid display and screen.
        let visual = unsafe { (xlib.XDefaultVisual)(display, screen) };
        let mut attributes: x11_dl::xlib::XSetWindowAttributes = unsafe { std::mem::zeroed() };
        attributes.background_pixel = black;
        attributes.override_redirect = 1;
        // SAFETY: a valid display, root window and visual; a window without a border whose
        // attributes are initialized above.
        let window = unsafe {
            (xlib.XCreateWindow)(
                display,
                root,
                0,
                0,
                width,
                height,
                0,
                depth,
                x11_dl::xlib::InputOutput as u32,
                visual,
                x11_dl::xlib::CWBackPixel | x11_dl::xlib::CWOverrideRedirect,
                &mut attributes,
            )
        };
        if window == 0 {
            // SAFETY: a valid display.
            unsafe { (xlib.XCloseDisplay)(display) };
            return Err("cannot create the editor window".into());
        }
        // A title with an inner NUL cannot be passed on; an empty name is used instead.
        let name = CString::new(title).unwrap_or_default();
        // SAFETY: a valid display and window; `name` outlives the call.
        unsafe { (xlib.XStoreName)(display, window, name.as_ptr()) };
        // A window manager that manages the window sends this message on close; an
        // override-redirect window only ever sees it from an explicit sender.
        // SAFETY: a valid display; the name is NUL-terminated.
        let delete = unsafe { (xlib.XInternAtom)(display, c"WM_DELETE_WINDOW".as_ptr(), 0) };
        if delete != 0 {
            let mut protocols = [delete];
            // SAFETY: a valid display, window and atom list.
            unsafe { (xlib.XSetWMProtocols)(display, window, protocols.as_mut_ptr(), 1) };
        }
        // SAFETY: a valid display and window.
        unsafe {
            (xlib.XSelectInput)(display, window, xlib::ExposureMask | xlib::StructureNotifyMask | xlib::FocusChangeMask | xlib::EnterWindowMask)
        };
        // The window is mapped later (after the plugin attached): mapping and then placing it
        // sends the ConfigureNotify a bridge like yabridge needs to learn the window's size.
        // Mapped earlier, that event would be missed and the plugin would be told its window
        // is 0x0.
        Ok(HostWindow { xlib, display, window, width, height, focus_checked: std::time::Instant::now(), delete, closing: false })
    }

    /// The window id the plugin embeds itself into (VST3's `X11EmbedWindowID`).
    pub fn parent(&self) -> *mut c_void {
        self.window as *mut c_void
    }

    /// The raw window handle (for the plug frame's resizes).
    pub fn raw(&self) -> *mut c_void {
        self.window as *mut c_void
    }

    /// Shows the window (after the plugin attached, so the configure event that follows
    /// reaches the plugin's view).
    pub fn map(&self) {
        // SAFETY: a valid display and window.
        unsafe {
            (self.xlib.XMapWindow)(self.display, self.window);
            // An override-redirect window gets no window manager placement, so no
            // `ConfigureNotify` follows the map. Moving it onto the origin sends one, which
            // is what a bridge needs to learn the window's real size.
            (self.xlib.XMoveResizeWindow)(self.display, self.window, 0, 0, self.width, self.height);
            (self.xlib.XFlush)(self.display);
        }
    }

    /// Gives the keyboard focus to the plugin's own X window (the deepest viewable child).
    /// An override-redirect host window never becomes the window manager's active window,
    /// and a bridge that only grabs focus for active windows (yabridge) then leaves the
    /// plugin without keyboard input; its text fields would stay dead.
    pub fn focus(&self) {
        self.tolerating_stale_windows(|this| {
            let target = this.focus_target();
            if target != this.window {
                // SAFETY: a valid display; `target` was a viewable child of our window (an
                // error from it having gone since is swallowed by the scoped handler).
                unsafe { (this.xlib.XSetInputFocus)(this.display, target, xlib::RevertToParent, xlib::CurrentTime) };
            }
        });
    }

    /// Runs `f`, whose requests name the plugin's child windows, with a scoped error handler
    /// that ignores `BadWindow`/`BadMatch` on this display (see `trap_stale_window`). The
    /// display is synchronized on both sides, so earlier requests' errors still reach the
    /// previous handler and every error of `f` arrives before that handler is restored.
    fn tolerating_stale_windows<R>(&self, f: impl FnOnce(&Self) -> R) -> R {
        // SAFETY: a valid display, used on its owner thread.
        unsafe { (self.xlib.XSync)(self.display, 0) };
        // SAFETY: installs a handler that only reads the event and forwards to the previous one.
        let previous = unsafe { (self.xlib.XSetErrorHandler)(Some(trap_stale_window)) };
        *TRAP.lock().unwrap_or_else(PoisonError::into_inner) = (self.display as usize, previous);
        let result = f(self);
        // SAFETY: a valid display; `previous` is the handler Xlib returned above.
        unsafe {
            (self.xlib.XSync)(self.display, 0);
            (self.xlib.XSetErrorHandler)(previous);
        }
        *TRAP.lock().unwrap_or_else(PoisonError::into_inner) = (0, None);
        result
    }

    /// Keeps the editor in front and the keyboard with the plugin while the pointer rests over
    /// it. An override-redirect window is not managed by the window manager: anything raised
    /// later (the app itself) would bury it for good, and nothing hands the focus back after
    /// another window took it, so the plugin's text fields would stop accepting input.
    fn follow_pointer(&mut self) {
        if self.focus_checked.elapsed() < std::time::Duration::from_millis(250) {
            return;
        }
        self.focus_checked = std::time::Instant::now();
        let mut root: XWindow = 0;
        let mut child: XWindow = 0;
        let (mut root_x, mut root_y, mut window_x, mut window_y) = (0, 0, 0, 0);
        let mut mask: u32 = 0;
        // SAFETY: a valid display and window; every out-parameter is writable.
        let same_screen = unsafe {
            (self.xlib.XQueryPointer)(
                self.display,
                self.window,
                &mut root,
                &mut child,
                &mut root_x,
                &mut root_y,
                &mut window_x,
                &mut window_y,
                &mut mask,
            )
        };
        if same_screen == 0 || window_x < 0 || window_y < 0 || window_x >= self.width as i32 || window_y >= self.height as i32 {
            return;
        }
        // An override-redirect window never takes part in the window manager's stacking, so a
        // window raised later (SoundCraft itself, after any click) covers it for good. Raising
        // it while the pointer is over its area brings it back, and keeps it usable without a
        // title bar.
        // SAFETY: a valid display and window.
        unsafe {
            (self.xlib.XRaiseWindow)(self.display, self.window);
            (self.xlib.XFlush)(self.display);
        }
        self.tolerating_stale_windows(|this| {
            let target = this.focus_target();
            if target == this.window {
                return;
            }
            let mut focused: XWindow = 0;
            let mut revert = 0;
            // SAFETY: a valid display; both out-parameters are writable.
            unsafe { (this.xlib.XGetInputFocus)(this.display, &mut focused, &mut revert) };
            if focused != target {
                // SAFETY: a valid display; `target` was a viewable child of our window (an
                // error from it having gone since is swallowed by the scoped handler).
                unsafe { (this.xlib.XSetInputFocus)(this.display, target, xlib::RevertToParent, xlib::CurrentTime) };
            }
        });
    }

    /// The deepest viewable child of our window: the window the plugin's own widgets live in.
    fn focus_target(&self) -> XWindow {
        let mut target = self.window;
        for _ in 0..4 {
            let mut root: XWindow = 0;
            let mut parent: XWindow = 0;
            let mut children: *mut XWindow = std::ptr::null_mut();
            let mut count: u32 = 0;
            // SAFETY: a valid display and window; the child array is freed below.
            let queried = unsafe { (self.xlib.XQueryTree)(self.display, target, &mut root, &mut parent, &mut children, &mut count) };
            if queried == 0 {
                break;
            }
            let mut next = target;
            for index in 0..count as usize {
                // SAFETY: XQueryTree returned `count` children in `children`.
                let child = unsafe { *children.add(index) };
                if child == 0 {
                    continue;
                }
                let mut attributes: x11_dl::xlib::XWindowAttributes = unsafe { std::mem::zeroed() };
                // SAFETY: a valid display and child window; `attributes` is writable.
                let got = unsafe { (self.xlib.XGetWindowAttributes)(self.display, child, &mut attributes) };
                if got != 0
                    && attributes.map_state == xlib::IsViewable
                    && attributes.class == xlib::InputOutput
                    && attributes.width > 32
                    && attributes.height > 32
                {
                    next = child;
                    break;
                }
            }
            // SAFETY: the array came from XQueryTree and is not used afterwards.
            unsafe { (self.xlib.XFree)(children.cast()) };
            if next == target {
                break;
            }
            target = next;
        }
        target
    }

    pub fn raise(&self) {
        // SAFETY: a valid display and window.
        unsafe {
            (self.xlib.XMapRaised)(self.display, self.window);
            (self.xlib.XFlush)(self.display);
        }
    }

    pub fn is_visible(&self) -> bool {
        !self.closing
    }

    /// Reads the window's X events: a close request ends the editor, and the pointer keeps
    /// the window in front of the app with the keyboard focus on the plugin.
    pub fn pump(&mut self) {
        self.follow_pointer();
        // SAFETY: a valid display.
        while unsafe { (self.xlib.XPending)(self.display) } > 0 {
            let mut event: XEvent = unsafe { std::mem::zeroed() };
            // SAFETY: a valid display; `event` is writable and gets a whole event.
            unsafe { (self.xlib.XNextEvent)(self.display, &mut event) };
            // SAFETY: the event was just filled by XNextEvent; reading its type is fine.
            let kind = unsafe { event.type_ };
            if kind == xlib::EnterNotify {
                self.focus();
                continue;
            }
            if kind != xlib::ClientMessage {
                continue;
            }
            // SAFETY: the event is a client message; reading its data is fine.
            let message = unsafe { event.client_message };
            if message.data.as_longs().first().copied() == Some(self.delete as std::os::raw::c_long) {
                self.closing = true;
            }
        }
    }

    pub fn close(self) {
        // SAFETY: a valid display and window; the display is closed afterwards, so the
        // pointers in `self` are not used again.
        unsafe {
            (self.xlib.XDestroyWindow)(self.display, self.window);
            (self.xlib.XFlush)(self.display);
            (self.xlib.XCloseDisplay)(self.display);
        }
    }
}

/// The plugin sizes its own child window; the parent keeps the size the plugin asked for.
pub fn resize(_window: *mut c_void, _w: f64, _h: f64) {}
