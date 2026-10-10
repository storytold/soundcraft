//! Test-only CLAP plugins for soundcraft-clap-host: a gain effect and a "DC synth".
//!
//! - `org.soundcraft.test.gain`: stereo in/out, param 7 "Gain" (0..2, default 1), param 3 "Mode"
//!   (stepped enum A/B/C), hidden param 9, latency 3 samples.
//! - `org.soundcraft.test.synth`: instrument, stereo out, CLAP note port; outputs a constant equal
//!   to the velocity of the held note (0 when none), switching at the event's sample offset.
//!
//! The gain plugin also implements `clap.state` (gain and mode as two little-endian f64s) and a
//! floating-only `clap.gui` without a real window: showing it "edits" the gain to 0.25 in the GUI,
//! reported as an output parameter event by the next `process`.
#![allow(clippy::missing_safety_doc, non_upper_case_globals)]

use clap_sys::audio_buffer::clap_audio_buffer;
use clap_sys::entry::clap_plugin_entry;
use clap_sys::events::*;
use clap_sys::ext::audio_ports::*;
use clap_sys::ext::gui::*;
use clap_sys::ext::state::*;
use clap_sys::stream::*;
use clap_sys::ext::latency::*;
use clap_sys::ext::log::*;
use clap_sys::ext::note_ports::*;
use clap_sys::ext::params::*;
use clap_sys::factory::plugin_factory::*;
use clap_sys::host::clap_host;
use clap_sys::id::clap_id;
use clap_sys::plugin::*;
use clap_sys::process::*;
use clap_sys::version::CLAP_VERSION;
use std::ffi::{CStr, c_char, c_void};

struct Features<const N: usize>([*const c_char; N]);
unsafe impl<const N: usize> Sync for Features<N> {}

static GAIN_FEATURES: Features<3> = Features([c"audio-effect".as_ptr(), c"stereo".as_ptr(), std::ptr::null()]);
static SYNTH_FEATURES: Features<3> = Features([c"instrument".as_ptr(), c"synthesizer".as_ptr(), std::ptr::null()]);

static GAIN_DESC: clap_plugin_descriptor = clap_plugin_descriptor {
    clap_version: CLAP_VERSION,
    id: c"org.soundcraft.test.gain".as_ptr(),
    name: c"Fixture Gain".as_ptr(),
    vendor: c"SoundCraft tests".as_ptr(),
    url: c"".as_ptr(),
    manual_url: c"".as_ptr(),
    support_url: c"".as_ptr(),
    version: c"1.0.0".as_ptr(),
    description: c"test gain".as_ptr(),
    features: GAIN_FEATURES.0.as_ptr(),
};

static SYNTH_DESC: clap_plugin_descriptor = clap_plugin_descriptor {
    clap_version: CLAP_VERSION,
    id: c"org.soundcraft.test.synth".as_ptr(),
    name: c"Fixture Synth".as_ptr(),
    vendor: c"SoundCraft tests".as_ptr(),
    url: std::ptr::null(),
    manual_url: std::ptr::null(),
    support_url: std::ptr::null(),
    version: c"1.0.0".as_ptr(),
    description: std::ptr::null(),
    features: SYNTH_FEATURES.0.as_ptr(),
};

struct Fx {
    plugin: clap_plugin,
    host: *const clap_host,
    synth: bool,
    gain: f64,
    mode: f64,
    level: f64,
    active: bool,
    processing: bool,
    gui_created: bool,
    gui_visible: bool,
    gui_edit_pending: bool,
}

unsafe fn fx<'a>(p: *const clap_plugin) -> &'a mut Fx {
    unsafe { &mut *((*p).plugin_data as *mut Fx) }
}

unsafe extern "C" fn init(p: *const clap_plugin) -> bool {
    let f = unsafe { fx(p) };
    // Exercise the host's extension lookup + log callback.
    let host = unsafe { &*f.host };
    if let Some(ge) = host.get_extension {
        let log = unsafe { ge(f.host, CLAP_EXT_LOG.as_ptr()) } as *const clap_host_log;
        if let Some(l) = unsafe { log.as_ref() }.and_then(|l| l.log) {
            unsafe { l(f.host, CLAP_LOG_INFO, c"fixture init".as_ptr()) };
        }
        let none = unsafe { ge(f.host, c"clap.does-not-exist".as_ptr()) };
        if !none.is_null() {
            return false;
        }
    }
    true
}

unsafe extern "C" fn destroy(p: *const clap_plugin) {
    assert!(!unsafe { fx(p) }.gui_created, "plugin destroyed with its GUI alive");
    let data = unsafe { (*p).plugin_data } as *mut Fx;
    drop(unsafe { Box::from_raw(data) });
}

unsafe extern "C" fn activate(p: *const clap_plugin, sr: f64, min: u32, max: u32) -> bool {
    let f = unsafe { fx(p) };
    if f.active || !(sr > 0.0) || min == 0 || max < min {
        return false;
    }
    f.active = true;
    true
}

unsafe extern "C" fn deactivate(p: *const clap_plugin) {
    let f = unsafe { fx(p) };
    assert!(!f.processing, "deactivate while processing");
    f.active = false;
}

unsafe extern "C" fn start_processing(p: *const clap_plugin) -> bool {
    let f = unsafe { fx(p) };
    assert!(f.active, "start_processing while inactive");
    f.processing = true;
    true
}

unsafe extern "C" fn stop_processing(p: *const clap_plugin) {
    unsafe { fx(p) }.processing = false;
}

unsafe extern "C" fn reset(p: *const clap_plugin) {
    unsafe { fx(p) }.level = 0.0;
}

fn apply(f: &mut Fx, h: &clap_event_header) {
    if h.space_id != CLAP_CORE_EVENT_SPACE_ID {
        return;
    }
    match h.type_ {
        CLAP_EVENT_PARAM_VALUE => {
            let e = unsafe { &*(h as *const clap_event_header as *const clap_event_param_value) };
            match e.param_id {
                7 => f.gain = e.value,
                3 => f.mode = e.value,
                _ => {}
            }
        }
        CLAP_EVENT_NOTE_ON => {
            let e = unsafe { &*(h as *const clap_event_header as *const clap_event_note) };
            f.level = e.velocity;
        }
        CLAP_EVENT_NOTE_OFF => f.level = 0.0,
        _ => {}
    }
}

unsafe extern "C" fn process(p: *const clap_plugin, pr: *const clap_process) -> clap_process_status {
    let f = unsafe { fx(p) };
    let pr = unsafe { &*pr };
    assert!(f.processing, "process while not processing");
    let n = pr.frames_count as usize;
    let ev = unsafe { &*pr.in_events };
    let count = unsafe { (ev.size.unwrap())(pr.in_events) };
    if pr.audio_outputs_count != 1 {
        return CLAP_PROCESS_ERROR;
    }
    let out: &clap_audio_buffer = unsafe { &*pr.audio_outputs };
    let input: Option<&clap_audio_buffer> = if f.synth {
        if pr.audio_inputs_count != 0 {
            return CLAP_PROCESS_ERROR;
        }
        None
    } else {
        if pr.audio_inputs_count != 1 {
            return CLAP_PROCESS_ERROR;
        }
        Some(unsafe { &*pr.audio_inputs })
    };
    if f.gui_edit_pending {
        f.gui_edit_pending = false;
        f.gain = 0.25;
        let e = clap_event_param_value {
            header: clap_event_header {
                size: std::mem::size_of::<clap_event_param_value>() as u32,
                time: 0,
                space_id: CLAP_CORE_EVENT_SPACE_ID,
                type_: CLAP_EVENT_PARAM_VALUE,
                flags: 0,
            },
            param_id: 7,
            cookie: std::ptr::null_mut(),
            note_id: -1,
            port_index: -1,
            channel: -1,
            key: -1,
            value: 0.25,
        };
        let out = unsafe { &*pr.out_events };
        assert!(unsafe { (out.try_push.unwrap())(pr.out_events, &e.header) });
        // Other event types are accepted and ignored by the host.
        let note = clap_event_note {
            header: clap_event_header {
                size: std::mem::size_of::<clap_event_note>() as u32,
                time: 0,
                space_id: CLAP_CORE_EVENT_SPACE_ID,
                type_: CLAP_EVENT_NOTE_END,
                flags: 0,
            },
            note_id: -1,
            port_index: 0,
            channel: 0,
            key: 60,
            velocity: 0.0,
        };
        assert!(unsafe { (out.try_push.unwrap())(pr.out_events, &note.header) });
    }
    let mut next = 0u32;
    let mut last_time = 0u32;
    for i in 0..n {
        while next < count {
            let h = unsafe { &*(ev.get.unwrap())(pr.in_events, next) };
            assert!(h.time >= last_time, "events not sorted");
            assert!((h.time as usize) < n, "event time outside the block");
            if h.time as usize > i {
                break;
            }
            last_time = h.time;
            apply(f, h);
            next += 1;
        }
        for c in 0..out.channel_count as usize {
            let o = unsafe { *out.data32.add(c) };
            let v = match input {
                Some(inp) => {
                    let x = unsafe { *(*inp.data32.add(c.min(inp.channel_count as usize - 1))).add(i) };
                    x * f.gain as f32
                }
                None => f.level as f32,
            };
            unsafe { *o.add(i) = v };
        }
    }
    CLAP_PROCESS_CONTINUE
}

// ---- extensions ----

unsafe extern "C" fn params_count(p: *const clap_plugin) -> u32 {
    if unsafe { fx(p) }.synth { 0 } else { 3 }
}

fn write_name(dst: &mut [c_char], s: &CStr) {
    for (d, b) in dst.iter_mut().zip(s.to_bytes_with_nul()) {
        *d = *b as c_char;
    }
}

unsafe extern "C" fn params_get_info(_p: *const clap_plugin, index: u32, info: *mut clap_param_info) -> bool {
    let info = unsafe { &mut *info };
    let (id, name, flags, min, max, def): (clap_id, &CStr, u32, f64, f64, f64) = match index {
        0 => (7, c"Gain", CLAP_PARAM_IS_AUTOMATABLE, 0.0, 2.0, 1.0),
        1 => (3, c"Mode", CLAP_PARAM_IS_STEPPED | CLAP_PARAM_IS_ENUM, 0.0, 2.0, 0.0),
        2 => (9, c"Secret", CLAP_PARAM_IS_HIDDEN, 0.0, 1.0, 0.0),
        _ => return false,
    };
    info.id = id;
    info.flags = flags;
    info.cookie = std::ptr::null_mut();
    write_name(&mut info.name, name);
    write_name(&mut info.module, c"");
    info.min_value = min;
    info.max_value = max;
    info.default_value = def;
    true
}

unsafe extern "C" fn params_get_value(p: *const clap_plugin, id: clap_id, out: *mut f64) -> bool {
    let f = unsafe { fx(p) };
    let v = match id {
        7 => f.gain,
        3 => f.mode,
        9 => 0.0,
        _ => return false,
    };
    unsafe { *out = v };
    true
}

unsafe extern "C" fn params_value_to_text(_p: *const clap_plugin, id: clap_id, v: f64, buf: *mut c_char, cap: u32) -> bool {
    if id != 3 || cap < 2 {
        return false;
    }
    let label = [b'A', b'B', b'C'].get(v.round() as usize).copied().unwrap_or(b'?');
    unsafe {
        *buf = label as c_char;
        *buf.add(1) = 0;
    }
    true
}

unsafe extern "C" fn params_flush(p: *const clap_plugin, inp: *const clap_input_events, _out: *const clap_output_events) {
    let f = unsafe { fx(p) };
    let ev = unsafe { &*inp };
    let n = unsafe { (ev.size.unwrap())(inp) };
    for i in 0..n {
        apply(f, unsafe { &*(ev.get.unwrap())(inp, i) });
    }
}

static PARAMS: clap_plugin_params = clap_plugin_params {
    count: Some(params_count),
    get_info: Some(params_get_info),
    get_value: Some(params_get_value),
    value_to_text: Some(params_value_to_text),
    text_to_value: None,
    flush: Some(params_flush),
};

unsafe extern "C" fn ports_count(p: *const clap_plugin, is_input: bool) -> u32 {
    if is_input && unsafe { fx(p) }.synth { 0 } else { 1 }
}

unsafe extern "C" fn ports_get(p: *const clap_plugin, index: u32, is_input: bool, info: *mut clap_audio_port_info) -> bool {
    if index != 0 || (is_input && unsafe { fx(p) }.synth) {
        return false;
    }
    let info = unsafe { &mut *info };
    info.id = if is_input { 0 } else { 1 };
    write_name(&mut info.name, c"main");
    info.flags = CLAP_AUDIO_PORT_IS_MAIN;
    info.channel_count = 2;
    info.port_type = CLAP_PORT_STEREO.as_ptr();
    info.in_place_pair = clap_sys::id::CLAP_INVALID_ID;
    true
}

static PORTS: clap_plugin_audio_ports = clap_plugin_audio_ports { count: Some(ports_count), get: Some(ports_get) };

unsafe extern "C" fn latency_get(_p: *const clap_plugin) -> u32 {
    3
}

static LATENCY: clap_plugin_latency = clap_plugin_latency { get: Some(latency_get) };

unsafe extern "C" fn notes_count(p: *const clap_plugin, is_input: bool) -> u32 {
    if is_input && unsafe { fx(p) }.synth { 1 } else { 0 }
}

unsafe extern "C" fn notes_get(_p: *const clap_plugin, index: u32, is_input: bool, info: *mut clap_note_port_info) -> bool {
    if index != 0 || !is_input {
        return false;
    }
    let info = unsafe { &mut *info };
    info.id = 0;
    info.supported_dialects = CLAP_NOTE_DIALECT_CLAP | CLAP_NOTE_DIALECT_MIDI;
    info.preferred_dialect = CLAP_NOTE_DIALECT_CLAP;
    write_name(&mut info.name, c"notes");
    true
}

static NOTES: clap_plugin_note_ports = clap_plugin_note_ports { count: Some(notes_count), get: Some(notes_get) };

unsafe extern "C" fn state_save(p: *const clap_plugin, stream: *const clap_ostream) -> bool {
    let f = unsafe { fx(p) };
    let mut bytes = [0u8; 16];
    bytes[..8].copy_from_slice(&f.gain.to_le_bytes());
    bytes[8..].copy_from_slice(&f.mode.to_le_bytes());
    let s = unsafe { &*stream };
    // Write in two pieces to exercise appending.
    let a = unsafe { (s.write.unwrap())(stream, bytes.as_ptr().cast(), 5) };
    let b = unsafe { (s.write.unwrap())(stream, bytes[5..].as_ptr().cast(), 11) };
    a == 5 && b == 11
}

unsafe extern "C" fn state_load(p: *const clap_plugin, stream: *const clap_istream) -> bool {
    let f = unsafe { fx(p) };
    let s = unsafe { &*stream };
    let mut bytes = [0u8; 17];
    let mut got = 0usize;
    loop {
        let n = unsafe { (s.read.unwrap())(stream, bytes[got..].as_mut_ptr().cast(), (17 - got) as u64) };
        if n <= 0 {
            break;
        }
        got += n as usize;
        if got == 17 {
            break;
        }
    }
    if got != 16 {
        return false;
    }
    let gain = f64::from_le_bytes(bytes[..8].try_into().unwrap());
    let mode = f64::from_le_bytes(bytes[8..16].try_into().unwrap());
    if !(0.0..=2.0).contains(&gain) || !(0.0..=2.0).contains(&mode) {
        return false;
    }
    f.gain = gain;
    f.mode = mode;
    true
}

static STATE: clap_plugin_state = clap_plugin_state { save: Some(state_save), load: Some(state_load) };

unsafe extern "C" fn gui_is_api_supported(_p: *const clap_plugin, api: *const c_char, floating: bool) -> bool {
    assert!(!api.is_null());
    floating
}

unsafe extern "C" fn gui_create(p: *const clap_plugin, _api: *const c_char, floating: bool) -> bool {
    let f = unsafe { fx(p) };
    assert!(floating && !f.gui_created, "create");
    f.gui_created = true;
    true
}

unsafe extern "C" fn gui_destroy(p: *const clap_plugin) {
    let f = unsafe { fx(p) };
    assert!(f.gui_created, "destroy without create");
    f.gui_created = false;
    f.gui_visible = false;
}

unsafe extern "C" fn gui_set_scale(_p: *const clap_plugin, scale: f64) -> bool {
    scale > 0.0
}

unsafe extern "C" fn gui_suggest_title(p: *const clap_plugin, title: *const c_char) {
    assert!(unsafe { fx(p) }.gui_created);
    assert!(!unsafe { CStr::from_ptr(title) }.to_bytes().is_empty());
}

unsafe extern "C" fn gui_show(p: *const clap_plugin) -> bool {
    let f = unsafe { fx(p) };
    assert!(f.gui_created, "show without create");
    f.gui_visible = true;
    f.gui_edit_pending = true;
    // Exercise the host GUI extension.
    let host = unsafe { &*f.host };
    let hg = unsafe { (host.get_extension.unwrap())(f.host, CLAP_EXT_GUI.as_ptr()) } as *const clap_host_gui;
    let hg = unsafe { hg.as_ref() }.expect("host gui extension");
    assert!(unsafe { (hg.request_resize.unwrap())(f.host, 300, 200) });
    true
}

unsafe extern "C" fn gui_hide(p: *const clap_plugin) -> bool {
    unsafe { fx(p) }.gui_visible = false;
    true
}

static GUI: clap_plugin_gui = clap_plugin_gui {
    is_api_supported: Some(gui_is_api_supported),
    get_preferred_api: None,
    create: Some(gui_create),
    destroy: Some(gui_destroy),
    set_scale: Some(gui_set_scale),
    get_size: None,
    can_resize: None,
    get_resize_hints: None,
    adjust_size: None,
    set_size: None,
    set_parent: None,
    set_transient: None,
    suggest_title: Some(gui_suggest_title),
    show: Some(gui_show),
    hide: Some(gui_hide),
};

unsafe extern "C" fn get_extension(p: *const clap_plugin, id: *const c_char) -> *const c_void {
    let id = unsafe { CStr::from_ptr(id) };
    let synth = unsafe { fx(p) }.synth;
    if id == CLAP_EXT_PARAMS {
        &PARAMS as *const _ as *const c_void
    } else if id == CLAP_EXT_AUDIO_PORTS {
        &PORTS as *const _ as *const c_void
    } else if id == CLAP_EXT_LATENCY && !synth {
        &LATENCY as *const _ as *const c_void
    } else if id == CLAP_EXT_NOTE_PORTS && synth {
        &NOTES as *const _ as *const c_void
    } else if id == CLAP_EXT_STATE && !synth {
        &STATE as *const _ as *const c_void
    } else if id == CLAP_EXT_GUI && !synth {
        &GUI as *const _ as *const c_void
    } else {
        std::ptr::null()
    }
}

unsafe extern "C" fn on_main_thread(_p: *const clap_plugin) {}

// ---- factory + entry ----

unsafe extern "C" fn factory_count(_f: *const clap_plugin_factory) -> u32 {
    2
}

unsafe extern "C" fn factory_desc(_f: *const clap_plugin_factory, i: u32) -> *const clap_plugin_descriptor {
    match i {
        0 => &GAIN_DESC,
        1 => &SYNTH_DESC,
        _ => std::ptr::null(),
    }
}

unsafe extern "C" fn factory_create(_f: *const clap_plugin_factory, host: *const clap_host, id: *const c_char) -> *const clap_plugin {
    let id = unsafe { CStr::from_ptr(id) };
    let (desc, synth) = if id == c"org.soundcraft.test.gain" {
        (&GAIN_DESC as *const _, false)
    } else if id == c"org.soundcraft.test.synth" {
        (&SYNTH_DESC as *const _, true)
    } else {
        return std::ptr::null();
    };
    let b = Box::new(Fx {
        plugin: clap_plugin {
            desc,
            plugin_data: std::ptr::null_mut(),
            init: Some(init),
            destroy: Some(destroy),
            activate: Some(activate),
            deactivate: Some(deactivate),
            start_processing: Some(start_processing),
            stop_processing: Some(stop_processing),
            reset: Some(reset),
            process: Some(process),
            get_extension: Some(get_extension),
            on_main_thread: Some(on_main_thread),
        },
        host,
        synth,
        gain: 1.0,
        mode: 0.0,
        level: 0.0,
        active: false,
        processing: false,
        gui_created: false,
        gui_visible: false,
        gui_edit_pending: false,
    });
    let raw = Box::into_raw(b);
    unsafe {
        (*raw).plugin.plugin_data = raw as *mut c_void;
        &(*raw).plugin
    }
}

static FACTORY: clap_plugin_factory =
    clap_plugin_factory { get_plugin_count: Some(factory_count), get_plugin_descriptor: Some(factory_desc), create_plugin: Some(factory_create) };

unsafe extern "C" fn entry_init(path: *const c_char) -> bool {
    // SAFETY: the host passes the plugin's path as a NUL-terminated string (or null).
    if !path.is_null() && unsafe { CStr::from_ptr(path) }.to_string_lossy().contains("CrashOnLoad") {
        std::process::exit(86);
    }
    true
}
unsafe extern "C" fn entry_deinit() {}
unsafe extern "C" fn entry_get_factory(id: *const c_char) -> *const c_void {
    if unsafe { CStr::from_ptr(id) } == CLAP_PLUGIN_FACTORY_ID { &FACTORY as *const _ as *const c_void } else { std::ptr::null() }
}

#[unsafe(no_mangle)]
pub static clap_entry: clap_plugin_entry =
    clap_plugin_entry { clap_version: CLAP_VERSION, init: Some(entry_init), deinit: Some(entry_deinit), get_factory: Some(entry_get_factory) };
