//! Controlled 1025-parameter effect. Its serialized receipt comes from the processor,
//! independently of the host's requested-value cache. IDs 0 and 1024 are audible.
#![allow(clippy::missing_safety_doc)]
use clap_sys::{
    audio_buffer::clap_audio_buffer,
    entry::clap_plugin_entry,
    events::*,
    ext::{audio_ports::*, note_ports::*, params::*, state::*},
    factory::plugin_factory::*,
    host::clap_host,
    plugin::*,
    process::*,
    stream::*,
    version::CLAP_VERSION,
};
use std::ffi::{CStr, c_char, c_void};
const P: usize = 1025;
const RECEIPT: usize = P * 8 + 40;
struct Features([*const c_char; 3]);
unsafe impl Sync for Features {}
static FEATURES: Features = Features([c"audio-effect".as_ptr(), c"stereo".as_ptr(), std::ptr::null()]);
static DESC: clap_plugin_descriptor = clap_plugin_descriptor {
    clap_version: CLAP_VERSION,
    id: c"org.soundcraft.test.capacity".as_ptr(),
    name: c"Capacity Affine".as_ptr(),
    vendor: c"SoundCraft controlled tests".as_ptr(),
    url: std::ptr::null(),
    manual_url: std::ptr::null(),
    support_url: std::ptr::null(),
    version: c"1.0".as_ptr(),
    description: std::ptr::null(),
    features: FEATURES.0.as_ptr(),
};
struct Fx {
    plugin: clap_plugin,
    values: [f64; P],
    calls: u64,
    params: u64,
    notes: u64,
    note_times: u64,
    frames: u64,
}
unsafe fn fx<'a>(p: *const clap_plugin) -> &'a mut Fx {
    unsafe { &mut *((*p).plugin_data as *mut Fx) }
}
unsafe extern "C" fn init(_: *const clap_plugin) -> bool {
    true
}
unsafe extern "C" fn destroy(p: *const clap_plugin) {
    drop(unsafe { Box::from_raw((*p).plugin_data as *mut Fx) });
}
unsafe extern "C" fn activate(_: *const clap_plugin, sr: f64, min: u32, max: u32) -> bool {
    sr > 0.0 && min > 0 && max >= min
}
unsafe extern "C" fn deactivate(_: *const clap_plugin) {}
unsafe extern "C" fn start(_: *const clap_plugin) -> bool {
    true
}
unsafe extern "C" fn stop(_: *const clap_plugin) {}
unsafe extern "C" fn reset(_: *const clap_plugin) {}
unsafe extern "C" fn process(p: *const clap_plugin, pr: *const clap_process) -> clap_process_status {
    let f = unsafe { fx(p) };
    let pr = unsafe { &*pr };
    let events = unsafe { &*pr.in_events };
    let count = unsafe { (events.size.unwrap())(pr.in_events) };
    // Validate event offsets before applying them.
    let mut previous_time = 0;
    for i in 0..count {
        let h = unsafe { &*(events.get.unwrap())(pr.in_events, i) };
        if h.time >= pr.frames_count || h.time < previous_time {
            return CLAP_PROCESS_ERROR;
        }
        previous_time = h.time;
    }
    if pr.audio_inputs_count != 1 || pr.audio_outputs_count != 1 {
        return CLAP_PROCESS_ERROR;
    }
    f.params = 0;
    f.notes = 0;
    f.note_times = 0;
    for i in 0..count {
        let h = unsafe { &*(events.get.unwrap())(pr.in_events, i) };
        if h.type_ == CLAP_EVENT_PARAM_VALUE {
            let e = unsafe { &*(h as *const _ as *const clap_event_param_value) };
            if let Some(v) = f.values.get_mut(e.param_id as usize) {
                *v = e.value;
                f.params += 1;
            }
        } else if h.type_ == CLAP_EVENT_NOTE_ON || h.type_ == CLAP_EVENT_NOTE_OFF {
            f.notes += 1;
            f.note_times += u64::from(h.time);
        }
    }
    let input: &clap_audio_buffer = unsafe { &*pr.audio_inputs };
    let output: &clap_audio_buffer = unsafe { &*pr.audio_outputs };
    for c in 0..output.channel_count.min(input.channel_count) as usize {
        let src = unsafe { std::slice::from_raw_parts(*input.data32.add(c), pr.frames_count as usize) };
        let dst = unsafe { std::slice::from_raw_parts_mut(*output.data32.add(c), pr.frames_count as usize) };
        for (x, y) in src.iter().zip(dst) {
            *y = (*x as f64 * f.values[0] + f.values[1024]) as f32;
        }
    }
    f.calls += 1;
    f.frames = u64::from(pr.frames_count);
    CLAP_PROCESS_CONTINUE
}
fn name(dst: &mut [c_char], s: &CStr) {
    for (d, b) in dst.iter_mut().zip(s.to_bytes_with_nul()) {
        *d = *b as c_char;
    }
}
unsafe extern "C" fn params_count(_: *const clap_plugin) -> u32 {
    P as u32
}
unsafe extern "C" fn param_info(_: *const clap_plugin, i: u32, out: *mut clap_param_info) -> bool {
    if i as usize >= P {
        return false;
    }
    let p = unsafe { &mut *out };
    p.id = i;
    p.flags = CLAP_PARAM_IS_AUTOMATABLE;
    p.cookie = std::ptr::null_mut();
    name(&mut p.name, c"Controlled");
    name(&mut p.module, c"");
    p.min_value = 0.0;
    p.max_value = if i == 0 || i == 1 { 2.0 } else { 1.0 };
    p.default_value = if i == 0 { 1.0 } else { 0.0 };
    if i == 1 {
        p.flags |= CLAP_PARAM_IS_ENUM | CLAP_PARAM_IS_STEPPED;
    }
    if i == 2 {
        p.flags |= CLAP_PARAM_IS_STEPPED;
    }
    true
}
unsafe extern "C" fn param_value(p: *const clap_plugin, i: u32, out: *mut f64) -> bool {
    let Some(v) = (unsafe { fx(p) }).values.get(i as usize) else { return false };
    unsafe {
        *out = *v;
    }
    true
}
unsafe extern "C" fn param_text(_: *const clap_plugin, i: u32, v: f64, out: *mut c_char, cap: u32) -> bool {
    if i != 1 || cap < 2 {
        return false;
    }
    unsafe {
        *out = b'A'.saturating_add(v.round() as u8) as c_char;
        *out.add(1) = 0;
    }
    true
}
static PARAMS: clap_plugin_params = clap_plugin_params {
    count: Some(params_count),
    get_info: Some(param_info),
    get_value: Some(param_value),
    value_to_text: Some(param_text),
    text_to_value: None,
    flush: None,
};
unsafe extern "C" fn port_count(_: *const clap_plugin, _: bool) -> u32 {
    1
}
unsafe extern "C" fn port_info(_: *const clap_plugin, i: u32, input: bool, out: *mut clap_audio_port_info) -> bool {
    if i != 0 {
        return false;
    }
    let p = unsafe { &mut *out };
    p.id = u32::from(!input);
    name(&mut p.name, c"main");
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
unsafe extern "C" fn note_info(_: *const clap_plugin, i: u32, input: bool, out: *mut clap_note_port_info) -> bool {
    if i != 0 || !input {
        return false;
    }
    let p = unsafe { &mut *out };
    p.id = 0;
    name(&mut p.name, c"receipt notes");
    p.supported_dialects = CLAP_NOTE_DIALECT_CLAP;
    p.preferred_dialect = CLAP_NOTE_DIALECT_CLAP;
    true
}
static NOTES: clap_plugin_note_ports = clap_plugin_note_ports { count: Some(note_count), get: Some(note_info) };
unsafe extern "C" fn save(p: *const clap_plugin, s: *const clap_ostream) -> bool {
    let f = unsafe { fx(p) };
    let mut bytes = [0u8; RECEIPT];
    for (v, b) in f.values.iter().zip(bytes[..P * 8].chunks_exact_mut(8)) {
        b.copy_from_slice(&v.to_le_bytes());
    }
    for (v, b) in [f.calls, f.params, f.notes, f.note_times, f.frames].iter().zip(bytes[P * 8..].chunks_exact_mut(8)) {
        b.copy_from_slice(&v.to_le_bytes());
    }
    unsafe { ((*s).write.unwrap())(s, bytes.as_ptr().cast(), RECEIPT as u64) == RECEIPT as i64 }
}
unsafe extern "C" fn load(p: *const clap_plugin, s: *const clap_istream) -> bool {
    let mut bytes = [0u8; RECEIPT];
    let mut got = 0;
    while got < bytes.len() {
        let n = unsafe { ((*s).read.unwrap())(s, bytes[got..].as_mut_ptr().cast(), (bytes.len() - got) as u64) };
        if n <= 0 {
            return false;
        }
        got += n as usize;
    }
    let f = unsafe { fx(p) };
    for (v, b) in f.values.iter_mut().zip(bytes[..P * 8].chunks_exact(8)) {
        let x = f64::from_le_bytes(b.try_into().unwrap());
        if !x.is_finite() {
            return false;
        }
        *v = x;
    }
    true
}
static STATE: clap_plugin_state = clap_plugin_state { save: Some(save), load: Some(load) };
unsafe extern "C" fn extension(_: *const clap_plugin, id: *const c_char) -> *const c_void {
    let id = unsafe { CStr::from_ptr(id) };
    if id == CLAP_EXT_PARAMS {
        (&PARAMS as *const clap_plugin_params).cast()
    } else if id == CLAP_EXT_AUDIO_PORTS {
        (&PORTS as *const clap_plugin_audio_ports).cast()
    } else if id == CLAP_EXT_NOTE_PORTS {
        (&NOTES as *const clap_plugin_note_ports).cast()
    } else if id == CLAP_EXT_STATE {
        (&STATE as *const clap_plugin_state).cast()
    } else {
        std::ptr::null()
    }
}
unsafe extern "C" fn main_thread(_: *const clap_plugin) {}
unsafe extern "C" fn count(_: *const clap_plugin_factory) -> u32 {
    1
}
unsafe extern "C" fn descriptor(_: *const clap_plugin_factory, i: u32) -> *const clap_plugin_descriptor {
    if i == 0 { &DESC } else { std::ptr::null() }
}
unsafe extern "C" fn create(_: *const clap_plugin_factory, _: *const clap_host, id: *const c_char) -> *const clap_plugin {
    if unsafe { CStr::from_ptr(id) } != c"org.soundcraft.test.capacity" {
        return std::ptr::null();
    }
    let mut values = [0.0; P];
    values[0] = 1.0;
    let f = Box::new(Fx {
        plugin: clap_plugin {
            desc: &DESC,
            plugin_data: std::ptr::null_mut(),
            init: Some(init),
            destroy: Some(destroy),
            activate: Some(activate),
            deactivate: Some(deactivate),
            start_processing: Some(start),
            stop_processing: Some(stop),
            reset: Some(reset),
            process: Some(process),
            get_extension: Some(extension),
            on_main_thread: Some(main_thread),
        },
        values,
        calls: 0,
        params: 0,
        notes: 0,
        note_times: 0,
        frames: 0,
    });
    let raw = Box::into_raw(f);
    unsafe {
        (*raw).plugin.plugin_data = raw.cast();
        &(*raw).plugin
    }
}
static FACTORY: clap_plugin_factory =
    clap_plugin_factory { get_plugin_count: Some(count), get_plugin_descriptor: Some(descriptor), create_plugin: Some(create) };
unsafe extern "C" fn entry_init(_: *const c_char) -> bool {
    true
}
unsafe extern "C" fn entry_deinit() {}
unsafe extern "C" fn factory(id: *const c_char) -> *const c_void {
    if unsafe { CStr::from_ptr(id) } == CLAP_PLUGIN_FACTORY_ID { (&FACTORY as *const clap_plugin_factory).cast() } else { std::ptr::null() }
}
#[unsafe(no_mangle)]
pub static clap_entry: clap_plugin_entry =
    clap_plugin_entry { clap_version: CLAP_VERSION, init: Some(entry_init), deinit: Some(entry_deinit), get_factory: Some(factory) };
