//! `AuPlugin`: a loaded Audio Unit behind the safe `soundcraft_dsp::Plugin` trait.

use crate::ffi::Instance;
use soundcraft_dsp::{Plugin, PluginInfo, Transport};

/// Most MIDI events queued between two blocks (extra events are dropped).
const EVENT_CAP: usize = 1024;
/// Most channels a host strip may ask for.
const MAX_CHANNELS: usize = 64;

/// A queued MIDI channel message at a frame offset within the next `process` call.
#[derive(Debug, Clone, Copy, PartialEq)]
struct MidiEvent {
    time: u32,
    status: u8,
    data1: u8,
    data2: u8,
}

/// A hosted Audio Unit. Parameter ids are global-scope `AudioUnitParameterID`s as decimal
/// strings; values are the unit's own values, with names and ranges in `info().params`.
pub struct AuPlugin {
    inst: Instance,
    info: &'static PluginInfo,
    /// Audio Unit id of each `info.params` entry (same order).
    ids: Vec<u32>,
    values: Vec<f32>,
    /// Pending MIDI, kept sorted by time. Never grows past `EVENT_CAP`.
    events: Vec<MidiEvent>,
    held: [bool; 128],
    /// Takes audio input (effects); instruments replace the buffer contents.
    has_input: bool,
    /// Accepts MIDI (instruments and music effects).
    takes_midi: bool,
    channels: usize,
    latency: usize,
    tail: usize,
}

impl AuPlugin {
    pub(crate) fn new(inst: Instance, info: &'static PluginInfo, has_input: bool, takes_midi: bool) -> AuPlugin {
        let ids: Vec<u32> = info.params.iter().map(|p| p.id.parse::<u32>().unwrap_or(u32::MAX)).collect();
        let values = info.params.iter().zip(&ids).map(|(p, &id)| inst.get_param(id).map_or(p.default, |v| p.clamp(v))).collect();
        AuPlugin {
            inst,
            info,
            ids,
            values,
            events: Vec::with_capacity(EVENT_CAP),
            held: [false; 128],
            has_input,
            takes_midi,
            channels: 2,
            latency: 0,
            tail: 0,
        }
    }

    /// Re-reads every parameter value from the unit (after a state load).
    fn refresh_values(&mut self) {
        for ((slot, p), &id) in self.values.iter_mut().zip(self.info.params).zip(&self.ids) {
            if let Some(v) = self.inst.get_param(id) {
                *slot = p.clamp(v);
            }
        }
    }

    /// Inserts keeping the queue time-sorted; drops the event when the queue is full (never
    /// allocates on the audio thread).
    fn push_event(&mut self, e: MidiEvent) {
        if !self.takes_midi || self.events.len() >= self.events.capacity() {
            return;
        }
        let pos = self.events.iter().rposition(|q| q.time <= e.time).map_or(0, |i| i + 1);
        self.events.insert(pos, e);
    }
}

impl Plugin for AuPlugin {
    fn info(&self) -> &'static PluginInfo {
        self.info
    }

    fn prepare(&mut self, sample_rate: f32, max_block: usize, channels: usize) {
        let sr = if sample_rate.is_finite() && sample_rate > 0.0 { f64::from(sample_rate) } else { 48_000.0 };
        self.channels = channels.clamp(1, MAX_CHANNELS);
        match self.inst.configure(sr, max_block, self.channels) {
            Ok(_) => {
                self.latency = self.inst.latency();
                self.tail = self.inst.tail();
            }
            Err(e) => {
                log::warn!("{}: {e}", self.info.id);
                self.latency = 0;
                self.tail = 0;
            }
        }
        // Re-send every value so a re-initialized unit matches the session.
        for (&id, &v) in self.ids.iter().zip(&self.values) {
            self.inst.set_param(id, v);
        }
    }

    fn reset(&mut self) {
        self.events.clear();
        self.held = [false; 128];
        self.inst.reset();
    }

    fn set_param(&mut self, id: &str, value: f32) -> bool {
        let Some(i) = self.info.params.iter().position(|p| p.id == id) else { return false };
        let (Some(pi), Some(&aid)) = (self.info.params.get(i), self.ids.get(i)) else { return false };
        let v = pi.clamp(value);
        if let Some(slot) = self.values.get_mut(i) {
            *slot = v;
        }
        self.inst.set_param(aid, v);
        true
    }

    fn param(&self, id: &str) -> Option<f32> {
        let i = self.info.params.iter().position(|p| p.id == id)?;
        self.values.get(i).copied()
    }

    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        let ch = self.channels.min(io.len());
        let frames = io.iter().take(ch).map(Vec::len).fold(frames, usize::min);
        if frames == 0 || ch == 0 {
            return;
        }
        if !self.inst.is_ready() {
            // Effects pass through; instruments are silent.
            if !self.has_input {
                for c in io.iter_mut().take(ch) {
                    c.iter_mut().take(frames).for_each(|s| *s = 0.0);
                }
            }
            self.events.clear();
            return;
        }
        let max = self.inst.max_frames();
        let au_ch = self.inst.channels();
        let last = u32::try_from(frames - 1).unwrap_or(u32::MAX);
        for e in &mut self.events {
            e.time = e.time.min(last);
        }
        let mut start = 0usize;
        while start < frames {
            let n = (frames - start).min(max);
            if self.has_input {
                for c in 0..au_ch {
                    let src = io.get(c.min(ch - 1)).and_then(|s| s.get(start..start + n));
                    if let (Some(src), Some(dst)) = (src, self.inst.input_mut(c).and_then(|d| d.get_mut(..n))) {
                        dst.copy_from_slice(src);
                    }
                }
            }
            let end = start + n;
            let k = self.events.iter().position(|e| e.time as usize >= end).unwrap_or(self.events.len());
            for e in self.events.iter().take(k) {
                let offset = u32::try_from((e.time as usize).saturating_sub(start)).unwrap_or(0);
                self.inst.midi(e.status, e.data1, e.data2, offset);
            }
            self.events.drain(..k);
            match self.inst.render(n) {
                Ok(()) => {
                    for (c, dst) in io.iter_mut().take(ch).enumerate() {
                        let Some(dst) = dst.get_mut(start..end) else { continue };
                        if ch == 1 && au_ch >= 2 {
                            if let (Some(l), Some(r)) = (self.inst.output(0, n), self.inst.output(1, n)) {
                                for ((d, a), b) in dst.iter_mut().zip(l).zip(r) {
                                    *d = 0.5 * (a + b);
                                }
                            }
                        } else if let Some(src) = self.inst.output(c.min(au_ch.saturating_sub(1)), n) {
                            for (d, &s) in dst.iter_mut().zip(src) {
                                *d = s;
                            }
                        }
                    }
                }
                Err(e) => {
                    log::debug!("{}: render failed ({e})", self.info.id);
                    if !self.has_input {
                        for c in io.iter_mut().take(ch) {
                            if let Some(d) = c.get_mut(start..end) {
                                d.iter_mut().for_each(|s| *s = 0.0);
                            }
                        }
                    }
                }
            }
            start = end;
        }
    }

    fn latency(&self) -> usize {
        self.latency
    }

    fn set_transport(&mut self, transport: &Transport) {
        self.inst.set_transport(transport);
    }

    fn tail_samples(&self) -> usize {
        self.tail
    }

    fn note_on(&mut self, offset: usize, note: u8, velocity: u8) {
        if note > 127 {
            return;
        }
        let time = u32::try_from(offset).unwrap_or(u32::MAX);
        // Velocity 0 would be a note-off.
        self.push_event(MidiEvent { time, status: 0x90, data1: note, data2: velocity.clamp(1, 127) });
        if let Some(h) = self.held.get_mut(note as usize) {
            *h = true;
        }
    }

    fn note_off(&mut self, offset: usize, note: u8) {
        if note > 127 {
            return;
        }
        let time = u32::try_from(offset).unwrap_or(u32::MAX);
        self.push_event(MidiEvent { time, status: 0x80, data1: note, data2: 0 });
        if let Some(h) = self.held.get_mut(note as usize) {
            *h = false;
        }
    }

    fn midi(&mut self, offset: usize, msg: [u8; 3]) {
        let [status, data1, data2] = msg;
        if status & 0xF0 >= 0xA0 && status & 0xF0 < 0xF0 {
            self.push_event(MidiEvent { time: u32::try_from(offset).unwrap_or(u32::MAX), status, data1: data1 & 0x7F, data2: data2 & 0x7F });
        }
    }

    fn all_notes_off(&mut self) {
        for k in 0..128u8 {
            if self.held.get(k as usize).copied().unwrap_or(false) {
                self.note_off(0, k);
            }
        }
        // CC 123: All Notes Off (catches anything the unit still sustains).
        self.push_event(MidiEvent { time: 0, status: 0xB0, data1: 123, data2: 0 });
    }

    fn load_state(&mut self, data: &[u8]) -> bool {
        let ok = self.inst.load_state(data);
        if ok {
            self.refresh_values();
        }
        ok
    }

    fn state_reader(&mut self) -> Option<Box<dyn soundcraft_dsp::PluginStateReader>> {
        let link = self.inst.state_link();
        Some(Box::new(move || link.save_state()))
    }
}
