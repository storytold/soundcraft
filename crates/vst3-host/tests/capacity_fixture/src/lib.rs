//! Separate controlled 1025-parameter VST3 effect, with independent processor receipts.
#![allow(non_snake_case, clippy::missing_safety_doc)]
use std::ffi::{c_char, c_void};
use std::sync::atomic::{AtomicU64, Ordering};
use vst3::Steinberg::Vst::*;
use vst3::Steinberg::*;
use vst3::{Class, ComRef, ComWrapper, uid};
const P: usize = 1025;
const RECEIPT: usize = P * 8 + 40;
const FX_CID: TUID = uid(0x5C0A7E00, 0x11112222, 0x33334444, 0x55556666);
const CTRL_CID: TUID = uid(0x5C0A7E01, 0x11112222, 0x33334444, 0x55556666);
fn initial_values() -> [AtomicU64; P] {
    std::array::from_fn(|i| AtomicU64::new((if i == 0 { 0.5f64 } else { 0.0f64 }).to_bits()))
}
fn read_values(state: *mut IBStream, values: &[AtomicU64; P]) -> tresult {
    let Some(s) = (unsafe { ComRef::from_raw(state) }) else {
        return kInvalidArgument;
    };
    let mut bytes = [0u8; RECEIPT];
    let mut got = 0;
    while got < bytes.len() {
        let mut n = 0;
        if unsafe { s.read(bytes[got..].as_mut_ptr().cast(), (bytes.len() - got) as i32, &mut n) } != kResultOk || n <= 0 {
            return kResultFalse;
        }
        got += n as usize;
    }
    for (v, b) in values.iter().zip(bytes[..P * 8].chunks_exact(8)) {
        let n = f64::from_le_bytes(b.try_into().unwrap());
        if !n.is_finite() {
            return kResultFalse;
        }
        v.store(n.to_bits(), Ordering::Relaxed);
    }
    kResultOk
}
fn copy_c8(src: &str, dst: &mut [c_char]) {
    for (d, s) in dst.iter_mut().zip(src.bytes().chain(std::iter::once(0))) {
        *d = s as c_char;
    }
}

fn copy_c16(src: &str, dst: &mut [TChar]) {
    let mut n = 0;
    for (d, s) in dst.iter_mut().zip(src.encode_utf16()) {
        *d = s;
        n += 1;
    }
    if let Some(z) = dst.get_mut(n) {
        *z = 0;
    }
}

fn audio_bus(bus: *mut BusInfo, dir: BusDirection, name: &str) {
    let b = unsafe { &mut *bus };
    b.mediaType = MediaTypes_::kAudio as MediaType;
    b.direction = dir;
    b.channelCount = 2;
    copy_c16(name, &mut b.name);
    b.busType = BusTypes_::kMain as BusType;
    b.flags = BusInfo_::BusFlags_::kDefaultActive as u32;
}

struct Processor {
    values: [AtomicU64; P],
    arrangement: AtomicU64,
    calls: AtomicU64,
    params: AtomicU64,
    notes: AtomicU64,
    times: AtomicU64,
    frames: AtomicU64,
}
impl Class for Processor {
    type Interfaces = (IComponent, IAudioProcessor, IConnectionPoint);
}

impl IPluginBaseTrait for Processor {
    unsafe fn initialize(&self, context: *mut FUnknown) -> tresult {
        assert!(!context.is_null(), "host context");
        kResultOk
    }
    unsafe fn terminate(&self) -> tresult {
        kResultOk
    }
}

impl IConnectionPointTrait for Processor {
    unsafe fn connect(&self, other: *mut IConnectionPoint) -> tresult {
        if other.is_null() { kInvalidArgument } else { kResultOk }
    }
    unsafe fn disconnect(&self, _other: *mut IConnectionPoint) -> tresult {
        kResultOk
    }
    unsafe fn notify(&self, _message: *mut IMessage) -> tresult {
        kResultOk
    }
}

impl IComponentTrait for Processor {
    unsafe fn getControllerClassId(&self, class_id: *mut TUID) -> tresult {
        unsafe { *class_id = CTRL_CID };
        kResultOk
    }
    unsafe fn setIoMode(&self, _mode: IoMode) -> tresult {
        kResultOk
    }
    unsafe fn getBusCount(&self, media: MediaType, dir: BusDirection) -> i32 {
        if media == MediaTypes_::kAudio as MediaType || (media == MediaTypes_::kEvent as MediaType && dir == BusDirections_::kInput as BusDirection) {
            1
        } else {
            0
        }
    }
    unsafe fn getBusInfo(&self, media: MediaType, dir: BusDirection, index: i32, bus: *mut BusInfo) -> tresult {
        if index != 0 {
            return kInvalidArgument;
        }
        if media == MediaTypes_::kAudio as MediaType {
            audio_bus(bus, dir, "Main");
            return kResultOk;
        }
        if media != MediaTypes_::kEvent as MediaType || dir != BusDirections_::kInput as BusDirection {
            return kInvalidArgument;
        }
        let b = unsafe { &mut *bus };
        b.mediaType = media;
        b.direction = dir;
        b.channelCount = 16;
        b.busType = BusTypes_::kMain as BusType;
        b.flags = BusInfo_::BusFlags_::kDefaultActive;
        copy_c16("Receipt notes", &mut b.name);
        kResultOk
    }
    unsafe fn getRoutingInfo(&self, _i: *mut RoutingInfo, _o: *mut RoutingInfo) -> tresult {
        kNotImplemented
    }
    unsafe fn activateBus(&self, _m: MediaType, _d: BusDirection, _i: i32, _s: TBool) -> tresult {
        kResultOk
    }
    unsafe fn setActive(&self, _state: TBool) -> tresult {
        kResultOk
    }
    unsafe fn setState(&self, state: *mut IBStream) -> tresult {
        read_values(state, &self.values)
    }
    unsafe fn getState(&self, state: *mut IBStream) -> tresult {
        let Some(s) = (unsafe { ComRef::from_raw(state) }) else {
            return kInvalidArgument;
        };
        let mut bytes = [0u8; RECEIPT];
        for (v, b) in self.values.iter().zip(bytes[..P * 8].chunks_exact_mut(8)) {
            b.copy_from_slice(&v.load(Ordering::Relaxed).to_le_bytes());
        }
        for (v, b) in [
            self.calls.load(Ordering::Relaxed),
            self.params.load(Ordering::Relaxed),
            self.notes.load(Ordering::Relaxed),
            self.times.load(Ordering::Relaxed),
            self.frames.load(Ordering::Relaxed),
        ]
        .iter()
        .zip(bytes[P * 8..].chunks_exact_mut(8))
        {
            b.copy_from_slice(&v.to_le_bytes());
        }
        let mut n = 0;
        unsafe { s.write(bytes.as_mut_ptr().cast(), RECEIPT as i32, &mut n) }
    }
}

impl IAudioProcessorTrait for Processor {
    unsafe fn setBusArrangements(&self, inputs: *mut SpeakerArrangement, n_in: i32, outputs: *mut SpeakerArrangement, n_out: i32) -> tresult {
        if n_in != 1 || n_out != 1 {
            return kResultFalse;
        }
        let (i, o) = unsafe { (*inputs, *outputs) };
        if i != o || (i != SpeakerArr::kStereo && i != SpeakerArr::kMono) {
            return kResultFalse;
        }
        self.arrangement.store(i, Ordering::Relaxed);
        kResultTrue
    }
    unsafe fn getBusArrangement(&self, _dir: BusDirection, index: i32, arr: *mut SpeakerArrangement) -> tresult {
        if index != 0 {
            return kInvalidArgument;
        }
        unsafe { *arr = self.arrangement.load(Ordering::Relaxed) };
        kResultOk
    }
    unsafe fn canProcessSampleSize(&self, size: i32) -> tresult {
        if size == SymbolicSampleSizes_::kSample32 as i32 { kResultOk } else { kNotImplemented }
    }
    unsafe fn getLatencySamples(&self) -> u32 {
        0
    }
    unsafe fn setupProcessing(&self, setup: *mut ProcessSetup) -> tresult {
        let s = unsafe { &*setup };
        assert_eq!(s.symbolicSampleSize, SymbolicSampleSizes_::kSample32 as i32);
        assert!(s.maxSamplesPerBlock > 0 && s.sampleRate > 0.0);
        kResultOk
    }
    unsafe fn setProcessing(&self, _state: TBool) -> tresult {
        kNotImplemented
    }
    unsafe fn process(&self, data: *mut ProcessData) -> tresult {
        let d = unsafe { &*data };
        let changes = unsafe { ComRef::from_raw(d.inputParameterChanges) };
        // Every input parameter must have exactly one point at offset0. This fixture proves
        // single-value/span delivery; it deliberately makes no multi-point timing claim.
        let mut count = 0u64;
        if let Some(changes) = changes {
            for i in 0..unsafe { changes.getParameterCount() } {
                let Some(q) = (unsafe { ComRef::from_raw(changes.getParameterData(i)) }) else {
                    return kResultFalse;
                };
                let (mut off, mut v) = (0, 0.0);
                if unsafe { q.getPointCount() } != 1 || unsafe { q.getPoint(0, &mut off, &mut v) } != kResultOk || off != 0 {
                    return kResultFalse;
                }
            }
            for i in 0..unsafe { changes.getParameterCount() } {
                let Some(q) = (unsafe { ComRef::from_raw(changes.getParameterData(i)) }) else {
                    return kResultFalse;
                };
                let (mut off, mut v) = (0, 0.0);
                unsafe { q.getPoint(0, &mut off, &mut v) };
                let id = unsafe { q.getParameterId() } as usize;
                if let Some(slot) = self.values.get(id) {
                    slot.store(v.to_bits(), Ordering::Relaxed);
                    count += 1;
                }
            }
        }
        let mut notes = 0u64;
        let mut times = 0u64;
        let mut previous_time = 0;
        if let Some(events) = unsafe { ComRef::from_raw(d.inputEvents) } {
            for i in 0..unsafe { events.getEventCount() } {
                let mut event: Event = unsafe { std::mem::zeroed() };
                if unsafe { events.getEvent(i, &mut event) } != kResultOk {
                    return kResultFalse;
                }
                if event.sampleOffset < previous_time || event.sampleOffset < 0 || event.sampleOffset >= d.numSamples {
                    return kResultFalse;
                }
                previous_time = event.sampleOffset;
                notes += 1;
                times += event.sampleOffset as u64;
            }
        }
        if d.numInputs != 1 || d.numOutputs != 1 {
            return kResultFalse;
        }
        let input = unsafe { &*d.inputs };
        let output = unsafe { &*d.outputs };
        let gain = 2.0 * f64::from_bits(self.values[0].load(Ordering::Relaxed));
        let bias = f64::from_bits(self.values[1024].load(Ordering::Relaxed));
        for c in 0..input.numChannels.min(output.numChannels).max(0) as usize {
            let src = unsafe { std::slice::from_raw_parts(*input.__field0.channelBuffers32.add(c), d.numSamples.max(0) as usize) };
            let dst = unsafe { std::slice::from_raw_parts_mut(*output.__field0.channelBuffers32.add(c), d.numSamples.max(0) as usize) };
            for (x, y) in src.iter().zip(dst) {
                *y = (*x as f64 * gain + bias) as f32;
            }
        }
        self.params.store(count, Ordering::Relaxed);
        self.notes.store(notes, Ordering::Relaxed);
        self.times.store(times, Ordering::Relaxed);
        self.calls.fetch_add(1, Ordering::Relaxed);
        self.frames.store(d.numSamples as u64, Ordering::Relaxed);
        kResultOk
    }
    unsafe fn getTailSamples(&self) -> u32 {
        0
    }
}

struct Controller {
    values: [AtomicU64; P],
}
impl Class for Controller {
    type Interfaces = (IEditController, IConnectionPoint);
}

impl IPluginBaseTrait for Controller {
    unsafe fn initialize(&self, _context: *mut FUnknown) -> tresult {
        kResultOk
    }
    unsafe fn terminate(&self) -> tresult {
        kResultOk
    }
}

impl IConnectionPointTrait for Controller {
    unsafe fn connect(&self, other: *mut IConnectionPoint) -> tresult {
        if other.is_null() { kInvalidArgument } else { kResultOk }
    }
    unsafe fn disconnect(&self, _other: *mut IConnectionPoint) -> tresult {
        kResultOk
    }
    unsafe fn notify(&self, _message: *mut IMessage) -> tresult {
        kResultOk
    }
}

impl IEditControllerTrait for Controller {
    unsafe fn setComponentState(&self, s: *mut IBStream) -> tresult {
        read_values(s, &self.values)
    }
    unsafe fn setState(&self, _: *mut IBStream) -> tresult {
        kResultOk
    }
    unsafe fn getState(&self, _: *mut IBStream) -> tresult {
        kResultOk
    }
    unsafe fn getParameterCount(&self) -> i32 {
        P as i32
    }
    unsafe fn getParameterInfo(&self, i: i32, out: *mut ParameterInfo) -> tresult {
        if i < 0 || i as usize >= P {
            return kInvalidArgument;
        }
        let p = unsafe { &mut *out };
        p.id = i as u32;
        copy_c16("Controlled", &mut p.title);
        copy_c16("Controlled", &mut p.shortTitle);
        copy_c16("", &mut p.units);
        p.stepCount = if i == 1 {
            2
        } else if i == 2 {
            1
        } else {
            0
        };
        p.defaultNormalizedValue = if i == 0 { 0.5 } else { 0.0 };
        p.unitId = 0;
        p.flags = ParameterInfo_::ParameterFlags_::kCanAutomate | if i == 1 { ParameterInfo_::ParameterFlags_::kIsList } else { 0 };
        kResultOk
    }
    unsafe fn getParamStringByValue(&self, id: u32, v: f64, out: *mut String128) -> tresult {
        copy_c16(if id == 1 { ["A", "B", "C"][((v * 2.0).round() as usize).min(2)] } else { "Controlled" }, unsafe { &mut *out });
        kResultOk
    }
    unsafe fn getParamValueByString(&self, _: u32, _: *mut TChar, _: *mut f64) -> tresult {
        kNotImplemented
    }
    unsafe fn normalizedParamToPlain(&self, id: u32, v: f64) -> f64 {
        if id <= 1 { v * 2.0 } else { v }
    }
    unsafe fn plainParamToNormalized(&self, id: u32, v: f64) -> f64 {
        if id <= 1 { v / 2.0 } else { v }
    }
    unsafe fn getParamNormalized(&self, id: u32) -> f64 {
        self.values.get(id as usize).map_or(0.0, |v| f64::from_bits(v.load(Ordering::Relaxed)))
    }
    unsafe fn setParamNormalized(&self, id: u32, v: f64) -> tresult {
        let Some(slot) = self.values.get(id as usize) else {
            return kInvalidArgument;
        };
        slot.store(v.to_bits(), Ordering::Relaxed);
        kResultOk
    }
    unsafe fn setComponentHandler(&self, _: *mut IComponentHandler) -> tresult {
        kResultOk
    }
    unsafe fn createView(&self, _: FIDString) -> *mut IPlugView {
        std::ptr::null_mut()
    }
}
struct Factory;
impl Class for Factory {
    type Interfaces = (IPluginFactory2,);
}

const CLASSES: [(TUID, &str, &str, &str); 2] =
    [(FX_CID, "Audio Module Class", "Capacity Affine", "Fx"), (CTRL_CID, "Component Controller Class", "Capacity Affine", "")];

impl IPluginFactoryTrait for Factory {
    unsafe fn getFactoryInfo(&self, info: *mut PFactoryInfo) -> tresult {
        let i = unsafe { &mut *info };
        copy_c8("SoundCraft tests", &mut i.vendor);
        copy_c8("https://example.invalid", &mut i.url);
        copy_c8("tests@example.invalid", &mut i.email);
        i.flags = PFactoryInfo_::FactoryFlags_::kUnicode as i32;
        kResultOk
    }
    unsafe fn countClasses(&self) -> i32 {
        CLASSES.len() as i32
    }
    unsafe fn getClassInfo(&self, index: i32, info: *mut PClassInfo) -> tresult {
        let Some(&(cid, cat, name, _)) = CLASSES.get(index as usize) else { return kInvalidArgument };
        let i = unsafe { &mut *info };
        i.cid = cid;
        i.cardinality = PClassInfo_::ClassCardinality_::kManyInstances as i32;
        copy_c8(cat, &mut i.category);
        copy_c8(name, &mut i.name);
        kResultOk
    }
    unsafe fn createInstance(&self, cid: FIDString, iid: FIDString, obj: *mut *mut c_void) -> tresult {
        let cid = unsafe { *(cid as *const TUID) };
        let unknown = if cid == FX_CID {
            ComWrapper::new(Processor {
                values: initial_values(),
                arrangement: AtomicU64::new(SpeakerArr::kStereo),
                calls: AtomicU64::new(0),
                params: AtomicU64::new(0),
                notes: AtomicU64::new(0),
                times: AtomicU64::new(0),
                frames: AtomicU64::new(0),
            })
            .to_com_ptr::<FUnknown>()
        } else if cid == CTRL_CID {
            ComWrapper::new(Controller { values: initial_values() }).to_com_ptr::<FUnknown>()
        } else {
            None
        };
        match unknown {
            Some(u) => {
                let p = u.as_ptr();
                unsafe { ((*(*p).vtbl).queryInterface)(p, iid as *const TUID, obj) }
            }
            None => kInvalidArgument,
        }
    }
}

impl IPluginFactory2Trait for Factory {
    unsafe fn getClassInfo2(&self, index: i32, info: *mut PClassInfo2) -> tresult {
        let Some(&(cid, cat, name, subs)) = CLASSES.get(index as usize) else { return kInvalidArgument };
        let i = unsafe { &mut *info };
        i.cid = cid;
        i.cardinality = PClassInfo_::ClassCardinality_::kManyInstances as i32;
        copy_c8(cat, &mut i.category);
        copy_c8(name, &mut i.name);
        i.classFlags = 0;
        copy_c8(subs, &mut i.subCategories);
        copy_c8("", &mut i.vendor);
        copy_c8("1.2.3", &mut i.version);
        copy_c8("VST 3.8.0", &mut i.sdkVersion);
        kResultOk
    }
}

#[cfg(target_os = "windows")]
#[unsafe(no_mangle)]
extern "system" fn InitDll() -> bool {
    true
}
#[cfg(target_os = "windows")]
#[unsafe(no_mangle)]
extern "system" fn ExitDll() -> bool {
    true
}
#[cfg(target_os = "macos")]
#[unsafe(no_mangle)]
extern "C" fn bundleEntry(p: *mut c_void) -> bool {
    !p.is_null()
}
#[cfg(target_os = "macos")]
#[unsafe(no_mangle)]
extern "C" fn bundleExit() -> bool {
    true
}
#[cfg(all(unix, not(target_os = "macos")))]
#[unsafe(no_mangle)]
extern "C" fn ModuleEntry(p: *mut c_void) -> bool {
    !p.is_null()
}
#[cfg(all(unix, not(target_os = "macos")))]
#[unsafe(no_mangle)]
extern "C" fn ModuleExit() -> bool {
    true
}
#[unsafe(no_mangle)]
extern "system" fn GetPluginFactory() -> *mut IPluginFactory {
    ComWrapper::new(Factory).to_com_ptr::<IPluginFactory>().map_or(std::ptr::null_mut(), |p| p.into_raw())
}
