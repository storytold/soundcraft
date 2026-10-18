//! `ClapPlugin`: a loaded CLAP instance behind the safe `soundcraft_dsp::Plugin` trait.

use crate::ffi::{EVENT_CAP, Event, GuiLink, Instance, NotePort, PortBuffers};
use clap_sys::ext::note_ports::{CLAP_NOTE_DIALECT_CLAP, CLAP_NOTE_DIALECT_MIDI};
use soundcraft_dsp::{
    BlockDeliveryCapacity, BlockDeliveryError, BlockDeliveryReceipt, BlockNote, BlockNoteMessage, BlockTarget, Plugin, PluginEditor, PluginInfo,
    Transport,
};
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

struct BlockDelivery {
    capacity: BlockDeliveryCapacity,
    index: Vec<(&'static str, usize)>,
    latest: Vec<Option<f32>>,
    acknowledged: Vec<Option<f32>>,
    candidate: Vec<Option<f32>>,
    events: Vec<Event>,
    ready: bool,
}

fn delivery_slots(count: usize) -> Result<Vec<Option<f32>>, BlockDeliveryError> {
    let mut slots = Vec::new();
    slots.try_reserve_exact(count).map_err(|_| BlockDeliveryError::Allocation)?;
    slots.resize(count, None);
    Ok(slots)
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
    delivery: Option<BlockDelivery>,
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
            delivery: None,
        }
    }

    fn invalidate_block_delivery(&mut self, clear_targets: bool) {
        if let Some(delivery) = &mut self.delivery {
            delivery.ready = false;
            delivery.acknowledged.fill(None);
            if clear_targets {
                delivery.latest.fill(None);
                delivery.candidate.fill(None);
            }
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

impl Plugin for ClapPlugin {
    fn info(&self) -> &'static PluginInfo {
        self.info
    }

    fn prepare(&mut self, sample_rate: f32, max_block: usize, channels: usize) {
        self.invalidate_block_delivery(false);
        let sr = if sample_rate.is_finite() && sample_rate > 0.0 { f64::from(sample_rate) } else { 48_000.0 };
        let mb = max_block.clamp(1, MAX_BLOCK);
        self.channels = channels.clamp(1, MAX_CHANNELS);
        self.inst.deactivate();
        self.inst.service_callback();
        let ins = self.inst.audio_ports(true);
        let outs = self.inst.audio_ports(false);
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
        if let Some(delivery) = &mut self.delivery {
            delivery.acknowledged.fill(None);
        }
        self.inst.reset();
        self.queue.retain(|e| matches!(e, Event::Param { .. }));
        self.held = [false; 128];
    }

    fn set_param(&mut self, id: &str, value: f32) -> bool {
        self.invalidate_block_delivery(true);
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
        self.invalidate_block_delivery(true);
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
                for c in 0..self.inputs.channels(p) {
                    let src = io.get(c.min(ch - 1)).and_then(|s| s.get(start..start + n));
                    if let (Some(src), Some(dst)) = (src, self.inputs.channel_mut(p, c).and_then(|d| d.get_mut(..n))) {
                        dst.copy_from_slice(src);
                    }
                }
            }
            let k = self.queue.iter().position(|e| e.time() >= end).unwrap_or(self.queue.len());
            let offset = u32::try_from(start).unwrap_or(u32::MAX);
            let events = self.queue.get(..k).unwrap_or(&[]);
            let r = self.inst.process(n, self.steady, &mut self.inputs, &mut self.outputs, events, offset, self.transport.as_ref());
            self.queue.drain(..k);
            self.steady = self.steady.saturating_add(i64::try_from(n).unwrap_or(0));
            match (r, self.out_main) {
                (Ok(_), Some(p)) => {
                    let oc = self.outputs.channels(p);
                    for (c, dst) in io.iter_mut().take(ch).enumerate() {
                        let Some(dst) = dst.get_mut(start..start + n) else { continue };
                        if ch == 1 && oc >= 2 {
                            if let (Some(l), Some(r)) = (self.outputs.channel(p, 0), self.outputs.channel(p, 1)) {
                                for ((d, a), b) in dst.iter_mut().zip(l).zip(r) {
                                    *d = 0.5 * (a + b);
                                }
                            }
                        } else if let Some(src) = self.outputs.channel(p, c.min(oc.saturating_sub(1))).and_then(|s| s.get(..n)) {
                            dst.copy_from_slice(src);
                        }
                    }
                }
                (Err(e), _) => log::debug!("{}: {e}", self.info.id),
                _ => {}
            }
            start += n;
        }
    }

    fn prepare_block_delivery(&mut self, capacity: BlockDeliveryCapacity) -> Result<(), BlockDeliveryError> {
        self.invalidate_block_delivery(false);
        if !self.inst.is_active() || self.max_block == 0 {
            return Err(BlockDeliveryError::NotPrepared);
        }
        if !self.queue.is_empty() {
            return Err(BlockDeliveryError::MixedLegacy);
        }
        let count = self.info.params.len();
        if count > 4096 || capacity.targets > 4096 || capacity.notes > 1024 {
            return Err(BlockDeliveryError::InvalidCapacity);
        }
        let total = capacity.targets.checked_add(capacity.notes).ok_or(BlockDeliveryError::InvalidCapacity)?;
        let mut index = Vec::new();
        index.try_reserve_exact(count).map_err(|_| BlockDeliveryError::Allocation)?;
        for (i, param) in self.info.params.iter().enumerate() {
            if param.id.len() > 10
                || param.id.parse::<u32>().ok() != self.ids.get(i).copied()
                || !param.min.is_finite()
                || !param.max.is_finite()
                || !param.default.is_finite()
                || param.min > param.max
            {
                return Err(BlockDeliveryError::InvalidTarget);
            }
            index.push((param.id, i));
        }
        index.sort_unstable_by(|a, b| a.0.cmp(b.0));
        if index.windows(2).any(|pair| matches!(pair, [a, b] if a.0 == b.0)) {
            return Err(BlockDeliveryError::InvalidTarget);
        }
        let mut latest = delivery_slots(count)?;
        if let Some(prior) = &self.delivery {
            if prior.latest.len() != count {
                return Err(BlockDeliveryError::InvalidCapacity);
            }
            latest.clone_from_slice(&prior.latest);
        }
        if latest.iter().flatten().count() > capacity.targets {
            return Err(BlockDeliveryError::InvalidCapacity);
        }
        let acknowledged = delivery_slots(count)?;
        let candidate = delivery_slots(count)?;
        let mut events = Vec::new();
        events.try_reserve_exact(total).map_err(|_| BlockDeliveryError::Allocation)?;
        self.inst.reserve_block_events(total)?;
        self.delivery = Some(BlockDelivery { capacity, index, latest, acknowledged, candidate, events, ready: true });
        Ok(())
    }

    fn process_block_delivery(
        &mut self,
        io: &mut [Vec<f32>],
        frames: usize,
        targets: &[BlockTarget<'_>],
        notes: &[BlockNote],
    ) -> Result<BlockDeliveryReceipt, BlockDeliveryError> {
        let delivery = self.delivery.as_mut().filter(|d| d.ready).ok_or(BlockDeliveryError::NotPrepared)?;
        if !self.queue.is_empty() {
            return Err(BlockDeliveryError::MixedLegacy);
        }
        if !self.inst.is_active()
            || frames == 0
            || frames > self.max_block
            || io.len() != self.channels
            || io.iter().any(|channel| channel.len() < frames)
        {
            return Err(BlockDeliveryError::InvalidBlock);
        }
        let output = self.out_main.filter(|p| self.outputs.channels(*p) > 0).ok_or(BlockDeliveryError::NotPrepared)?;
        if targets.len() > 4096 || notes.len() > delivery.capacity.notes {
            return Err(BlockDeliveryError::InvalidCapacity);
        }
        delivery.candidate.clone_from_slice(&delivery.latest);
        for target in targets {
            if target.id.len() > 10 {
                return Err(BlockDeliveryError::InvalidTarget);
            }
            let index = delivery.index.binary_search_by(|entry| entry.0.cmp(target.id)).map_err(|_| BlockDeliveryError::InvalidTarget)?;
            let (_, index) = delivery.index.get(index).copied().ok_or(BlockDeliveryError::InvalidTarget)?;
            let param = self.info.params.get(index).ok_or(BlockDeliveryError::InvalidTarget)?;
            *delivery.candidate.get_mut(index).ok_or(BlockDeliveryError::InvalidTarget)? = Some(param.clamp(target.value));
        }
        if delivery.candidate.iter().flatten().count() > delivery.capacity.targets {
            return Err(BlockDeliveryError::InvalidCapacity);
        }
        let mut last_offset = 0;
        for note in notes {
            if note.offset >= frames || note.offset < last_offset {
                return Err(BlockDeliveryError::InvalidBlock);
            }
            match note.message {
                BlockNoteMessage::On { note, .. } | BlockNoteMessage::Off { note } if note > 127 => return Err(BlockDeliveryError::InvalidTarget),
                BlockNoteMessage::On { .. } | BlockNoteMessage::Off { .. } if self.notes == NoteMode::None => {
                    return Err(BlockDeliveryError::Unsupported);
                }
                BlockNoteMessage::Midi(_) if !self.midi_in => return Err(BlockDeliveryError::Unsupported),
                _ => {}
            }
            last_offset = note.offset;
        }
        // Every admission check precedes mutation of latest targets or a processor call.
        delivery.events.clear();
        for (index, (value, acknowledged)) in delivery.candidate.iter().zip(&delivery.acknowledged).enumerate() {
            if let Some(value) = value
                && acknowledged.is_none_or(|prior| prior.to_bits() != value.to_bits())
            {
                let id = self.ids.get(index).copied().ok_or(BlockDeliveryError::InvalidTarget)?;
                delivery.events.push(Event::Param { time: 0, id, value: f64::from(*value) });
            }
        }
        let target_count = delivery.events.len();
        for note in notes {
            let time = u32::try_from(note.offset).map_err(|_| BlockDeliveryError::InvalidBlock)?;
            let event = match (note.message, self.notes) {
                (BlockNoteMessage::On { note, velocity }, NoteMode::Clap) => {
                    Event::NoteOn { time, key: note, velocity: f64::from(velocity.min(127)) / 127.0 }
                }
                (BlockNoteMessage::Off { note }, NoteMode::Clap) => Event::NoteOff { time, key: note },
                (BlockNoteMessage::On { note, velocity }, _) => Event::Midi { time, data: [0x90, note, velocity.min(127)] },
                (BlockNoteMessage::Off { note }, _) => Event::Midi { time, data: [0x80, note, 0] },
                (BlockNoteMessage::Midi(data), _) => Event::Midi { time, data },
            };
            delivery.events.push(event);
        }
        delivery.latest.clone_from_slice(&delivery.candidate);
        if let Some(port) = self.in_main {
            for channel in 0..self.inputs.channels(port) {
                if let (Some(src), Some(dst)) = (
                    io.get(channel.min(self.channels.saturating_sub(1))).and_then(|v| v.get(..frames)),
                    self.inputs.channel_mut(port, channel).and_then(|v| v.get_mut(..frames)),
                ) {
                    dst.copy_from_slice(src);
                }
            }
        }
        let result =
            self.inst.process_block_checked(frames, self.steady, &mut self.inputs, &mut self.outputs, &delivery.events, self.transport.as_ref());
        delivery.events.clear();
        self.steady = self.steady.saturating_add(i64::try_from(frames).unwrap_or(0));
        let status = match result {
            Ok(status) => status,
            Err(error) => {
                delivery.acknowledged.fill(None);
                for channel in io.iter_mut() {
                    if let Some(samples) = channel.get_mut(..frames) {
                        samples.fill(0.0);
                    }
                }
                return Err(error);
            }
        };
        delivery.acknowledged.clone_from_slice(&delivery.latest);
        let output_channels = self.outputs.channels(output);
        for (channel, dst) in io.iter_mut().enumerate() {
            let Some(dst) = dst.get_mut(..frames) else { continue };
            if self.channels == 1 && output_channels >= 2 {
                if let (Some(left), Some(right)) = (self.outputs.channel(output, 0), self.outputs.channel(output, 1)) {
                    for ((sample, left), right) in dst.iter_mut().zip(left).zip(right) {
                        *sample = 0.5 * (left + right);
                    }
                }
            } else if let Some(src) = self.outputs.channel(output, channel.min(output_channels.saturating_sub(1))).and_then(|v| v.get(..frames)) {
                dst.copy_from_slice(src);
            }
        }
        Ok(BlockDeliveryReceipt { frames, targets: target_count, notes: notes.len(), status })
    }

    fn clear_block_delivery_targets(&mut self) -> Result<(), BlockDeliveryError> {
        let delivery = self.delivery.as_mut().ok_or(BlockDeliveryError::NotPrepared)?;
        delivery.latest.fill(None);
        delivery.acknowledged.fill(None);
        delivery.candidate.fill(None);
        Ok(())
    }

    fn latency(&self) -> usize {
        self.latency
    }

    fn set_transport(&mut self, transport: &Transport) {
        self.transport = Some(*transport);
    }

    fn note_on(&mut self, offset: usize, note: u8, velocity: u8) {
        self.invalidate_block_delivery(true);
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
        self.invalidate_block_delivery(true);
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
        self.invalidate_block_delivery(true);
        if self.midi_in {
            self.push(Event::Midi { time: u32::try_from(offset).unwrap_or(u32::MAX), data: msg });
        }
    }

    fn all_notes_off(&mut self) {
        self.invalidate_block_delivery(true);
        for k in 0..128u8 {
            if self.held.get(k as usize).copied().unwrap_or(false) {
                self.note_off(0, k);
            }
        }
    }

    fn load_state(&mut self, data: &[u8]) -> bool {
        let ok = self.inst.load_state(data);
        if let Some(delivery) = &mut self.delivery {
            delivery.acknowledged.fill(None);
            if ok {
                delivery.latest.fill(None);
                delivery.candidate.fill(None);
            }
        }
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
        let _ = self.link.host().dirty.swap(false, Ordering::Relaxed);
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
}
