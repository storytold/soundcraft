use super::{
    AtomicBool, AtomicU64, CLAP_EXT_GUI, CLAP_EXT_LATENCY, CLAP_EXT_LOG, CLAP_EXT_PARAMS, CLAP_EXT_STATE, CLAP_LOG_ERROR, CLAP_LOG_FATAL,
    CLAP_LOG_WARNING, CStr, EDIT_CAP, Mutex, Ordering, c_char, c_void, clap_host, clap_host_gui, clap_host_latency, clap_host_log, clap_host_params,
    clap_host_state, clap_id, clap_log_severity, clap_param_clear_flags, clap_param_rescan_flags, read_cstr,
};

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
    } else {
        // Everything else (thread-check, timers, fd support, …) is unsupported: CLAP allows null.
        std::ptr::null()
    }
}
