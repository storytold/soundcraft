use super::instance::Instance;
#[cfg(all(unix, not(target_os = "macos")))]
use super::run_loop;
use super::{
    Class, ComPtr, ComWrapper, IComponentTrait, IEditControllerTrait, IPlugFrameTrait, IPlugViewTrait, Mutex, Ordering, PoisonError, STATE_MAGIC,
    c_void, ok, read_c16, sb, sv, window,
};

// ---- editor (IPlugView) --------------------------------------------------------------------

/// The `IPlugFrame` handed to a plugin view: `resizeView` resizes our host window. On X11 it is
/// also the view's `Linux::IRunLoop` (plugins query the frame for it).
pub(crate) struct PlugFrame {
    /// The host window (an `NSWindow*` on macOS, the X11 window id on Linux), not owned; null
    /// when there is none.
    window: std::sync::atomic::AtomicPtr<c_void>,
    #[cfg(all(unix, not(target_os = "macos")))]
    run_loop: run_loop::RunLoop,
}

impl PlugFrame {
    fn new(window: *mut c_void) -> PlugFrame {
        PlugFrame {
            window: std::sync::atomic::AtomicPtr::new(window),
            #[cfg(all(unix, not(target_os = "macos")))]
            run_loop: run_loop::RunLoop::default(),
        }
    }

    /// Services the view's run loop (X11 only): ready file descriptors and due timers.
    fn dispatch(&self) {
        #[cfg(all(unix, not(target_os = "macos")))]
        self.run_loop.dispatch(std::time::Instant::now());
    }
}

#[cfg(not(all(unix, not(target_os = "macos"))))]
impl Class for PlugFrame {
    type Interfaces = (sb::IPlugFrame,);
}

#[cfg(all(unix, not(target_os = "macos")))]
impl Class for PlugFrame {
    type Interfaces = (sb::IPlugFrame, sb::Linux::IRunLoop);
}

#[cfg(all(unix, not(target_os = "macos")))]
impl sb::Linux::IRunLoopTrait for PlugFrame {
    unsafe fn registerEventHandler(&self, handler: *mut sb::Linux::IEventHandler, fd: sb::Linux::FileDescriptor) -> sb::tresult {
        // SAFETY: in-pointer from the plugin (or null, handled); we take our own reference.
        match unsafe { vst3::ComRef::from_raw(handler) } {
            Some(h) if self.run_loop.add_fd(h.to_com_ptr(), fd) => sb::kResultOk,
            _ => sb::kInvalidArgument,
        }
    }

    unsafe fn unregisterEventHandler(&self, handler: *mut sb::Linux::IEventHandler) -> sb::tresult {
        if self.run_loop.remove_fd(handler) { sb::kResultOk } else { sb::kInvalidArgument }
    }

    unsafe fn registerTimer(&self, handler: *mut sb::Linux::ITimerHandler, milliseconds: sb::Linux::TimerInterval) -> sb::tresult {
        // SAFETY: as in `registerEventHandler`.
        match unsafe { vst3::ComRef::from_raw(handler) } {
            Some(h) if self.run_loop.add_timer(h.to_com_ptr(), milliseconds) => sb::kResultOk,
            _ => sb::kInvalidArgument,
        }
    }

    unsafe fn unregisterTimer(&self, handler: *mut sb::Linux::ITimerHandler) -> sb::tresult {
        if self.run_loop.remove_timer(handler) { sb::kResultOk } else { sb::kInvalidArgument }
    }
}

impl IPlugFrameTrait for PlugFrame {
    unsafe fn resizeView(&self, view: *mut sb::IPlugView, new_size: *mut sb::ViewRect) -> sb::tresult {
        // SAFETY: in-pointer from the plugin (or null, handled).
        let Some(r) = (unsafe { new_size.as_mut() }) else { return sb::kInvalidArgument };
        // SAFETY: the plugin passes its own live view (or null, handled).
        let Some(v) = (unsafe { vst3::ComRef::from_raw(view) }) else { return sb::kInvalidArgument };
        // #1448: negotiate before touching the host. The request is capped at the work area's
        // fit, then `checkSizeConstraint` writes the nearest size the view supports — the size
        // the window is moved to and the size `onSize` is told, so both sides agree. A view
        // that refuses the fitted size keeps everything as it was.
        let (w, h) = rect_size(r);
        let (fw, fh) = window::fit_client(w, h);
        let mut fit = sb::ViewRect { left: r.left, top: r.top, right: r.left + fw as i32, bottom: r.top + fh as i32 };
        // SAFETY: valid view; `fit` is writable and the plugin may adjust it in place.
        if (fw, fh) != (w, h) && !ok(unsafe { v.checkSizeConstraint(&mut fit) }) {
            return sb::kResultFalse;
        }
        let (tw, th) = rect_size(&fit);
        let win = self.window.load(Ordering::Relaxed);
        let before = window::client_size(win);
        let (aw, ah) = window::resize(win, tw, th);
        fit.right = fit.left + aw as i32;
        fit.bottom = fit.top + ah as i32;
        // SAFETY: valid view; `fit` is writable. A view that refuses the applied size gets its
        // window restored, so the window and the view it holds stay the same size (#1448).
        if !ok(unsafe { v.onSize(&mut fit) }) {
            if let Some((bw, bh)) = before {
                window::resize(win, bw, bh);
            }
            return sb::kResultFalse;
        }
        sb::kResultOk
    }
}

/// Width and height of a view rect, clamped to a sane window size.
fn rect_size(r: &sb::ViewRect) -> (f64, f64) {
    let w = r.right.saturating_sub(r.left).clamp(1, 16_384);
    let h = r.bottom.saturating_sub(r.top).clamp(1, 16_384);
    (f64::from(w), f64::from(h))
}

#[derive(Default)]
pub(super) struct EditorInner {
    /// `None` once the instance is gone.
    pub(super) component: Option<ComPtr<sv::IComponent>>,
    /// `None` once the instance is gone, or without an edit controller.
    pub(super) controller: Option<ComPtr<sv::IEditController>>,
    pub(super) view: Option<ComPtr<sb::IPlugView>>,
    pub(super) frame: Option<ComWrapper<PlugFrame>>,
    pub(super) window: Option<window::HostWindow>,
}

/// The link between an instance and its editor and state handles. The instance may process on
/// the audio thread while the editor (the edit controller's `IPlugView`) is driven and the state
/// read from the main thread, which is VST3's own threading model (controller, views and
/// `getState` on the UI thread). All such calls happen under this lock, and the instance closes
/// the view and drops the component and controller references under the same lock before it
/// terminates, so a handle never calls into a dead plugin.
#[derive(Default)]
pub(crate) struct EditorLink {
    pub(super) inner: Mutex<EditorInner>,
}

impl EditorLink {
    fn lock(&self) -> std::sync::MutexGuard<'_, EditorInner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub(super) fn has_controller(&self) -> bool {
        self.lock().controller.is_some()
    }

    fn create_view(g: &EditorInner) -> Result<ComPtr<sb::IPlugView>, String> {
        let c = g.controller.as_ref().ok_or_else(|| "the plugin is gone".to_string())?;
        // SAFETY: valid controller; the name is a static NUL-terminated string.
        let raw = unsafe { c.createView(c"editor".as_ptr()) };
        // SAFETY: a non-null view comes with one reference owned by the caller.
        unsafe { ComPtr::from_raw(raw) }.ok_or_else(|| "this plugin has no editor".to_string())
    }

    /// Creates the view, checks it supports this platform's window type and reads its size,
    /// without attaching it anywhere. For diagnostics and tests.
    pub fn probe(&self) -> Result<(u32, u32), String> {
        let g = self.lock();
        let view = EditorLink::create_view(&g)?;
        let platform = window::PLATFORM_TYPE.ok_or_else(|| "plugin editors are not supported on this platform".to_string())?;
        // SAFETY: valid view; `platform` is a static NUL-terminated string.
        if unsafe { view.isPlatformTypeSupported(platform) } != sb::kResultTrue {
            return Err("the editor does not support this platform's windows".into());
        }
        let mut r = sb::ViewRect { left: 0, top: 0, right: 0, bottom: 0 };
        // SAFETY: valid view; `r` is writable.
        if !ok(unsafe { view.getSize(&mut r) }) {
            return Err("the editor reports no size".into());
        }
        let (w, h) = rect_size(&r);
        Ok((w as u32, h as u32))
    }

    /// Opens (or raises) the editor in a host window. Main thread only.
    pub fn open(&self, title: &str) -> Result<(), String> {
        if !window::on_main_thread() {
            return Err("plugin editors must be opened on the main thread".into());
        }
        let platform = window::PLATFORM_TYPE.ok_or_else(|| "VST3 editors are not hosted on this platform yet".to_string())?;
        let mut g = self.lock();
        if let Some(w) = &g.window {
            if w.is_visible() {
                w.raise();
                return Ok(());
            }
            // The user closed the window: start over.
            EditorLink::teardown(&mut g);
        }
        let view = EditorLink::create_view(&g)?;
        // SAFETY: valid view; `platform` is a static NUL-terminated string.
        if unsafe { view.isPlatformTypeSupported(platform) } != sb::kResultTrue {
            return Err("the editor does not support this platform's windows".into());
        }
        let mut r = sb::ViewRect { left: 0, top: 0, right: 400, bottom: 300 };
        // SAFETY: valid view; `r` is writable. On failure the default size stays.
        unsafe { view.getSize(&mut r) };
        let (mut w, mut h) = rect_size(&r);
        // #1448: a view too big for the monitor under the pointer gets the plugin's own nearest
        // supported size; a view that cannot resize keeps its size — the window then opens with
        // its title bar on-screen so every control stays reachable by dragging, never clipped.
        let (fw, fh) = window::fit_client(w, h);
        let mut negotiated = false;
        if (fw, fh) != (w, h) && unsafe { view.canResize() } == sb::kResultTrue {
            let mut fit = sb::ViewRect { left: r.left, top: r.top, right: r.left + fw as i32, bottom: r.top + fh as i32 };
            // SAFETY: valid view; `fit` is writable and the plugin may adjust it in place.
            if ok(unsafe { view.checkSizeConstraint(&mut fit) }) {
                r = fit;
                (w, h) = rect_size(&r);
                negotiated = true;
            }
        }
        let win = window::HostWindow::new(title, w, h)?;
        let frame = ComWrapper::new(PlugFrame::new(win.raw()));
        if let Some(f) = frame.as_com_ref::<sb::IPlugFrame>() {
            // SAFETY: valid view; the frame outlives the view's attachment (kept in `g.frame`
            // and detached in `teardown`).
            unsafe { view.setFrame(f.as_ptr()) };
        }
        // SAFETY: valid view; the parent is the live content view of `win`, of the type named.
        if !ok(unsafe { view.attached(win.parent(), platform) }) {
            // SAFETY: valid view; detach our frame again.
            unsafe { view.setFrame(std::ptr::null_mut()) };
            // Release anything it registered while attaching, as `teardown` does.
            #[cfg(all(unix, not(target_os = "macos")))]
            frame.run_loop.clear();
            win.close();
            return Err("the editor could not attach to its window".into());
        }
        if negotiated {
            // SAFETY: valid, attached view; `r` is the size the plugin agreed to and got.
            unsafe { view.onSize(&mut r) };
        }
        win.raise();
        g.view = Some(view);
        g.frame = Some(frame);
        g.window = Some(win);
        Ok(())
    }

    fn teardown(g: &mut EditorInner) {
        if let Some(view) = g.view.take() {
            // SAFETY: valid, attached view; main thread (callers check). `removed` before the
            // window goes, and the frame is detached before it is released.
            unsafe {
                view.removed();
                view.setFrame(std::ptr::null_mut());
            }
        }
        // Release the view's run-loop handlers even if the plugin still holds the frame.
        #[cfg(all(unix, not(target_os = "macos")))]
        if let Some(f) = &g.frame {
            f.run_loop.clear();
        }
        g.frame = None;
        if let Some(w) = g.window.take() {
            w.close();
        }
    }

    /// Hands a host-set value to the controller before a state save (`setParamNormalized`,
    /// under the lock like [`EditorLink::save_state`]). No-op once the instance is gone.
    pub fn sync_param(&self, id: u32, n: f64) {
        if !n.is_finite() {
            return;
        }
        let g = self.lock();
        if let Some(c) = &g.controller {
            // SAFETY: valid controller (the instance clears it under this lock before it terminates).
            unsafe { c.setParamNormalized(id, n.clamp(0.0, 1.0)) };
        }
    }

    /// The component state plus the controller state, in SoundCraft's container (see
    /// `STATE_MAGIC`). `None` when the component refuses `getState` or the instance is gone.
    pub fn save_state(&self) -> Option<Vec<u8>> {
        let g = self.lock();
        let component = g.component.as_ref()?;
        // SAFETY: valid component (the instance clears it under this lock before it terminates);
        // the stream lives in `read_state` for the whole call.
        let comp = Instance::read_state(|s| unsafe { component.getState(s) })?;
        let ctrl = match &g.controller {
            // SAFETY: valid controller; as above.
            Some(c) => Instance::read_state(|s| unsafe { c.getState(s) }).unwrap_or_default(),
            None => Vec::new(),
        };
        let mut out = Vec::with_capacity(12 + comp.len() + ctrl.len());
        out.extend_from_slice(STATE_MAGIC);
        out.extend_from_slice(&u32::try_from(comp.len()).ok()?.to_le_bytes());
        out.extend_from_slice(&comp);
        out.extend_from_slice(&u32::try_from(ctrl.len()).ok()?.to_le_bytes());
        out.extend_from_slice(&ctrl);
        Some(out)
    }

    /// Closes the editor. Main thread only (no-op elsewhere).
    pub fn close(&self) {
        if window::on_main_thread() {
            EditorLink::teardown(&mut self.lock());
        }
    }

    pub fn is_open(&self) -> bool {
        let g = self.lock();
        match &g.window {
            Some(w) if window::on_main_thread() => w.is_visible(),
            Some(_) => true,
            None => false,
        }
    }

    /// Main-thread housekeeping: tears the view down when the user closed its window.
    pub fn idle(&self) {
        if !window::on_main_thread() {
            return;
        }
        let mut g = self.lock();
        if g.window.as_ref().is_some_and(|w| !w.is_visible()) {
            EditorLink::teardown(&mut g);
        } else if let Some(f) = &g.frame {
            f.dispatch();
        }
    }

    /// Normalized → plain through the controller (main thread).
    pub fn to_plain(&self, id: u32, n: f64) -> Option<f64> {
        let g = self.lock();
        let c = g.controller.as_ref()?;
        // SAFETY: valid controller.
        let v = unsafe { c.normalizedParamToPlain(id, n.clamp(0.0, 1.0)) };
        v.is_finite().then_some(v)
    }

    /// Plain → normalized through the controller (main thread).
    pub fn to_normalized(&self, id: u32, plain: f64) -> Option<f64> {
        let g = self.lock();
        let c = g.controller.as_ref()?;
        if !plain.is_finite() {
            return None;
        }
        // SAFETY: valid controller.
        let v = unsafe { c.plainParamToNormalized(id, plain) };
        v.is_finite().then(|| v.clamp(0.0, 1.0))
    }

    /// The plugin's own text for a normalized value (`getParamStringByValue`). Main thread only
    /// (`None` elsewhere, and once the instance is gone).
    pub fn value_text(&self, id: u32, n: f64) -> Option<String> {
        if !window::on_main_thread() || !n.is_finite() {
            return None;
        }
        let g = self.lock();
        let c = g.controller.as_ref()?;
        let mut s: sv::String128 = [0; 128];
        // SAFETY: valid controller (the instance clears it under this lock before it terminates),
        // main thread; `s` is a writable String128.
        ok(unsafe { c.getParamStringByValue(id, n.clamp(0.0, 1.0), &mut s) }).then(|| read_c16(&s)).filter(|t| !t.is_empty())
    }

    /// Shows a value change in the editor (`setParamNormalized`, main thread).
    pub fn set_normalized(&self, id: u32, n: f64) {
        if !window::on_main_thread() || !n.is_finite() {
            return;
        }
        let g = self.lock();
        if let Some(c) = &g.controller {
            // SAFETY: valid controller, main thread.
            unsafe { c.setParamNormalized(id, n.clamp(0.0, 1.0)) };
        }
    }

    /// Called by the instance before it terminates: closes the editor and cuts the handles off.
    /// Returns whether plugin objects had to be leaked.
    pub(super) fn kill(&self) -> bool {
        let mut g = self.lock();
        let mut leaked = false;
        if g.view.is_some() || g.window.is_some() {
            if window::on_main_thread() {
                EditorLink::teardown(&mut g);
            } else {
                // Window calls are main-thread only; leak the view and window rather than crash.
                log::warn!("vst3: editor still open while its plugin is destroyed off the main thread");
                if let Some(v) = g.view.take() {
                    std::mem::forget(v);
                    leaked = true;
                }
                // `HostWindow` has no destructor: dropping it leaks its retain on purpose.
                g.window = None;
                if let Some(f) = g.frame.take() {
                    std::mem::forget(f);
                }
            }
        }
        g.component = None;
        g.controller = None;
        leaked
    }
}

/// The run loop is X11-only, and so are its tests.
#[cfg(all(test, unix, not(target_os = "macos")))]
mod tests {
    use super::*;

    /// A plugin-side handler for the run-loop test: counts its callbacks.
    #[derive(Default)]
    struct Ticks {
        timer: std::sync::atomic::AtomicU32,
        fd: std::sync::atomic::AtomicU32,
    }

    impl Class for Ticks {
        type Interfaces = (sb::Linux::ITimerHandler, sb::Linux::IEventHandler);
    }

    impl sb::Linux::ITimerHandlerTrait for Ticks {
        unsafe fn onTimer(&self) {
            self.timer.fetch_add(1, Ordering::Relaxed);
        }
    }

    impl sb::Linux::IEventHandlerTrait for Ticks {
        unsafe fn onFDIsSet(&self, _fd: sb::Linux::FileDescriptor) {
            self.fd.fetch_add(1, Ordering::Relaxed);
        }
    }

    #[test]
    fn the_run_loop_fires_due_timers_and_ready_descriptors() {
        use sb::Linux::IRunLoopTrait;
        use std::time::{Duration, Instant};
        let frame = ComWrapper::new(PlugFrame::new(std::ptr::null_mut()));
        let rl = frame.as_com_ref::<sb::Linux::IRunLoop>().expect("the frame is the view's run loop");
        let ticks = ComWrapper::new(Ticks::default());
        let timer = ticks.as_com_ref::<sb::Linux::ITimerHandler>().unwrap().as_ptr();
        let handler = ticks.as_com_ref::<sb::Linux::IEventHandler>().unwrap().as_ptr();
        // Before registering: every timer's first tick is then later than `t0`.
        let t0 = Instant::now();
        let mut fds = [0; 2];
        // SAFETY: `fds` holds the two descriptors `pipe` writes.
        assert_eq!(unsafe { libc::pipe(fds.as_mut_ptr()) }, 0);
        // SAFETY: test calls through our own implementation with live handler objects.
        unsafe {
            assert_eq!(rl.registerTimer(timer, 10), sb::kResultOk);
            assert_eq!(rl.registerEventHandler(handler, fds[0]), sb::kResultOk);
            assert_ne!(rl.registerEventHandler(handler, -1), sb::kResultOk);
            assert_ne!(rl.registerTimer(std::ptr::null_mut(), 10), sb::kResultOk);
        }
        frame.run_loop.dispatch(t0);
        assert_eq!(ticks.timer.load(Ordering::Relaxed), 0, "not due yet");
        assert_eq!(ticks.fd.load(Ordering::Relaxed), 0, "nothing to read");
        // SAFETY: a valid one-byte buffer written to our own pipe.
        assert_eq!(unsafe { libc::write(fds[1], b"x".as_ptr().cast(), 1) }, 1);
        frame.run_loop.dispatch(t0 + Duration::from_secs(1));
        assert_eq!(ticks.timer.load(Ordering::Relaxed), 1, "a late frame fires once, not once per missed period");
        assert_eq!(ticks.fd.load(Ordering::Relaxed), 1);
        // SAFETY: as above.
        unsafe {
            assert_eq!(rl.unregisterTimer(timer), sb::kResultOk);
            assert_ne!(rl.unregisterTimer(timer), sb::kResultOk, "already gone");
            assert_eq!(rl.unregisterEventHandler(handler), sb::kResultOk);
        }
        frame.run_loop.dispatch(t0 + Duration::from_secs(2));
        assert_eq!((ticks.timer.load(Ordering::Relaxed), ticks.fd.load(Ordering::Relaxed)), (1, 1), "unregistered handlers stay quiet");
        // SAFETY: as above.
        unsafe { assert_eq!(rl.registerTimer(timer, 10), sb::kResultOk) };
        frame.run_loop.clear();
        frame.run_loop.dispatch(t0 + Duration::from_secs(3));
        assert_eq!(ticks.timer.load(Ordering::Relaxed), 1, "cleared on teardown");
        // SAFETY: closing the pipe we opened.
        unsafe {
            libc::close(fds[0]);
            libc::close(fds[1]);
        }
    }

    /// A plugin-side handler that calls back into the run loop from its destructor, as a plugin's
    /// handler may when its last reference is released.
    struct Reentrant {
        run_loop: ComPtr<sb::Linux::IRunLoop>,
    }

    impl Class for Reentrant {
        type Interfaces = (sb::Linux::ITimerHandler, sb::Linux::IEventHandler);
    }

    impl sb::Linux::ITimerHandlerTrait for Reentrant {
        unsafe fn onTimer(&self) {}
    }

    impl sb::Linux::IEventHandlerTrait for Reentrant {
        unsafe fn onFDIsSet(&self, _fd: sb::Linux::FileDescriptor) {}
    }

    impl Drop for Reentrant {
        fn drop(&mut self) {
            use sb::Linux::IRunLoopTrait;
            // SAFETY: a call through our own run loop, which the handler keeps alive.
            unsafe { self.run_loop.unregisterTimer(std::ptr::null_mut()) };
        }
    }

    /// Unregistering or clearing releases the plugin's handler after the run loop's lock, so a
    /// handler whose destructor calls back into the run loop does not deadlock.
    #[test]
    fn releasing_a_handler_may_call_back_into_the_run_loop() {
        use sb::Linux::IRunLoopTrait;
        let frame = ComWrapper::new(PlugFrame::new(std::ptr::null_mut()));
        let rl = frame.to_com_ptr::<sb::Linux::IRunLoop>().expect("the frame is the view's run loop");
        for case in 0..3 {
            let handler = ComWrapper::new(Reentrant { run_loop: rl.clone() });
            let timer = handler.as_com_ref::<sb::Linux::ITimerHandler>().unwrap().as_ptr();
            let events = handler.as_com_ref::<sb::Linux::IEventHandler>().unwrap().as_ptr();
            // SAFETY: test calls through our own implementation with a live handler object.
            unsafe {
                assert_eq!(rl.registerTimer(timer, 10), sb::kResultOk);
                assert_eq!(rl.registerEventHandler(events, 0), sb::kResultOk);
            }
            // From here on the run loop holds the only references.
            drop(handler);
            // SAFETY: as above; the handler stays alive until the run loop lets it go.
            unsafe {
                match case {
                    0 => {
                        assert_eq!(rl.unregisterTimer(timer), sb::kResultOk);
                        assert_eq!(rl.unregisterEventHandler(events), sb::kResultOk);
                    }
                    1 => {
                        assert_eq!(rl.unregisterEventHandler(events), sb::kResultOk);
                        assert_eq!(rl.unregisterTimer(timer), sb::kResultOk);
                    }
                    _ => frame.run_loop.clear(),
                }
            }
        }
    }
}

/// Windows regressions for `resizeView`: a view that constrains every size to its supported
/// minimum, and one that refuses `checkSizeConstraint` or `onSize` (#1448).
#[cfg(all(test, windows))]
mod win_tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicU64};
    use vst3::Steinberg::IPluginBaseTrait;

    /// Serialises the window tests: the fake view's state and real windows are shared.
    static GUARD: Mutex<()> = Mutex::new(());
    static ONSIZE: AtomicU64 = AtomicU64::new(0);
    static REFUSE_CONSTRAINT: AtomicBool = AtomicBool::new(false);
    static REFUSE_SIZE: AtomicBool = AtomicBool::new(false);
    /// The fake view's minimum size (`width << 32 | height`).
    static MIN: AtomicU64 = AtomicU64::new(0);

    /// A view with a supported-size contract: `checkSizeConstraint` never allows less than its
    /// minimum and `onSize` can refuse outright.
    struct ConstrainedView;

    impl Class for ConstrainedView {
        type Interfaces = (sb::IPlugView,);
    }

    impl IPluginBaseTrait for ConstrainedView {
        unsafe fn initialize(&self, _context: *mut sb::FUnknown) -> sb::tresult {
            sb::kResultOk
        }

        unsafe fn terminate(&self) -> sb::tresult {
            sb::kResultOk
        }
    }

    impl IPlugViewTrait for ConstrainedView {
        unsafe fn isPlatformTypeSupported(&self, _type: sb::FIDString) -> sb::tresult {
            sb::kResultTrue
        }

        unsafe fn attached(&self, _parent: *mut c_void, _type: sb::FIDString) -> sb::tresult {
            sb::kResultOk
        }

        unsafe fn removed(&self) -> sb::tresult {
            sb::kResultOk
        }

        unsafe fn onWheel(&self, _distance: f32) -> sb::tresult {
            sb::kResultOk
        }

        unsafe fn onKeyDown(&self, _key: sb::char16, _key_code: sb::int16, _modifiers: sb::int16) -> sb::tresult {
            sb::kResultOk
        }

        unsafe fn onKeyUp(&self, _key: sb::char16, _key_code: sb::int16, _modifiers: sb::int16) -> sb::tresult {
            sb::kResultOk
        }

        unsafe fn getSize(&self, size: *mut sb::ViewRect) -> sb::tresult {
            // SAFETY: the host's writable out-rect.
            unsafe {
                *size = sb::ViewRect { left: 0, top: 0, right: 400, bottom: 300 };
            }
            sb::kResultOk
        }

        unsafe fn onSize(&self, new_size: *mut sb::ViewRect) -> sb::tresult {
            // SAFETY: the host's readable in-rect.
            let r = unsafe { &*new_size };
            ONSIZE.store((u64::from((r.right - r.left) as u32) << 32) | u64::from((r.bottom - r.top) as u32), Ordering::Relaxed);
            if REFUSE_SIZE.load(Ordering::Relaxed) { sb::kResultFalse } else { sb::kResultOk }
        }

        unsafe fn onFocus(&self, _state: sb::TBool) -> sb::tresult {
            sb::kResultOk
        }

        unsafe fn setFrame(&self, _frame: *mut sb::IPlugFrame) -> sb::tresult {
            sb::kResultOk
        }

        unsafe fn canResize(&self) -> sb::tresult {
            sb::kResultTrue
        }

        unsafe fn checkSizeConstraint(&self, rect: *mut sb::ViewRect) -> sb::tresult {
            if REFUSE_CONSTRAINT.load(Ordering::Relaxed) {
                return sb::kResultFalse;
            }
            // SAFETY: the host's writable in/out rect; the only supported size is written in place.
            let r = unsafe { &mut *rect };
            let min = MIN.load(Ordering::Relaxed);
            if min != 0 {
                let (mw, mh) = ((min >> 32) as i32, min as u32 as i32);
                r.right = r.left + mw;
                r.bottom = r.top + mh;
            }
            sb::kResultTrue
        }
    }

    /// A frame on a real host window and a fake view, as the plugin would hand `resizeView`.
    fn fixture() -> Result<(window::HostWindow, ComWrapper<PlugFrame>, ComWrapper<ConstrainedView>, *mut sb::IPlugView), String> {
        let win = window::HostWindow::new("editor", 400.0, 300.0)?;
        let frame = ComWrapper::new(PlugFrame::new(win.raw()));
        let view = ComWrapper::new(ConstrainedView);
        let ptr = view.as_com_ref::<sb::IPlugView>().ok_or_else(|| "the fake view exposes no IPlugView".to_string())?.as_ptr();
        Ok((win, frame, view, ptr))
    }

    fn on_main(f: impl FnOnce() + Send + 'static) -> Result<(), String> {
        std::thread::Builder::new()
            .name("main".into())
            .spawn(f)
            .map_err(|e| e.to_string())?
            .join()
            .map_err(|_| "the test thread panicked".to_string())
    }

    fn reset() {
        ONSIZE.store(0, Ordering::Relaxed);
        REFUSE_CONSTRAINT.store(false, Ordering::Relaxed);
        REFUSE_SIZE.store(false, Ordering::Relaxed);
        MIN.store(0, Ordering::Relaxed);
    }

    /// An oversized request is negotiated to the view's supported size — a fixed-size editor's
    /// only answer — and that size is applied to both the window and `onSize`, never the shrunk
    /// work-area fit (#1448).
    #[test]
    fn a_resize_is_negotiated_before_the_window_moves() {
        let _guard = GUARD.lock().unwrap_or_else(PoisonError::into_inner);
        on_main(|| {
            reset();
            MIN.store((800u64 << 32) | 600, Ordering::Relaxed);
            let (win, frame, view, ptr) = fixture().unwrap();
            let mut r = sb::ViewRect { left: 0, top: 0, right: 9_000, bottom: 9_000 };
            // SAFETY: `frame` and `view` are our own live objects; `r` is writable.
            let result = unsafe { frame.resizeView(ptr, &mut r) };
            assert_eq!(result, sb::kResultOk);
            assert_eq!(ONSIZE.load(Ordering::Relaxed), (800u64 << 32) | 600, "onSize got the negotiated size");
            assert_eq!(window::client_size(win.raw()), Some((800.0, 600.0)), "the window got it too");
            drop((frame, view));
            win.close();
        })
        .unwrap();
    }

    /// A view that refuses the constraint check gets `kResultFalse`: the window is untouched and
    /// `onSize` is never called (#1448).
    #[test]
    fn a_refusing_view_keeps_its_window() {
        let _guard = GUARD.lock().unwrap_or_else(PoisonError::into_inner);
        on_main(|| {
            reset();
            REFUSE_CONSTRAINT.store(true, Ordering::Relaxed);
            let (win, frame, view, ptr) = fixture().unwrap();
            let mut r = sb::ViewRect { left: 0, top: 0, right: 9_000, bottom: 9_000 };
            // SAFETY: as above.
            let result = unsafe { frame.resizeView(ptr, &mut r) };
            assert_eq!(result, sb::kResultFalse);
            assert_eq!(ONSIZE.load(Ordering::Relaxed), 0, "onSize was not called");
            assert_eq!(window::client_size(win.raw()), Some((400.0, 300.0)), "the window was untouched");
            drop((frame, view));
            win.close();
        })
        .unwrap();
    }

    /// A view that refuses `onSize` gets `kResultFalse` and the window is restored to what it
    /// was, so the window and the view it holds stay the same size (#1448).
    #[test]
    fn a_refused_size_restores_the_window() {
        let _guard = GUARD.lock().unwrap_or_else(PoisonError::into_inner);
        on_main(|| {
            reset();
            REFUSE_SIZE.store(true, Ordering::Relaxed);
            let (win, frame, view, ptr) = fixture().unwrap();
            let mut r = sb::ViewRect { left: 0, top: 0, right: 700, bottom: 500 };
            // SAFETY: as above.
            let result = unsafe { frame.resizeView(ptr, &mut r) };
            assert_eq!(result, sb::kResultFalse);
            assert_eq!(ONSIZE.load(Ordering::Relaxed), (700u64 << 32) | 500, "the view was still asked");
            assert_eq!(window::client_size(win.raw()), Some((400.0, 300.0)), "the window was restored");
            drop((frame, view));
            win.close();
        })
        .unwrap();
    }
}
