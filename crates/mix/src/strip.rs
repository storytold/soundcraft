use super::automation::{GainRamp, transport_at, volume_db_at};
use super::clips::{ClipFx, clip_fx_params, render_clip};
use super::instances::{INSTRUMENT_SLOT, PluginFailures, PreparedInstance, create_instance, is_third_party};
use super::midi::{chase_controllers, schedule_notes};
use super::pan::{PanSpec, apply_matrix, build_matrix, identity_matrix};
use super::routing::receives_solo;
use super::{
    AutoParam, BusId, ChannelFormat, MAX_CHANNELS, PanLaw, Plugin, Range, Route, Samples, Session, SourceAudio, Track, TrackId, TrackKind, Transport,
    db_to_gain,
};
use std::collections::HashMap;
use std::sync::Arc;

/// A bus: its format and one buffer per channel.
pub(crate) struct BusBuf {
    pub(crate) fmt: ChannelFormat,
    pub(crate) buf: Vec<Vec<f32>>,
}

pub(crate) struct PluginSlot {
    pub(crate) id: String,
    /// Channel count the plugin was prepared for.
    pub(crate) ch: usize,
    pub(crate) plugin: Box<dyn Plugin>,
    /// Last values pushed to the plugin, in the insert's parameter-map order.
    applied: Vec<f32>,
    pub(crate) failed: bool,
}

pub(crate) struct Strip {
    pub(crate) buf: Vec<Vec<f32>>,
    /// Pre-fader copy for pre-fader sends.
    pub(crate) pre: Vec<Vec<f32>>,
    pub(crate) scratch: Vec<f32>,
    pub(crate) plugins: Vec<Option<PluginSlot>>,
    pub(crate) instrument: Option<PluginSlot>,
    /// The pool audio handed to the instrument with `set_sample` (sample-playing instruments).
    pub(crate) sample: Option<Arc<SourceAudio>>,
    /// MIDI notes currently sounding on the instrument (pitch), for note-offs at stops/seek.
    pub(crate) held: Vec<u8>,
    /// The instrument's next block starts somewhere new: send it the controllers in effect there.
    pub(crate) chase: bool,
    pub(crate) muted: bool,
    pub(crate) gr: f32,
    /// Per-clip effect chains (Clip Effects) and a scratch buffer to render clips into.
    pub(crate) clip_fx: HashMap<u64, ClipFx>,
    clipbuf: Vec<Vec<f32>>,
    /// Total latency of the active inserts (samples).
    pub(crate) latency: usize,
    /// Delay compensation: align this strip with the slowest one (post- and pre-fader paths),
    /// plus extra delay for direct-to-main outputs so they line up with aux returns.
    pub(crate) align: DelayLine,
    pub(crate) align_pre: DelayLine,
    pub(crate) out_extra: DelayLine,
    pub(crate) outbuf: Vec<Vec<f32>>,
    pub(crate) failures: PluginFailures,
}

/// Most latency delay compensation handles: 2^20 samples, about 22 s at 48 kHz. A strip reports
/// at most this, whatever its plugins claim, so a bogus value can't stall offline renders.
pub(crate) const MAX_LATENCY: usize = 1 << 20;

/// A multichannel delay line (samples). The rings only grow (keeping their contents), so a delay
/// change in the callback moves the read offset instead of reallocating and zeroing.
#[derive(Default)]
pub(crate) struct DelayLine {
    pub(crate) bufs: Vec<Vec<f32>>,
    /// Write position in the rings.
    idx: usize,
    /// Current delay (samples).
    pub(crate) len: usize,
}

impl DelayLine {
    pub(crate) fn set(&mut self, len: usize, ch: usize) {
        let len = len.min(MAX_LATENCY);
        self.len = len;
        let cap = self.bufs.first().map_or(0, Vec::len);
        if self.bufs.len() != ch {
            self.bufs.resize_with(ch, || vec![0.0; cap]);
        }
        let need = if len == 0 { 0 } else { len + 1 };
        if need > cap {
            let new_cap = need.next_power_of_two().min(MAX_LATENCY + 1);
            let start = self.idx.min(cap);
            for ring in &mut self.bufs {
                let mut grown = Vec::with_capacity(new_cap);
                grown.extend_from_slice(ring.get(start..).unwrap_or(&[]));
                grown.extend_from_slice(ring.get(..start).unwrap_or(&[]));
                grown.resize(new_cap, 0.0);
                *ring = grown;
            }
            self.idx = cap;
        }
    }
    pub(crate) fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        if self.len == 0 {
            return;
        }
        let start = self.idx;
        let mut end = start;
        for (c, ch) in io.iter_mut().enumerate() {
            let Some(ring) = self.bufs.get_mut(c) else { continue };
            let cap = ring.len();
            if cap <= self.len {
                continue;
            }
            let mut i = start % cap;
            for x in ch.iter_mut().take(frames) {
                if let Some(w) = ring.get_mut(i) {
                    *w = *x;
                }
                if let Some(r) = ring.get((i + cap - self.len) % cap) {
                    *x = *r;
                }
                i += 1;
                if i >= cap {
                    i = 0;
                }
            }
            end = i;
        }
        self.idx = end;
    }
    pub(crate) fn clear(&mut self) {
        self.bufs.iter_mut().for_each(|b| b.fill(0.0));
    }
}

impl Strip {
    pub(crate) fn new() -> Self {
        Strip {
            buf: Vec::new(),
            pre: Vec::new(),
            scratch: Vec::new(),
            plugins: Vec::new(),
            instrument: None,
            sample: None,
            held: Vec::new(),
            chase: true,
            muted: false,
            gr: 0.0,
            clip_fx: HashMap::new(),
            clipbuf: Vec::new(),
            latency: 0,
            align: DelayLine::default(),
            align_pre: DelayLine::default(),
            out_extra: DelayLine::default(),
            outbuf: Vec::new(),
            failures: PluginFailures::default(),
        }
    }
}

/// What `sync` needs to fill a slot.
pub(crate) struct SlotCtx<'a> {
    pub(crate) sr: f32,
    pub(crate) mb: usize,
    pub(crate) external: bool,
    pub(crate) adopt: &'a mut HashMap<(TrackId, usize), PreparedInstance>,
    pub(crate) retired: &'a mut Vec<Box<dyn Plugin>>,
}

impl SlotCtx<'_> {
    pub(crate) fn retire(&mut self, slot: Option<PluginSlot>) {
        if let Some(s) = slot {
            self.retired.push(s.plugin);
        }
    }

    /// True when the host handed over an instance for this slot. It replaces whatever the slot
    /// holds, even an instance of the same plugin: the host made it because it no longer tracks
    /// the old one (it was removed and re-added between two renders), and drives its editor.
    pub(crate) fn waiting(&self, t: TrackId, idx: usize) -> bool {
        self.external && self.adopt.contains_key(&(t, idx))
    }

    /// A new instance for `ins` on track `t`, slot `idx`, prepared for `ch` channels.
    pub(crate) fn make(&mut self, t: TrackId, idx: usize, ins: &soundcraft_model::Insert, ch: usize) -> Option<PluginSlot> {
        let third = is_third_party(&ins.plugin);
        let plugin = if third && self.external {
            match self.adopt.remove(&(t, idx)) {
                Some(p) if p.id == ins.plugin && p.ch == ch => Some(p.plugin),
                Some(p) => {
                    self.retired.push(p.plugin);
                    None
                }
                None => None,
            }
        } else {
            let state = if third { ins.state_bytes() } else { None };
            create_instance(&ins.plugin, state.as_deref(), self.sr, self.mb, ch).map(|n| n.plugin)
        }?;
        Some(PluginSlot { id: ins.plugin.clone(), ch, plugin, applied: Vec::new(), failed: false })
    }
}

/// Source → trim → inserts → (pre-fader copy) → mute/fader for one strip. Independent of other
/// strips except that aux inputs read `busses`, so non-aux strips can run in parallel.
/// Live input routed to monitored tracks.
pub(crate) struct LiveInput<'a> {
    pub(crate) input: &'a [Vec<f32>],
    pub(crate) monitor_only: bool,
    pub(crate) recording: bool,
}

/// The block being rendered: where it starts, its length, and whether any track is soloed.
#[derive(Clone, Copy)]
pub(crate) struct Block {
    pub(crate) pos: Samples,
    pub(crate) frames: usize,
    pub(crate) any_solo: bool,
}

fn zero_buf(buf: &mut [Vec<f32>], frames: usize) {
    for c in buf.iter_mut() {
        c.iter_mut().take(frames).for_each(|x| *x = 0.0);
    }
}

/// Input monitoring: the track hears its hardware input instead of its clips.
fn monitor_input(t: &Track, strip: &mut Strip, live: &LiveInput<'_>, frames: usize) {
    let first = match &t.mixer.input {
        Route::Hardware(h) => {
            h.trim_start_matches("In ").split(['-', ' ']).next().and_then(|n| n.parse::<usize>().ok()).map_or(0, |n| n.saturating_sub(1))
        }
        _ => 0,
    };
    let n_in = live.input.len().max(1);
    for (k, dst) in strip.buf.iter_mut().enumerate() {
        if let Some(src) = live.input.get((first + k) % n_in) {
            for (d, x) in dst.iter_mut().zip(src.iter()).take(frames) {
                *d = *x;
            }
        }
    }
}

/// Render the audio clips overlapping the block into the strip, through their clip effects.
fn render_audio_clips(s: &Session, t: &Track, strip: &mut Strip, blk: Block) {
    let Block { pos, frames, .. } = blk;
    let block = Range { start: pos, end: pos.saturating_add(frames as i64) };
    for clip in t.clips() {
        if clip.muted || !clip.is_audio() || !clip.range().overlaps(&block) {
            continue;
        }
        match clip_fx_params(s, clip.id.0) {
            None => render_clip(s, clip, pos, frames, &mut strip.buf, &mut strip.scratch),
            Some(params) => {
                let nch = strip.buf.len();
                if strip.clipbuf.len() != nch || strip.clipbuf.first().map_or(0, Vec::len) < frames {
                    strip.clipbuf = vec![vec![0.0; frames.max(strip.scratch.len())]; nch];
                }
                zero_buf(&mut strip.clipbuf, frames);
                render_clip(s, clip, pos, frames, &mut strip.clipbuf, &mut strip.scratch);
                let fx = strip.clip_fx.entry(clip.id.0).or_insert_with(|| ClipFx::new(s.sample_rate.as_f64() as f32, nch));
                fx.configure(params);
                fx.process(&mut strip.clipbuf, frames);
                for (d, c) in strip.buf.iter_mut().zip(strip.clipbuf.iter()) {
                    for (x, y) in d.iter_mut().zip(c.iter()).take(frames) {
                        *x += *y;
                    }
                }
            }
        }
    }
}

/// An aux or folder reads the bus its input is routed from.
fn read_aux_bus(t: &Track, strip: &mut Strip, busses: &HashMap<BusId, BusBuf>, frames: usize) {
    if let Route::Bus(b) = &t.mixer.input
        && let Some(src) = busses.get(b)
    {
        let (ns, nd) = (src.buf.len(), strip.buf.len());
        let dst_fmt = strip_format(t, nd);
        let m = if src.fmt == dst_fmt && ns == nd {
            identity_matrix(ns)
        } else if ns == 1 && nd == 2 {
            // A mono bus feeds both sides of a stereo aux.
            let mut m = [[0.0; MAX_CHANNELS]; MAX_CHANNELS];
            m[0][0] = 1.0;
            m[0][1] = 1.0;
            m
        } else {
            let stereo = if ns == 1 { [0.0, 0.0] } else { [-1.0, 1.0] };
            build_matrix(src.fmt, ns, dst_fmt, nd, &PanSpec { stereo, surround: None }, PanLaw::Minus3)
        };
        apply_matrix(&src.buf, frames, &mut strip.buf, &m, 1.0);
    }
}

/// Render the track's instrument into the strip.
fn render_instrument(s: &Session, t: &Track, strip: &mut Strip, blk: Block, transport: &Transport) {
    let Block { pos, frames, .. } = blk;
    if let Some(inst) = strip.instrument.as_mut().filter(|i| !i.failed) {
        // #152: push the instrument's stored parameter values into the live plugin
        // before processing, the same way inserts do below. Without this, the values
        // loaded from a session (or set via `mix.insert_state`) never reach the plugin
        // instance and the rendered audio matches the plugin's defaults — e.g. a saved
        // `level = -12 dB` sounds the same as `level = -6 dB`.
        if let Some(ins) = &t.instrument {
            apply_params(inst, ins, t, INSTRUMENT_SLOT, pos);
        }
        inst.plugin.set_transport(transport);
        if std::mem::take(&mut strip.chase) {
            chase_controllers(s, t, pos, inst);
        }
        schedule_notes(s, t, pos, frames, inst, &mut strip.held);
        if strip.buf.len() >= 2 {
            inst.plugin.process(&mut strip.buf, frames);
        } else if let Some(c0) = strip.buf.first_mut() {
            // Mono MIDI track: render the instrument in stereo and fold.
            let mut st = [std::mem::take(c0), std::mem::take(&mut strip.scratch)];
            inst.plugin.process(&mut st, frames);
            let [a, b] = st;
            *c0 = a;
            strip.scratch = b;
            for (x, y) in c0.iter_mut().zip(strip.scratch.iter()).take(frames) {
                *x = 0.5 * (*x + *y);
            }
        }
        if contain_failure(inst, &mut strip.buf, frames) {
            strip.failures.record(INSTRUMENT_SLOT);
        }
    }
}

/// Run the active inserts in order; records their latency and the first gain reduction.
fn run_inserts(t: &Track, strip: &mut Strip, blk: Block, transport: &Transport) {
    let Block { pos, frames, .. } = blk;
    let mut gr = 0.0f32;
    let mut latency = 0usize;
    for (i, ins) in t.mixer.inserts.iter().enumerate() {
        let (Some(ins), Some(Some(slot))) = (ins, strip.plugins.get_mut(i)) else { continue };
        if !ins.active || ins.bypass || slot.failed {
            continue;
        }
        apply_params(slot, ins, t, i, pos);
        slot.plugin.set_transport(transport);
        slot.plugin.process(&mut strip.buf, frames);
        if contain_failure(slot, &mut strip.buf, frames) {
            strip.failures.record(i);
            continue;
        }
        latency = latency.saturating_add(slot.plugin.latency());
        if gr == 0.0 {
            gr = slot.plugin.gain_reduction_db();
        }
    }
    strip.gr = gr;
    strip.latency = latency.min(MAX_LATENCY);
}

/// Fader gain (volume, trim and VCA automation), ramped across the block.
fn apply_fader(s: &Session, t: &Track, strip: &mut Strip, blk: Block) {
    let Block { pos, frames, .. } = blk;
    // Endpoints are the first and last sample of this block. The next block starts on its own
    // first sample, so a breakpoint there does not ramp the block before it.
    let last = pos.saturating_add(frames.saturating_sub(1) as i64);
    let mut v0 = volume_db_at(t, pos);
    let mut v1 = volume_db_at(t, last);
    // Trim automation (an offset on top of the volume curve).
    if let Some(l) = t
        .automation
        .iter()
        .find(|l| matches!(&l.param, AutoParam::Plugin { slot: u8::MAX, param } if param == "trim"))
        .filter(|l| !l.points.is_empty())
    {
        v0 += l.value_at(pos, 0.0);
        v1 += l.value_at(last, 0.0);
    }
    // VCA master: its fader offsets the member's, and its mute mutes the member.
    if let Some(vid) = t.mixer.vca
        && let Some(vca) = s.tracks.iter().find(|x| x.id.0 == vid && x.kind == TrackKind::Vca)
    {
        v0 += volume_db_at(vca, pos);
        v1 += volume_db_at(vca, last);
        if vca.mixer.mute {
            strip.muted = true;
        }
    }
    let (g0, g1) = if strip.muted { (-144.0, -144.0) } else { (v0.min(12.0), v1.min(12.0)) };
    let ramp = GainRamp::new(g0, g1, frames.saturating_sub(1));
    for c in strip.buf.iter_mut() {
        for (i, x) in c.iter_mut().enumerate().take(frames) {
            *x *= ramp.at(i);
        }
    }
}

/// Track mute gates the audio and its pre-fader tap without splitting processor blocks.
/// Keep the existing curve interpolation, first-lane and mode policy, and render-position
/// clock. Processor/bus age and Master/VCA automation policy are separate concerns.
fn gate_mute_automation(t: &Track, strip: &mut Strip, blk: Block) {
    let Block { pos, frames, .. } = blk;
    if t.mixer.automation_mode.reads()
        && let Some(lane) = t.lane(&AutoParam::Mute).filter(|lane| !lane.points.is_empty())
    {
        for i in 0..frames {
            let at = pos.saturating_add(i64::try_from(i).unwrap_or(i64::MAX));
            if lane.value_at(at, 0.0) >= 0.5 {
                for channel in strip.buf.iter_mut().chain(strip.pre.iter_mut()) {
                    if let Some(sample) = channel.get_mut(i) {
                        *sample = 0.0;
                    }
                }
            }
        }
    }
}

pub(crate) fn process_strip(s: &Session, t: &Track, strip: &mut Strip, busses: &HashMap<BusId, BusBuf>, live: &LiveInput<'_>, blk: Block) {
    let Block { pos, frames, any_solo } = blk;
    zero_buf(&mut strip.buf, frames);
    let transport = transport_at(s, pos, !live.monitor_only, live.recording);
    match t.kind {
        TrackKind::Audio if !live.input.is_empty() && (t.mixer.input_monitor || (t.mixer.record_arm && (live.monitor_only || live.recording))) => {
            monitor_input(t, strip, live, frames);
        }
        TrackKind::Audio if live.monitor_only => {}
        TrackKind::Instrument | TrackKind::Midi if live.monitor_only => {}
        TrackKind::Audio => render_audio_clips(s, t, strip, blk),
        TrackKind::Aux | TrackKind::Folder => read_aux_bus(t, strip, busses, frames),
        TrackKind::Instrument | TrackKind::Midi => render_instrument(s, t, strip, blk, &transport),
        _ => {}
    }
    let trim = db_to_gain(t.mixer.trim_db) * if t.mixer.phase_invert { -1.0 } else { 1.0 };
    if (trim - 1.0).abs() > f32::EPSILON {
        for c in strip.buf.iter_mut() {
            c.iter_mut().take(frames).for_each(|x| *x *= trim);
        }
    }
    run_inserts(t, strip, blk, &transport);
    let soloed_out = any_solo && !t.mixer.solo && !t.mixer.solo_safe && !receives_solo(s, t);
    strip.muted = t.mixer.mute || soloed_out;
    if t.mixer.sends.iter().flatten().any(|x| x.pre_fader) {
        strip.pre.resize_with(strip.buf.len(), Vec::new);
        for (d, src) in strip.pre.iter_mut().zip(strip.buf.iter()) {
            d.clear();
            d.extend_from_slice(src.get(..frames).unwrap_or(&[]));
        }
    }
    apply_fader(s, t, strip, blk);
    gate_mute_automation(t, strip, blk);
}

/// Silences a block the plugin filled with NaN or infinity and takes the plugin out until the
/// next [`MixEngine::reset`]. True then.
pub(crate) fn contain_failure(slot: &mut PluginSlot, buf: &mut [Vec<f32>], frames: usize) -> bool {
    // A branch-free fold vectorizes; `all` runs about 8 times slower here.
    if buf.iter().all(|c| c.get(..frames).unwrap_or(c).iter().fold(true, |ok, x| ok & x.is_finite())) {
        return false;
    }
    for c in buf.iter_mut() {
        c.iter_mut().take(frames).for_each(|x| *x = 0.0);
    }
    slot.failed = true;
    true
}

pub(crate) fn strip_channels(t: &Track, main_ch: usize) -> usize {
    match t.kind {
        TrackKind::Master => main_ch,
        TrackKind::Instrument => t.channels().clamp(2, MAX_CHANNELS),
        _ => t.channels().clamp(1, MAX_CHANNELS),
    }
}

/// The format a strip's buffer actually carries (a mono instrument renders in stereo).
pub(crate) fn strip_format(t: &Track, n: usize) -> ChannelFormat {
    if t.format.channels().min(MAX_CHANNELS) == n { t.format } else { ChannelFormat::for_channels(n) }
}

/// Record insert latency without processing audio, so this block can compensate for it.
pub(crate) fn note_insert_latency(t: &Track, strip: &mut Strip, pos: Samples) {
    let mut latency = 0usize;
    for (i, ins) in t.mixer.inserts.iter().enumerate() {
        let (Some(ins), Some(Some(slot))) = (ins, strip.plugins.get_mut(i)) else { continue };
        if !ins.active || ins.bypass || slot.failed {
            continue;
        }
        apply_params(slot, ins, t, i, pos);
        latency = latency.saturating_add(slot.plugin.latency());
    }
    strip.latency = latency.min(MAX_LATENCY);
}

pub(crate) fn apply_params(slot: &mut PluginSlot, ins: &soundcraft_model::Insert, t: &Track, i: usize, pos: Samples) {
    // Automation lanes address insert slots; the instrument (`INSTRUMENT_SLOT`) has none, so it
    // must not pick up slot A's lanes.
    let Ok(slot_u8) = u8::try_from(i) else {
        return apply_values(slot, ins, |_, v| v);
    };
    let automated = t.mixer.automation_mode.reads()
        && t.automation.iter().any(|l| matches!(&l.param, AutoParam::Plugin { slot, .. } if *slot == slot_u8) && !l.points.is_empty());
    apply_values(slot, ins, |k, val| {
        if automated
            && let Some(l) = t.automation.iter().find(|l| matches!(&l.param, AutoParam::Plugin { slot, param } if *slot == slot_u8 && param == k))
            && !l.points.is_empty()
        {
            return l.value_at(pos, val);
        }
        val
    });
}

/// Sends each stored param (as mapped by `value`) to the plugin when it changed since last time.
fn apply_values(slot: &mut PluginSlot, ins: &soundcraft_model::Insert, value: impl Fn(&str, f32) -> f32) {
    if slot.applied.len() != ins.params.len() {
        slot.applied = vec![f32::NAN; ins.params.len()];
    }
    for ((k, v), last) in ins.params.iter().zip(slot.applied.iter_mut()) {
        let val = value(k, *v);
        if val.to_bits() != last.to_bits() {
            slot.plugin.set_param(k, val);
            *last = val;
        }
    }
}
