use super::{
    AtomicI32, AtomicU32, AtomicU64, Class, ComWrapper, IBStreamTrait, IEventListTrait, IParamValueQueueTrait, IParameterChangesTrait,
    MAX_BUS_CHANNELS, MAX_BUSES, MAX_STATE_BYTES, Mutex, Ordering, PoisonError, c_void, sb, sv,
};

/// One parameter's points for one block. SoundCraft sends at most one point per block; a
/// plugin writing output changes may add more, of which only the last is kept.
#[derive(Default)]
pub(crate) struct ParamQueue {
    id: AtomicU32,
    offset: AtomicI32,
    value: AtomicU64,
    points: AtomicI32,
}

impl Class for ParamQueue {
    type Interfaces = (sv::IParamValueQueue,);
}

impl IParamValueQueueTrait for ParamQueue {
    unsafe fn getParameterId(&self) -> sv::ParamID {
        self.id.load(Ordering::Relaxed)
    }
    unsafe fn getPointCount(&self) -> sb::int32 {
        self.points.load(Ordering::Relaxed)
    }
    unsafe fn getPoint(&self, index: sb::int32, offset: *mut sb::int32, value: *mut sv::ParamValue) -> sb::tresult {
        if index != 0 || self.points.load(Ordering::Relaxed) < 1 {
            return sb::kInvalidArgument;
        }
        // SAFETY: out-pointers from the plugin (or null, handled).
        if let (Some(o), Some(v)) = (unsafe { offset.as_mut() }, unsafe { value.as_mut() }) {
            *o = self.offset.load(Ordering::Relaxed);
            *v = f64::from_bits(self.value.load(Ordering::Relaxed));
            sb::kResultOk
        } else {
            sb::kInvalidArgument
        }
    }
    unsafe fn addPoint(&self, offset: sb::int32, value: sv::ParamValue, index: *mut sb::int32) -> sb::tresult {
        self.offset.store(offset, Ordering::Relaxed);
        self.value.store(value.to_bits(), Ordering::Relaxed);
        self.points.store(1, Ordering::Relaxed);
        // SAFETY: out-pointer from the plugin (or null, handled).
        if let Some(i) = unsafe { index.as_mut() } {
            *i = 0;
        }
        sb::kResultOk
    }
}

/// A fixed pool of parameter queues (`IParameterChanges`), reused every block.
pub(crate) struct ParamChanges {
    pub(super) queues: Vec<ComWrapper<ParamQueue>>,
    count: AtomicI32,
}

impl Class for ParamChanges {
    type Interfaces = (sv::IParameterChanges,);
}

impl ParamChanges {
    pub(super) fn new(cap: usize) -> ParamChanges {
        ParamChanges { queues: (0..cap).map(|_| ComWrapper::new(ParamQueue::default())).collect(), count: AtomicI32::new(0) }
    }

    pub(super) fn clear(&self) {
        self.count.store(0, Ordering::Relaxed);
    }

    /// Finds or adds the queue for `id`; `None` when the pool is full.
    fn queue_for(&self, id: u32) -> Option<(i32, &ParamQueue)> {
        let n = self.count.load(Ordering::Relaxed).max(0);
        for (i, q) in self.queues.iter().enumerate().take(n as usize) {
            if q.id.load(Ordering::Relaxed) == id {
                return Some((i32::try_from(i).ok()?, q));
            }
        }
        let q = self.queues.get(n as usize)?;
        q.id.store(id, Ordering::Relaxed);
        q.points.store(0, Ordering::Relaxed);
        self.count.store(n + 1, Ordering::Relaxed);
        Some((n, q))
    }

    /// Host side: one point for `id` at `offset` (replacing an earlier one this block).
    pub(super) fn set(&self, id: u32, offset: i32, value: f64) {
        if let Some((_, q)) = self.queue_for(id) {
            q.offset.store(offset, Ordering::Relaxed);
            q.value.store(value.to_bits(), Ordering::Relaxed);
            q.points.store(1, Ordering::Relaxed);
        }
    }

    fn queue_ptr(&self, index: usize) -> *mut sv::IParamValueQueue {
        self.queues.get(index).and_then(|q| q.as_com_ref::<sv::IParamValueQueue>()).map_or(std::ptr::null_mut(), |r| r.as_ptr())
    }
}

impl IParameterChangesTrait for ParamChanges {
    unsafe fn getParameterCount(&self) -> sb::int32 {
        self.count.load(Ordering::Relaxed)
    }
    unsafe fn getParameterData(&self, index: sb::int32) -> *mut sv::IParamValueQueue {
        if index < 0 || index >= self.count.load(Ordering::Relaxed) {
            return std::ptr::null_mut();
        }
        self.queue_ptr(index as usize)
    }
    unsafe fn addParameterData(&self, id: *const sv::ParamID, index: *mut sb::int32) -> *mut sv::IParamValueQueue {
        // SAFETY: in-pointer from the plugin (or null, handled).
        let Some(&id) = (unsafe { id.as_ref() }) else { return std::ptr::null_mut() };
        let Some((i, _)) = self.queue_for(id) else { return std::ptr::null_mut() };
        // SAFETY: out-pointer from the plugin (or null, handled).
        if let Some(out) = unsafe { index.as_mut() } {
            *out = i;
        }
        self.queue_ptr(i as usize)
    }
}

/// A bounded event list (`IEventList`). Only ever touched by one thread at a time (the host
/// fills it, then the plugin reads it inside `process`), so the lock is never contended; the
/// plugin side uses `try_lock` and never blocks.
pub(crate) struct EventList {
    events: Mutex<Vec<sv::Event>>,
    cap: usize,
}

impl Class for EventList {
    type Interfaces = (sv::IEventList,);
}

impl EventList {
    pub(super) fn new(cap: usize) -> EventList {
        EventList { events: Mutex::new(Vec::with_capacity(cap)), cap }
    }
    pub(super) fn with<R>(&self, f: impl FnOnce(&mut Vec<sv::Event>) -> R) -> Option<R> {
        match self.events.try_lock() {
            Ok(mut g) => Some(f(&mut g)),
            Err(std::sync::TryLockError::Poisoned(p)) => Some(f(&mut p.into_inner())),
            Err(std::sync::TryLockError::WouldBlock) => None,
        }
    }
}

impl IEventListTrait for EventList {
    unsafe fn getEventCount(&self) -> sb::int32 {
        self.with(|v| i32::try_from(v.len()).unwrap_or(i32::MAX)).unwrap_or(0)
    }
    unsafe fn getEvent(&self, index: sb::int32, e: *mut sv::Event) -> sb::tresult {
        // SAFETY: out-pointer from the plugin (or null, handled).
        let Some(out) = (unsafe { e.as_mut() }) else { return sb::kInvalidArgument };
        let Ok(i) = usize::try_from(index) else { return sb::kInvalidArgument };
        match self.with(|v| v.get(i).copied()).flatten() {
            Some(ev) => {
                *out = ev;
                sb::kResultOk
            }
            None => sb::kInvalidArgument,
        }
    }
    unsafe fn addEvent(&self, e: *mut sv::Event) -> sb::tresult {
        // SAFETY: in-pointer from the plugin (or null, handled).
        let Some(&ev) = (unsafe { e.as_ref() }) else { return sb::kInvalidArgument };
        let cap = self.cap;
        // Output events (e.g. MIDI out) are accepted while there is room; SoundCraft ignores them.
        match self.with(|v| {
            if v.len() < cap {
                v.push(ev);
                true
            } else {
                false
            }
        }) {
            Some(true) => sb::kResultOk,
            _ => sb::kResultFalse,
        }
    }
}

/// An in-memory `IBStream`, used to hand the component's state to its controller.
pub(crate) struct MemoryStream {
    pub(super) inner: Mutex<(Vec<u8>, usize)>,
}

impl Class for MemoryStream {
    type Interfaces = (sb::IBStream,);
}

impl MemoryStream {
    pub(super) fn new(data: Vec<u8>) -> ComWrapper<MemoryStream> {
        ComWrapper::new(MemoryStream { inner: Mutex::new((data, 0)) })
    }

    pub(super) fn take(&self) -> Vec<u8> {
        std::mem::take(&mut self.lock().0)
    }

    pub(super) fn lock(&self) -> std::sync::MutexGuard<'_, (Vec<u8>, usize)> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl IBStreamTrait for MemoryStream {
    unsafe fn read(&self, buffer: *mut c_void, num_bytes: sb::int32, num_read: *mut sb::int32) -> sb::tresult {
        let mut g = self.lock();
        let (data, pos) = &mut *g;
        let want = usize::try_from(num_bytes).unwrap_or(0);
        let avail = data.len().saturating_sub(*pos);
        let n = want.min(avail);
        if n > 0 && !buffer.is_null() {
            let Some(src) = data.get(*pos..*pos + n) else { return sb::kInternalError };
            // SAFETY: the plugin provides `buffer` with room for `num_bytes` ≥ `n` bytes; `src`
            // is `n` valid bytes; the regions cannot overlap (ours is a private Vec).
            unsafe { std::ptr::copy_nonoverlapping(src.as_ptr(), buffer.cast::<u8>(), n) };
            *pos += n;
        }
        // SAFETY: out-pointer from the plugin (or null, handled).
        if let Some(r) = unsafe { num_read.as_mut() } {
            *r = i32::try_from(n).unwrap_or(0);
        }
        sb::kResultOk
    }
    unsafe fn write(&self, buffer: *mut c_void, num_bytes: sb::int32, num_written: *mut sb::int32) -> sb::tresult {
        let mut g = self.lock();
        let (data, pos) = &mut *g;
        let n = usize::try_from(num_bytes).unwrap_or(0);
        let end = pos.saturating_add(n);
        if buffer.is_null() || end > MAX_STATE_BYTES {
            return sb::kResultFalse;
        }
        if data.len() < end {
            data.resize(end, 0);
        }
        // SAFETY: the plugin provides `n` readable bytes at `buffer`; the destination range was
        // just made valid.
        let src = unsafe { std::slice::from_raw_parts(buffer.cast::<u8>(), n) };
        if let Some(dst) = data.get_mut(*pos..end) {
            dst.copy_from_slice(src);
        }
        *pos = end;
        // SAFETY: out-pointer from the plugin (or null, handled).
        if let Some(w) = unsafe { num_written.as_mut() } {
            *w = num_bytes;
        }
        sb::kResultOk
    }
    unsafe fn seek(&self, pos: sb::int64, mode: sb::int32, result: *mut sb::int64) -> sb::tresult {
        let mut g = self.lock();
        let len = i64::try_from(g.0.len()).unwrap_or(i64::MAX);
        let cur = i64::try_from(g.1).unwrap_or(0);
        let base = match mode as u32 {
            x if x == sb::IBStream_::IStreamSeekMode_::kIBSeekSet as u32 => 0,
            x if x == sb::IBStream_::IStreamSeekMode_::kIBSeekCur as u32 => cur,
            x if x == sb::IBStream_::IStreamSeekMode_::kIBSeekEnd as u32 => len,
            _ => return sb::kInvalidArgument,
        };
        let new = base.saturating_add(pos).clamp(0, len);
        g.1 = usize::try_from(new).unwrap_or(0);
        // SAFETY: out-pointer from the plugin (or null, handled).
        if let Some(r) = unsafe { result.as_mut() } {
            *r = new;
        }
        sb::kResultOk
    }
    unsafe fn tell(&self, pos: *mut sb::int64) -> sb::tresult {
        let g = self.lock();
        // SAFETY: out-pointer from the plugin (or null, handled).
        match unsafe { pos.as_mut() } {
            Some(p) => {
                *p = i64::try_from(g.1).unwrap_or(0);
                sb::kResultOk
            }
            None => sb::kInvalidArgument,
        }
    }
}

// ---- events and buffers --------------------------------------------------------------------

/// A host → plugin note event, safe to build anywhere; converted inside `process`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum NoteEvent {
    On { time: u32, key: u8, velocity: f32 },
    Off { time: u32, key: u8 },
    Pressure { time: u32, key: u8, pressure: f32 },
}

impl NoteEvent {
    pub fn time(&self) -> u32 {
        match *self {
            NoteEvent::On { time, .. } | NoteEvent::Off { time, .. } | NoteEvent::Pressure { time, .. } => time,
        }
    }
    pub fn set_time(&mut self, t: u32) {
        match self {
            NoteEvent::On { time, .. } | NoteEvent::Off { time, .. } | NoteEvent::Pressure { time, .. } => *time = t,
        }
    }

    pub(super) fn to_raw(self, offset: u32) -> sv::Event {
        // SAFETY: `Event` is plain data (numbers and a union of plain structs); all-zero is valid.
        let mut e: sv::Event = unsafe { std::mem::zeroed() };
        e.busIndex = 0;
        e.sampleOffset = i32::try_from(self.time().saturating_sub(offset)).unwrap_or(0);
        e.flags = sv::Event_::EventFlags_::kIsLive as u16;
        match self {
            NoteEvent::On { key, velocity, .. } => {
                e.r#type = sv::Event_::EventTypes_::kNoteOnEvent as u16;
                e.__field0.noteOn = sv::NoteOnEvent { channel: 0, pitch: i16::from(key), tuning: 0.0, velocity, length: 0, noteId: -1 };
            }
            NoteEvent::Off { key, .. } => {
                e.r#type = sv::Event_::EventTypes_::kNoteOffEvent as u16;
                e.__field0.noteOff = sv::NoteOffEvent { channel: 0, pitch: i16::from(key), velocity: 0.0, noteId: -1, tuning: 0.0 };
            }
            NoteEvent::Pressure { key, pressure, .. } => {
                e.r#type = sv::Event_::EventTypes_::kPolyPressureEvent as u16;
                e.__field0.polyPressure = sv::PolyPressureEvent { channel: 0, pitch: i16::from(key), pressure, noteId: -1 };
            }
        }
        e
    }
}

/// Planar f32 buffers for every audio bus in one direction, with the pointer tables VST3 needs.
/// Allocated in `new`; `process` only rewrites pointers in place (no allocation).
pub(crate) struct BusBuffers {
    data: Vec<Vec<Vec<f32>>>,
    ptrs: Vec<Vec<*mut f32>>,
    pub(super) bufs: Vec<sv::AudioBusBuffers>,
    pub(super) frames: usize,
}

// SAFETY: the raw pointers only ever point into `data`, which this struct owns; moving the struct
// to another thread moves that ownership with it.
unsafe impl Send for BusBuffers {}

impl BusBuffers {
    /// One buffer set per bus, `channels[b]` channels each (capped), `frames` long.
    pub fn new(channels: &[u32], frames: usize) -> BusBuffers {
        let data: Vec<Vec<Vec<f32>>> =
            channels.iter().take(MAX_BUSES as usize).map(|&c| (0..c.min(MAX_BUS_CHANNELS)).map(|_| vec![0.0; frames]).collect()).collect();
        let ptrs = data.iter().map(|p| vec![std::ptr::null_mut(); p.len()]).collect();
        let bufs = data
            .iter()
            .map(|_| sv::AudioBusBuffers {
                numChannels: 0,
                silenceFlags: 0,
                __field0: sv::AudioBusBuffers__type0 { channelBuffers32: std::ptr::null_mut() },
            })
            .collect();
        BusBuffers { data, ptrs, bufs, frames }
    }

    pub fn channels(&self, bus: usize) -> usize {
        self.data.get(bus).map_or(0, Vec::len)
    }

    pub fn channel(&self, bus: usize, ch: usize) -> Option<&[f32]> {
        self.data.get(bus).and_then(|p| p.get(ch)).map(Vec::as_slice)
    }

    pub fn channel_mut(&mut self, bus: usize, ch: usize) -> Option<&mut [f32]> {
        self.data.get_mut(bus).and_then(|p| p.get_mut(ch)).map(Vec::as_mut_slice)
    }

    /// Points the VST3 tables at the current data (no allocation).
    pub(super) fn refresh(&mut self) {
        for ((d, p), b) in self.data.iter_mut().zip(self.ptrs.iter_mut()).zip(self.bufs.iter_mut()) {
            for (dc, pc) in d.iter_mut().zip(p.iter_mut()) {
                *pc = dc.as_mut_ptr();
            }
            b.numChannels = i32::try_from(p.len()).unwrap_or(0);
            b.silenceFlags = 0;
            b.__field0.channelBuffers32 = p.as_mut_ptr();
        }
    }
}
