use std::collections::HashMap;
use std::ffi::{c_char, c_void};
use std::sync::{Mutex, OnceLock};

use x11rb::connection::Connection;
use x11rb::protocol::xproto::{
    self, Atom, AtomEnum, ConfigureWindowAux, CreateWindowAux, EventMask, MapState, PropMode, Screen, StackMode, VisualClass, Visualid, Window,
    WindowClass,
};
use x11rb::rust_connection::RustConnection;

pub const PLATFORM_TYPE: Option<*const c_char> = Some(vst3::Steinberg::kPlatformTypeX11EmbedWindowID);

static MAIN_THREAD_ID: OnceLock<std::thread::ThreadId> = OnceLock::new();
static WINDOWS: OnceLock<Mutex<HashMap<Window, std::sync::Arc<RustConnection>>>> = OnceLock::new();

pub fn set_main_thread() {
    let _ = MAIN_THREAD_ID.set(std::thread::current().id());
}

pub fn on_main_thread() -> bool {
    if let Some(id) = MAIN_THREAD_ID.get() {
        return *id == std::thread::current().id();
    }
    std::thread::current().name() == Some("main")
}

fn atom_val(conn: &RustConnection, name: &str) -> Option<Atom> {
    let cookie = xproto::intern_atom(conn, false, name.as_bytes()).ok()?;
    let reply = cookie.reply().ok()?;
    Some(reply.atom)
}

fn find_visual_and_depth(_conn: &RustConnection, screen: &Screen) -> Option<(Visualid, u8)> {
    for depth in &screen.allowed_depths {
        for vis in &depth.visuals {
            if vis.class == VisualClass::TRUE_COLOR || vis.class == VisualClass::DIRECT_COLOR {
                return Some((vis.visual_id, depth.depth));
            }
        }
    }
    Some((screen.root_visual, screen.root_depth))
}

fn connect() -> Option<(RustConnection, usize)> {
    let (conn, screen_num) = RustConnection::connect(None).ok()?;
    Some((conn, screen_num))
}

fn windows_map() -> &'static Mutex<HashMap<Window, std::sync::Arc<RustConnection>>> {
    WINDOWS.get_or_init(|| Mutex::new(HashMap::new()))
}

pub struct HostWindow {
    window: Window,
    conn: std::sync::Arc<RustConnection>,
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
        let (visual, depth) = find_visual_and_depth(&conn, screen).ok_or_else(|| "cannot find visual".to_string())?;
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
        let utf8_string: u32 = atom_val(&conn, "UTF8_STRING").map_or(AtomEnum::STRING.into(), Into::into);
        let _ = xproto::change_property(&conn, PropMode::REPLACE, window, wm_name, utf8_string, 8, title.len() as u32, title.as_bytes());
        let _ = xproto::change_property(&conn, PropMode::REPLACE, window, AtomEnum::WM_NAME, utf8_string, 8, title.len() as u32, title.as_bytes());
        let net_wm_name: Option<Atom> = atom_val(&conn, "_NET_WM_NAME");
        if let Some(a) = net_wm_name {
            let _ = xproto::change_property(&conn, PropMode::REPLACE, window, a, utf8_string, 8, title.len() as u32, title.as_bytes());
        }
        let net_wm_window_type: Option<Atom> = atom_val(&conn, "_NET_WM_WINDOW_TYPE");
        let net_wm_window_type_normal: Option<Atom> = atom_val(&conn, "_NET_WM_WINDOW_TYPE_NORMAL");
        if let (Some(wt), Some(wn)) = (net_wm_window_type, net_wm_window_type_normal) {
            let mut data: Vec<u8> = Vec::with_capacity(4);
            data.extend_from_slice(&wn.to_le_bytes());
            let _ = xproto::change_property(&conn, PropMode::REPLACE, window, wt, AtomEnum::ATOM, 32, 1, &data);
        }
        let wm_protocols: Option<Atom> = atom_val(&conn, "WM_PROTOCOLS");
        let wm_delete: Option<Atom> = atom_val(&conn, "WM_DELETE_WINDOW");
        let wm_take_focus: Option<Atom> = atom_val(&conn, "WM_TAKE_FOCUS");
        let mut protos: Vec<u32> = Vec::with_capacity(2);
        if let Some(a) = wm_delete {
            protos.push(a);
        }
        if let Some(a) = wm_take_focus {
            protos.push(a);
        }
        if !protos.is_empty()
            && let Some(wp) = wm_protocols
        {
            let mut data: Vec<u8> = Vec::with_capacity(protos.len() * 4);
            for a in &protos {
                data.extend_from_slice(&(*a).to_le_bytes());
            }
            let _ = xproto::change_property(&conn, PropMode::REPLACE, window, wp, AtomEnum::ATOM, 32, protos.len() as u32, &data);
        }
        conn.flush().map_err(|e| e.to_string())?;
        let arc = std::sync::Arc::new(conn);
        if let Ok(mut map) = windows_map().lock() {
            map.insert(window, arc.clone());
        }
        Ok(HostWindow { window, conn: arc })
    }

    pub fn raw(&self) -> *mut c_void {
        self.window as *mut c_void
    }

    pub fn parent(&self) -> *mut c_void {
        self.window as *mut c_void
    }

    pub fn raise(&self) {
        let _ = xproto::map_window(&self.conn, self.window);
        let _ = xproto::configure_window(&self.conn, self.window, &ConfigureWindowAux::new().stack_mode(StackMode::ABOVE));
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
        if let Ok(mut map) = windows_map().lock() {
            map.remove(&self.window);
        }
    }
}

pub fn resize(window: *mut c_void, w: f64, h: f64) {
    if !on_main_thread() {
        return;
    }
    if window.is_null() {
        return;
    }
    let win = window as Window;
    if let Ok(map) = windows_map().lock()
        && let Some(conn) = map.get(&win)
    {
        let _ = xproto::configure_window(
            conn,
            win,
            &ConfigureWindowAux::new().width(if w < 1.0 { 400 } else { w as u32 }).height(if h < 1.0 { 300 } else { h as u32 }),
        );
        let _ = conn.flush();
        return;
    }
    if let Some((conn, _screen_num)) = connect() {
        let _ = xproto::configure_window(
            &conn,
            win,
            &ConfigureWindowAux::new().width(if w < 1.0 { 400 } else { w as u32 }).height(if h < 1.0 { 300 } else { h as u32 }),
        );
        let _ = conn.flush();
    }
}
