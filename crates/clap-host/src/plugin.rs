//! `ClapPlugin`: a loaded CLAP instance behind the safe `soundcraft_dsp::Plugin` trait.

use crate::ffi::{EVENT_CAP, Event, GuiLink, Instance, NotePort, PortBuffers};
use clap_sys::ext::note_ports::{CLAP_NOTE_DIALECT_CLAP, CLAP_NOTE_DIALECT_MIDI};
use soundcraft_dsp::{Plugin, PluginEditor, PluginInfo, Transport};
use std::sync::Arc;
use std::sync::atomic::Ordering;

/// Largest block we activate a plugin for.
const MAX_BLOCK: usize = 1 << 16;
/// Most channels a host strip may ask for.
const MAX_CHANNELS: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq)]
enum NoteMode {
    None,
    Clap,
    Midi,
}

/// A hosted CLAP plugin. Parameter ids are the CLAP param ids as decimal strings ("0", "17", …);
/// their human names are in `info().params`.
pub struct ClapPlugin {
    inst: Instance,
    info: &'static PluginInfo,
    /// CLAP id of each `info.params` entry (same order).
    ids: Vec<u32>,
    values: Vec<f32>,
    /// Pending events, kept sorted by time (stable for equal times). Never grows past `EVENT_CAP`.
    queue: Vec<Event>,
    inputs: PortBuffers,
    outputs: PortBuffers,
    in_main: Option<usize>,
    out_main: Option<usize>,
    notes: NoteMode,
    /// The note port takes raw MIDI (controllers, pitch bend, program changes).
    midi_in: bool,
    held: [bool; 128],
    channels: usize,
    max_block: usize,
    latency: usize,
    steady: i64,
    transport: Option<Transport>,
}

impl ClapPlugin {
    pub(crate) fn new(inst: Instance, info: &'static PluginInfo) -> ClapPlugin {
        let ids: Vec<u32> = info.params.iter().map(|p| p.id.parse::<u32>().unwrap_or(u32::MAX)).collect();
        let values = info.params.iter().zip(&ids).map(|(p, id)| inst.param_value(*id).map_or(p.default, |v| p.clamp(v as f32))).collect();
        ClapPlugin {
            inst,
            info,
            ids,
            values,
            queue: Vec::with_capacity(EVENT_CAP),
            inputs: PortBuffers::new(&[], 0),
            outputs: PortBuffers::new(&[], 0),
            in_main: None,
            out_main: None,
            notes: NoteMode::None,
            midi_in: false,
            held: [false; 128],
            channels: 2,
            max_block: 0,
            latency: 0,
            steady: 0,
            transport: None,
        }
    }

    /// Re-reads every parameter value from the plugin (after a state load).
    fn refresh_values(&mut self) {
        for ((v, p), id) in self.values.iter_mut().zip(self.info.params).zip(&self.ids) {
            if let Some(x) = self.inst.param_value(*id) {
                *v = p.clamp(x as f32);
            }
        }
        // Pending changes from before the load would undo it.
        self.queue.retain(|e| !matches!(e, Event::Param { .. }));
    }

    /// Inserts keeping the queue time-sorted; drops the event when the queue is full (never
    /// allocates on the audio thread).
    fn push(&mut self, e: Event) {
        if self.queue.len() >= self.queue.capacity() {
            return;
        }
        let pos = self.queue.iter().rposition(|q| q.time() <= e.time()).map_or(0, |i| i + 1);
        self.queue.insert(pos, e);
    }
}

fn pick_main(ports: &[crate::ffi::PortInfo]) -> Option<usize> {
    ports.iter().position(|p| p.main && p.channels > 0).or_else(|| ports.iter().position(|p| p.channels > 0))
}

fn note_mode(p: Option<NotePort>) -> NoteMode {
    match p {
        Some(p) if p.supported & CLAP_NOTE_DIALECT_CLAP != 0 => NoteMode::Clap,
        Some(p) if p.supported & CLAP_NOTE_DIALECT_MIDI != 0 => NoteMode::Midi,
        _ => NoteMode::None,
    }
}

/// Copy `n` frames from `start` of the host buffers into the plugin's input `port` (mono sources
/// fill every port channel).
fn feed_input(inputs: &mut PortBuffers, port: usize, io: &[Vec<f32>], ch: usize, (start, n): (usize, usize)) {
    for c in 0..inputs.channels(port) {
        let src = io.get(c.min(ch - 1)).and_then(|s| s.get(start..start + n));
        if let (Some(src), Some(dst)) = (src, inputs.channel_mut(port, c).and_then(|d| d.get_mut(..n))) {
            dst.copy_from_slice(src);
        }
    }
}

/// Copy `n` frames of the plugin's output `port` into the host buffers from `start`; a mono host
/// buffer takes the average of a stereo port.
fn read_output(outputs: &PortBuffers, port: usize, io: &mut [Vec<f32>], ch: usize, (start, n): (usize, usize)) {
    let oc = outputs.channels(port);
    for (c, dst) in io.iter_mut().take(ch).enumerate() {
        let Some(dst) = dst.get_mut(start..start + n) else { continue };
        if ch == 1 && oc >= 2 {
            if let (Some(l), Some(r)) = (outputs.channel(port, 0), outputs.channel(port, 1)) {
                for ((d, a), b) in dst.iter_mut().zip(l).zip(r) {
                    *d = 0.5 * (a + b);
                }
            }
        } else if let Some(src) = outputs.channel(port, c.min(oc.saturating_sub(1))).and_then(|s| s.get(..n)) {
            dst.copy_from_slice(src);
        }
    }
}

impl Plugin for ClapPlugin {
    fn info(&self) -> &'static PluginInfo {
        self.info
    }

    fn prepare(&mut self, sample_rate: f32, max_block: usize, channels: usize) {
        let sr = if sample_rate.is_finite() && sample_rate > 0.0 { f64::from(sample_rate) } else { 48_000.0 };
        let mb = max_block.clamp(1, MAX_BLOCK);
        self.channels = channels.clamp(1, MAX_CHANNELS);
        self.inst.deactivate();
        self.inst.service_callback();
        let ports = self.inst.audio_ports(true).and_then(|ins| Ok((ins, self.inst.audio_ports(false)?)));
        let (ins, outs) = match ports {
            Ok(p) => p,
            Err(e) => {
                // Unsupported layout: stay inactive (`process` passes audio through untouched).
                log::warn!("{}: {e}", self.info.id);
                self.inputs = PortBuffers::new(&[], 0);
                self.outputs = PortBuffers::new(&[], 0);
                self.in_main = None;
                self.out_main = None;
                self.max_block = 0;
                self.latency = 0;
                return;
            }
        };
        self.in_main = pick_main(&ins);
        self.out_main = pick_main(&outs);
        self.inputs = PortBuffers::new(&ins.iter().map(|p| p.channels).collect::<Vec<_>>(), mb);
        self.outputs = PortBuffers::new(&outs.iter().map(|p| p.channels).collect::<Vec<_>>(), mb);
        let port = self.inst.note_port();
        self.notes = note_mode(port);
        self.midi_in = port.is_some_and(|p| p.supported & CLAP_NOTE_DIALECT_MIDI != 0);
        self.max_block = mb;
        self.inst.state().restart.store(false, Ordering::Relaxed);
        match self.inst.activate(sr, 1, u32::try_from(mb).unwrap_or(u32::MAX)) {
            Ok(()) => {
                self.latency = self.inst.latency() as usize;
                self.inst.state().latency_changed.store(false, Ordering::Relaxed);
            }
            Err(e) => {
                log::warn!("{}: {e}", self.info.id);
                self.latency = 0;
            }
        }
    }

    fn reset(&mut self) {
        self.inst.reset();
        self.queue.retain(|e| matches!(e, Event::Param { .. }));
        self.held = [false; 128];
    }

    fn set_param(&mut self, id: &str, value: f32) -> bool {
        let Some(i) = self.info.params.iter().position(|p| p.id == id) else { return false };
        let (Some(pi), Some(&cid)) = (self.info.params.get(i), self.ids.get(i)) else { return false };
        let v = pi.clamp(value);
        if let Some(slot) = self.values.get_mut(i) {
            *slot = v;
        }
        // Coalesce with a pending change of the same parameter.
        if let Some(Event::Param { value, .. }) = self.queue.iter_mut().find(|e| matches!(e, Event::Param { id, .. } if *id == cid)) {
            *value = f64::from(v);
        } else {
            self.push(Event::Param { time: 0, id: cid, value: f64::from(v) });
        }
        true
    }

    fn param(&self, id: &str) -> Option<f32> {
        let i = self.info.params.iter().position(|p| p.id == id)?;
        self.values.get(i).copied()
    }

    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        if !self.inst.is_active() || self.max_block == 0 {
            return;
        }
        let ch = self.channels.min(io.len());
        let frames = io.iter().take(ch).map(Vec::len).fold(frames, usize::min);
        if frames == 0 || ch == 0 {
            return;
        }
        let last = u32::try_from(frames - 1).unwrap_or(u32::MAX);
        for e in &mut self.queue {
            if e.time() > last {
                e.set_time(last);
            }
        }
        let mut start = 0usize;
        while start < frames {
            let n = (frames - start).min(self.max_block);
            let end = u32::try_from(start + n).unwrap_or(u32::MAX);
            // Feed the main input port (mono sources fill every port channel).
            if let Some(p) = self.in_main {
                feed_input(&mut self.inputs, p, io, ch, (start, n));
            }
            let k = self.queue.iter().position(|e| e.time() >= end).unwrap_or(self.queue.len());
            let offset = u32::try_from(start).unwrap_or(u32::MAX);
            let events = self.queue.get(..k).unwrap_or(&[]);
            let r = self.inst.process(n, self.steady, &mut self.inputs, &mut self.outputs, events, offset, self.transport.as_ref());
            self.queue.drain(..k);
            self.steady = self.steady.saturating_add(i64::try_from(n).unwrap_or(0));
            match (r, self.out_main) {
                (Ok(_), Some(p)) => {
                    read_output(&self.outputs, p, io, ch, (start, n));
                }
                (Err(e), _) => log::debug!("{}: {e}", self.info.id),
                _ => {}
            }
            start += n;
        }
    }

    fn latency(&self) -> usize {
        self.latency
    }

    fn set_transport(&mut self, transport: &Transport) {
        self.transport = Some(*transport);
    }

    fn note_on(&mut self, offset: usize, note: u8, velocity: u8) {
        if note > 127 {
            return;
        }
        let time = u32::try_from(offset).unwrap_or(u32::MAX);
        let vel = velocity.min(127);
        match self.notes {
            NoteMode::Clap => self.push(Event::NoteOn { time, key: note, velocity: f64::from(vel) / 127.0 }),
            NoteMode::Midi => self.push(Event::Midi { time, data: [0x90, note, vel] }),
            NoteMode::None => return,
        }
        if let Some(h) = self.held.get_mut(note as usize) {
            *h = true;
        }
    }

    fn note_off(&mut self, offset: usize, note: u8) {
        if note > 127 {
            return;
        }
        let time = u32::try_from(offset).unwrap_or(u32::MAX);
        match self.notes {
            NoteMode::Clap => self.push(Event::NoteOff { time, key: note }),
            NoteMode::Midi => self.push(Event::Midi { time, data: [0x80, note, 0] }),
            NoteMode::None => return,
        }
        if let Some(h) = self.held.get_mut(note as usize) {
            *h = false;
        }
    }

    fn midi(&mut self, offset: usize, msg: [u8; 3]) {
        if self.midi_in {
            self.push(Event::Midi { time: u32::try_from(offset).unwrap_or(u32::MAX), data: msg });
        }
    }

    fn all_notes_off(&mut self) {
        for k in 0..128u8 {
            if self.held.get(k as usize).copied().unwrap_or(false) {
                self.note_off(0, k);
            }
        }
    }

    fn load_state(&mut self, data: &[u8]) -> bool {
        let ok = self.inst.load_state(data);
        if ok {
            self.refresh_values();
        }
        ok
    }

    fn open_editor(&mut self) -> Result<(), String> {
        let link = self.inst.gui().ok_or_else(|| "this plugin has no editor".to_string())?;
        link.open(self.info.name)
    }

    fn close_editor(&mut self) {
        if let Some(link) = self.inst.gui() {
            link.close();
        }
    }

    fn editor(&mut self) -> Option<Box<dyn PluginEditor>> {
        let link = self.inst.gui()?;
        Some(Box::new(ClapEditor { link, info: self.info, ids: self.ids.clone() }))
    }

    fn state_reader(&mut self) -> Option<Box<dyn soundcraft_dsp::PluginStateReader>> {
        let link = self.inst.state_link()?;
        Some(Box::new(move || link.save_state()))
    }
}

/// A CLAP plugin's floating editor, driven from the main thread (see [`GuiLink`]).
struct ClapEditor {
    link: Arc<GuiLink>,
    info: &'static PluginInfo,
    ids: Vec<u32>,
}

impl PluginEditor for ClapEditor {
    fn open(&mut self) -> Result<(), String> {
        // Edits reported before the editor opened are stale.
        let _ = self.link.idle();
        self.link.open(self.info.name)
    }

    fn close(&mut self) {
        self.link.close();
    }

    fn is_open(&self) -> bool {
        self.link.is_open()
    }

    fn idle(&mut self) -> Vec<(String, f32)> {
        let raw = self.link.idle();
        let mut out: Vec<(String, f32)> = Vec::new();
        for (cid, v) in raw {
            let Some(i) = self.ids.iter().position(|&x| x == cid) else { continue };
            let Some(p) = self.info.params.get(i) else { continue };
            let v = p.clamp(v as f32);
            // Keep only the latest value per parameter.
            match out.iter_mut().find(|(id, _)| id == p.id) {
                Some(e) => e.1 = v,
                None => out.push((p.id.to_string(), v)),
            }
        }
        out
    }

    fn state_changed(&mut self) -> bool {
        self.link.host().dirty.swap(false, Ordering::Relaxed)
    }
}
