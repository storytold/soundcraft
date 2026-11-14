use clap_sys::ext::gui::clap_window_handle;
use std::collections::HashSet;
use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock, PoisonError};
use x11rb::connection::Connection;
use x11rb::properties::WmSizeHints;
use x11rb::protocol::Event;
use x11rb::protocol::xproto::{
    AtomEnum, ConfigureWindowAux, ConnectionExt as _, CreateWindowAux, EventMask, PropMode, StackMode, Window, WindowClass,
};
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;

pub const SUPPORTED: bool = true;

/// Rust names the process's first thread "main"; the app runs its UI (and event loop) there.
fn on_main_thread() -> bool {
    std::thread::current().name() == Some("main")
}

struct X {
    conn: RustConnection,
    root: Window,
    visual: u32,
    background: u32,
    wm_protocols: u32,
    wm_delete: u32,
    net_wm_name: u32,
    utf8: u32,
    /// Windows whose close button was pressed (hidden, not yet destroyed).
    closed: Mutex<HashSet<Window>>,
}

/// The process-wide connection, opened on first use (`None` without an X server).
fn x() -> Option<&'static X> {
    static X: OnceLock<Option<X>> = OnceLock::new();
    X.get_or_init(|| match connect() {
        Ok(x) => Some(x),
        Err(e) => {
            log::warn!("clap: no X11 display for plugin editors: {e}");
            None
        }
    })
    .as_ref()
}

fn connect() -> Result<X, Box<dyn std::error::Error>> {
    let (conn, screen) = x11rb::connect(None)?;
    let s = conn.setup().roots.get(screen).ok_or("no X11 screen")?;
    let (root, visual, background) = (s.root, s.root_visual, s.black_pixel);
    let atom = |name: &[u8]| -> Result<u32, Box<dyn std::error::Error>> { Ok(conn.intern_atom(false, name)?.reply()?.atom) };
    Ok(X {
        wm_protocols: atom(b"WM_PROTOCOLS")?,
        wm_delete: atom(b"WM_DELETE_WINDOW")?,
        net_wm_name: atom(b"_NET_WM_NAME")?,
        utf8: atom(b"UTF8_STRING")?,
        root,
        visual,
        background,
        closed: Mutex::new(HashSet::new()),
        conn,
    })
}

impl X {
    fn closed(&self) -> std::sync::MutexGuard<'_, HashSet<Window>> {
        self.closed.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Reads pending events: a close request hides its window and marks it closed.
    fn pump(&self) {
        while let Ok(Some(ev)) = self.conn.poll_for_event() {
            if let Event::ClientMessage(m) = ev
                && m.type_ == self.wm_protocols
                && m.format == 32
                && m.data.as_data32().first() == Some(&self.wm_delete)
            {
                let _ = self.conn.unmap_window(m.window);
                self.closed().insert(m.window);
            }
        }
        let _ = self.conn.flush();
    }

    /// Fixes the window's size for the window manager (plugins resize through the host).
    fn size_hints(&self, w: Window, width: u32, height: u32) {
        let (width, height) = (width as i32, height as i32);
        let hints = WmSizeHints { min_size: Some((width, height)), max_size: Some((width, height)), ..WmSizeHints::new() };
        let _ = hints.set_normal_hints(&self.conn, w);
    }
}

fn dims(w: f64, h: f64) -> (u16, u16) {
    (w.clamp(1.0, 16_384.0) as u16, h.clamp(1.0, 16_384.0) as u16)
}

pub struct HostWindow {
    id: Window,
    shown: AtomicBool,
}

impl HostWindow {
    pub fn new(title: &str, w: f64, h: f64) -> Result<HostWindow, String> {
        if !on_main_thread() {
            return Err("plugin editors must be opened on the main thread".into());
        }
        let x = x().ok_or_else(|| "plugin editors need an X11 display (XWayland on Wayland desktops)".to_string())?;
        let err = |e: &dyn std::fmt::Display| format!("cannot create the editor window: {e}");
        let id = x.conn.generate_id().map_err(|e| err(&e))?;
        let (width, height) = dims(w, h);
        let aux = CreateWindowAux::new().background_pixel(x.background).event_mask(EventMask::STRUCTURE_NOTIFY);
        x.conn
            .create_window(x11rb::COPY_DEPTH_FROM_PARENT, id, x.root, 0, 0, width, height, 0, WindowClass::INPUT_OUTPUT, x.visual, &aux)
            .map_err(|e| err(&e))?;
        let _ = x.conn.change_property8(PropMode::REPLACE, id, AtomEnum::WM_NAME, AtomEnum::STRING, title.as_bytes());
        let _ = x.conn.change_property8(PropMode::REPLACE, id, x.net_wm_name, x.utf8, title.as_bytes());
        let _ = x.conn.change_property8(PropMode::REPLACE, id, AtomEnum::WM_CLASS, AtomEnum::STRING, b"soundcraft\0SoundCraft\0");
        let _ = x.conn.change_property32(PropMode::REPLACE, id, x.wm_protocols, AtomEnum::ATOM, &[x.wm_delete]);
        x.size_hints(id, width.into(), height.into());
        // A round trip, so the window exists before the plugin (on its own connection)
        // parents its GUI in it.
        x.conn.get_input_focus().map_err(|e| err(&e))?.reply().map_err(|e| err(&e))?;
        Ok(HostWindow { id, shown: AtomicBool::new(false) })
    }

    /// The window id (for resizes).
    pub fn raw(&self) -> *mut c_void {
        self.id as usize as *mut c_void
    }

    pub fn handle(&self) -> clap_window_handle {
        clap_window_handle { x11: std::ffi::c_ulong::from(self.id) }
    }

    pub fn raise(&self) {
        let Some(x) = x().filter(|_| on_main_thread()) else { return };
        x.closed().remove(&self.id);
        let _ = x.conn.map_window(self.id);
        let _ = x.conn.configure_window(self.id, &ConfigureWindowAux::new().stack_mode(StackMode::ABOVE));
        let _ = x.conn.flush();
        self.shown.store(true, Ordering::Relaxed);
    }

    pub fn is_visible(&self) -> bool {
        let Some(x) = x().filter(|_| on_main_thread()) else { return false };
        x.pump();
        self.shown.load(Ordering::Relaxed) && !x.closed().contains(&self.id)
    }

    pub fn close(self) {
        let Some(x) = x().filter(|_| on_main_thread()) else { return };
        x.closed().remove(&self.id);
        let _ = x.conn.destroy_window(self.id);
        let _ = x.conn.flush();
    }
}

/// Resizes a host window (`window` from [`HostWindow::raw`]); main thread only.
pub fn resize(window: *mut c_void, w: f64, h: f64) {
    let Some(x) = x().filter(|_| on_main_thread()) else { return };
    let Ok(id) = Window::try_from(window as usize) else { return };
    if id == 0 {
        return;
    }
    let (width, height) = dims(w, h);
    x.size_hints(id, width.into(), height.into());
    let _ = x.conn.configure_window(id, &ConfigureWindowAux::new().width(u32::from(width)).height(u32::from(height)));
    let _ = x.conn.flush();
}
