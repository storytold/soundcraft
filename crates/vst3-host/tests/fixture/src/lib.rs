//! Test-only VST3 plugin for soundcraft-vst3-host (two audio module classes):
//!
//! - "Fixture Gain" (`Fx`): stereo or mono in → out × gain, with a separate edit controller
//!   (connected through IConnectionPoint, synced through the component state). Params: 7 "Gain"
//!   (plain 0..2, linear, units dB), 3 "Mode" (3 steps: A/B/C), 12 "Freq" (20..20000 Hz, log),
//!   9 hidden, 11 bypass. Starts at gain 0.5 (its state) although the declared default is 1.0.
//!   Reports 3 samples of latency. State: the component stores the gain (one LE f64), the
//!   controller the Freq value (one LE f64). Its controller has an editor view (`IPlugView`,
//!   320×240, no real drawing) supporting NSView/HWND/X11.
//! - "Fixture Synth" (`Instrument|Synth`): a single-component instrument (the component is also
//!   its edit controller). No audio input, one stereo output, one event input; every output
//!   sample is the velocity of the held note (sample-accurate), 0 when none.
//!
//! The module checks the host's module lifecycle the way real plugins depend on it (see "module
//! lifecycle" below) and aborts the process when it is broken.
//!
//! Test code: panics are fine here, the host must survive anything anyway.

#![allow(non_snake_case, clippy::missing_safety_doc)]

use std::ffi::{c_char, c_int, c_void};
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use vst3::Steinberg::Vst::*;
use vst3::Steinberg::*;
use vst3::{Class, ComRef, ComWrapper, uid};

const GAIN_CID: TUID = uid(0x5C0A1D00, 0x11112222, 0x33334444, 0x55556666);
const GAIN_CTRL_CID: TUID = uid(0x5C0A1D01, 0x11112222, 0x33334444, 0x55556666);
const SYNTH_CID: TUID = uid(0x5C0A1D02, 0x11112222, 0x33334444, 0x55556666);

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
    b.flags = BusInfo_::BusFlags_::kDefaultActive;
}

unsafe fn channels<'a>(bus: &AudioBusBuffers, frames: usize) -> Vec<&'a mut [f32]> {
    let n = bus.numChannels.max(0) as usize;
    let table = unsafe { std::slice::from_raw_parts(bus.__field0.channelBuffers32, n) };
    table.iter().map(|&p| unsafe { std::slice::from_raw_parts_mut(p, frames) }).collect()
}

// ---- gain ----------------------------------------------------------------------------------

struct GainProcessor {
    /// Normalized gain (plain = 2 × normalized).
    gain: AtomicU64,
    arrangement: AtomicU64,
    _live: Live,
}

impl Class for GainProcessor {
    type Interfaces = (IComponent, IAudioProcessor, IConnectionPoint);
}

impl IPluginBaseTrait for GainProcessor {
    unsafe fn initialize(&self, context: *mut FUnknown) -> tresult {
        assert!(!context.is_null(), "host context");
        kResultOk
    }
    unsafe fn terminate(&self) -> tresult {
        kResultOk
    }
}

impl IConnectionPointTrait for GainProcessor {
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

impl IComponentTrait for GainProcessor {
    unsafe fn getControllerClassId(&self, class_id: *mut TUID) -> tresult {
        unsafe { *class_id = GAIN_CTRL_CID };
        kResultOk
    }
    unsafe fn setIoMode(&self, _mode: IoMode) -> tresult {
        kResultOk
    }
    unsafe fn getBusCount(&self, media: MediaType, _dir: BusDirection) -> i32 {
        if media == MediaTypes_::kAudio as MediaType { 1 } else { 0 }
    }
    unsafe fn getBusInfo(&self, media: MediaType, dir: BusDirection, index: i32, bus: *mut BusInfo) -> tresult {
        if media != MediaTypes_::kAudio as MediaType || index != 0 {
            return kInvalidArgument;
        }
        audio_bus(bus, dir, "Main");
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
        let Some(s) = (unsafe { ComRef::from_raw(state) }) else { return kInvalidArgument };
        let mut bytes = [0u8; 8];
        let mut n = 0;
        if unsafe { s.read(bytes.as_mut_ptr().cast(), 8, &mut n) } != kResultOk || n != 8 {
            return kResultFalse;
        }
        let g = f64::from_le_bytes(bytes);
        if !(0.0..=1.0).contains(&g) {
            return kResultFalse;
        }
        self.gain.store(g.to_bits(), Ordering::Relaxed);
        kResultOk
    }
    unsafe fn getState(&self, state: *mut IBStream) -> tresult {
        let Some(s) = (unsafe { ComRef::from_raw(state) }) else { return kInvalidArgument };
        let mut bytes = f64::from_bits(self.gain.load(Ordering::Relaxed)).to_le_bytes();
        let mut n = 0;
        unsafe { s.write(bytes.as_mut_ptr().cast(), 8, &mut n) }
    }
}

impl IAudioProcessorTrait for GainProcessor {
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
        3
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
        assert!(!d.processContext.is_null());
        if let Some(changes) = unsafe { ComRef::from_raw(d.inputParameterChanges) } {
            for i in 0..unsafe { changes.getParameterCount() } {
                let Some(q) = (unsafe { ComRef::from_raw(changes.getParameterData(i)) }) else { continue };
                let (mut off, mut v) = (0, 0.0);
                let n = unsafe { q.getPointCount() };
                if unsafe { q.getParameterId() } == 7 && n > 0 && unsafe { q.getPoint(n - 1, &mut off, &mut v) } == kResultOk {
                    self.gain.store(v.to_bits(), Ordering::Relaxed);
                }
            }
        }
        if d.numInputs != 1 || d.numOutputs != 1 {
            return kResultOk;
        }
        let frames = d.numSamples.max(0) as usize;
        let g = 2.0 * f64::from_bits(self.gain.load(Ordering::Relaxed)) as f32;
        let (ins, outs) = unsafe { (channels(&*d.inputs, frames), channels(&*d.outputs, frames)) };
        for (i, o) in ins.iter().zip(outs) {
            for (x, y) in i.iter().zip(o.iter_mut()) {
                *y = g * x;
            }
        }
        kResultOk
    }
    unsafe fn getTailSamples(&self) -> u32 {
        0
    }
}

struct GainController {
    gain: AtomicU64,
    freq: AtomicU64,
    _live: Live,
}

struct View {
    attached: std::sync::atomic::AtomicBool,
    _live: Live,
}

impl Class for View {
    type Interfaces = (IPlugView,);
}

impl IPlugViewTrait for View {
    unsafe fn isPlatformTypeSupported(&self, t: FIDString) -> tresult {
        let t = unsafe { std::ffi::CStr::from_ptr(t) }.to_bytes();
        if t == b"NSView" || t == b"HWND" || t == b"X11EmbedWindowID" { kResultTrue } else { kResultFalse }
    }
    unsafe fn attached(&self, parent: *mut c_void, _t: FIDString) -> tresult {
        if parent.is_null() {
            return kInvalidArgument;
        }
        self.attached.store(true, Ordering::Relaxed);
        kResultOk
    }
    unsafe fn removed(&self) -> tresult {
        assert!(self.attached.swap(false, Ordering::Relaxed), "removed without attached");
        kResultOk
    }
    unsafe fn onWheel(&self, _d: f32) -> tresult {
        kResultFalse
    }
    unsafe fn onKeyDown(&self, _k: char16, _c: i16, _m: i16) -> tresult {
        kResultFalse
    }
    unsafe fn onKeyUp(&self, _k: char16, _c: i16, _m: i16) -> tresult {
        kResultFalse
    }
    unsafe fn getSize(&self, size: *mut ViewRect) -> tresult {
        let r = unsafe { &mut *size };
        *r = ViewRect { left: 0, top: 0, right: 320, bottom: 240 };
        kResultOk
    }
    unsafe fn onSize(&self, _s: *mut ViewRect) -> tresult {
        kResultOk
    }
    unsafe fn onFocus(&self, _s: TBool) -> tresult {
        kResultOk
    }
    unsafe fn setFrame(&self, _f: *mut IPlugFrame) -> tresult {
        kResultOk
    }
    unsafe fn canResize(&self) -> tresult {
        kResultFalse
    }
    unsafe fn checkSizeConstraint(&self, _r: *mut ViewRect) -> tresult {
        kResultOk
    }
}

impl Class for GainController {
    type Interfaces = (IEditController, IConnectionPoint);
}

impl IPluginBaseTrait for GainController {
    unsafe fn initialize(&self, _context: *mut FUnknown) -> tresult {
        kResultOk
    }
    unsafe fn terminate(&self) -> tresult {
        kResultOk
    }
}

impl IConnectionPointTrait for GainController {
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

const GAIN_PARAMS: [(u32, &str, &str, i32, f64, i32); 5] = [
    (7, "Gain", "dB", 0, 0.5, ParameterInfo_::ParameterFlags_::kCanAutomate),
    (9, "Secret", "", 0, 0.0, ParameterInfo_::ParameterFlags_::kIsHidden),
    (3, "Mode", "", 2, 0.0, ParameterInfo_::ParameterFlags_::kIsList),
    (11, "Bypass", "", 1, 0.0, ParameterInfo_::ParameterFlags_::kIsBypass),
    (12, "Freq", "Hz", 0, 0.5, 0),
];

impl IEditControllerTrait for GainController {
    unsafe fn setComponentState(&self, state: *mut IBStream) -> tresult {
        let Some(s) = (unsafe { ComRef::from_raw(state) }) else { return kInvalidArgument };
        let mut bytes = [0u8; 8];
        let mut n = 0;
        if unsafe { s.read(bytes.as_mut_ptr().cast(), 8, &mut n) } == kResultOk && n == 8 {
            self.gain.store(f64::from_le_bytes(bytes).to_bits(), Ordering::Relaxed);
        }
        kResultOk
    }
    unsafe fn setState(&self, state: *mut IBStream) -> tresult {
        let Some(s) = (unsafe { ComRef::from_raw(state) }) else { return kInvalidArgument };
        let mut bytes = [0u8; 8];
        let mut n = 0;
        if unsafe { s.read(bytes.as_mut_ptr().cast(), 8, &mut n) } == kResultOk && n == 8 {
            self.freq.store(f64::from_le_bytes(bytes).clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
        }
        kResultOk
    }
    unsafe fn getState(&self, state: *mut IBStream) -> tresult {
        let Some(s) = (unsafe { ComRef::from_raw(state) }) else { return kInvalidArgument };
        let mut bytes = f64::from_bits(self.freq.load(Ordering::Relaxed)).to_le_bytes();
        let mut n = 0;
        unsafe { s.write(bytes.as_mut_ptr().cast(), 8, &mut n) }
    }
    unsafe fn getParameterCount(&self) -> i32 {
        GAIN_PARAMS.len() as i32
    }
    unsafe fn getParameterInfo(&self, index: i32, info: *mut ParameterInfo) -> tresult {
        let Some(&(id, title, units, steps, def, flags)) = GAIN_PARAMS.get(index as usize) else { return kInvalidArgument };
        let i = unsafe { &mut *info };
        i.id = id;
        copy_c16(title, &mut i.title);
        copy_c16(title, &mut i.shortTitle);
        copy_c16(units, &mut i.units);
        i.stepCount = steps;
        i.defaultNormalizedValue = def;
        i.unitId = 0;
        i.flags = flags;
        kResultOk
    }
    unsafe fn getParamStringByValue(&self, id: u32, n: f64, string: *mut String128) -> tresult {
        let s = unsafe { &mut *string };
        let text = match id {
            3 => ["A", "B", "C"][((n * 2.0).round() as usize).min(2)].to_string(),
            _ => format!("{:.2}", unsafe { self.normalizedParamToPlain(id, n) }),
        };
        copy_c16(&text, s);
        kResultOk
    }
    unsafe fn getParamValueByString(&self, _id: u32, _s: *mut TChar, _v: *mut f64) -> tresult {
        kNotImplemented
    }
    unsafe fn normalizedParamToPlain(&self, id: u32, n: f64) -> f64 {
        match id {
            7 => 2.0 * n,
            3 => (n * 2.0).round(),
            12 => 20.0 * 1000f64.powf(n),
            _ => n,
        }
    }
    unsafe fn plainParamToNormalized(&self, id: u32, v: f64) -> f64 {
        match id {
            7 => v / 2.0,
            3 => v / 2.0,
            12 => (v / 20.0).ln() / 1000f64.ln(),
            _ => v,
        }
    }
    unsafe fn getParamNormalized(&self, id: u32) -> f64 {
        match id {
            7 => f64::from_bits(self.gain.load(Ordering::Relaxed)),
            12 => f64::from_bits(self.freq.load(Ordering::Relaxed)),
            _ => 0.0,
        }
    }
    unsafe fn setParamNormalized(&self, id: u32, v: f64) -> tresult {
        if id == 7 {
            self.gain.store(v.to_bits(), Ordering::Relaxed);
        } else if id == 12 {
            self.freq.store(v.to_bits(), Ordering::Relaxed);
        }
        kResultOk
    }
    unsafe fn setComponentHandler(&self, handler: *mut IComponentHandler) -> tresult {
        // Exercise the host's handler: an edit from the "plugin GUI" must be accepted.
        if let Some(h) = unsafe { ComRef::from_raw(handler) } {
            assert_eq!(unsafe { h.beginEdit(7) }, kResultOk);
            assert_eq!(unsafe { h.performEdit(7, 0.9) }, kResultOk);
            assert_eq!(unsafe { h.endEdit(7) }, kResultOk);
        }
        kResultOk
    }
    unsafe fn createView(&self, name: FIDString) -> *mut IPlugView {
        if name.is_null() || unsafe { std::ffi::CStr::from_ptr(name) }.to_bytes() != b"editor" {
            return std::ptr::null_mut();
        }
        match ComWrapper::new(View { attached: std::sync::atomic::AtomicBool::new(false), _live: Live::new(&OBJECTS) }).to_com_ptr::<IPlugView>() {
            Some(p) => p.into_raw(),
            None => std::ptr::null_mut(),
        }
    }
}

// ---- synth ---------------------------------------------------------------------------------

struct Synth {
    level: AtomicU32,
    _live: Live,
}

impl Class for Synth {
    type Interfaces = (IComponent, IAudioProcessor, IEditController);
}

impl IPluginBaseTrait for Synth {
    unsafe fn initialize(&self, _context: *mut FUnknown) -> tresult {
        kResultOk
    }
    unsafe fn terminate(&self) -> tresult {
        kResultOk
    }
}

impl IComponentTrait for Synth {
    unsafe fn getControllerClassId(&self, _class_id: *mut TUID) -> tresult {
        kNotImplemented
    }
    unsafe fn setIoMode(&self, _mode: IoMode) -> tresult {
        kResultOk
    }
    unsafe fn getBusCount(&self, media: MediaType, dir: BusDirection) -> i32 {
        let out = dir == BusDirections_::kOutput as BusDirection;
        if media == MediaTypes_::kAudio as MediaType && out {
            1
        } else if media == MediaTypes_::kEvent as MediaType && !out {
            1
        } else {
            0
        }
    }
    unsafe fn getBusInfo(&self, media: MediaType, dir: BusDirection, index: i32, bus: *mut BusInfo) -> tresult {
        if index != 0 {
            return kInvalidArgument;
        }
        if media == MediaTypes_::kAudio as MediaType && dir == BusDirections_::kOutput as BusDirection {
            audio_bus(bus, dir, "Out");
            return kResultOk;
        }
        if media == MediaTypes_::kEvent as MediaType && dir == BusDirections_::kInput as BusDirection {
            let b = unsafe { &mut *bus };
            b.mediaType = media;
            b.direction = dir;
            b.channelCount = 16;
            copy_c16("Notes", &mut b.name);
            b.busType = BusTypes_::kMain as BusType;
            b.flags = BusInfo_::BusFlags_::kDefaultActive;
            return kResultOk;
        }
        kInvalidArgument
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
    unsafe fn setState(&self, _state: *mut IBStream) -> tresult {
        kResultOk
    }
    unsafe fn getState(&self, _state: *mut IBStream) -> tresult {
        kResultOk
    }
}

impl IAudioProcessorTrait for Synth {
    unsafe fn setBusArrangements(&self, _i: *mut SpeakerArrangement, n_in: i32, outputs: *mut SpeakerArrangement, n_out: i32) -> tresult {
        if n_in == 0 && n_out == 1 && unsafe { *outputs } == SpeakerArr::kStereo { kResultTrue } else { kResultFalse }
    }
    unsafe fn getBusArrangement(&self, dir: BusDirection, index: i32, arr: *mut SpeakerArrangement) -> tresult {
        if dir != BusDirections_::kOutput as BusDirection || index != 0 {
            return kInvalidArgument;
        }
        unsafe { *arr = SpeakerArr::kStereo };
        kResultOk
    }
    unsafe fn canProcessSampleSize(&self, size: i32) -> tresult {
        if size == SymbolicSampleSizes_::kSample32 as i32 { kResultOk } else { kNotImplemented }
    }
    unsafe fn getLatencySamples(&self) -> u32 {
        0
    }
    unsafe fn setupProcessing(&self, _setup: *mut ProcessSetup) -> tresult {
        kResultOk
    }
    unsafe fn setProcessing(&self, _state: TBool) -> tresult {
        kResultOk
    }
    unsafe fn process(&self, data: *mut ProcessData) -> tresult {
        let d = unsafe { &*data };
        if d.numOutputs != 1 {
            return kResultOk;
        }
        let frames = d.numSamples.max(0) as usize;
        let mut events = Vec::new();
        if let Some(list) = unsafe { ComRef::from_raw(d.inputEvents) } {
            for i in 0..unsafe { list.getEventCount() } {
                let mut e: Event = unsafe { std::mem::zeroed() };
                if unsafe { list.getEvent(i, &mut e) } == kResultOk {
                    events.push(e);
                }
            }
        }
        let outs = unsafe { channels(&*d.outputs, frames) };
        let mut level = f32::from_bits(self.level.load(Ordering::Relaxed));
        let mut out = vec![0.0f32; frames];
        for (i, o) in out.iter_mut().enumerate() {
            for e in events.iter().filter(|e| e.sampleOffset as usize == i) {
                if e.r#type == Event_::EventTypes_::kNoteOnEvent as u16 {
                    level = unsafe { e.__field0.noteOn.velocity };
                } else if e.r#type == Event_::EventTypes_::kNoteOffEvent as u16 {
                    level = 0.0;
                }
            }
            *o = level;
        }
        self.level.store(level.to_bits(), Ordering::Relaxed);
        for ch in outs {
            ch.copy_from_slice(&out);
        }
        kResultOk
    }
    unsafe fn getTailSamples(&self) -> u32 {
        0
    }
}

impl IEditControllerTrait for Synth {
    unsafe fn setComponentState(&self, _state: *mut IBStream) -> tresult {
        kResultOk
    }
    unsafe fn setState(&self, _state: *mut IBStream) -> tresult {
        kResultOk
    }
    unsafe fn getState(&self, _state: *mut IBStream) -> tresult {
        kResultOk
    }
    unsafe fn getParameterCount(&self) -> i32 {
        0
    }
    unsafe fn getParameterInfo(&self, _index: i32, _info: *mut ParameterInfo) -> tresult {
        kInvalidArgument
    }
    unsafe fn getParamStringByValue(&self, _id: u32, _n: f64, _s: *mut String128) -> tresult {
        kInvalidArgument
    }
    unsafe fn getParamValueByString(&self, _id: u32, _s: *mut TChar, _v: *mut f64) -> tresult {
        kInvalidArgument
    }
    unsafe fn normalizedParamToPlain(&self, _id: u32, n: f64) -> f64 {
        n
    }
    unsafe fn plainParamToNormalized(&self, _id: u32, v: f64) -> f64 {
        v
    }
    unsafe fn getParamNormalized(&self, _id: u32) -> f64 {
        0.0
    }
    unsafe fn setParamNormalized(&self, _id: u32, _v: f64) -> tresult {
        kResultOk
    }
    unsafe fn setComponentHandler(&self, _handler: *mut IComponentHandler) -> tresult {
        kResultOk
    }
    unsafe fn createView(&self, _name: FIDString) -> *mut IPlugView {
        std::ptr::null_mut()
    }
}

// ---- factory -------------------------------------------------------------------------------

struct Factory {
    _live: Live,
}

impl Class for Factory {
    type Interfaces = (IPluginFactory2,);
}

const CLASSES: [(TUID, &str, &str, &str); 3] = [
    (GAIN_CID, "Audio Module Class", "Fixture Gain", "Fx"),
    (GAIN_CTRL_CID, "Component Controller Class", "Fixture Gain", ""),
    (SYNTH_CID, "Audio Module Class", "Fixture Synth", "Instrument|Synth"),
];

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
        set_up_late_state();
        let cid = unsafe { *(cid as *const TUID) };
        let unknown = if cid == GAIN_CID {
            ComWrapper::new(GainProcessor {
                gain: AtomicU64::new(0.25f64.to_bits()),
                arrangement: AtomicU64::new(SpeakerArr::kStereo),
                _live: Live::new(&OBJECTS),
            })
            .to_com_ptr::<FUnknown>()
        } else if cid == GAIN_CTRL_CID {
            ComWrapper::new(GainController {
                gain: AtomicU64::new(0.5f64.to_bits()),
                freq: AtomicU64::new(0.5f64.to_bits()),
                _live: Live::new(&OBJECTS),
            })
            .to_com_ptr::<FUnknown>()
        } else if cid == SYNTH_CID {
            ComWrapper::new(Synth { level: AtomicU32::new(0), _live: Live::new(&OBJECTS) }).to_com_ptr::<FUnknown>()
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

// ---- module lifecycle ----------------------------------------------------------------------
//
// The contract real plugins rely on: the host runs the module exit once the factory and every
// plugin object are released, on the thread that ran the module entry, and before the process
// tears down the module's statics. Breaking it ends the process with status 3, much as real
// plugins crash (HALion Sonic segfaults in its static destructors when the module exit never ran,
// and in its module exit when that runs from an exit handler after it was used; Massive X in its
// module exit when that runs on another thread than the entry).

/// Module entries minus module exits (they nest, as in the VST3 SDK).
static ENTERED: AtomicI64 = AtomicI64::new(0);
/// The OS thread that ran the outermost module entry.
static ENTRY_THREAD: AtomicUsize = AtomicUsize::new(0);
/// Factories handed out and not yet released.
static FACTORIES: AtomicI64 = AtomicI64::new(0);
/// Plugin objects alive: components, controllers, views.
static OBJECTS: AtomicI64 = AtomicI64::new(0);
static TEARDOWN_REGISTERED: AtomicBool = AtomicBool::new(false);
/// With `VST3_FIXTURE_LATE_STATE` set, the first plugin made sets up state that the process exit
/// tears down, like a static a real plugin initialises lazily; the module exit needs it.
static LATE_STATE_REGISTERED: AtomicBool = AtomicBool::new(false);
static LATE_STATE_GONE: AtomicBool = AtomicBool::new(false);

unsafe extern "C" {
    fn atexit(f: extern "C" fn()) -> c_int;
    fn _exit(status: c_int) -> !;
}

/// Counts itself in one of the counters above for as long as it exists.
struct Live(&'static AtomicI64);

impl Live {
    fn new(counter: &'static AtomicI64) -> Live {
        counter.fetch_add(1, Ordering::SeqCst);
        Live(counter)
    }
}

impl Drop for Live {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Ends the process at once with status 3 (no crash report: one test expects this).
fn violation(what: &str) -> ! {
    eprintln!("vst3-fixture: host contract violation: {what}");
    unsafe { _exit(3) }
}

/// The OS id of the calling thread (`std::thread::current` is gone in exit handlers).
fn os_thread() -> usize {
    #[cfg(unix)]
    {
        unsafe extern "C" {
            fn pthread_self() -> usize;
        }
        unsafe { pthread_self() }
    }
    #[cfg(windows)]
    {
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn GetCurrentThreadId() -> u32;
        }
        unsafe { GetCurrentThreadId() as usize }
    }
}

fn module_entry() -> bool {
    if ENTERED.fetch_add(1, Ordering::SeqCst) == 0 {
        ENTRY_THREAD.store(os_thread(), Ordering::SeqCst);
    }
    if !TEARDOWN_REGISTERED.swap(true, Ordering::SeqCst) {
        unsafe { atexit(static_teardown) };
    }
    true
}

/// Called whenever the factory makes a plugin object.
fn set_up_late_state() {
    if std::env::var_os("VST3_FIXTURE_LATE_STATE").is_some() && !LATE_STATE_REGISTERED.swap(true, Ordering::SeqCst) {
        unsafe { atexit(late_state_teardown) };
    }
}

extern "C" fn late_state_teardown() {
    LATE_STATE_GONE.store(true, Ordering::SeqCst);
}

fn module_exit() -> bool {
    let left = ENTERED.fetch_sub(1, Ordering::SeqCst) - 1;
    if left < 0 {
        violation("module exit without a module entry");
    }
    let (factories, objects) = (FACTORIES.load(Ordering::SeqCst), OBJECTS.load(Ordering::SeqCst));
    if left == 0 && (factories != 0 || objects != 0) {
        violation(&format!("module exit with {factories} factories and {objects} plugin objects alive"));
    }
    if left == 0 && os_thread() != ENTRY_THREAD.load(Ordering::SeqCst) {
        violation("module exit on another thread than the module entry");
    }
    if LATE_STATE_GONE.load(Ordering::SeqCst) {
        violation("module exit after the process exit tore down state the module set up while in use");
    }
    true
}

/// Stands in for the C++ static destructors a real module registers while it loads.
extern "C" fn static_teardown() {
    if ENTERED.load(Ordering::SeqCst) > 0 && OBJECTS.load(Ordering::SeqCst) == 0 {
        violation("static teardown of a module that is not in use but was never exited");
    }
}

#[cfg(target_os = "windows")]
#[unsafe(no_mangle)]
extern "system" fn InitDll() -> bool {
    module_entry()
}

#[cfg(target_os = "windows")]
#[unsafe(no_mangle)]
extern "system" fn ExitDll() -> bool {
    module_exit()
}

#[cfg(target_os = "macos")]
#[unsafe(no_mangle)]
extern "C" fn bundleEntry(bundle: *mut c_void) -> bool {
    !bundle.is_null() && module_entry()
}

#[cfg(target_os = "macos")]
#[unsafe(no_mangle)]
extern "C" fn bundleExit() -> bool {
    module_exit()
}

#[cfg(all(unix, not(target_os = "macos")))]
#[unsafe(no_mangle)]
extern "C" fn ModuleEntry(handle: *mut c_void) -> bool {
    !handle.is_null() && module_entry()
}

#[cfg(all(unix, not(target_os = "macos")))]
#[unsafe(no_mangle)]
extern "C" fn ModuleExit() -> bool {
    module_exit()
}

#[unsafe(no_mangle)]
extern "system" fn GetPluginFactory() -> *mut IPluginFactory {
    match ComWrapper::new(Factory { _live: Live::new(&FACTORIES) }).to_com_ptr::<IPluginFactory>() {
        Some(p) => p.into_raw(),
        None => std::ptr::null_mut(),
    }
}
