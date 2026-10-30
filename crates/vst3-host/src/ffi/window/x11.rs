use std::ffi::{c_char, c_void};

use x11rb::connection::Connection;
use x11rb::protocol::xproto::{self, Atom, AtomEnum, ConfigureWindowAux, CreateWindowAux, EventMask, MapState, PropMode, Screen, StackMode, VisualClass, Visualid, Window, WindowClass};
use x11rb::rust_connection::RustConnection;

pub const PLATFORM_TYPE: Option<*const c_char> = Some(vst3::Steinberg::kPlatformTypeX11EmbedWindowID);

pub fn on_main_thread() -> bool {
    std::thread::current().name() == Some("main")
}

fn atom_val(conn: &RustConnection, name: &str) -> Option<Atom> {
    let cookie = xproto::intern_atom(conn, false, name.as_bytes()).ok()?;
    let reply = cookie.reply().ok()?;
    Some(reply.atom)
}

fn find_visual(_conn: &RustConnection, screen: &Screen) -> Option<Visualid> {
    for depth in &screen.allowed_depths {
        for vis in &depth.visuals {
            if vis.class == VisualClass::TRUE_COLOR || vis.class == VisualClass::DIRECT_COLOR {
                return Some(vis.visual_id);
            }
        }
    }
    Some(screen.root_visual)
}

fn connect() -> Option<(RustConnection, usize)> {
    let (conn, screen_num) = RustConnection::connect(None).ok()?;
    Some((conn, screen_num))
}

pub struct HostWindow {
    window: Window,
    conn: RustConnection,
    screen_num: usize,
}

unsafe impl Send for HostWindow {}

impl HostWindow {
    pub fn new(title: &str, w: f64, h: f64) -> Result<HostWindow, String> {
        if !on_main_thread() {
            return Err("plugin editors must be opened on the main thread".into());
        }
        let (conn, screen_num) = connect().ok_or_else(|| "cannot connect to X11".to_string())?;
        let setup = conn.setup();
        let screen = &setup.roots[screen_num];
        let depth = screen.root_depth;
        let visual = find_visual(&conn, screen).ok_or_else(|| "cannot find visual".to_string())?;
        let window: Window = conn.generate_id().map_err(|e| e.to_string())?;
        let cw = CreateWindowAux::new().background_pixel(screen.white_pixel).border_pixel(screen.black_pixel).event_mask(
            EventMask::EXPOSURE
                | EventMask::STRUCTURE_NOTIFY
                | EventMask::KEY_PRESS
                | EventMask::KEY_RELEASE
                | EventMask::BUTTON_PRESS
                | EventMask::BUTTON_RELEASE
                | EventMask::POINTER_MOTION,
        );
        xproto::create_window(
            &conn,
            depth,
            window,
            screen.root,
            0,
            0,
            if w < 1.0 { 400 } else { w as u16 },
            if h < 1.0 { 300 } else { h as u16 },
            1,
            WindowClass::INPUT_OUTPUT,
            visual,
            &cw,
        )
        .map_err(|e| e.to_string())?;
        let wm_name: u32 = atom_val(&conn, "WM_NAME").map_or(AtomEnum::WM_NAME.into(), Into::into);
        let _ = xproto::change_property(&conn, PropMode::REPLACE, window, wm_name, AtomEnum::STRING, 8, title.len() as u32, title.as_bytes());
        let wm_protocols: u32 = atom_val(&conn, "WM_PROTOCOLS").unwrap_or(4);
        let wm_delete: u32 = atom_val(&conn, "WM_DELETE_WINDOW").unwrap_or(0);
        let wm_take_focus: u32 = atom_val(&conn, "WM_TAKE_FOCUS").unwrap_or(0);
        let mut protos: Vec<u32> = Vec::with_capacity(2);
        if wm_delete != 0 {
            protos.push(wm_delete);
        }
        if wm_take_focus != 0 {
            protos.push(wm_take_focus);
        }
        if !protos.is_empty() {
            let mut data: Vec<u8> = Vec::with_capacity(protos.len() * 4);
            for a in &protos {
                data.extend_from_slice(&a.to_le_bytes());
            }
            let _ = xproto::change_property(&conn, PropMode::REPLACE, window, wm_protocols, AtomEnum::ATOM, 32, protos.len() as u32, &data);
        }
        conn.flush().map_err(|e| e.to_string())?;
        Ok(HostWindow { window, conn, screen_num })
    }

    pub fn raw(&self) -> *mut c_void {
        self.window as *mut c_void
    }

    pub fn parent(&self) -> *mut c_void {
        self.window as *mut c_void
    }

    pub fn raise(&self) {
        let _ = xproto::map_window(&self.conn, self.window);
        let setup = self.conn.setup();
        let screen = &setup.roots[self.screen_num];
        let _ = xproto::configure_window(&self.conn, self.window, &ConfigureWindowAux::new().stack_mode(StackMode::ABOVE).sibling(screen.root));
        let _ = self.conn.flush();
    }

    pub fn is_visible(&self) -> bool {
        if let Ok(cookie) = xproto::get_window_attributes(&self.conn, self.window)
            && let Ok(reply) = cookie.reply()
        {
            return reply.map_state == MapState::VIEWABLE;
        }
        false
    }

    pub fn close(self) {
        let _ = xproto::unmap_window(&self.conn, self.window);
        let _ = xproto::destroy_window(&self.conn, self.window);
        let _ = self.conn.flush();
    }
}

pub fn resize(window: *mut c_void, w: f64, h: f64) {
    if !on_main_thread() {
        return;
    }
    if window.is_null() {
        return;
    }
    if let Some((conn, _screen_num)) = connect() {
        let win = window as Window;
        let _ = xproto::configure_window(
            &conn,
            win,
            &ConfigureWindowAux::new().width(if w < 1.0 { 400 } else { w as u32 }).height(if h < 1.0 { 300 } else { h as u32 }),
        );
        let _ = conn.flush();
    }
}
