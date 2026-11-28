use super::events::ostream_write;
use super::host::HostState;
use super::{
    Arc, CStr, CString, Mutex, Ordering, PoisonError, c_void, clap_ostream, clap_plugin, clap_plugin_gui, clap_plugin_state, clap_window, hostwin,
};
#[cfg(all(unix, not(target_os = "macos")))]
use super::{clap_plugin_posix_fd_support, clap_plugin_timer_support};

// ---- editor (clap.gui) ---------------------------------------------------------------------

/// True on the process's main thread (AppKit requires GUI calls there).
#[cfg(target_os = "macos")]
pub(crate) fn on_main_thread() -> bool {
    unsafe extern "C" {
        fn pthread_main_np() -> std::ffi::c_int;
    }
    // SAFETY: a libSystem function without arguments or preconditions.
    unsafe { pthread_main_np() == 1 }
}

/// True on the process's main thread: Rust names it "main", and the app runs its UI there (the
/// rule `hostwin` and the VST3 host use). CLAP makes `clap.gui` main-thread only on every platform.
#[cfg(not(target_os = "macos"))]
pub(crate) fn on_main_thread() -> bool {
    std::thread::current().name() == Some("main")
}

/// The windowing API of this platform.
fn window_api() -> Option<&'static CStr> {
    if cfg!(target_os = "macos") {
        Some(clap_sys::ext::gui::CLAP_WINDOW_API_COCOA)
    } else if cfg!(windows) {
        Some(clap_sys::ext::gui::CLAP_WINDOW_API_WIN32)
    } else if cfg!(all(unix, not(target_os = "android"))) {
        Some(clap_sys::ext::gui::CLAP_WINDOW_API_X11)
    } else {
        None
    }
}

pub(super) struct GuiInner {
    pub(super) plugin: *const clap_plugin,
    pub(super) gui: *const clap_plugin_gui,
    pub(super) state: *const clap_plugin_state,
    /// The plugin instance exists (cleared by `Instance::drop` before it destroys the plugin).
    pub(super) alive: bool,
    pub(super) created: bool,
    pub(super) visible: bool,
    /// The host window of an embedded editor (`None` for floating editors).
    pub(super) window: Option<hostwin::HostWindow>,
    /// The plugin's `clap.timer-support` and `clap.posix-fd-support` (null without them).
    #[cfg(all(unix, not(target_os = "macos")))]
    pub(super) timer: *const clap_plugin_timer_support,
    #[cfg(all(unix, not(target_os = "macos")))]
    pub(super) fd: *const clap_plugin_posix_fd_support,
}

/// The link between an instance and its editor and state handles. The instance may process on
/// the audio thread while the editor is driven and the state saved from the main thread: CLAP
/// allows `clap.gui` and `clap.state` (main-thread) calls concurrently with audio-thread calls.
/// Every such call happens under this lock, and the instance clears `alive` (destroying any GUI)
/// under the same lock before it destroys the plugin, so a handle can never call into a destroyed
/// plugin.
pub(crate) struct GuiLink {
    pub(super) inner: Mutex<GuiInner>,
    pub(super) host: Arc<HostState>,
}

impl Default for GuiLink {
    fn default() -> Self {
        GuiLink {
            inner: Mutex::new(GuiInner {
                plugin: std::ptr::null(),
                gui: std::ptr::null(),
                state: std::ptr::null(),
                alive: false,
                created: false,
                visible: false,
                window: None,
                #[cfg(all(unix, not(target_os = "macos")))]
                timer: std::ptr::null(),
                #[cfg(all(unix, not(target_os = "macos")))]
                fd: std::ptr::null(),
            }),
            host: Arc::new(HostState::default()),
        }
    }
}

// SAFETY: the raw pointers are only dereferenced under the mutex, while `alive` guarantees the
// plugin exists; CLAP's threading rules (main-thread GUI calls) are checked at each call.
unsafe impl Send for GuiInner {}

impl GuiLink {
    fn lock(&self) -> std::sync::MutexGuard<'_, GuiInner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub fn available(&self) -> bool {
        let g = self.lock();
        g.alive && !g.gui.is_null()
    }

    pub fn host(&self) -> &HostState {
        &self.host
    }

    fn vt(g: &GuiInner) -> Option<clap_plugin_gui> {
        // SAFETY: `gui` is null or the plugin's static extension vtable, valid while `alive`.
        (g.alive).then(|| unsafe { g.gui.as_ref() }.copied()).flatten()
    }

    /// Opens (creates and shows) the plugin's floating editor window.
    pub fn open(&self, title: &str) -> Result<(), String> {
        if !on_main_thread() {
            return Err("plugin editors must be opened on the main thread".into());
        }
        let mut g = self.lock();
        let vt = GuiLink::vt(&g).ok_or_else(|| "this plugin has no editor".to_string())?;
        if g.created {
            if let Some(show) = vt.show {
                // SAFETY: live plugin with a created GUI; main thread (checked).
                unsafe { show(g.plugin) };
            }
            if let Some(w) = &g.window {
                w.raise();
            }
            g.visible = true;
            return Ok(());
        }
        let api = window_api().ok_or_else(|| "plugin editors are not supported on this platform".to_string())?;
        let (Some(supported), Some(create)) = (vt.is_api_supported, vt.create) else { return Err("incomplete clap.gui extension".into()) };
        // SAFETY: live plugin; `api` is a static NUL-terminated string; main thread.
        if !unsafe { supported(g.plugin, api.as_ptr(), true) } {
            // SAFETY: as above.
            if hostwin::SUPPORTED && unsafe { supported(g.plugin, api.as_ptr(), false) } {
                return self.open_embedded(&mut g, vt, api, title);
            }
            return Err("this plugin offers only an embedded editor, which SoundCraft hosts on Windows and X11 (Linux, BSDs) only".into());
        }
        self.host.gui_closed.store(false, Ordering::Relaxed);
        self.host.gui_destroyed.store(false, Ordering::Relaxed);
        // SAFETY: as above; no GUI exists yet.
        if !unsafe { create(g.plugin, api.as_ptr(), true) } {
            return Err("the plugin could not create its editor".into());
        }
        g.created = true;
        if !cfg!(target_os = "macos")
            && let Some(f) = vt.set_scale
        {
            // SAFETY: live plugin with a created GUI. (Cocoa uses logical pixels: not called.)
            unsafe { f(g.plugin, 1.0) };
        }
        if let (Some(f), Ok(t)) = (vt.suggest_title, CString::new(title)) {
            // SAFETY: live plugin with a created floating GUI; `t` outlives the call.
            unsafe { f(g.plugin, t.as_ptr()) };
        }
        let shown = match vt.show {
            // SAFETY: as above.
            Some(f) => unsafe { f(g.plugin) },
            None => false,
        };
        if !shown {
            GuiLink::destroy(&mut g);
            return Err("the plugin could not show its editor".into());
        }
        g.visible = true;
        Ok(())
    }

    /// Embeds the editor in a host window of ours (plugins that offer no floating editor).
    fn open_embedded(&self, g: &mut GuiInner, vt: clap_plugin_gui, api: &CStr, title: &str) -> Result<(), String> {
        let (Some(create), Some(set_parent)) = (vt.create, vt.set_parent) else { return Err("incomplete clap.gui extension".into()) };
        self.host.gui_closed.store(false, Ordering::Relaxed);
        self.host.gui_destroyed.store(false, Ordering::Relaxed);
        self.host.gui_resize.store(0, Ordering::Relaxed);
        // SAFETY: live plugin; `api` is a static NUL-terminated string; main thread; no GUI yet.
        if !unsafe { create(g.plugin, api.as_ptr(), false) } {
            return Err("the plugin could not create its editor".into());
        }
        g.created = true;
        if let Some(f) = vt.set_scale {
            // SAFETY: live plugin with a created GUI.
            unsafe { f(g.plugin, 1.0) };
        }
        let (mut w, mut h) = (640u32, 480u32);
        if let Some(f) = vt.get_size {
            let (mut a, mut b) = (0u32, 0u32);
            // SAFETY: live plugin with a created GUI; both out-pointers are valid locals.
            if unsafe { f(g.plugin, &mut a, &mut b) } && a > 0 && b > 0 {
                (w, h) = (a.min(16_384), b.min(16_384));
            }
        }
        // #1448: a GUI too big for the monitor under the pointer gets the plugin's own nearest
        // supported size; one that cannot resize keeps its size — the window then opens with its
        // title bar on-screen so every control stays reachable by dragging, never clipped.
        let (fw, fh) = hostwin::fit_client(f64::from(w), f64::from(h));
        let mut negotiated = false;
        if (fw, fh) != (f64::from(w), f64::from(h))
            && let (Some(can), Some(adjust)) = (vt.can_resize, vt.adjust_size)
            // SAFETY: live plugin with a created GUI; both out-pointers are valid locals.
            && unsafe { can(g.plugin) }
        {
            let (mut aw, mut ah) = (fw as u32, fh as u32);
            // SAFETY: as above; `adjust` writes the nearest size the plugin supports.
            if unsafe { adjust(g.plugin, &mut aw, &mut ah) } && aw > 0 && ah > 0 {
                (w, h) = (aw.min(16_384), ah.min(16_384));
                negotiated = true;
            }
        }
        let win = match hostwin::HostWindow::new(title, f64::from(w), f64::from(h)) {
            Ok(win) => win,
            Err(e) => {
                GuiLink::destroy(g);
                return Err(e);
            }
        };
        let parent = clap_window { api: api.as_ptr(), specific: win.handle() };
        // SAFETY: live plugin with a created embedded GUI; `parent` names our live window and
        // outlives the call.
        if !unsafe { set_parent(g.plugin, &parent) } {
            win.close();
            GuiLink::destroy(g);
            return Err("the editor could not attach to its window".into());
        }
        if negotiated && let Some(f) = vt.set_size {
            // SAFETY: live plugin with a parented embedded GUI; it is told the size it agreed to.
            unsafe { f(g.plugin, w, h) };
        }
        g.window = Some(win);
        let shown = match vt.show {
            // SAFETY: live plugin with a created, parented GUI.
            Some(f) => unsafe { f(g.plugin) },
            None => true,
        };
        if !shown {
            GuiLink::destroy(g);
            return Err("the plugin could not show its editor".into());
        }
        if let Some(win) = &g.window {
            win.raise();
        }
        g.visible = true;
        Ok(())
    }

    fn destroy(g: &mut GuiInner) {
        if g.created
            && let Some(f) = GuiLink::vt(g).and_then(|v| v.destroy)
        {
            // SAFETY: live plugin with a created GUI; destroyed exactly once.
            unsafe { f(g.plugin) };
        }
        g.created = false;
        g.visible = false;
        // The plugin's child window is gone; now its host window can go too.
        if let Some(w) = g.window.take() {
            w.close();
        }
    }

    /// `clap_plugin_state.save` into memory (`None` without the extension, on failure, or once the
    /// plugin is gone).
    pub fn save_state(&self) -> Option<Vec<u8>> {
        let g = self.lock();
        if !g.alive {
            return None;
        }
        // SAFETY: `state` is null or the plugin's static extension vtable, valid while `alive`.
        let save = unsafe { g.state.as_ref() }?.save?;
        let mut out: Vec<u8> = Vec::new();
        let stream = clap_ostream { ctx: (&mut out as *mut Vec<u8>).cast::<c_void>(), write: Some(ostream_write) };
        // SAFETY: live plugin (`alive`, under the lock); the stream and its context live on this
        // frame for the call.
        unsafe { save(g.plugin, &stream) }.then_some(out)
    }

    /// Closes (destroys) the editor.
    pub fn close(&self) {
        if !on_main_thread() {
            return;
        }
        GuiLink::destroy(&mut self.lock());
    }

    pub fn is_open(&self) -> bool {
        let g = self.lock();
        g.alive && g.created && g.visible && g.window.as_ref().is_none_or(hostwin::HostWindow::is_visible)
    }

    /// Main-thread housekeeping: runs a requested `on_main_thread`, follows the plugin's
    /// show/hide/closed requests, and returns the parameter edits reported since the last call.
    pub fn idle(&self) -> Vec<(u32, f64)> {
        if !on_main_thread() {
            return Vec::new();
        }
        let mut g = self.lock();
        if !g.alive {
            return Vec::new();
        }
        if self.host.callback.swap(false, Ordering::Relaxed) {
            // SAFETY: `plugin` is valid while `alive`; we copy its vtable struct.
            if let Some(f) = unsafe { (*g.plugin).on_main_thread } {
                // SAFETY: live plugin, main thread, requested via `request_callback`.
                unsafe { f(g.plugin) };
            }
        }
        #[cfg(all(unix, not(target_os = "macos")))]
        self.dispatch(&g);
        if g.created {
            let resize = self.host.gui_resize.swap(0, Ordering::Relaxed);
            if resize != 0
                && let Some(win) = &g.window
            {
                let (w, h) = ((resize >> 32) as u32, resize as u32);
                // #1448: negotiate before touching the window — a request that outgrows the work
                // area becomes the plugin's nearest supported size (`adjust_size`), or the work-
                // area fit when the plugin cannot say; a plugin that cannot resize keeps the
                // window as it is. Only then is the window moved, to a size it keeps.
                let (mut tw, mut th) = (f64::from(w), f64::from(h));
                let vt = GuiLink::vt(&g);
                let resizable = vt.and_then(|v| v.can_resize).is_some_and(|f| unsafe { f(g.plugin) });
                if resizable {
                    let (fw, fh) = hostwin::fit_client(tw, th);
                    if (fw, fh) != (tw, th) {
                        tw = fw;
                        th = fh;
                        if let Some(adj) = vt.and_then(|v| v.adjust_size) {
                            let (mut aw, mut ah) = (fw as u32, fh as u32);
                            // SAFETY: live plugin, main thread; `adj` writes the nearest size
                            // the plugin supports into valid locals.
                            if unsafe { adj(g.plugin, &mut aw, &mut ah) } && aw > 0 && ah > 0 {
                                tw = f64::from(aw.min(16_384));
                                th = f64::from(ah.min(16_384));
                            }
                        }
                    }
                    let before = hostwin::client_size(win.raw());
                    let (aw, ah) = hostwin::resize(win.raw(), tw, th);
                    let accepted = vt.and_then(|v| v.set_size).is_some_and(|f| unsafe { f(g.plugin, aw.max(0.0) as u32, ah.max(0.0) as u32) });
                    if !accepted && let Some((bw, bh)) = before {
                        // The plugin refused the size: restore the window to what it was, so the
                        // window and the GUI it holds stay the same size.
                        hostwin::resize(win.raw(), bw, bh);
                    }
                }
            }
            if g.window.as_ref().is_some_and(|w| !w.is_visible()) {
                // The user closed the embedded editor's host window: free the editor.
                GuiLink::destroy(&mut g);
            } else if self.host.gui_closed.swap(false, Ordering::Relaxed) {
                // The user closed the floating window (or the plugin lost its GUI): free it.
                self.host.gui_destroyed.store(false, Ordering::Relaxed);
                GuiLink::destroy(&mut g);
            } else if let Some(vt) = GuiLink::vt(&g) {
                if self.host.gui_show.swap(false, Ordering::Relaxed)
                    && let Some(f) = vt.show
                {
                    // SAFETY: live plugin with a created GUI, main thread.
                    g.visible = unsafe { f(g.plugin) };
                }
                if self.host.gui_hide.swap(false, Ordering::Relaxed)
                    && let Some(f) = vt.hide
                {
                    // SAFETY: as above.
                    unsafe { f(g.plugin) };
                    g.visible = false;
                }
            }
        }
        drop(g);
        let mut e = self.host.edits.lock().unwrap_or_else(PoisonError::into_inner);
        let out = e.clone();
        e.clear();
        out
    }

    /// Called by the instance before it destroys the plugin.
    pub(super) fn kill(&self) {
        let mut g = self.lock();
        if g.created {
            if on_main_thread() {
                GuiLink::destroy(&mut g);
            } else {
                log::warn!("clap: editor still open while its plugin is destroyed off the main thread");
                g.created = false;
                // Window calls are main-thread only: leave the host window rather than crash.
                g.window = None;
            }
        }
        g.alive = false;
        #[cfg(all(unix, not(target_os = "macos")))]
        self.host.run_loop.clear();
    }

    /// Calls the plugin back for its ready descriptors and due timers (main thread, under the
    /// lock, plugin alive). One unregistered earlier in the same pass is skipped.
    #[cfg(all(unix, not(target_os = "macos")))]
    fn dispatch(&self, g: &GuiInner) {
        if !g.alive {
            return;
        }
        let rl = &self.host.run_loop;
        // SAFETY: null or the plugin's static extension vtables, valid while `alive`.
        let (timer, fd) = unsafe { (g.timer.as_ref().and_then(|t| t.on_timer), g.fd.as_ref().and_then(|f| f.on_fd)) };
        if let Some(on_fd) = fd {
            for (d, flags) in rl.ready() {
                if rl.has_fd(d) {
                    // SAFETY: live plugin, main thread, a descriptor it registered.
                    unsafe { on_fd(g.plugin, d, flags) };
                }
            }
        }
        if let Some(on_timer) = timer {
            for id in rl.due(std::time::Instant::now()) {
                if rl.has_timer(id) {
                    // SAFETY: live plugin, main thread, a timer it registered.
                    unsafe { on_timer(g.plugin, id) };
                }
            }
        }
    }
}

/// Windows regressions for the resize request path: a constrained editor that rounds every size
/// to its supported 800×500, and one that refuses `set_size` (#1448).
#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicU64};

    /// Serialises the window tests: the fake editor's state and real windows are shared.
    static GUARD: Mutex<()> = Mutex::new(());
    static SET_SIZE: AtomicU64 = AtomicU64::new(0);
    static RESIZABLE: AtomicBool = AtomicBool::new(true);
    static ACCEPT: AtomicBool = AtomicBool::new(true);

    unsafe extern "C" fn can_resize(_: *const clap_plugin) -> bool {
        RESIZABLE.load(Ordering::Relaxed)
    }

    /// A constrained editor: whatever it is offered becomes its only supported size, 800×500.
    unsafe extern "C" fn adjust_size(_: *const clap_plugin, w: *mut u32, h: *mut u32) -> bool {
        // SAFETY: the host's out-pointers.
        unsafe {
            *w = 800;
            *h = 500;
        }
        true
    }

    unsafe extern "C" fn set_size(_: *const clap_plugin, w: u32, h: u32) -> bool {
        SET_SIZE.store((u64::from(w) << 32) | u64::from(h), Ordering::Relaxed);
        ACCEPT.load(Ordering::Relaxed)
    }

    static GUI: clap_plugin_gui = clap_plugin_gui {
        is_api_supported: None,
        get_preferred_api: None,
        create: None,
        destroy: None,
        set_scale: None,
        get_size: None,
        can_resize: Some(can_resize),
        get_resize_hints: None,
        adjust_size: Some(adjust_size),
        set_size: Some(set_size),
        set_parent: None,
        set_transient: None,
        suggest_title: None,
        show: None,
        hide: None,
    };

    static PLUGIN: clap_plugin = clap_plugin {
        desc: std::ptr::null(),
        plugin_data: std::ptr::null_mut(),
        init: None,
        destroy: None,
        activate: None,
        deactivate: None,
        start_processing: None,
        stop_processing: None,
        reset: None,
        process: None,
        get_extension: None,
        on_main_thread: None,
    };

    /// A link whose fake embedded editor is open and shown in a real host window of `w`×`h`.
    fn editor_link(w: f64, h: f64) -> Result<GuiLink, String> {
        let link = GuiLink::default();
        let win = hostwin::HostWindow::new("editor", w, h)?;
        win.raise();
        let mut g = link.lock();
        g.plugin = &PLUGIN;
        g.gui = &GUI;
        g.alive = true;
        g.created = true;
        g.visible = true;
        g.window = Some(win);
        drop(g);
        Ok(link)
    }

    fn on_main(f: impl FnOnce() + Send + 'static) -> Result<(), String> {
        std::thread::Builder::new()
            .name("main".into())
            .spawn(f)
            .map_err(|e| e.to_string())?
            .join()
            .map_err(|_| "the test thread panicked".to_string())
    }

    /// An oversized request is negotiated to the editor's supported size and that size is
    /// applied to both the window and `set_size` — never the shrunk work-area fit (#1448).
    #[test]
    fn a_resize_is_negotiated_before_the_window_moves() {
        let _guard = GUARD.lock().unwrap_or_else(PoisonError::into_inner);
        on_main(|| {
            SET_SIZE.store(0, Ordering::Relaxed);
            RESIZABLE.store(true, Ordering::Relaxed);
            ACCEPT.store(true, Ordering::Relaxed);
            let link = editor_link(400.0, 300.0).unwrap();
            link.host.gui_resize.store((9_000u64 << 32) | 9_000, Ordering::Relaxed);
            link.idle();
            assert_eq!(SET_SIZE.load(Ordering::Relaxed), (800u64 << 32) | 500, "set_size got the negotiated size");
            let win = link.lock().window.as_ref().unwrap().raw();
            assert_eq!(hostwin::client_size(win), Some((800.0, 500.0)), "the window got it too");
            link.close();
        })
        .unwrap();
    }

    /// When the editor refuses the negotiated size, the window is restored to what it was so the
    /// window and the GUI it holds stay the same size (#1448).
    #[test]
    fn a_refused_resize_restores_the_window() {
        let _guard = GUARD.lock().unwrap_or_else(PoisonError::into_inner);
        on_main(|| {
            SET_SIZE.store(0, Ordering::Relaxed);
            RESIZABLE.store(true, Ordering::Relaxed);
            ACCEPT.store(false, Ordering::Relaxed);
            let link = editor_link(400.0, 300.0).unwrap();
            link.host.gui_resize.store((9_000u64 << 32) | 9_000, Ordering::Relaxed);
            link.idle();
            assert_eq!(SET_SIZE.load(Ordering::Relaxed), (800u64 << 32) | 500, "the plugin was still asked");
            let win = link.lock().window.as_ref().unwrap().raw();
            assert_eq!(hostwin::client_size(win), Some((400.0, 300.0)), "the window was restored");
            link.close();
        })
        .unwrap();
    }

    /// An editor that cannot resize gets its request refused: the window is untouched and
    /// `set_size` is never called (#1448).
    #[test]
    fn an_unresizable_editor_keeps_its_window() {
        let _guard = GUARD.lock().unwrap_or_else(PoisonError::into_inner);
        on_main(|| {
            SET_SIZE.store(0, Ordering::Relaxed);
            RESIZABLE.store(false, Ordering::Relaxed);
            ACCEPT.store(true, Ordering::Relaxed);
            let link = editor_link(400.0, 300.0).unwrap();
            link.host.gui_resize.store((700u64 << 32) | 500, Ordering::Relaxed);
            link.idle();
            assert_eq!(SET_SIZE.load(Ordering::Relaxed), 0);
            let win = link.lock().window.as_ref().unwrap().raw();
            assert_eq!(hostwin::client_size(win), Some((400.0, 300.0)));
            link.close();
        })
        .unwrap();
    }
}
