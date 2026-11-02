use super::{
    CLAP_BEATTIME_FACTOR, CLAP_CORE_EVENT_SPACE_ID, CLAP_EVENT_MIDI, CLAP_EVENT_NOTE_OFF, CLAP_EVENT_NOTE_ON, CLAP_EVENT_PARAM_VALUE,
    CLAP_EVENT_TRANSPORT, CLAP_SECTIME_FACTOR, CLAP_TRANSPORT_HAS_BEATS_TIMELINE, CLAP_TRANSPORT_HAS_SECONDS_TIMELINE, CLAP_TRANSPORT_HAS_TEMPO,
    CLAP_TRANSPORT_HAS_TIME_SIGNATURE, CLAP_TRANSPORT_IS_PLAYING, CLAP_TRANSPORT_IS_RECORDING, MAX_STATE_BYTES, Transport, c_void, clap_event_header,
    clap_event_midi, clap_event_note, clap_event_param_value, clap_event_transport, clap_input_events, clap_istream, clap_ostream,
    clap_output_events,
};

// ---- events --------------------------------------------------------------------------------

/// A host → plugin event, safe to build anywhere; converted to CLAP structs inside `process`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Event {
    Param {
        time: u32,
        id: u32,
        value: f64,
    },
    NoteOn {
        time: u32,
        key: u8,
        velocity: f64,
    },
    NoteOff {
        time: u32,
        key: u8,
    },
    /// A raw MIDI 1.0 message (for plugins whose note port speaks only MIDI).
    Midi {
        time: u32,
        data: [u8; 3],
    },
}

impl Event {
    pub fn time(&self) -> u32 {
        match *self {
            Event::Param { time, .. } | Event::NoteOn { time, .. } | Event::NoteOff { time, .. } | Event::Midi { time, .. } => time,
        }
    }
    pub fn set_time(&mut self, t: u32) {
        match self {
            Event::Param { time, .. } | Event::NoteOn { time, .. } | Event::NoteOff { time, .. } | Event::Midi { time, .. } => *time = t,
        }
    }
}

#[derive(Clone, Copy)]
pub(super) enum RawEvent {
    Note(clap_event_note),
    Param(clap_event_param_value),
    Midi(clap_event_midi),
}

impl RawEvent {
    fn header(&self) -> *const clap_event_header {
        // The header is the first field of every `repr(C)` CLAP event struct, so a pointer to it is
        // a pointer to the whole event, as CLAP expects.
        match self {
            RawEvent::Note(e) => &e.header,
            RawEvent::Param(e) => &e.header,
            RawEvent::Midi(e) => &e.header,
        }
    }

    pub(super) fn from_event(e: &Event, offset: u32) -> RawEvent {
        let header = |size: usize, type_: u16, time: u32| clap_event_header {
            size: u32::try_from(size).unwrap_or(u32::MAX),
            time: time.saturating_sub(offset),
            space_id: CLAP_CORE_EVENT_SPACE_ID,
            type_,
            flags: 0,
        };
        match *e {
            Event::Param { time, id, value } => RawEvent::Param(clap_event_param_value {
                header: header(size_of::<clap_event_param_value>(), CLAP_EVENT_PARAM_VALUE, time),
                param_id: id,
                cookie: std::ptr::null_mut(),
                note_id: -1,
                port_index: -1,
                channel: -1,
                key: -1,
                value,
            }),
            Event::NoteOn { time, key, velocity } => RawEvent::Note(clap_event_note {
                header: header(size_of::<clap_event_note>(), CLAP_EVENT_NOTE_ON, time),
                note_id: -1,
                port_index: 0,
                channel: 0,
                key: i16::from(key),
                velocity,
            }),
            Event::NoteOff { time, key } => RawEvent::Note(clap_event_note {
                header: header(size_of::<clap_event_note>(), CLAP_EVENT_NOTE_OFF, time),
                note_id: -1,
                port_index: 0,
                channel: 0,
                key: i16::from(key),
                velocity: 0.0,
            }),
            Event::Midi { time, data } => {
                RawEvent::Midi(clap_event_midi { header: header(size_of::<clap_event_midi>(), CLAP_EVENT_MIDI, time), port_index: 0, data })
            }
        }
    }
}

pub(super) fn transport_event(t: &Transport) -> clap_event_transport {
    let fixed = |x: f64, factor: i64| if x.is_finite() { (x * factor as f64).round() as i64 } else { 0 };
    let mut flags =
        CLAP_TRANSPORT_HAS_TEMPO | CLAP_TRANSPORT_HAS_BEATS_TIMELINE | CLAP_TRANSPORT_HAS_SECONDS_TIMELINE | CLAP_TRANSPORT_HAS_TIME_SIGNATURE;
    if t.playing {
        flags |= CLAP_TRANSPORT_IS_PLAYING;
    }
    if t.recording {
        flags |= CLAP_TRANSPORT_IS_RECORDING;
    }
    clap_event_transport {
        header: clap_event_header {
            size: u32::try_from(size_of::<clap_event_transport>()).unwrap_or(u32::MAX),
            time: 0,
            space_id: CLAP_CORE_EVENT_SPACE_ID,
            type_: CLAP_EVENT_TRANSPORT,
            flags: 0,
        },
        flags,
        song_pos_beats: fixed(t.quarters, CLAP_BEATTIME_FACTOR),
        song_pos_seconds: fixed(t.seconds, CLAP_SECTIME_FACTOR),
        tempo: t.tempo,
        tempo_inc: 0.0,
        loop_start_beats: 0,
        loop_end_beats: 0,
        loop_start_seconds: 0,
        loop_end_seconds: 0,
        bar_start: fixed(t.bar_start, CLAP_BEATTIME_FACTOR),
        bar_number: i32::try_from(t.bar).unwrap_or(0),
        tsig_num: u16::try_from(t.numerator).unwrap_or(4),
        tsig_denom: u16::try_from(t.denominator).unwrap_or(4),
    }
}

pub(super) unsafe extern "C" fn in_events_size(list: *const clap_input_events) -> u32 {
    if list.is_null() {
        return 0;
    }
    // SAFETY: `list` is the `clap_input_events` we built in `process`; its ctx is the event Vec.
    let v = unsafe { ((*list).ctx as *const Vec<RawEvent>).as_ref() };
    v.map_or(0, |v| u32::try_from(v.len()).unwrap_or(u32::MAX))
}

pub(super) unsafe extern "C" fn in_events_get(list: *const clap_input_events, index: u32) -> *const clap_event_header {
    if list.is_null() {
        return std::ptr::null();
    }
    // SAFETY: as in `in_events_size`; the Vec is not mutated while the plugin processes.
    let v = unsafe { ((*list).ctx as *const Vec<RawEvent>).as_ref() };
    v.and_then(|v| v.get(index as usize)).map_or(std::ptr::null(), RawEvent::header)
}

pub(super) unsafe extern "C" fn out_events_try_push(list: *const clap_output_events, event: *const clap_event_header) -> bool {
    // Plugin → host events are accepted. Parameter values (typically edits in the plugin's GUI)
    // are kept, while there is room, so the UI can write them into the session; the rest is
    // ignored.
    if list.is_null() || event.is_null() {
        return true;
    }
    // SAFETY: CLAP passes a valid event header for the duration of the call.
    let h = unsafe { &*event };
    if h.space_id != CLAP_CORE_EVENT_SPACE_ID || h.type_ != CLAP_EVENT_PARAM_VALUE || (h.size as usize) < size_of::<clap_event_param_value>() {
        return true;
    }
    // SAFETY: the header says this is a complete `clap_event_param_value` (type and size checked).
    let e = unsafe { &*event.cast::<clap_event_param_value>() };
    // SAFETY: `ctx` is null or the instance's preallocated `out_params` Vec, which `process`
    // borrows exclusively for the duration of the plugin call.
    if let Some(v) = unsafe { ((*list).ctx as *mut Vec<(u32, f64)>).as_mut() }
        && v.len() < v.capacity()
        && e.value.is_finite()
    {
        v.push((e.param_id, e.value));
    }
    true
}

// ---- state streams -------------------------------------------------------------------------

/// Read side of an in-memory state blob.
pub(super) struct ReadCursor<'a> {
    pub(super) data: &'a [u8],
    pub(super) pos: usize,
}

pub(super) unsafe extern "C" fn istream_read(stream: *const clap_istream, buffer: *mut c_void, size: u64) -> i64 {
    if stream.is_null() || buffer.is_null() {
        return -1;
    }
    // SAFETY: `ctx` is the `ReadCursor` on `load_state`'s stack frame, alive during the call.
    let Some(c) = (unsafe { ((*stream).ctx as *mut ReadCursor<'_>).as_mut() }) else { return -1 };
    let rest = c.data.get(c.pos..).unwrap_or(&[]);
    let n = rest.len().min(usize::try_from(size).unwrap_or(usize::MAX));
    if let Some(src) = rest.get(..n) {
        // SAFETY: the plugin provides `size` ≥ `n` writable bytes at `buffer`; `src` is `n`
        // readable bytes of our own slice; they cannot overlap.
        unsafe { std::ptr::copy_nonoverlapping(src.as_ptr(), buffer.cast::<u8>(), n) };
    }
    c.pos += n;
    i64::try_from(n).unwrap_or(-1)
}

pub(super) unsafe extern "C" fn ostream_write(stream: *const clap_ostream, buffer: *const c_void, size: u64) -> i64 {
    if stream.is_null() || buffer.is_null() {
        return -1;
    }
    // SAFETY: `ctx` is the output Vec on `save_state`'s stack frame, alive during the call.
    let Some(v) = (unsafe { ((*stream).ctx as *mut Vec<u8>).as_mut() }) else { return -1 };
    let Ok(n) = usize::try_from(size) else { return -1 };
    if v.len().saturating_add(n) > MAX_STATE_BYTES {
        return -1;
    }
    // SAFETY: the plugin provides `size` readable bytes at `buffer` for the call.
    let src = unsafe { std::slice::from_raw_parts(buffer.cast::<u8>(), n) };
    v.extend_from_slice(src);
    i64::try_from(n).unwrap_or(-1)
}
