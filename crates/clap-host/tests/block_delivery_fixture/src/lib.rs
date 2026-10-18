//! Original processor oracle for whole-block delivery. All receipts come from process(), not
//! host-side parameter getters. No allocation occurs in process(). State is an out-of-band probe.
#![allow(clippy::missing_safety_doc)]
use clap_sys::{
    entry::*,
    events::*,
    ext::{audio_ports::*, note_ports::*, params::*, state::*},
    factory::plugin_factory::*,
    host::*,
    plugin::*,
    process::*,
    stream::*,
    version::CLAP_VERSION,
};
use std::ffi::{CStr, c_char, c_void};

struct Features([*const c_char; 3]);
unsafe impl Sync for Features {}
static FEATURES: Features = Features([c"audio-effect".as_ptr(), c"stereo".as_ptr(), std::ptr::null()]);
static DESC: clap_plugin_descriptor = clap_plugin_descriptor {
    clap_version: CLAP_VERSION,
    id: c"org.soundcraft.test.block-delivery".as_ptr(),
    name: c"Block Delivery Oracle".as_ptr(),
    vendor: c"SoundCraft tests".as_ptr(),
    url: c"".as_ptr(),
    manual_url: c"".as_ptr(),
    support_url: c"".as_ptr(),
    version: c"1".as_ptr(),
    description: c"Original whole-block processor oracle".as_ptr(),
    features: FEATURES.0.as_ptr(),
};
struct Oracle {
    plugin: clap_plugin,
    values: [f64; 4096],
    receipt: [u64; 8],
    mode: u32,
    level: f32,
}
unsafe fn oracle<'a>(p: *const clap_plugin) -> &'a mut Oracle {
    unsafe { &mut *((*p).plugin_data.cast::<Oracle>()) }
}
unsafe extern "C" fn init(_: *const clap_plugin) -> bool {
    true
}
unsafe extern "C" fn destroy(p: *const clap_plugin) {
    unsafe {
        drop(Box::from_raw((*p).plugin_data.cast::<Oracle>()));
    }
}
unsafe extern "C" fn activate(_: *const clap_plugin, _: f64, _: u32, _: u32) -> bool {
    true
}
unsafe extern "C" fn empty(_: *const clap_plugin) {}
unsafe extern "C" fn reset(p: *const clap_plugin) {
    unsafe { oracle(p) }.level = 0.0;
}
unsafe extern "C" fn start(p: *const clap_plugin) -> bool {
    let f = unsafe { oracle(p) };
    if f.mode == 3 {
        f.mode = 0;
        false
    } else {
        true
    }
}
fn apply(f: &mut Oracle, h: &clap_event_header) {
    if h.space_id != CLAP_CORE_EVENT_SPACE_ID {
        return;
    }
    match h.type_ {
        CLAP_EVENT_PARAM_VALUE => {
            let e = unsafe { &*(h as *const _ as *const clap_event_param_value) };
            if let Some(v) = f.values.get_mut(e.param_id as usize) {
                *v = e.value;
                f.receipt[3] += 1;
            }
        }
        CLAP_EVENT_NOTE_ON => {
            let e = unsafe { &*(h as *const _ as *const clap_event_note) };
            f.level = e.velocity as f32;
            f.receipt[4] += 1;
        }
        CLAP_EVENT_NOTE_OFF => {
            f.level = 0.0;
            f.receipt[5] += 1;
        }
        CLAP_EVENT_MIDI => {
            let e = unsafe { &*(h as *const _ as *const clap_event_midi) };
            if e.data[0] == 0xb0 && e.data[1] == 7 {
                f.level = f32::from(e.data[2]) / 127.0;
            }
            f.receipt[6] += 1;
        }
        _ => {}
    }
}
unsafe extern "C" fn process(p: *const clap_plugin, pr: *const clap_process) -> clap_process_status {
    let f = unsafe { oracle(p) };
    let pr = unsafe { &*pr };
    f.receipt[0] += 1;
    f.receipt[1] += u64::from(pr.frames_count);
    f.receipt[2] = u64::from(pr.frames_count);
    f.receipt[3..7].fill(0);
    if pr.audio_outputs_count != 1 || pr.audio_inputs_count != 1 {
        return CLAP_PROCESS_ERROR;
    }
    let events = unsafe { &*pr.in_events };
    let count = unsafe { (events.size.unwrap())(pr.in_events) };
    let mut next = 0;
    let mut last = 0;
    let out = unsafe { &*pr.audio_outputs };
    let input = unsafe { &*pr.audio_inputs };
    for i in 0..pr.frames_count as usize {
        while next < count {
            let h = unsafe { &*(events.get.unwrap())(pr.in_events, next) };
            if h.time < last || h.time >= pr.frames_count {
                f.receipt[7] += 1;
            }
            if h.time as usize > i {
                break;
            }
            last = h.time;
            apply(f, h);
            next += 1;
        }
        for ch in 0..out.channel_count as usize {
            let x = unsafe { *(*input.data32.add(ch)).add(i) };
            unsafe {
                *(*out.data32.add(ch)).add(i) = x * f.values[0] as f32 + f.values[4095] as f32 * 0.125 + f.level;
            }
        }
    }
    match f.mode {
        1 => {
            f.mode = 0;
            CLAP_PROCESS_ERROR
        }
        2 => {
            f.mode = 0;
            99
        }
        5 => CLAP_PROCESS_CONTINUE_IF_NOT_QUIET,
        6 => CLAP_PROCESS_TAIL,
        7 => CLAP_PROCESS_SLEEP,
        _ => CLAP_PROCESS_CONTINUE,
    }
}
unsafe extern "C" fn param_count(_: *const clap_plugin) -> u32 {
    4096
}
unsafe extern "C" fn param_info(_: *const clap_plugin, i: u32, p: *mut clap_param_info) -> bool {
    if i >= 4096 {
        return false;
    }
    let p = unsafe { &mut *p };
    p.id = i;
    p.flags = CLAP_PARAM_IS_AUTOMATABLE;
    p.cookie = std::ptr::null_mut();
    p.name.fill(0);
    p.name[0] = b'P' as c_char;
    p.module.fill(0);
    p.min_value = 0.0;
    p.max_value = 2.0;
    p.default_value = 1.0;
    true
}
unsafe extern "C" fn param_value(p: *const clap_plugin, id: u32, v: *mut f64) -> bool {
    match unsafe { oracle(p) }.values.get(id as usize) {
        Some(x) => {
            unsafe {
                *v = *x;
            }
            true
        }
        None => false,
    }
}
unsafe extern "C" fn flush(p: *const clap_plugin, e: *const clap_input_events, _: *const clap_output_events) {
    let f = unsafe { oracle(p) };
    let e = unsafe { &*e };
    for i in 0..unsafe { (e.size.unwrap())(e) } {
        apply(f, unsafe { &*(e.get.unwrap())(e, i) });
    }
}
static PARAMS: clap_plugin_params = clap_plugin_params {
    count: Some(param_count),
    get_info: Some(param_info),
    get_value: Some(param_value),
    value_to_text: None,
    text_to_value: None,
    flush: Some(flush),
};
unsafe extern "C" fn port_count(_: *const clap_plugin, _: bool) -> u32 {
    1
}
unsafe extern "C" fn port_info(_: *const clap_plugin, i: u32, input: bool, p: *mut clap_audio_port_info) -> bool {
    if i != 0 {
        return false;
    }
    let p = unsafe { &mut *p };
    p.id = u32::from(!input);
    p.name.fill(0);
    p.flags = CLAP_AUDIO_PORT_IS_MAIN;
    p.channel_count = 2;
    p.port_type = CLAP_PORT_STEREO.as_ptr();
    p.in_place_pair = clap_sys::id::CLAP_INVALID_ID;
    true
}
static PORTS: clap_plugin_audio_ports = clap_plugin_audio_ports { count: Some(port_count), get: Some(port_info) };
unsafe extern "C" fn note_count(_: *const clap_plugin, input: bool) -> u32 {
    u32::from(input)
}
unsafe extern "C" fn note_info(_: *const clap_plugin, i: u32, input: bool, p: *mut clap_note_port_info) -> bool {
    if i != 0 || !input {
        return false;
    }
    let p = unsafe { &mut *p };
    p.id = 0;
    p.name.fill(0);
    p.supported_dialects = CLAP_NOTE_DIALECT_CLAP | CLAP_NOTE_DIALECT_MIDI;
    p.preferred_dialect = CLAP_NOTE_DIALECT_CLAP;
    true
}
static NOTES: clap_plugin_note_ports = clap_plugin_note_ports { count: Some(note_count), get: Some(note_info) };
unsafe extern "C" fn save(p: *const clap_plugin, s: *const clap_ostream) -> bool {
    let f = unsafe { oracle(p) };
    let stream = unsafe { &*s };
    let mut bytes = Vec::with_capacity(64 + 4096 * 8);
    for v in f.receipt {
        bytes.extend_from_slice(&v.to_le_bytes());
    }
    for v in f.values {
        bytes.extend_from_slice(&v.to_le_bytes());
    }
    unsafe { (stream.write.unwrap())(s, bytes.as_ptr().cast(), bytes.len() as u64) == bytes.len() as i64 }
}
unsafe extern "C" fn load(p: *const clap_plugin, s: *const clap_istream) -> bool {
    let stream = unsafe { &*s };
    let mut bytes = [0u8; 9];
    let mut got = 0;
    while got < bytes.len() {
        let n = unsafe { (stream.read.unwrap())(s, bytes[got..].as_mut_ptr().cast(), (bytes.len() - got) as u64) };
        if n <= 0 {
            break;
        }
        got += n as usize;
    }
    if got != 8 || &bytes[..4] != b"BD01" {
        return false;
    }
    let mode = u32::from_le_bytes(bytes[4..8].try_into().unwrap());
    if mode > 7 {
        return false;
    }
    unsafe { oracle(p) }.mode = mode;
    true
}
static STATE: clap_plugin_state = clap_plugin_state { save: Some(save), load: Some(load) };
unsafe extern "C" fn extension(_: *const clap_plugin, id: *const c_char) -> *const c_void {
    let id = unsafe { CStr::from_ptr(id) };
    if id == CLAP_EXT_PARAMS {
        &PARAMS as *const _ as *const c_void
    } else if id == CLAP_EXT_AUDIO_PORTS {
        &PORTS as *const _ as *const c_void
    } else if id == CLAP_EXT_NOTE_PORTS {
        &NOTES as *const _ as *const c_void
    } else if id == CLAP_EXT_STATE {
        &STATE as *const _ as *const c_void
    } else {
        std::ptr::null()
    }
}
unsafe extern "C" fn count(_: *const clap_plugin_factory) -> u32 {
    1
}
unsafe extern "C" fn descriptor(_: *const clap_plugin_factory, i: u32) -> *const clap_plugin_descriptor {
    if i == 0 { &DESC } else { std::ptr::null() }
}
unsafe extern "C" fn create(_: *const clap_plugin_factory, _: *const clap_host, id: *const c_char) -> *const clap_plugin {
    if unsafe { CStr::from_ptr(id) } != c"org.soundcraft.test.block-delivery" {
        return std::ptr::null();
    }
    let f = Box::new(Oracle {
        plugin: clap_plugin {
            desc: &DESC,
            plugin_data: std::ptr::null_mut(),
            init: Some(init),
            destroy: Some(destroy),
            activate: Some(activate),
            deactivate: Some(empty),
            start_processing: Some(start),
            stop_processing: Some(empty),
            reset: Some(reset),
            process: Some(process),
            get_extension: Some(extension),
            on_main_thread: Some(empty),
        },
        values: [1.0; 4096],
        receipt: [0; 8],
        mode: 0,
        level: 0.0,
    });
    let p = Box::into_raw(f);
    unsafe {
        (*p).plugin.plugin_data = p.cast();
        &(*p).plugin
    }
}
static FACTORY: clap_plugin_factory =
    clap_plugin_factory { get_plugin_count: Some(count), get_plugin_descriptor: Some(descriptor), create_plugin: Some(create) };
unsafe extern "C" fn entry_init(_: *const c_char) -> bool {
    true
}
unsafe extern "C" fn entry_deinit() {}
unsafe extern "C" fn factory(id: *const c_char) -> *const c_void {
    if unsafe { CStr::from_ptr(id) } == CLAP_PLUGIN_FACTORY_ID { &FACTORY as *const _ as *const c_void } else { std::ptr::null() }
}
#[unsafe(no_mangle)]
pub static clap_entry: clap_plugin_entry =
    clap_plugin_entry { clap_version: CLAP_VERSION, init: Some(entry_init), deinit: Some(entry_deinit), get_factory: Some(factory) };
