use super::{
    AtomicBool, AtomicU64, CLAP_EXT_GUI, CLAP_EXT_LATENCY, CLAP_EXT_LOG, CLAP_EXT_PARAMS, CLAP_EXT_STATE, CLAP_LOG_ERROR, CLAP_LOG_FATAL,
    CLAP_LOG_WARNING, CStr, EDIT_CAP, Mutex, Ordering, PoisonError, c_char, c_void, clap_host, clap_host_gui, clap_host_latency, clap_host_log,
    clap_host_params, clap_host_state, clap_id, clap_log_severity, clap_param_clear_flags, clap_param_rescan_flags, read_cstr,
};
use clap_sys::ext::thread_check::{CLAP_EXT_THREAD_CHECK, clap_host_thread_check};
use clap_sys::ext::timer_support::{CLAP_EXT_TIMER_SUPPORT, clap_host_timer_support};
use std::time::{Duration, Instant};

/// Most timers one plugin may register (hostile-input cap).
const MAX_TIMERS: usize = 64;
/// Shortest timer period we honour (CLAP lets the host clamp it).
const MIN_TIMER_MS: u32 = 10;

/// A stable, process-unique number for the calling thread (never 0).
pub(crate) fn thread_token() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    thread_local! {
        static TOKEN: u64 = NEXT.fetch_add(1, Ordering::Relaxed);
    }
    TOKEN.try_with(|t| *t).unwrap_or(0)
}

/// A `clap.timer-support` timer a plugin registered.
struct Timer {
    id: clap_id,
    period: Duration,
    next: Instant,
}

#[derive(Default)]
struct Timers {
    next_id: clap_id,
    items: Vec<Timer>,
}

// ---- host side -----------------------------------------------------------------------------

/// Flags the plugin raises through the host callbacks (any thread).
pub(crate) struct HostState {
    pub restart: AtomicBool,
    pub process: AtomicBool,
    pub callback: AtomicBool,
    pub latency_changed: AtomicBool,
    pub params_rescan: AtomicBool,
    /// `clap_host_state.mark_dirty`.
    pub dirty: AtomicBool,
    /// `clap_host_gui` requests, serviced by the editor's `idle` on the main thread.
    pub gui_closed: AtomicBool,
    pub gui_destroyed: AtomicBool,
    pub gui_show: AtomicBool,
    pub gui_hide: AtomicBool,
    /// A size an embedded editor asked for (`width << 32 | height`; 0 = none pending).
    pub gui_resize: AtomicU64,
    /// Parameter changes the plugin reported (output events: typically edits in its GUI),
    /// waiting for the UI. Bounded by `EDIT_CAP`; the audio thread only `try_lock`s it.
    pub edits: Mutex<Vec<(u32, f64)>>,
    /// `thread_token` of the thread that drives the editor (0 = not seen yet).
    pub main_thread: AtomicU64,
    /// `thread_token` of the thread that last ran `process` (0 = not seen yet).
    pub audio_thread: AtomicU64,
    /// Timers registered through `clap.timer-support`; fired by the editor's `idle`.
    timers: Mutex<Timers>,
}

impl HostState {
    /// Notes the calling thread as the main (UI) thread.
    pub fn mark_main_thread(&self) {
        self.main_thread.store(thread_token(), Ordering::Relaxed);
    }

    /// Notes the calling thread as the audio thread.
    pub fn mark_audio_thread(&self) {
        self.audio_thread.store(thread_token(), Ordering::Relaxed);
    }

    /// Ids of the timers that are due at `now`, each rescheduled one period later.
    pub fn due_timers(&self, now: Instant) -> Vec<clap_id> {
        let mut t = self.timers.lock().unwrap_or_else(PoisonError::into_inner);
        let mut due = Vec::new();
        for timer in &mut t.items {
            if now >= timer.next {
                due.push(timer.id);
                timer.next = now + timer.period;
            }
        }
        due
    }

    fn register_timer(&self, period_ms: u32) -> Option<clap_id> {
        let mut t = self.timers.lock().unwrap_or_else(PoisonError::into_inner);
        if t.items.len() >= MAX_TIMERS {
            return None;
        }
        t.next_id = t.next_id.wrapping_add(1);
        let id = t.next_id;
        let period = Duration::from_millis(u64::from(period_ms.max(MIN_TIMER_MS)));
        t.items.push(Timer { id, period, next: Instant::now() + period });
        Some(id)
    }

    fn unregister_timer(&self, id: clap_id) -> bool {
        let mut t = self.timers.lock().unwrap_or_else(PoisonError::into_inner);
        let before = t.items.len();
        t.items.retain(|x| x.id != id);
        t.items.len() != before
    }
}

impl Default for HostState {
    fn default() -> Self {
        HostState {
            restart: AtomicBool::new(false),
            process: AtomicBool::new(false),
            callback: AtomicBool::new(false),
            latency_changed: AtomicBool::new(false),
            params_rescan: AtomicBool::new(false),
            dirty: AtomicBool::new(false),
            gui_closed: AtomicBool::new(false),
            gui_destroyed: AtomicBool::new(false),
            gui_show: AtomicBool::new(false),
            gui_hide: AtomicBool::new(false),
            gui_resize: AtomicU64::new(0),
            edits: Mutex::new(Vec::with_capacity(EDIT_CAP)),
            main_thread: AtomicU64::new(0),
            audio_thread: AtomicU64::new(0),
            timers: Mutex::new(Timers::default()),
        }
    }
}

/// SAFETY contract: `host` is null or one of our `clap_host` structs, whose `host_data` points
/// to the `HostState` owned by the same `Instance`.
unsafe fn state<'a>(host: *const clap_host) -> Option<&'a HostState> {
    if host.is_null() {
        return None;
    }
    // SAFETY: non-null host created by `Bundle::instantiate` (contract above).
    let data = unsafe { (*host).host_data } as *const HostState;
    // SAFETY: `host_data` points to the boxed `HostState`, alive as long as the instance.
    unsafe { data.as_ref() }
}

pub(super) unsafe extern "C" fn host_request_restart(host: *const clap_host) {
    // SAFETY: the plugin passes back the host pointer we gave it.
    if let Some(s) = unsafe { state(host) } {
        s.restart.store(true, Ordering::Relaxed);
    }
}

pub(super) unsafe extern "C" fn host_request_process(host: *const clap_host) {
    // SAFETY: the plugin passes back the host pointer we gave it.
    if let Some(s) = unsafe { state(host) } {
        s.process.store(true, Ordering::Relaxed);
    }
}

pub(super) unsafe extern "C" fn host_request_callback(host: *const clap_host) {
    // SAFETY: the plugin passes back the host pointer we gave it.
    if let Some(s) = unsafe { state(host) } {
        s.callback.store(true, Ordering::Relaxed);
    }
}

unsafe extern "C" fn host_log(_host: *const clap_host, severity: clap_log_severity, msg: *const c_char) {
    // SAFETY: CLAP passes a NUL-terminated message (or null, handled).
    let m = unsafe { read_cstr(msg) }.unwrap_or_default();
    match severity {
        CLAP_LOG_ERROR | CLAP_LOG_FATAL => log::error!("clap plugin: {m}"),
        CLAP_LOG_WARNING => log::warn!("clap plugin: {m}"),
        _ => log::debug!("clap plugin: {m}"),
    }
}

unsafe extern "C" fn host_params_rescan(host: *const clap_host, _flags: clap_param_rescan_flags) {
    // SAFETY: the plugin passes back the host pointer we gave it.
    if let Some(s) = unsafe { state(host) } {
        s.params_rescan.store(true, Ordering::Relaxed);
    }
}

unsafe extern "C" fn host_params_clear(_host: *const clap_host, _id: clap_id, _flags: clap_param_clear_flags) {}

unsafe extern "C" fn host_params_request_flush(host: *const clap_host) {
    // SAFETY: the plugin passes back the host pointer we gave it.
    if let Some(s) = unsafe { state(host) } {
        s.process.store(true, Ordering::Relaxed);
    }
}

unsafe extern "C" fn host_latency_changed(host: *const clap_host) {
    // SAFETY: the plugin passes back the host pointer we gave it.
    if let Some(s) = unsafe { state(host) } {
        s.latency_changed.store(true, Ordering::Relaxed);
    }
}

unsafe extern "C" fn host_state_mark_dirty(host: *const clap_host) {
    // SAFETY: the plugin passes back the host pointer we gave it.
    if let Some(s) = unsafe { state(host) } {
        s.dirty.store(true, Ordering::Relaxed);
    }
}

unsafe extern "C" fn host_gui_resize_hints_changed(_host: *const clap_host) {}

unsafe extern "C" fn host_gui_request_resize(host: *const clap_host, width: u32, height: u32) -> bool {
    // Floating windows belong to the plugin, which resizes them itself; an embedded editor's host
    // window is resized by the editor's `idle` on the main thread.
    // SAFETY: the plugin passes back the host pointer we gave it.
    if let Some(s) = unsafe { state(host) }
        && width > 0
        && height > 0
    {
        s.gui_resize.store((u64::from(width) << 32) | u64::from(height), Ordering::Relaxed);
    }
    true
}

unsafe extern "C" fn host_gui_request_show(host: *const clap_host) -> bool {
    // SAFETY: the plugin passes back the host pointer we gave it.
    match unsafe { state(host) } {
        Some(s) => {
            s.gui_show.store(true, Ordering::Relaxed);
            true
        }
        None => false,
    }
}

unsafe extern "C" fn host_gui_request_hide(host: *const clap_host) -> bool {
    // SAFETY: the plugin passes back the host pointer we gave it.
    match unsafe { state(host) } {
        Some(s) => {
            s.gui_hide.store(true, Ordering::Relaxed);
            true
        }
        None => false,
    }
}

unsafe extern "C" fn host_gui_closed(host: *const clap_host, was_destroyed: bool) {
    // SAFETY: the plugin passes back the host pointer we gave it.
    if let Some(s) = unsafe { state(host) } {
        if was_destroyed {
            s.gui_destroyed.store(true, Ordering::Relaxed);
        }
        s.gui_closed.store(true, Ordering::Relaxed);
    }
}

static HOST_STATE: clap_host_state = clap_host_state { mark_dirty: Some(host_state_mark_dirty) };

static HOST_GUI: clap_host_gui = clap_host_gui {
    resize_hints_changed: Some(host_gui_resize_hints_changed),
    request_resize: Some(host_gui_request_resize),
    request_show: Some(host_gui_request_show),
    request_hide: Some(host_gui_request_hide),
    closed: Some(host_gui_closed),
};

unsafe extern "C" fn host_is_main_thread(host: *const clap_host) -> bool {
    // SAFETY: the plugin passes back the host pointer we gave it.
    match unsafe { state(host) } {
        // Until the editor has been driven once, assume the plugin runs on the main thread (it is
        // created there).
        Some(s) => match s.main_thread.load(Ordering::Relaxed) {
            0 => true,
            m => m == thread_token(),
        },
        None => false,
    }
}

unsafe extern "C" fn host_is_audio_thread(host: *const clap_host) -> bool {
    // SAFETY: the plugin passes back the host pointer we gave it.
    match unsafe { state(host) } {
        Some(s) => match s.audio_thread.load(Ordering::Relaxed) {
            0 => false,
            a => a == thread_token(),
        },
        None => false,
    }
}

unsafe extern "C" fn host_register_timer(host: *const clap_host, period_ms: u32, timer_id: *mut clap_id) -> bool {
    if timer_id.is_null() {
        return false;
    }
    // SAFETY: the plugin passes back the host pointer we gave it.
    let Some(id) = (unsafe { state(host) }).and_then(|s| s.register_timer(period_ms)) else { return false };
    // SAFETY: non-null (checked), and CLAP says it points to a writable `clap_id`.
    unsafe { *timer_id = id };
    true
}

unsafe extern "C" fn host_unregister_timer(host: *const clap_host, timer_id: clap_id) -> bool {
    // SAFETY: the plugin passes back the host pointer we gave it.
    (unsafe { state(host) }).is_some_and(|s| s.unregister_timer(timer_id))
}

static HOST_THREAD_CHECK: clap_host_thread_check =
    clap_host_thread_check { is_main_thread: Some(host_is_main_thread), is_audio_thread: Some(host_is_audio_thread) };

static HOST_TIMER: clap_host_timer_support =
    clap_host_timer_support { register_timer: Some(host_register_timer), unregister_timer: Some(host_unregister_timer) };

static HOST_LOG: clap_host_log = clap_host_log { log: Some(host_log) };

static HOST_PARAMS: clap_host_params =
    clap_host_params { rescan: Some(host_params_rescan), clear: Some(host_params_clear), request_flush: Some(host_params_request_flush) };

static HOST_LATENCY: clap_host_latency = clap_host_latency { changed: Some(host_latency_changed) };

pub(super) unsafe extern "C" fn host_get_extension(_host: *const clap_host, id: *const c_char) -> *const c_void {
    if id.is_null() {
        return std::ptr::null();
    }
    // SAFETY: extension ids are NUL-terminated strings (CLAP contract); non-null checked above.
    let id = unsafe { CStr::from_ptr(id) };
    if id == CLAP_EXT_LOG {
        (&HOST_LOG as *const clap_host_log).cast()
    } else if id == CLAP_EXT_PARAMS {
        (&HOST_PARAMS as *const clap_host_params).cast()
    } else if id == CLAP_EXT_LATENCY {
        (&HOST_LATENCY as *const clap_host_latency).cast()
    } else if id == CLAP_EXT_STATE {
        (&HOST_STATE as *const clap_host_state).cast()
    } else if id == CLAP_EXT_GUI {
        (&HOST_GUI as *const clap_host_gui).cast()
    } else if id == CLAP_EXT_THREAD_CHECK {
        (&HOST_THREAD_CHECK as *const clap_host_thread_check).cast()
    } else if id == CLAP_EXT_TIMER_SUPPORT {
        (&HOST_TIMER as *const clap_host_timer_support).cast()
    } else {
        // Everything else (fd support, …) is unsupported: CLAP allows null.
        std::ptr::null()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timers_fire_when_due_and_stop_when_unregistered() {
        let s = HostState::default();
        let id = s.register_timer(20);
        assert!(id.is_some());
        let id = id.unwrap_or(0);
        let t0 = Instant::now();
        assert!(s.due_timers(t0).is_empty(), "not due yet");
        assert_eq!(s.due_timers(t0 + Duration::from_millis(30)), vec![id]);
        assert!(s.due_timers(t0 + Duration::from_millis(31)).is_empty(), "rescheduled one period later");
        assert!(s.unregister_timer(id));
        assert!(!s.unregister_timer(id), "already gone");
        assert!(s.due_timers(t0 + Duration::from_secs(5)).is_empty());
    }

    #[test]
    fn plugins_cannot_register_unbounded_timers() {
        let s = HostState::default();
        for _ in 0..MAX_TIMERS {
            assert!(s.register_timer(100).is_some());
        }
        assert!(s.register_timer(100).is_none());
    }

    #[test]
    fn thread_tokens_tell_threads_apart() {
        let here = thread_token();
        assert_ne!(here, 0);
        assert_eq!(here, thread_token());
        let there = std::thread::spawn(thread_token).join().unwrap_or(0);
        assert_ne!(there, 0);
        assert_ne!(here, there);
    }

    #[test]
    fn the_host_answers_thread_checks_per_thread() {
        let s = HostState::default();
        s.mark_main_thread();
        s.audio_thread.store(thread_token() + 1_000, Ordering::Relaxed);
        assert_eq!(s.main_thread.load(Ordering::Relaxed), thread_token());
        let host = clap_host {
            clap_version: clap_sys::version::CLAP_VERSION,
            host_data: (&s as *const HostState).cast_mut().cast(),
            name: c"test".as_ptr(),
            vendor: c"test".as_ptr(),
            url: c"".as_ptr(),
            version: c"0".as_ptr(),
            get_extension: Some(host_get_extension),
            request_restart: Some(host_request_restart),
            request_process: Some(host_request_process),
            request_callback: Some(host_request_callback),
        };
        // SAFETY: `host` is a valid `clap_host` whose `host_data` points to `s`, alive for the test.
        unsafe {
            assert!(host_is_main_thread(&host));
            assert!(!host_is_audio_thread(&host));
        }
    }
}
