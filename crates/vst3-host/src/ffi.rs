//! The FFI boundary: the only module of this crate allowed to use `unsafe`.
//!
//! It loads VST3 binaries (`libloading`), runs the platform module entry, calls the COM
//! interfaces of the factory, component, audio processor and edit controller (`vst3` raw
//! bindings), and implements the host-side COM objects (host context, parameter changes, event
//! lists, a memory stream). Everything it exposes to the rest of the crate is a safe API.
//!
//! Soundness rests on the VST3 contract (the plugin honours the COM vtable signatures and the
//! documented pointer lifetimes). A plugin that violates it can still crash the process: hosting
//! in-process can't prevent that. What this module guarantees is that *SoundCraft* never hands
//! the plugin dangling or mis-sized pointers and never reads plugin strings beyond their bounds.

// VST3 enum constants are `c_int` on Windows and `c_uint` elsewhere, so casts that are no-ops on
// one platform are needed on another.
#![allow(clippy::unnecessary_cast)]

use crate::{Vst3Error, scan};
use soundcraft_dsp::Transport;
use std::ffi::{c_char, c_int, c_void};
use std::mem::ManuallyDrop;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock, PoisonError, TryLockError, mpsc};
use std::time::Duration;
use vst3::Steinberg::Vst::{self as sv};
use vst3::Steinberg::Vst::{
    IAudioProcessorTrait, IComponentHandlerTrait, IComponentTrait, IConnectionPointTrait, IEditControllerTrait, IEventListTrait,
    IHostApplicationTrait, IMidiMappingTrait, IParamValueQueueTrait, IParameterChangesTrait,
};
use vst3::Steinberg::{self as sb};
use vst3::Steinberg::{FUnknown, IBStreamTrait, IPlugFrameTrait, IPlugViewTrait, IPluginBaseTrait, IPluginFactory2Trait, IPluginFactoryTrait};
use vst3::{Class, ComPtr, ComWrapper, Interface};

mod editor;
mod host;
mod instance;
mod module;
mod process;

pub(crate) use editor::EditorLink;
pub(crate) use host::HostContext;
pub(crate) use instance::{Instance, PARAM_BYPASS, PARAM_HIDDEN, PARAM_READ_ONLY, ProcessInput, RawParam};
#[cfg(test)]
use instance::{apply_transport, event_to_midi, split_state};
pub(crate) use module::{Bundle, exit_modules, hex};
pub(crate) use process::{BusBuffers, NoteEvent};
#[cfg(test)]
pub(crate) use process::{EventList, MemoryStream, ParamChanges};

/// Most classes one factory may report (hostile-input cap).
const MAX_CLASSES: i32 = 1024;
/// Most parameters read from one controller.
pub(crate) const MAX_PARAMS: i32 = 4096;
/// Most audio buses per direction we provide buffers for. Multi-out instruments declare dozens
/// of (mostly inactive) aux buses, e.g. 32 for a drum sampler; every one still gets buffers.
const MAX_BUSES: i32 = 64;
/// Most channels per audio bus.
const MAX_BUS_CHANNELS: u32 = 64;
/// Capacity of the per-block event lists (preallocated: `process` never allocates).
pub(crate) const EVENT_CAP: usize = 1024;
/// Most parameter queues per block (preallocated).
pub(crate) const PARAM_QUEUE_CAP: usize = 1024;
/// Largest plugin state we buffer (component or controller state).
const MAX_STATE_BYTES: usize = 64 << 20;
/// Most editor parameter edits buffered between two UI frames.
const EDIT_CAP: usize = 256;
/// Magic of SoundCraft's state container: `SCV3` + u32 LE component length + component state +
/// u32 LE controller length + controller state.
const STATE_MAGIC: &[u8; 4] = b"SCV3";

// ---- strings -------------------------------------------------------------------------------

/// Reads a fixed-size `char8[N]` buffer, stopping at the first NUL or the end of the array.
fn read_c8(buf: &[c_char]) -> String {
    let bytes: Vec<u8> = buf.iter().take_while(|c| **c != 0).map(|c| *c as u8).collect();
    String::from_utf8_lossy(&bytes).trim().to_string()
}

/// Reads a fixed-size UTF-16 `String128`, stopping at the first NUL or the end of the array.
fn read_c16(buf: &[u16]) -> String {
    let units: Vec<u16> = buf.iter().take_while(|c| **c != 0).copied().collect();
    String::from_utf16_lossy(&units).trim().to_string()
}

/// Writes `s` into a UTF-16 buffer, truncated and always NUL-terminated.
fn write_c16(s: &str, dst: &mut [u16]) {
    let Some(room) = dst.len().checked_sub(1) else { return };
    let mut n = 0;
    for (d, u) in dst.iter_mut().zip(s.encode_utf16().take(room)) {
        *d = u;
        n += 1;
    }
    if let Some(z) = dst.get_mut(n) {
        *z = 0;
    }
}

fn ok(r: sb::tresult) -> bool {
    r == sb::kResultOk
}

/// VST3's numbers for the MIDI controllers that aren't CCs.
pub(crate) const AFTERTOUCH: usize = sv::ControllerNumbers_::kAfterTouch as usize;
pub(crate) const PITCH_BEND: usize = sv::ControllerNumbers_::kPitchBend as usize;

/// The platform host window for plugin views.
#[cfg(target_os = "macos")]
#[path = "ffi/window/macos.rs"]
mod window;

/// The Windows host window: a top-level window owned by the app's active window (so it stays
/// above it and has no taskbar button) whose client area is the plugin view's `HWND` parent.
/// Closing it only hides it; the editor link notices in `idle`, detaches the view, then destroys
/// the window. Messages are dispatched by the app's own event loop on the main thread.
#[cfg(windows)]
#[path = "ffi/window/windows.rs"]
mod window;

/// Linux and the BSDs: an X11 parent window for plugin views.
///
/// VST3's platform type there is `X11EmbedWindowID`: the host creates a window, hands the
/// plugin its X11 window id, and the plugin creates its own child window inside it. `libX11` is
/// loaded at run time through `x11-dl`, so building SoundCraft needs no X11 development files.
#[cfg(all(unix, not(target_os = "macos"), not(target_os = "android")))]
#[path = "ffi/window/x11.rs"]
mod window;

/// Other platforms: no editor windows yet (opening reports "unsupported").
#[cfg(not(any(target_os = "macos", windows, all(unix, not(target_os = "macos"), not(target_os = "android")))))]
#[path = "ffi/window/other.rs"]
mod window;

#[cfg(test)]
mod tests {
    use super::*;
    use sv::{IAttributeListTrait as _, IMessageTrait as _};

    fn tuid(iid: [u8; 16]) -> sb::TUID {
        iid.map(|b| b as c_char)
    }

    /// Asks the host for an object the way a plugin does.
    fn create<I: Interface>(host: &ComWrapper<HostContext>, cid: [u8; 16]) -> Option<ComPtr<I>> {
        let app = host.to_com_ptr::<sv::IHostApplication>()?;
        let (mut c, mut i) = (tuid(cid), tuid(I::IID));
        let mut obj: *mut c_void = std::ptr::null_mut();
        // SAFETY: valid TUIDs and out-pointer, as a plugin passes them.
        let r = unsafe { app.createInstance(&mut c, &mut i, &mut obj) };
        // SAFETY: on success `obj` is an owned (add-ref'ed) pointer to `I`.
        (r == sb::kResultOk).then(|| unsafe { ComPtr::from_raw(obj.cast::<I>()) }).flatten()
    }

    #[test]
    fn plugins_get_messages_and_attribute_lists_from_the_host() {
        let host = ComWrapper::new(HostContext::default());
        let msg = create::<sv::IMessage>(&host, sv::IMessage::IID).expect("IMessage");
        // SAFETY: calls through the interface with valid C strings and buffers.
        unsafe {
            msg.setMessageID(c"DatabaseInfo".as_ptr());
            assert_eq!(std::ffi::CStr::from_ptr(msg.getMessageID()).to_str(), Ok("DatabaseInfo"));
            let attrs = vst3::ComRef::from_raw(msg.getAttributes()).expect("attributes");
            assert_eq!(attrs.setInt(c"count".as_ptr(), 42), sb::kResultOk);
            assert_eq!(attrs.setFloat(c"gain".as_ptr(), 0.5), sb::kResultOk);
            let text: Vec<u16> = "Helix".encode_utf16().chain([0]).collect();
            assert_eq!(attrs.setString(c"name".as_ptr(), text.as_ptr()), sb::kResultOk);
            assert_eq!(attrs.setBinary(c"blob".as_ptr(), [1u8, 2, 3].as_ptr().cast(), 3), sb::kResultOk);

            let (mut i, mut f) = (0i64, 0f64);
            assert_eq!(attrs.getInt(c"count".as_ptr(), &mut i), sb::kResultOk);
            assert_eq!(attrs.getFloat(c"gain".as_ptr(), &mut f), sb::kResultOk);
            assert_eq!((i, f), (42, 0.5));
            assert_eq!(attrs.getInt(c"gain".as_ptr(), &mut i), sb::kResultFalse, "a float is not an int");
            assert_eq!(attrs.getInt(c"missing".as_ptr(), &mut i), sb::kResultFalse);
            // A buffer of 4 units takes 3 characters and the NUL.
            let mut buf = [0xFFFFu16; 4];
            assert_eq!(attrs.getString(c"name".as_ptr(), buf.as_mut_ptr(), 8), sb::kResultOk);
            assert_eq!(buf, ['H' as u16, 'e' as u16, 'l' as u16, 0]);
            let (mut p, mut n): (*const c_void, u32) = (std::ptr::null(), 0);
            assert_eq!(attrs.getBinary(c"blob".as_ptr(), &mut p, &mut n), sb::kResultOk);
            assert_eq!(std::slice::from_raw_parts(p.cast::<u8>(), n as usize), [1, 2, 3]);
            assert_eq!(attrs.setInt(std::ptr::null(), 1), sb::kInvalidArgument, "a null id is refused");
        }
        assert!(create::<sv::IAttributeList>(&host, sv::IAttributeList::IID).is_some());
        // Anything else is still not provided.
        assert!(create::<sv::IMessage>(&host, sv::IComponentHandler::IID).is_none());
    }

    #[test]
    fn the_transport_fills_the_process_context() {
        use sv::ProcessContext_::StatesAndFlags_::*;
        // SAFETY: `ProcessContext` is plain data (numbers and small structs); all-zero is valid.
        let mut c: sv::ProcessContext = unsafe { std::mem::zeroed() };
        let t = Transport {
            playing: true,
            tempo: 90.0,
            numerator: 3,
            denominator: 4,
            samples: 96_000,
            quarters: 3.5,
            bar_start: 3.0,
            ..Transport::default()
        };
        apply_transport(&mut c, &t);
        let playing = kPlaying as u32;
        assert_eq!(c.state & playing, playing);
        assert_eq!(c.state & kRecording as u32, 0);
        assert_eq!((c.tempo, c.timeSigNumerator, c.timeSigDenominator), (90.0, 3, 4));
        assert_eq!((c.projectTimeSamples, c.projectTimeMusic, c.barPositionMusic), (96_000, 3.5, 3.0));
        apply_transport(&mut c, &Transport { playing: false, ..t });
        assert_eq!(c.state & playing, 0);
    }

    #[test]
    fn state_container_parses_strictly() {
        let mut blob = b"SCV3".to_vec();
        blob.extend_from_slice(&3u32.to_le_bytes());
        blob.extend_from_slice(b"abc");
        blob.extend_from_slice(&2u32.to_le_bytes());
        blob.extend_from_slice(b"xy");
        assert_eq!(split_state(&blob), Some((&b"abc"[..], &b"xy"[..])));
        assert_eq!(split_state(b"raw component"), Some((&b"raw component"[..], &[][..])), "bare component state");
        assert_eq!(split_state(b""), None);
        let mut trailing = blob.clone();
        trailing.push(0);
        assert_eq!(split_state(&trailing), None);
        assert_eq!(split_state(&blob[..blob.len() - 1]), None);
        let mut huge = b"SCV3".to_vec();
        huge.extend_from_slice(&u32::MAX.to_le_bytes());
        assert_eq!(split_state(&huge), None);
        assert_eq!(split_state(b"SCV3"), None);
    }

    #[test]
    fn strings_are_bounded() {
        let mut b = [0u16; 4];
        write_c16("SoundCraft", &mut b);
        assert_eq!(read_c16(&b), "Sou");
        let full: Vec<u16> = "abcd".encode_utf16().collect();
        assert_eq!(read_c16(&full), "abcd");
        let c: Vec<c_char> = b"Fx|EQ\0junk".iter().map(|&x| x as c_char).collect();
        assert_eq!(read_c8(&c), "Fx|EQ");
        write_c16("x", &mut []);
    }

    #[test]
    fn host_objects_behave() {
        let pc = ParamChanges::new(2);
        pc.set(5, 0, 0.25);
        pc.set(5, 0, 0.5);
        pc.set(6, 0, 1.0);
        pc.set(7, 0, 1.0);
        // SAFETY: test calls through our own implementation.
        unsafe {
            assert_eq!(pc.getParameterCount(), 2, "full pool drops extra params");
            assert!(pc.getParameterData(2).is_null());
            assert!(pc.getParameterData(-1).is_null());
            let q = pc.queues[0].as_com_ref::<sv::IParamValueQueue>().unwrap();
            let (mut o, mut v) = (9, 0.0);
            assert_eq!(q.getPoint(0, &mut o, &mut v), sb::kResultOk);
            assert_eq!((o, v), (0, 0.5));
            assert_ne!(q.getPoint(1, &mut o, &mut v), sb::kResultOk);
            assert_ne!(q.getPoint(0, std::ptr::null_mut(), &mut v), sb::kResultOk);
        }
        let el = EventList::new(1);
        let mut e = NoteEvent::On { time: 5, channel: 2, key: 60, velocity: 1.0 }.to_raw(2);
        // SAFETY: as above.
        unsafe {
            assert_eq!(el.addEvent(&mut e), sb::kResultOk);
            assert_ne!(el.addEvent(&mut e), sb::kResultOk, "bounded");
            assert_eq!(el.getEventCount(), 1);
            let mut out: sv::Event = std::mem::zeroed();
            assert_eq!(el.getEvent(0, &mut out), sb::kResultOk);
            assert_eq!(out.sampleOffset, 3);
            assert_eq!(out.__field0.noteOn.channel, 2);
            assert_eq!(out.__field0.noteOn.pitch, 60);
            assert_ne!(el.getEvent(1, &mut out), sb::kResultOk);
        }
        let s = MemoryStream { inner: Mutex::new((Vec::new(), 0)) };
        let mut buf = *b"hello";
        // SAFETY: as above; buffers are valid for the given sizes.
        unsafe {
            let mut n = 0;
            assert_eq!(s.write(buf.as_mut_ptr().cast(), 5, &mut n), sb::kResultOk);
            assert_eq!(n, 5);
            let mut pos = 0i64;
            assert_eq!(s.seek(1, sb::IBStream_::IStreamSeekMode_::kIBSeekSet as i32, &mut pos), sb::kResultOk);
            let mut out = [0u8; 8];
            assert_eq!(s.read(out.as_mut_ptr().cast(), 8, &mut n), sb::kResultOk);
            assert_eq!((n, &out[..4]), (4, &b"ello"[..]));
            assert_eq!(s.seek(-100, sb::IBStream_::IStreamSeekMode_::kIBSeekEnd as i32, &mut pos), sb::kResultOk);
            assert_eq!(pos, 0);
            assert_eq!(s.write(std::ptr::null_mut(), 5, &mut n), sb::kResultFalse);
        }
        let h = HostContext::default();
        let mut name: sv::String128 = [0; 128];
        // SAFETY: as above.
        unsafe {
            assert_eq!(h.getName(&mut name), sb::kResultOk);
            assert_eq!(h.restartComponent(sv::RestartFlags_::kLatencyChanged as i32), sb::kResultOk);
        }
        assert_eq!(read_c16(&name), "SoundCraft");
        assert!(h.latency_changed.load(Ordering::Relaxed));
        assert_eq!(hex(&[0xAB; 16]), "AB".repeat(16));
    }

    /// An `Event` with one of the union fields filled in, the way a plugin writes it.
    fn event(kind: u16, fill: impl FnOnce(&mut sv::Event__type0)) -> sv::Event {
        let mut e: sv::Event = unsafe { std::mem::zeroed() };
        e.r#type = kind;
        // The union fields are all `Copy`, so writing one is safe; the test keeps `kind` and the
        // field in step, the way a plugin does.
        fill(&mut e.__field0);
        e
    }

    const NOTE_ON: u16 = sv::Event_::EventTypes_::kNoteOnEvent as u16;
    const NOTE_OFF: u16 = sv::Event_::EventTypes_::kNoteOffEvent as u16;
    const POLY: u16 = sv::Event_::EventTypes_::kPolyPressureEvent as u16;
    const CC_OUT: u16 = sv::Event_::EventTypes_::kLegacyMIDICCOutEvent as u16;

    #[test]
    fn output_events_become_midi_channel_messages() {
        // Note on/off: channel, note number and velocity survive; the offsets travel with the
        // event, not through the message.
        let e = event(NOTE_ON, |f| f.noteOn = sv::NoteOnEvent { channel: 9, pitch: 64, tuning: 0.0, velocity: 1.0, length: 0, noteId: 3 });
        assert_eq!(event_to_midi(&e), Some([0x99, 64, 127]));
        let e = event(NOTE_OFF, |f| f.noteOff = sv::NoteOffEvent { channel: 0, pitch: 127, velocity: 0.5, noteId: 0, tuning: 0.0 });
        assert_eq!(event_to_midi(&e), Some([0x80, 127, 64]));
        let e = NoteEvent::Off { time: 0, channel: 3, key: 60, velocity: 0.5 }.to_raw(0);
        // SAFETY: `to_raw` sets the event kind and matching `noteOff` union member.
        unsafe {
            assert_eq!(e.__field0.noteOff.channel, 3);
            assert_eq!(e.__field0.noteOff.velocity, 0.5);
        }
        // Poly pressure, both as its own event and as the SDK's extra controller number.
        let e = event(POLY, |f| f.polyPressure = sv::PolyPressureEvent { channel: 15, pitch: 60, pressure: 77.0 / 127.0, noteId: 0 });
        assert_eq!(event_to_midi(&e), Some([0xAF, 60, 77]));
        let e = event(CC_OUT, |f| f.midiCCOut = sv::LegacyMIDICCOutEvent { channel: 2, controlNumber: 131, value: 60, value2: 77 });
        assert_eq!(event_to_midi(&e), Some([0xA2, 60, 77]));
        // A plain control change keeps its number and value.
        let e = event(CC_OUT, |f| f.midiCCOut = sv::LegacyMIDICCOutEvent { channel: 3, controlNumber: 7, value: 64, value2: 0 });
        assert_eq!(event_to_midi(&e), Some([0xB3, 7, 64]));
        // Pitch bend is 14-bit: value is the low 7 bits, value2 the high ones.
        let e = event(CC_OUT, |f| f.midiCCOut = sv::LegacyMIDICCOutEvent { channel: 4, controlNumber: 129, value: 52, value2: 42 });
        assert_eq!(event_to_midi(&e), Some([0xE4, 52, 42]));
        // Centre is 64/0 in MIDI 1.0 terms, which is what a plugin sending 0..127 means.
        let e = event(CC_OUT, |f| f.midiCCOut = sv::LegacyMIDICCOutEvent { channel: 4, controlNumber: 129, value: 0, value2: 64 });
        assert_eq!(event_to_midi(&e), Some([0xE4, 0, 64]));
        // Program change and channel pressure.
        let e = event(CC_OUT, |f| f.midiCCOut = sv::LegacyMIDICCOutEvent { channel: 11, controlNumber: 130, value: 42, value2: 0 });
        assert_eq!(event_to_midi(&e), Some([0xCB, 42, 0]));
        let e = event(CC_OUT, |f| f.midiCCOut = sv::LegacyMIDICCOutEvent { channel: 1, controlNumber: 128, value: 33, value2: 0 });
        assert_eq!(event_to_midi(&e), Some([0xD1, 33, 0]));
    }

    #[test]
    fn hostile_or_untranslatable_output_events_are_dropped() {
        // Raw data (a pointer into the plugin's memory) and the note-expression, chord and scale
        // events have no MIDI 1.0 channel equivalent.
        for kind in [sv::Event_::EventTypes_::kDataEvent as u16, 4, 5, 8, 6, 7, 16_384, 60_000] {
            let e = event(kind, |_| {});
            assert_eq!(event_to_midi(&e), None, "event type {kind}");
        }
        // System realtime / common numbers (132 and up) are not channel messages either.
        let e = event(CC_OUT, |f| f.midiCCOut = sv::LegacyMIDICCOutEvent { channel: 0, controlNumber: 132, value: 1, value2: 0 });
        assert_eq!(event_to_midi(&e), None);
        // A channel outside 0..15 can't be a status nibble; a NaN velocity is not a byte.
        let e = event(NOTE_ON, |f| f.noteOn = sv::NoteOnEvent { channel: 16, pitch: 60, tuning: 0.0, velocity: 1.0, length: 0, noteId: 0 });
        assert_eq!(event_to_midi(&e), None);
        let e = event(NOTE_ON, |f| f.noteOn = sv::NoteOnEvent { channel: -3, pitch: 60, tuning: 0.0, velocity: 1.0, length: 0, noteId: 0 });
        assert_eq!(event_to_midi(&e), None);
        let e = event(NOTE_ON, |f| f.noteOn = sv::NoteOnEvent { channel: 0, pitch: 60, tuning: 0.0, velocity: f32::NAN, length: 0, noteId: 0 });
        assert_eq!(event_to_midi(&e), None);
        // Invalid note numbers are dropped rather than wrapped or silently changed.
        let e = event(NOTE_ON, |f| f.noteOn = sv::NoteOnEvent { channel: 0, pitch: -5, tuning: 0.0, velocity: 2.0, length: 0, noteId: 0 });
        assert_eq!(event_to_midi(&e), None);
        let e = event(NOTE_ON, |f| f.noteOn = sv::NoteOnEvent { channel: 0, pitch: 900, tuning: 0.0, velocity: 0.5, length: 0, noteId: 0 });
        assert_eq!(event_to_midi(&e), None);
    }

    /// A bridge that runs the plugin in another process (yabridge through Wine) translates the
    /// plugin's pointer events against the Win32 screen origin, so the editor window has to stay
    /// an unmanaged window at (0, 0): a window manager that decorates or moves it shifts every
    /// click away from the control under the pointer. The window is never mapped here, so this
    /// neither flashes nor takes the focus.
    #[cfg(all(unix, not(target_os = "macos"), not(target_os = "android")))]
    #[test]
    fn the_editor_window_is_override_redirect_at_the_screen_origin() {
        let Ok(win) = window::HostWindow::new("SoundCraft editor test", 320.0, 240.0) else {
            return; // No X display (headless run): there is nothing to observe.
        };
        let Ok(xlib) = x11_dl::xlib::Xlib::open() else {
            win.close();
            return;
        };
        // SAFETY: a null display name opens the display named by $DISPLAY; `attributes` is
        // writable. The window id is the one this thread just created and never mapped.
        unsafe {
            let display = (xlib.XOpenDisplay)(std::ptr::null());
            if !display.is_null() {
                let mut attributes: x11_dl::xlib::XWindowAttributes = std::mem::zeroed();
                let got = (xlib.XGetWindowAttributes)(display, win.raw() as x11_dl::xlib::Window, &mut attributes);
                (xlib.XCloseDisplay)(display);
                assert_ne!(got, 0, "the editor window has attributes");
                assert_eq!(attributes.override_redirect, 1, "the window manager must not move the editor off the origin");
                assert_eq!((attributes.x, attributes.y), (0, 0), "screen and window coordinates coincide only at the origin");
            }
        }
        win.close();
    }
}
