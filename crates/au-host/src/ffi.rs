//! The only module with `unsafe`: hand-written AudioToolbox / CoreFoundation declarations and a
//! thin safe wrapper over one Audio Unit instance.
//!
//! Every pointer handed to the Audio Unit points into memory this module owns for at least as
//! long as the unit can use it: the input state the render callback reads and the host time the
//! host callbacks read are leaked `Box`es freed only after the unit is disposed, and output buffers
//! are re-pointed before each render.

use soundcraft_dsp::Transport;
use std::ffi::{CStr, c_char, c_void};
use std::ptr;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU32, AtomicU64, Ordering};

type OSStatus = i32;
type AudioComponent = *mut c_void;
type AudioUnit = *mut c_void;
type CFTypeRef = *const c_void;
type CFIndex = isize;

/// Most channels on one Audio Unit bus that we set up.
pub const MAX_AU_CHANNELS: usize = 8;
/// Largest render slice we configure.
pub const MAX_SLICE: usize = 8192;
/// Most parameters read from one unit.
const MAX_PARAMS: usize = 4096;
/// Most components listed for one type.
const MAX_COMPONENTS: usize = 4096;
/// Most value strings read for one indexed parameter.
const MAX_VALUE_STRINGS: CFIndex = 256;
/// Largest state blob accepted or produced.
const MAX_STATE: usize = 64 << 20;
/// Longest CFString converted (bytes).
const MAX_STRING: CFIndex = 4096;

const K_AUDIO_COMPONENT_FLAG_REQUIRES_ASYNC: u32 = 8;

const PROP_CLASS_INFO: u32 = 0;
const PROP_PARAMETER_LIST: u32 = 3;
const PROP_PARAMETER_INFO: u32 = 4;
const PROP_STREAM_FORMAT: u32 = 8;
const PROP_LATENCY: u32 = 12;
const PROP_MAX_FRAMES: u32 = 14;
const PROP_PARAMETER_VALUE_STRINGS: u32 = 16;
const PROP_TAIL_TIME: u32 = 20;
const PROP_SET_RENDER_CALLBACK: u32 = 23;
const PROP_HOST_CALLBACKS: u32 = 27;

const SCOPE_GLOBAL: u32 = 0;
const SCOPE_INPUT: u32 = 1;
const SCOPE_OUTPUT: u32 = 2;

const FORMAT_LINEAR_PCM: u32 = u32::from_be_bytes(*b"lpcm");
const FORMAT_FLAG_FLOAT: u32 = 1;
const FORMAT_FLAG_PACKED: u32 = 1 << 3;
const FORMAT_FLAG_NON_INTERLEAVED: u32 = 1 << 5;

const TIMESTAMP_SAMPLE_TIME_VALID: u32 = 1;

const CF_STRING_ENCODING_UTF8: u32 = 0x0800_0100;
const CF_PROPERTY_LIST_BINARY_FORMAT: CFIndex = 200;

pub const PARAM_FLAG_CF_NAME_RELEASE: u32 = 1 << 4;
pub const PARAM_FLAG_HAS_CF_NAME: u32 = 1 << 27;
pub const PARAM_FLAG_DISPLAY_LOG: u32 = 1 << 22;
#[cfg(test)]
pub const PARAM_FLAG_READABLE: u32 = 1 << 30;
pub const PARAM_FLAG_WRITABLE: u32 = 1 << 31;
pub const PARAM_FLAG_METER_READ_ONLY: u32 = 1 << 15;

pub const UNIT_INDEXED: u32 = 1;
pub const UNIT_CUSTOM: u32 = 26;

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct AudioComponentDescription {
    component_type: u32,
    component_sub_type: u32,
    component_manufacturer: u32,
    component_flags: u32,
    component_flags_mask: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct AudioStreamBasicDescription {
    sample_rate: f64,
    format_id: u32,
    format_flags: u32,
    bytes_per_packet: u32,
    frames_per_packet: u32,
    bytes_per_frame: u32,
    channels_per_frame: u32,
    bits_per_channel: u32,
    reserved: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct SmpteTime {
    subframes: i16,
    subframe_divisor: i16,
    counter: u32,
    smpte_type: u32,
    flags: u32,
    hours: i16,
    minutes: i16,
    seconds: i16,
    frames: i16,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct AudioTimeStamp {
    sample_time: f64,
    host_time: u64,
    rate_scalar: f64,
    word_clock_time: u64,
    smpte_time: SmpteTime,
    flags: u32,
    reserved: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct AudioBuffer {
    number_channels: u32,
    data_byte_size: u32,
    data: *mut c_void,
}

/// `AudioBufferList` with room for [`MAX_AU_CHANNELS`] buffers (the unit reads only
/// `number_buffers` of them).
#[repr(C)]
struct AudioBufferList {
    number_buffers: u32,
    buffers: [AudioBuffer; MAX_AU_CHANNELS],
}

/// The variable-length list the unit hands the render callback (we only see its header and
/// index into the buffers it says it has).
#[repr(C)]
struct AudioBufferListHeader {
    number_buffers: u32,
    first: AudioBuffer,
}

type BeatAndTempoProc = unsafe extern "C" fn(*mut c_void, *mut f64, *mut f64) -> OSStatus;
type MusicalTimeProc = unsafe extern "C" fn(*mut c_void, *mut u32, *mut f32, *mut u32, *mut f64) -> OSStatus;
type TransportStateProc = unsafe extern "C" fn(*mut c_void, *mut u8, *mut u8, *mut f64, *mut u8, *mut f64, *mut f64) -> OSStatus;
type TransportState2Proc = unsafe extern "C" fn(*mut c_void, *mut u8, *mut u8, *mut u8, *mut f64, *mut u8, *mut f64, *mut f64) -> OSStatus;

#[repr(C)]
struct HostCallbackInfo {
    host_user_data: *mut c_void,
    beat_and_tempo: Option<BeatAndTempoProc>,
    musical_time_location: Option<MusicalTimeProc>,
    transport_state: Option<TransportStateProc>,
    transport_state2: Option<TransportState2Proc>,
}

/// What the host callbacks report: the transport of the block being rendered. Atomics, because a
/// unit may also ask from another thread.
struct HostTime {
    playing: AtomicBool,
    recording: AtomicBool,
    changed: AtomicBool,
    samples: AtomicI64,
    tempo: AtomicU64,
    quarters: AtomicU64,
    bar_start: AtomicU64,
    numerator: AtomicU32,
    denominator: AtomicU32,
    sample_rate: AtomicU64,
}

impl HostTime {
    fn new() -> HostTime {
        HostTime {
            playing: AtomicBool::new(false),
            recording: AtomicBool::new(false),
            changed: AtomicBool::new(false),
            samples: AtomicI64::new(0),
            tempo: AtomicU64::new(120f64.to_bits()),
            quarters: AtomicU64::new(0),
            bar_start: AtomicU64::new(0),
            numerator: AtomicU32::new(4),
            denominator: AtomicU32::new(4),
            sample_rate: AtomicU64::new(48_000f64.to_bits()),
        }
    }

    fn set(&self, t: &Transport) {
        if self.playing.swap(t.playing, Ordering::Relaxed) != t.playing {
            self.changed.store(true, Ordering::Relaxed);
        }
        self.recording.store(t.recording, Ordering::Relaxed);
        self.samples.store(t.samples, Ordering::Relaxed);
        self.tempo.store(t.tempo.to_bits(), Ordering::Relaxed);
        self.quarters.store(t.quarters.to_bits(), Ordering::Relaxed);
        self.bar_start.store(t.bar_start.to_bits(), Ordering::Relaxed);
        self.numerator.store(t.numerator, Ordering::Relaxed);
        self.denominator.store(t.denominator, Ordering::Relaxed);
    }
}

fn load_f64(a: &AtomicU64) -> f64 {
    f64::from_bits(a.load(Ordering::Relaxed))
}

/// Writes through an out-pointer the unit may leave null.
///
/// # Safety
/// `p` is null or valid for writing a `T`.
unsafe fn put<T>(p: *mut T, v: T) {
    // SAFETY: per the contract above.
    if let Some(p) = unsafe { p.as_mut() } {
        *p = v;
    }
}

unsafe extern "C" fn beat_and_tempo(user: *mut c_void, beat: *mut f64, tempo: *mut f64) -> OSStatus {
    // SAFETY: `user` is the `HostTime` leaked by `Instance::new`, alive until after the unit is
    // disposed.
    let Some(t) = (unsafe { (user as *const HostTime).as_ref() }) else { return -50 };
    // SAFETY: the unit's out-pointers (or null).
    unsafe {
        put(beat, load_f64(&t.quarters));
        put(tempo, load_f64(&t.tempo));
    }
    0
}

unsafe extern "C" fn musical_time(
    user: *mut c_void,
    to_next_beat: *mut u32,
    numerator: *mut f32,
    denominator: *mut u32,
    downbeat: *mut f64,
) -> OSStatus {
    // SAFETY: as in `beat_and_tempo`.
    let Some(t) = (unsafe { (user as *const HostTime).as_ref() }) else { return -50 };
    let q = load_f64(&t.quarters);
    let per_quarter = load_f64(&t.sample_rate) * 60.0 / load_f64(&t.tempo).max(1.0);
    let next = ((q.ceil() - q) * per_quarter).round();
    let next = if next.is_finite() { next.clamp(0.0, f64::from(u32::MAX)) as u32 } else { 0 };
    // SAFETY: the unit's out-pointers (or null).
    unsafe {
        put(to_next_beat, next);
        put(numerator, t.numerator.load(Ordering::Relaxed) as f32);
        put(denominator, t.denominator.load(Ordering::Relaxed));
        put(downbeat, load_f64(&t.bar_start));
    }
    0
}

unsafe extern "C" fn transport_state2(
    user: *mut c_void,
    playing: *mut u8,
    recording: *mut u8,
    changed: *mut u8,
    sample: *mut f64,
    cycling: *mut u8,
    cycle_start: *mut f64,
    cycle_end: *mut f64,
) -> OSStatus {
    // SAFETY: as in `beat_and_tempo`.
    let Some(t) = (unsafe { (user as *const HostTime).as_ref() }) else { return -50 };
    // SAFETY: the unit's out-pointers (or null).
    unsafe {
        put(playing, u8::from(t.playing.load(Ordering::Relaxed)));
        put(recording, u8::from(t.recording.load(Ordering::Relaxed)));
        put(changed, u8::from(t.changed.swap(false, Ordering::Relaxed)));
        put(sample, t.samples.load(Ordering::Relaxed) as f64);
        put(cycling, 0);
        put(cycle_start, 0.0);
        put(cycle_end, 0.0);
    }
    0
}

unsafe extern "C" fn transport_state(
    user: *mut c_void,
    playing: *mut u8,
    changed: *mut u8,
    sample: *mut f64,
    cycling: *mut u8,
    cycle_start: *mut f64,
    cycle_end: *mut f64,
) -> OSStatus {
    // SAFETY: the same pointers as `transport_state2`, without the recording flag.
    unsafe { transport_state2(user, playing, ptr::null_mut(), changed, sample, cycling, cycle_start, cycle_end) }
}

type AuRenderCallback = unsafe extern "C" fn(
    ref_con: *mut c_void,
    action_flags: *mut u32,
    time_stamp: *const AudioTimeStamp,
    bus: u32,
    frames: u32,
    data: *mut AudioBufferListHeader,
) -> OSStatus;

#[repr(C)]
struct AuRenderCallbackStruct {
    input_proc: Option<AuRenderCallback>,
    input_proc_ref_con: *mut c_void,
}

#[repr(C)]
struct AudioUnitParameterInfo {
    name: [c_char; 52],
    unit_name: CFTypeRef,
    clump_id: u32,
    cf_name_string: CFTypeRef,
    unit: u32,
    min_value: f32,
    max_value: f32,
    default_value: f32,
    flags: u32,
}

#[link(name = "AudioToolbox", kind = "framework")]
unsafe extern "C" {
    fn AudioComponentFindNext(component: AudioComponent, desc: *const AudioComponentDescription) -> AudioComponent;
    fn AudioComponentCopyName(component: AudioComponent, name: *mut CFTypeRef) -> OSStatus;
    fn AudioComponentGetDescription(component: AudioComponent, desc: *mut AudioComponentDescription) -> OSStatus;
    fn AudioComponentGetVersion(component: AudioComponent, version: *mut u32) -> OSStatus;
    fn AudioComponentInstanceNew(component: AudioComponent, instance: *mut AudioUnit) -> OSStatus;
    fn AudioComponentInstanceDispose(instance: AudioUnit) -> OSStatus;
    fn AudioUnitInitialize(unit: AudioUnit) -> OSStatus;
    fn AudioUnitUninitialize(unit: AudioUnit) -> OSStatus;
    fn AudioUnitGetPropertyInfo(unit: AudioUnit, id: u32, scope: u32, element: u32, size: *mut u32, writable: *mut u8) -> OSStatus;
    fn AudioUnitGetProperty(unit: AudioUnit, id: u32, scope: u32, element: u32, data: *mut c_void, size: *mut u32) -> OSStatus;
    fn AudioUnitSetProperty(unit: AudioUnit, id: u32, scope: u32, element: u32, data: *const c_void, size: u32) -> OSStatus;
    fn AudioUnitGetParameter(unit: AudioUnit, id: u32, scope: u32, element: u32, value: *mut f32) -> OSStatus;
    fn AudioUnitSetParameter(unit: AudioUnit, id: u32, scope: u32, element: u32, value: f32, offset: u32) -> OSStatus;
    fn AudioUnitRender(
        unit: AudioUnit,
        action_flags: *mut u32,
        time_stamp: *const AudioTimeStamp,
        bus: u32,
        frames: u32,
        data: *mut AudioBufferList,
    ) -> OSStatus;
    fn AudioUnitReset(unit: AudioUnit, scope: u32, element: u32) -> OSStatus;
    fn MusicDeviceMIDIEvent(unit: AudioUnit, status: u32, data1: u32, data2: u32, offset: u32) -> OSStatus;
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFRelease(cf: CFTypeRef);
    fn CFGetTypeID(cf: CFTypeRef) -> usize;
    fn CFStringGetTypeID() -> usize;
    fn CFDictionaryGetTypeID() -> usize;
    fn CFStringGetLength(s: CFTypeRef) -> CFIndex;
    fn CFStringGetMaximumSizeForEncoding(length: CFIndex, encoding: u32) -> CFIndex;
    fn CFStringGetCString(s: CFTypeRef, buffer: *mut c_char, size: CFIndex, encoding: u32) -> u8;
    fn CFArrayGetCount(a: CFTypeRef) -> CFIndex;
    fn CFArrayGetValueAtIndex(a: CFTypeRef, i: CFIndex) -> CFTypeRef;
    fn CFDataCreate(alloc: CFTypeRef, bytes: *const u8, length: CFIndex) -> CFTypeRef;
    fn CFDataGetLength(d: CFTypeRef) -> CFIndex;
    fn CFDataGetBytePtr(d: CFTypeRef) -> *const u8;
    fn CFPropertyListCreateData(alloc: CFTypeRef, plist: CFTypeRef, format: CFIndex, options: usize, error: *mut CFTypeRef) -> CFTypeRef;
    fn CFPropertyListCreateWithData(alloc: CFTypeRef, data: CFTypeRef, options: usize, format: *mut CFIndex, error: *mut CFTypeRef) -> CFTypeRef;
}

/// Releases a CF object we own (null is ignored).
fn release(cf: CFTypeRef) {
    if !cf.is_null() {
        // SAFETY: `cf` is a non-null CF object the caller owns a +1 reference to.
        unsafe { CFRelease(cf) }
    }
}

/// Converts a borrowed CFString to a Rust string (empty on any failure or a non-string).
fn cf_string(s: CFTypeRef) -> String {
    if s.is_null() {
        return String::new();
    }
    // SAFETY: `s` is a live, non-null CF object (borrowed from the caller); the type check
    // guards the CFString calls, and the buffer is sized by CoreFoundation's own maximum.
    unsafe {
        if CFGetTypeID(s) != CFStringGetTypeID() {
            return String::new();
        }
        let len = CFStringGetLength(s).clamp(0, MAX_STRING);
        let cap = CFStringGetMaximumSizeForEncoding(len, CF_STRING_ENCODING_UTF8);
        if cap <= 0 {
            return String::new();
        }
        let cap = cap.saturating_add(1).min(MAX_STRING * 4 + 1);
        let mut buf = vec![0 as c_char; cap as usize];
        if CFStringGetCString(s, buf.as_mut_ptr(), cap, CF_STRING_ENCODING_UTF8) == 0 {
            return String::new();
        }
        CStr::from_ptr(buf.as_ptr()).to_string_lossy().into_owned()
    }
}

/// One registered component.
#[derive(Debug, Clone)]
pub struct RawComponent {
    pub ty: u32,
    pub subtype: u32,
    pub manufacturer: u32,
    /// `"Vendor: Name"` as registered.
    pub name: String,
    pub version: u32,
}

/// Every component of type `ty` that can be instantiated synchronously.
pub fn components(ty: u32) -> Vec<RawComponent> {
    let desc = AudioComponentDescription { component_type: ty, ..Default::default() };
    let mut out = Vec::new();
    let mut comp: AudioComponent = ptr::null_mut();
    for _ in 0..MAX_COMPONENTS {
        // SAFETY: `desc` is a valid description; `comp` is null or a component returned by the
        // previous call (components live for the process).
        comp = unsafe { AudioComponentFindNext(comp, &desc) };
        if comp.is_null() {
            break;
        }
        let mut d = AudioComponentDescription::default();
        let mut name: CFTypeRef = ptr::null();
        let mut version = 0u32;
        // SAFETY: `comp` is a live component; the out pointers point at locals of the right types.
        let (ds, ns) = unsafe {
            let ds = AudioComponentGetDescription(comp, &mut d);
            let ns = AudioComponentCopyName(comp, &mut name);
            let _ = AudioComponentGetVersion(comp, &mut version);
            (ds, ns)
        };
        let full = if ns == 0 { cf_string(name) } else { String::new() };
        release(name);
        if ds != 0 || d.component_type != ty || d.component_flags & K_AUDIO_COMPONENT_FLAG_REQUIRES_ASYNC != 0 {
            continue;
        }
        out.push(RawComponent { ty, subtype: d.component_sub_type, manufacturer: d.component_manufacturer, name: full, version });
    }
    out
}

/// One global-scope parameter as the unit describes it.
#[derive(Debug, Clone)]
pub struct RawParam {
    pub id: u32,
    pub name: String,
    pub unit: u32,
    pub custom_unit: String,
    pub min: f32,
    pub max: f32,
    pub default: f32,
    pub flags: u32,
    /// Names of the values of an indexed parameter (when it has them).
    pub value_strings: Vec<String>,
}

/// The state the input render callback reads: one buffer per channel, filled by the host before
/// each render.
struct InputState {
    bufs: Vec<Vec<f32>>,
    frames: usize,
}

/// Supplies the effect's input: copies (or lends) our input buffers into the unit's list.
unsafe extern "C" fn input_callback(
    ref_con: *mut c_void,
    _action_flags: *mut u32,
    _time_stamp: *const AudioTimeStamp,
    _bus: u32,
    frames: u32,
    data: *mut AudioBufferListHeader,
) -> OSStatus {
    if ref_con.is_null() || data.is_null() {
        return -50; // paramErr
    }
    // SAFETY: `ref_con` is the `InputState` leaked by `Instance::new`, alive until after the unit
    // is disposed, and only read here while the host is inside `AudioUnitRender` (it does not
    // touch it concurrently). `data` is the unit's buffer list with `number_buffers` buffers laid
    // out contiguously from `first`.
    unsafe {
        let input = &mut *(ref_con as *mut InputState);
        let n = (*data).number_buffers as usize;
        let first: *mut AudioBuffer = ptr::addr_of_mut!((*data).first);
        let frames = (frames as usize).min(MAX_SLICE);
        let avail = input.frames.min(frames);
        for i in 0..n.min(64) {
            let b = &mut *first.add(i);
            let last = input.bufs.len().saturating_sub(1);
            let Some(src) = input.bufs.get_mut(i.min(last)) else { continue };
            if b.data.is_null() {
                // Lend our buffer (the unit may process in place; it is ours to scribble on).
                if src.len() >= frames {
                    b.data = src.as_mut_ptr().cast();
                    b.data_byte_size = (frames * 4) as u32;
                    b.number_channels = 1;
                }
                continue;
            }
            let room = (b.data_byte_size as usize / 4).min(frames);
            let dst = std::slice::from_raw_parts_mut(b.data as *mut f32, room);
            for (k, d) in dst.iter_mut().enumerate() {
                *d = if k < avail { src.get(k).copied().unwrap_or(0.0) } else { 0.0 };
            }
        }
    }
    0
}

/// A live Audio Unit instance.
pub struct Instance {
    unit: AudioUnit,
    /// Leaked `Box<InputState>` read by `input_callback`; freed in `Drop` after disposal.
    input: *mut InputState,
    /// Leaked `Box<HostTime>` read by the host callbacks; freed in `Drop` after disposal.
    time: *mut HostTime,
    outputs: Vec<Vec<f32>>,
    abl: AudioBufferList,
    has_input: bool,
    initialized: bool,
    channels: usize,
    max_frames: usize,
    sample_rate: f64,
    sample_time: f64,
    link: std::sync::Arc<StateLink>,
}

// SAFETY: an Audio Unit instance may be used from any thread as long as calls are not
// concurrent; `Instance` is only reachable through `&mut`/`&` of its owner, which the `Plugin`
// contract keeps on one thread at a time. The input state and host time are owned exclusively by
// it (the host time is all atomics).
unsafe impl Send for Instance {}

impl Instance {
    /// Creates (but does not initialize) the component `(ty, subtype, manufacturer)`.
    pub fn new(ty: u32, subtype: u32, manufacturer: u32, has_input: bool) -> Result<Instance, String> {
        let desc =
            AudioComponentDescription { component_type: ty, component_sub_type: subtype, component_manufacturer: manufacturer, ..Default::default() };
        // SAFETY: `desc` is a valid description.
        let comp = unsafe { AudioComponentFindNext(ptr::null_mut(), &desc) };
        if comp.is_null() {
            return Err("component not registered".into());
        }
        let mut d = AudioComponentDescription::default();
        // SAFETY: `comp` is a live component; `d` is a local of the right type.
        if unsafe { AudioComponentGetDescription(comp, &mut d) } == 0 && d.component_flags & K_AUDIO_COMPONENT_FLAG_REQUIRES_ASYNC != 0 {
            return Err("requires asynchronous instantiation (not supported)".into());
        }
        let mut unit: AudioUnit = ptr::null_mut();
        // SAFETY: `comp` is a live component; `unit` receives the new instance.
        let st = unsafe { AudioComponentInstanceNew(comp, &mut unit) };
        if st != 0 || unit.is_null() {
            return Err(format!("AudioComponentInstanceNew failed ({st})"));
        }
        let input = Box::into_raw(Box::new(InputState { bufs: Vec::new(), frames: 0 }));
        let time = Box::into_raw(Box::new(HostTime::new()));
        let empty = AudioBuffer { number_channels: 1, data_byte_size: 0, data: ptr::null_mut() };
        let inst = Instance {
            unit,
            input,
            time,
            outputs: Vec::new(),
            abl: AudioBufferList { number_buffers: 0, buffers: [empty; MAX_AU_CHANNELS] },
            has_input,
            link: std::sync::Arc::new(StateLink(std::sync::Mutex::new(LinkedUnit(Some(unit))))),
            initialized: false,
            channels: 0,
            max_frames: 0,
            sample_rate: 48_000.0,
            sample_time: 0.0,
        };
        let callbacks = HostCallbackInfo {
            host_user_data: time.cast(),
            beat_and_tempo: Some(beat_and_tempo),
            musical_time_location: Some(musical_time),
            transport_state: Some(transport_state),
            transport_state2: Some(transport_state2),
        };
        // Units that never ask for host time may refuse it.
        let _ = inst.set_property(PROP_HOST_CALLBACKS, SCOPE_GLOBAL, 0, &callbacks);
        Ok(inst)
    }

    fn time(&self) -> &HostTime {
        // SAFETY: `time` is the `HostTime` leaked in `new`, freed only in `Drop`.
        unsafe { &*self.time }
    }

    /// The transport the host callbacks report during the next renders.
    pub fn set_transport(&self, t: &Transport) {
        self.time().set(t);
    }

    fn set_property<T>(&self, id: u32, scope: u32, element: u32, value: &T) -> OSStatus {
        // SAFETY: `value` points at a live `T` of `size_of::<T>()` bytes, matching what the
        // property expects (callers pass the documented type).
        unsafe { AudioUnitSetProperty(self.unit, id, scope, element, (value as *const T).cast(), std::mem::size_of::<T>() as u32) }
    }

    fn get_f64(&self, id: u32) -> Option<f64> {
        let mut v = 0f64;
        let mut size = std::mem::size_of::<f64>() as u32;
        // SAFETY: the property is a `Float64`; `v`/`size` are locals of the right types.
        let st = unsafe { AudioUnitGetProperty(self.unit, id, SCOPE_GLOBAL, 0, (&mut v as *mut f64).cast(), &mut size) };
        (st == 0 && v.is_finite()).then_some(v)
    }

    fn format(&self, scope: u32, sample_rate: f64, channels: usize) -> OSStatus {
        let asbd = AudioStreamBasicDescription {
            sample_rate,
            format_id: FORMAT_LINEAR_PCM,
            format_flags: FORMAT_FLAG_FLOAT | FORMAT_FLAG_PACKED | FORMAT_FLAG_NON_INTERLEAVED,
            bytes_per_packet: 4,
            frames_per_packet: 1,
            bytes_per_frame: 4,
            channels_per_frame: channels as u32,
            bits_per_channel: 32,
            reserved: 0,
        };
        self.set_property(PROP_STREAM_FORMAT, scope, 0, &asbd)
    }

    /// (Re)configures and initializes the unit for `sample_rate`, `max_frames`-frame slices and
    /// (preferably) `channels` channels on its main input and output buses. Returns the channel
    /// count it accepted. Not realtime-safe (allocates the buffers).
    pub fn configure(&mut self, sample_rate: f64, max_frames: usize, channels: usize) -> Result<usize, String> {
        if self.initialized {
            // SAFETY: the unit is live and initialized.
            unsafe { AudioUnitUninitialize(self.unit) };
            self.initialized = false;
        }
        self.max_frames = 0;
        let mf = max_frames.clamp(1, MAX_SLICE);
        let _ = self.set_property(PROP_MAX_FRAMES, SCOPE_GLOBAL, 0, &(mf as u32));
        if self.has_input {
            let cb = AuRenderCallbackStruct { input_proc: Some(input_callback), input_proc_ref_con: self.input.cast() };
            let st = self.set_property(PROP_SET_RENDER_CALLBACK, SCOPE_INPUT, 0, &cb);
            if st != 0 {
                return Err(format!("cannot set the input callback ({st})"));
            }
        }
        let want = channels.clamp(1, MAX_AU_CHANNELS);
        let mut tried = [0usize; 3];
        let mut ok = None;
        let mut last = 0;
        for (k, ch) in [want, 2, 1].into_iter().enumerate() {
            if tried.contains(&ch) {
                continue;
            }
            if let Some(t) = tried.get_mut(k) {
                *t = ch;
            }
            let o = self.format(SCOPE_OUTPUT, sample_rate, ch);
            let i = if self.has_input { self.format(SCOPE_INPUT, sample_rate, ch) } else { 0 };
            if o != 0 || i != 0 {
                last = if o != 0 { o } else { i };
                continue;
            }
            // SAFETY: the unit is live and uninitialized.
            let st = unsafe { AudioUnitInitialize(self.unit) };
            if st == 0 {
                ok = Some(ch);
                break;
            }
            last = st;
        }
        let ch = ok.ok_or_else(|| format!("the unit accepts no 1/2/{want}-channel float format ({last})"))?;
        self.initialized = true;
        self.channels = ch;
        self.sample_rate = sample_rate;
        self.time().sample_rate.store(sample_rate.to_bits(), Ordering::Relaxed);
        self.outputs = (0..ch).map(|_| vec![0.0; mf]).collect();
        // SAFETY: the callback only runs inside `AudioUnitRender`, which is not running now
        // (we hold `&mut self`), so this exclusive access does not alias.
        let input = unsafe { &mut *self.input };
        input.bufs = if self.has_input { (0..ch).map(|_| vec![0.0; mf]).collect() } else { Vec::new() };
        input.frames = 0;
        self.max_frames = mf;
        Ok(ch)
    }

    pub fn is_ready(&self) -> bool {
        self.initialized && self.max_frames > 0
    }

    pub fn max_frames(&self) -> usize {
        self.max_frames
    }

    /// Input buffer `ch` (for effects), `max_frames` long.
    pub fn input_mut(&mut self, ch: usize) -> Option<&mut [f32]> {
        // SAFETY: see `configure`: no render is running while we hold `&mut self`.
        let input = unsafe { &mut *self.input };
        input.bufs.get_mut(ch).map(Vec::as_mut_slice)
    }

    /// Renders `frames` (≤ `max_frames`) into the output buffers. Never allocates.
    pub fn render(&mut self, frames: usize) -> Result<(), OSStatus> {
        if !self.is_ready() || frames == 0 || frames > self.max_frames {
            return Err(-50);
        }
        {
            // SAFETY: as in `input_mut`.
            let input = unsafe { &mut *self.input };
            input.frames = frames;
        }
        let n = self.outputs.len().min(MAX_AU_CHANNELS);
        for (b, out) in self.abl.buffers.iter_mut().zip(self.outputs.iter_mut()) {
            *b = AudioBuffer { number_channels: 1, data_byte_size: (frames * 4) as u32, data: out.as_mut_ptr().cast() };
        }
        self.abl.number_buffers = n as u32;
        let ts = AudioTimeStamp { sample_time: self.sample_time, flags: TIMESTAMP_SAMPLE_TIME_VALID, ..Default::default() };
        let mut flags = 0u32;
        // SAFETY: the unit is initialized; `ts` and `flags` are locals; `abl` holds `n` buffers
        // each pointing at `frames` floats we own; the input callback's state is alive.
        let st = unsafe { AudioUnitRender(self.unit, &mut flags, &ts, 0, frames as u32, &mut self.abl) };
        self.sample_time += frames as f64;
        if st == 0 { Ok(()) } else { Err(st) }
    }

    /// Output channel `ch` of the last render (`frames` long), wherever the unit left it.
    pub fn output(&self, ch: usize, frames: usize) -> Option<&[f32]> {
        if ch >= self.abl.number_buffers as usize {
            return None;
        }
        let b = self.abl.buffers.get(ch)?;
        if b.data.is_null() {
            return None;
        }
        let len = (b.data_byte_size as usize / 4).min(frames);
        // SAFETY: after a successful render each buffer points at `data_byte_size` bytes of
        // floats, either ours or the unit's own (valid until its next render).
        Some(unsafe { std::slice::from_raw_parts(b.data as *const f32, len) })
    }

    pub fn channels(&self) -> usize {
        self.channels
    }

    pub fn reset(&mut self) {
        // SAFETY: the unit is live.
        unsafe { AudioUnitReset(self.unit, SCOPE_GLOBAL, 0) };
    }

    /// Latency in samples at the configured rate.
    pub fn latency(&self) -> usize {
        self.seconds_to_samples(self.get_f64(PROP_LATENCY), 10.0)
    }

    /// Tail in samples at the configured rate (capped at a minute).
    pub fn tail(&self) -> usize {
        self.seconds_to_samples(self.get_f64(PROP_TAIL_TIME), 60.0)
    }

    fn seconds_to_samples(&self, s: Option<f64>, cap: f64) -> usize {
        match s {
            Some(s) if s > 0.0 => (s.min(cap) * self.sample_rate).round() as usize,
            _ => 0,
        }
    }

    /// A MIDI channel message at `offset` frames into the next render.
    pub fn midi(&self, status: u8, data1: u8, data2: u8, offset: u32) -> bool {
        // SAFETY: the unit is live; the call only takes integers.
        unsafe { MusicDeviceMIDIEvent(self.unit, u32::from(status), u32::from(data1), u32::from(data2), offset) == 0 }
    }

    pub fn get_param(&self, id: u32) -> Option<f32> {
        let mut v = 0f32;
        // SAFETY: the unit is live; `v` is a local `f32`.
        let st = unsafe { AudioUnitGetParameter(self.unit, id, SCOPE_GLOBAL, 0, &mut v) };
        (st == 0 && v.is_finite()).then_some(v)
    }

    pub fn set_param(&self, id: u32, value: f32) -> bool {
        // SAFETY: the unit is live; the call only takes plain values.
        unsafe { AudioUnitSetParameter(self.unit, id, SCOPE_GLOBAL, 0, value, 0) == 0 }
    }

    /// The global-scope parameters.
    pub fn params(&self) -> Vec<RawParam> {
        let mut size = 0u32;
        let mut writable = 0u8;
        // SAFETY: the unit is live; out pointers are locals.
        let st = unsafe { AudioUnitGetPropertyInfo(self.unit, PROP_PARAMETER_LIST, SCOPE_GLOBAL, 0, &mut size, &mut writable) };
        if st != 0 || size == 0 {
            return Vec::new();
        }
        let n = (size as usize / 4).min(MAX_PARAMS);
        let mut ids = vec![0u32; n];
        let mut size = (n * 4) as u32;
        // SAFETY: `ids` has room for `size` bytes.
        let st = unsafe { AudioUnitGetProperty(self.unit, PROP_PARAMETER_LIST, SCOPE_GLOBAL, 0, ids.as_mut_ptr().cast(), &mut size) };
        if st != 0 {
            return Vec::new();
        }
        ids.truncate(size as usize / 4);
        ids.into_iter().filter_map(|id| self.param_info(id)).collect()
    }

    fn param_info(&self, id: u32) -> Option<RawParam> {
        // SAFETY: an all-zero `AudioUnitParameterInfo` is valid (null pointers, zero numbers).
        let mut info: AudioUnitParameterInfo = unsafe { std::mem::zeroed() };
        let mut size = std::mem::size_of::<AudioUnitParameterInfo>() as u32;
        // SAFETY: `info` is a local of the documented type and size.
        let st = unsafe {
            AudioUnitGetProperty(self.unit, PROP_PARAMETER_INFO, SCOPE_GLOBAL, id, (&mut info as *mut AudioUnitParameterInfo).cast(), &mut size)
        };
        if st != 0 {
            return None;
        }
        let owned = info.flags & PARAM_FLAG_CF_NAME_RELEASE != 0;
        let mut name = if info.flags & PARAM_FLAG_HAS_CF_NAME != 0 { cf_string(info.cf_name_string) } else { String::new() };
        if name.is_empty() {
            let bytes: Vec<u8> = info.name.iter().take_while(|&&c| c != 0).map(|&c| c as u8).collect();
            name = String::from_utf8_lossy(&bytes).into_owned();
        }
        let custom_unit = if info.unit == UNIT_CUSTOM { cf_string(info.unit_name) } else { String::new() };
        if owned {
            if info.flags & PARAM_FLAG_HAS_CF_NAME != 0 {
                release(info.cf_name_string);
            }
            if info.unit == UNIT_CUSTOM {
                release(info.unit_name);
            }
        }
        let value_strings = if info.unit == UNIT_INDEXED { self.value_strings(id) } else { Vec::new() };
        Some(RawParam {
            id,
            name,
            unit: info.unit,
            custom_unit,
            min: info.min_value,
            max: info.max_value,
            default: info.default_value,
            flags: info.flags,
            value_strings,
        })
    }

    fn value_strings(&self, id: u32) -> Vec<String> {
        let mut arr: CFTypeRef = ptr::null();
        let mut size = std::mem::size_of::<CFTypeRef>() as u32;
        // SAFETY: the property is a `CFArrayRef` (returned +1); `arr` is a local pointer.
        let st = unsafe {
            AudioUnitGetProperty(self.unit, PROP_PARAMETER_VALUE_STRINGS, SCOPE_GLOBAL, id, (&mut arr as *mut CFTypeRef).cast(), &mut size)
        };
        if st != 0 || arr.is_null() {
            return Vec::new();
        }
        // SAFETY: `arr` is a live CFArray we own; indices are below its count.
        let out = unsafe {
            let n = CFArrayGetCount(arr).clamp(0, MAX_VALUE_STRINGS);
            (0..n).map(|i| cf_string(CFArrayGetValueAtIndex(arr, i))).collect()
        };
        release(arr);
        out
    }

    /// The link for state handles.
    pub fn state_link(&self) -> std::sync::Arc<StateLink> {
        std::sync::Arc::clone(&self.link)
    }

    /// Restores a blob from [`StateLink::save_state`] (hostile input: parsed by CoreFoundation,
    /// must be a dictionary, and the unit validates it).
    pub fn load_state(&self, bytes: &[u8]) -> bool {
        if bytes.is_empty() || bytes.len() > MAX_STATE {
            return false;
        }
        // SAFETY: `bytes` is a live slice of `len` bytes (copied by CFDataCreate).
        let data = unsafe { CFDataCreate(ptr::null(), bytes.as_ptr(), bytes.len() as CFIndex) };
        if data.is_null() {
            return false;
        }
        let mut err: CFTypeRef = ptr::null();
        // SAFETY: `data` is a live CFData; the format out pointer may be null; `err` is local.
        let plist = unsafe { CFPropertyListCreateWithData(ptr::null(), data, 0, ptr::null_mut(), &mut err) };
        release(err);
        release(data);
        if plist.is_null() {
            return false;
        }
        // SAFETY: `plist` is a live CF object we own.
        let is_dict = unsafe { CFGetTypeID(plist) == CFDictionaryGetTypeID() };
        let st = if is_dict { self.set_property(PROP_CLASS_INFO, SCOPE_GLOBAL, 0, &plist) } else { -50 };
        release(plist);
        st == 0
    }
}

struct LinkedUnit(Option<AudioUnit>);

// SAFETY: the pointer is only used under the link's mutex, while the instance keeps the unit alive.
unsafe impl Send for LinkedUnit {}

/// The link between an instance and its state handles: the unit's state is read on the main
/// thread while the instance renders on the audio thread (hosts make property calls from other
/// threads during rendering). The instance clears the link under its lock before it disposes of
/// the unit, so a handle never reads a dead unit.
pub(crate) struct StateLink(std::sync::Mutex<LinkedUnit>);

impl StateLink {
    fn lock(&self) -> std::sync::MutexGuard<'_, LinkedUnit> {
        self.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// The unit's `ClassInfo` as a binary property list (`None` once the instance is gone).
    pub fn save_state(&self) -> Option<Vec<u8>> {
        let g = self.lock();
        let unit = g.0?;
        let mut plist: CFTypeRef = ptr::null();
        let mut size = std::mem::size_of::<CFTypeRef>() as u32;
        // SAFETY: the unit is live while the link holds it (see the type); the property is a
        // `CFPropertyListRef` returned +1; `plist` is a local pointer.
        let st = unsafe { AudioUnitGetProperty(unit, PROP_CLASS_INFO, SCOPE_GLOBAL, 0, (&mut plist as *mut CFTypeRef).cast(), &mut size) };
        if st != 0 || plist.is_null() {
            release(plist);
            return None;
        }
        let mut err: CFTypeRef = ptr::null();
        // SAFETY: `plist` is a live property list; `err` is a local out pointer.
        let data = unsafe { CFPropertyListCreateData(ptr::null(), plist, CF_PROPERTY_LIST_BINARY_FORMAT, 0, &mut err) };
        release(err);
        release(plist);
        if data.is_null() {
            return None;
        }
        // SAFETY: `data` is a live CFData we own; its byte pointer is valid for its length.
        let out = unsafe {
            let len = CFDataGetLength(data);
            let p = CFDataGetBytePtr(data);
            if len <= 0 || len as usize > MAX_STATE || p.is_null() { None } else { Some(std::slice::from_raw_parts(p, len as usize).to_vec()) }
        };
        release(data);
        out
    }

    fn kill(&self) {
        self.lock().0 = None;
    }
}

impl Drop for Instance {
    fn drop(&mut self) {
        self.link.kill();
        // SAFETY: the unit is live (disposed only here); after disposal nothing can call the
        // input callback or the host callbacks, so the leaked input state and host time can be
        // freed.
        unsafe {
            if self.initialized {
                AudioUnitUninitialize(self.unit);
            }
            AudioComponentInstanceDispose(self.unit);
            drop(Box::from_raw(self.input));
            drop(Box::from_raw(self.time));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_callbacks_report_the_transport() {
        let time = HostTime::new();
        time.set(&Transport { playing: true, tempo: 120.0, numerator: 3, denominator: 4, samples: 48_000, quarters: 2.25, ..Transport::default() });
        let user = ptr::from_ref(&time).cast_mut().cast::<c_void>();
        let (mut beat, mut tempo) = (0.0, 0.0);
        // SAFETY: `user` points at a live `HostTime`; the out-pointers are locals.
        assert_eq!(unsafe { beat_and_tempo(user, &mut beat, &mut tempo) }, 0);
        assert_eq!((beat, tempo), (2.25, 120.0));
        let (mut next, mut num, mut den, mut down) = (0u32, 0f32, 0u32, -1.0);
        // SAFETY: as above.
        assert_eq!(unsafe { musical_time(user, &mut next, &mut num, &mut den, &mut down) }, 0);
        assert_eq!((next, num, den, down), (18_000, 3.0, 4, 0.0));
        let (mut playing, mut recording, mut changed, mut sample) = (0u8, 1u8, 0u8, 0.0);
        let (no_flag, no_beat) = (ptr::null_mut::<u8>(), ptr::null_mut::<f64>());
        // SAFETY: as above; the unit may pass null for what it doesn't want.
        let st = unsafe { transport_state2(user, &mut playing, &mut recording, &mut changed, &mut sample, no_flag, no_beat, no_beat) };
        assert_eq!((st, playing, recording, changed, sample), (0, 1, 0, 1, 48_000.0));
        // SAFETY: as above.
        unsafe { transport_state(user, &mut playing, &mut changed, no_beat, no_flag, no_beat, no_beat) };
        assert_eq!(changed, 0, "a change is reported once");
        // SAFETY: a null user pointer is refused before anything is read.
        assert_eq!(unsafe { beat_and_tempo(ptr::null_mut(), &mut beat, &mut tempo) }, -50);
    }

    #[test]
    fn struct_layouts_match_the_c_headers() {
        assert_eq!(std::mem::size_of::<AudioComponentDescription>(), 20);
        assert_eq!(std::mem::size_of::<AudioStreamBasicDescription>(), 40);
        assert_eq!(std::mem::size_of::<SmpteTime>(), 24);
        assert_eq!(std::mem::size_of::<AudioTimeStamp>(), 64);
        assert_eq!(std::mem::size_of::<AudioBuffer>(), 16);
        assert_eq!(std::mem::offset_of!(AudioBufferList, buffers), 8);
        assert_eq!(std::mem::offset_of!(AudioBufferListHeader, first), 8);
        assert_eq!(std::mem::size_of::<AuRenderCallbackStruct>(), 16);
        assert_eq!(std::mem::size_of::<HostCallbackInfo>(), 40);
        assert_eq!(std::mem::offset_of!(AudioUnitParameterInfo, unit_name), 56);
        assert_eq!(std::mem::offset_of!(AudioUnitParameterInfo, cf_name_string), 72);
        assert_eq!(std::mem::offset_of!(AudioUnitParameterInfo, flags), 96);
        assert_eq!(std::mem::size_of::<AudioUnitParameterInfo>(), 104);
    }
}
