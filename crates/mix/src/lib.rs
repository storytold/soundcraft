//! The SoundCraft mix engine.
//!
//! [`MixEngine::render`] produces one block of the main output for a session at a timeline
//! position: clips (with clip gain and fades) → trim/phase → inserts → pre-fader sends → fader and
//! mute (with automation) → pan → post-fader sends → busses → aux inputs → main → master faders.
//! The same code drives realtime playback and offline bounces, so what you hear is what you get.
//! Steady-state rendering does not allocate.
//!
//! ## Surround
//! The main mix has the format of the session's main output path ([`Session::main_format`]:
//! stereo by default, up to 16 channels such as 7.1.4 or 9.1.6) and every bus has its own format.
//! Channels are in SMPTE / WAV order (L R C LFE Ls Rs …, see
//! [`ChannelFormat::speakers`](soundcraft_model::ChannelFormat::speakers)). Routing a strip into a
//! destination builds a gain matrix per block:
//! - mono/stereo sources into a stereo destination use the stereo pan exactly as before;
//! - into a multichannel destination they use the track's [`SurroundPan`] (pairwise
//!   constant-power panner over the speaker layout, see `soundcraft_dsp::pan::surround_gains`),
//!   or go to L/R with the stereo pan when the track has no surround pan;
//! - sources whose format equals the destination's pass straight through, channel for channel;
//! - other multichannel sources fold to stereo with ITU-R BS.775 coefficients (C and surrounds at
//!   -3 dB, LFE dropped), or map speaker by speaker into other multichannel layouts.
//!
//! ## Plugin instances and threads
//! [`MixEngine::sync`] brings plugin instances in line with the session. Built-in plugins are
//! cheap (allocation only) and are always created there, on whichever thread renders. Hosted
//! third-party plugins (`clap:` / `vst3:` ids, see [`is_third_party`]) are expensive and have
//! main-thread rules, so a realtime host keeps them off the audio thread:
//! - with [`MixEngine::set_external_instances`]`(true)` (realtime playback), `sync` never creates a
//!   third-party plugin. The host's control (UI) thread creates them with [`create_instance`] from
//!   [`instance_specs`] (restoring each insert's stored [`Insert::state`] and calling `prepare`
//!   there), and hands them over with [`MixEngine::adopt`]; `sync` then moves the matching one
//!   into its slot. A slot whose instance has not arrived yet passes audio through.
//! - every instance `sync` replaces or removes (built-in or not) goes to a retired list instead of
//!   being dropped; the host takes it with [`MixEngine::take_retired`] and drops it on the control
//!   thread, so plugin destruction never runs on the audio thread either.
//! - without external instances (the default: offline [`render_range`], bounces, tests) `sync`
//!   creates third-party plugins itself, synchronously, restores their stored state before the
//!   first `process`, and drops retired instances at the end of `sync`.
//!
//! An instance starts from its stored state; the insert's parameter values are then applied on
//! top before the first block (the session's values are kept in step with the plugin, including
//! edits made in its own editor, so both agree; the state carries everything else). A host reads
//! states back for saving through each instance's [`Plugin::state_reader`], on its UI thread.
//!
//! A built-in instrument's parameter values are applied the same way. A sample-playing
//! instrument (the Sampler) is handed the decoded audio of its insert's
//! [`Insert::sample`] source straight from the session's pool, shared and never copied.
//!
//! [`Insert::state`]: soundcraft_model::Insert::state
//! [`Insert::sample`]: soundcraft_model::Insert::sample
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use soundcraft_dsp::pan::{PanLaw, SpeakerPos, SurroundParams, gains};
use soundcraft_dsp::{Plugin, SampleSource, Transport, db_to_gain};
use soundcraft_midi::CtrlKind;
use soundcraft_model::{
    AutoParam, BusId, ChannelFormat, Clip, ClipContent, Route, Session, SourceAudio, Speaker, SurroundPan, Track, TrackId, TrackKind,
};
use soundcraft_time::{Range, Samples, TICKS_PER_QUARTER};
use std::collections::HashMap;
use std::ops::Bound;

mod automation;
mod clips;
mod instances;
mod meter;
mod midi;
mod offline;
mod pan;
mod routing;
mod strip;
mod threads;

use automation::{GainRamp, pan_at, pan_segment, send_level, transport_at, volume_db_at};
pub use instances::{INSTRUMENT_SLOT, InstanceSpec, NewInstance, PluginFailures, PreparedInstance, create_instance, instance_specs, is_third_party};
pub use meter::StripMeter;
use midi::sync_sample;
pub use offline::{
    MAX_RENDER_SAMPLES, RangeRender, fold_down_stereo, render_bus, render_clips, render_range, render_track_pre_fader, source_channels,
};
pub use pan::itu_stereo_fold;
use pan::{PanSpec, apply_matrix, apply_matrix_span, build_matrix};
use routing::{processing_order, structure_fingerprint};
use strip::{
    Block, BusBuf, LiveInput, SlotCtx, Strip, apply_params, contain_failure, note_insert_latency, process_strip, strip_channels, strip_format,
};
#[cfg(test)]
use strip::{DelayLine, MAX_LATENCY};
use threads::run_strips;
pub use threads::{mark_audio_thread, on_audio_thread};

/// Default (stereo) main mix width. The actual width is [`main_channels`].
pub const MAIN_CHANNELS: usize = 2;

/// Widest strip, bus or main mix (9.1.6 / 3rd-order Ambisonics). Wider formats are truncated.
pub const MAX_CHANNELS: usize = 16;

/// Number of channels of a session's main mix.
pub fn main_channels(s: &Session) -> usize {
    s.main_format().channels().clamp(MAIN_CHANNELS, MAX_CHANNELS)
}

/// Renders sessions. Keep one per playback stream; plugin state persists across blocks.
pub struct MixEngine {
    sample_rate: f32,
    max_block: usize,
    strips: HashMap<TrackId, Strip>,
    busses: HashMap<BusId, BusBuf>,
    main: Vec<Vec<f32>>,
    main_fmt: ChannelFormat,
    pub meters: HashMap<TrackId, StripMeter>,
    pub main_meter: StripMeter,
    pub pan_law: PanLaw,
    /// Last rendered position end (to detect seeks).
    last_end: Samples,
    /// Snapshot key of the session the strips/order were synced to.
    synced: (usize, usize, usize),
    order: Vec<usize>,
    /// Current total compensation delay (samples) of the main mix: the slowest track and aux
    /// returns, and the master faders' inserts.
    pub latency: usize,
    /// Delay already applied to audio written onto busses: the slowest track insert.
    /// A bus render drops this so the bus lines up with the timeline. The extra delay
    /// that lines direct outputs up with aux returns stays in `latency` and is removed
    /// only from the main mix.
    track_latency: usize,
    /// Busses an aux returns onto, and the latency of the slowest aux return: audio written onto
    /// such a bus waits for it, so a bounce of the bus drops it as well.
    aux_fed: Vec<BusId>,
    aux_latency: usize,
    /// Live input for the next block (planar), set by the audio host for input monitoring.
    pub input: Vec<Vec<f32>>,
    /// Transport stopped: only monitored inputs sound (no clips).
    pub monitor_only: bool,
    /// Recording: record-armed tracks hear their input (auto input).
    pub recording: bool,
    /// Mix the metronome (Options > Click) into the output. Playback turns this on; offline renders
    /// (bounces, stems, exports) leave it off so the click is never printed into a file.
    pub metronome: bool,
    /// Third-party instances come from [`MixEngine::adopt`] instead of being created in `sync`.
    external: bool,
    /// Instances handed over by the host, waiting for `sync` to place them.
    adopt: HashMap<(TrackId, usize), PreparedInstance>,
    /// Instances `sync` took out of service, for the host to drop (see [`MixEngine::take_retired`]).
    retired: Vec<Box<dyn Plugin>>,
    /// Ownership workspace reused for parallel strip processing.
    work: Vec<(usize, Strip)>,
}

impl MixEngine {
    pub fn new(sample_rate: f32, max_block: usize) -> Self {
        let max_block = max_block.clamp(16, 16_384);
        MixEngine {
            sample_rate: if sample_rate.is_finite() && sample_rate > 0.0 { sample_rate } else { 48_000.0 },
            max_block,
            strips: HashMap::new(),
            busses: HashMap::new(),
            main: vec![vec![0.0; max_block]; MAIN_CHANNELS],
            main_fmt: ChannelFormat::Stereo,
            meters: HashMap::new(),
            main_meter: StripMeter::default(),
            pan_law: PanLaw::Minus3,
            last_end: 0,
            synced: (0, 0, 0),
            order: Vec::new(),
            latency: 0,
            track_latency: 0,
            aux_fed: Vec::new(),
            aux_latency: 0,
            input: Vec::new(),
            monitor_only: false,
            recording: false,
            metronome: false,
            external: false,
            adopt: HashMap::new(),
            retired: Vec::new(),
            work: Vec::new(),
        }
    }

    /// Realtime mode: third-party plugins are created by the host and handed over with
    /// [`MixEngine::adopt`] instead of being created in `sync` (see the crate docs).
    pub fn set_external_instances(&mut self, on: bool) {
        self.external = on;
    }

    /// Takes instances created off the audio thread; the next render's `sync` places them.
    /// An earlier instance waiting for the same slot is retired.
    pub fn adopt(&mut self, instances: Vec<PreparedInstance>) {
        for p in instances {
            if let Some(old) = self.adopt.insert((p.track, p.slot), p) {
                self.retired.push(old.plugin);
            }
        }
        // Force a re-sync so waiting slots pick their instance up.
        self.synced = (usize::MAX, usize::MAX, usize::MAX);
    }

    /// Instances taken out of service since the last call (drop them off the audio thread).
    /// Does not allocate.
    pub fn take_retired(&mut self) -> Vec<Box<dyn Plugin>> {
        std::mem::take(&mut self.retired)
    }

    /// Every instance this engine holds (strips, instruments, clip effects excluded) plus the
    /// retired and waiting ones, leaving the engine empty: hand them to another thread to drop
    /// before replacing the engine.
    pub fn drain_plugins(&mut self) -> Vec<Box<dyn Plugin>> {
        let mut out = std::mem::take(&mut self.retired);
        for st in self.strips.values_mut() {
            out.extend(st.plugins.iter_mut().filter_map(Option::take).map(|s| s.plugin));
            out.extend(st.instrument.take().map(|s| s.plugin));
        }
        out.extend(self.adopt.drain().map(|(_, p)| p.plugin));
        self.synced = (usize::MAX, usize::MAX, usize::MAX);
        out
    }

    pub fn max_block(&self) -> usize {
        self.max_block
    }

    /// Format of the main mix as of the last sync.
    pub fn main_format(&self) -> ChannelFormat {
        self.main_fmt
    }

    /// Channels of the main mix as of the last sync.
    pub fn main_channels(&self) -> usize {
        self.main.len()
    }

    /// Reset all plugin state (on stop / seek).
    pub fn reset(&mut self) {
        for s in self.strips.values_mut() {
            for p in s.plugins.iter_mut().flatten() {
                p.plugin.reset();
                p.failed = false;
            }
            if let Some(i) = &mut s.instrument {
                i.plugin.all_notes_off();
                i.plugin.reset();
                i.failed = false;
            }
            s.held.clear();
            s.chase = true;
            s.align.clear();
            s.align_pre.clear();
            s.out_extra.clear();
            for fx in s.clip_fx.values_mut() {
                fx.reset();
            }
        }
        // Zeroed in place: rebuilding the entries would allocate on the audio thread at every
        // play, stop and locate.
        self.meters.values_mut().for_each(StripMeter::reset_levels);
        self.main_meter.reset_levels();
    }

    /// Bring plugin instances in line with the session (creates/destroys as needed). Allocates;
    /// call outside the realtime thread when the document changes, or let `render` do it.
    pub fn sync(&mut self, s: &Session) {
        let mb = self.max_block;
        let main_ch = main_channels(s);
        self.main_fmt = s.main_format();
        if self.main.len() != main_ch {
            self.main = vec![vec![0.0; mb]; main_ch];
        }
        let mut cx = SlotCtx { sr: self.sample_rate, mb, external: self.external, adopt: &mut self.adopt, retired: &mut self.retired };
        let gone: Vec<TrackId> = self.strips.keys().filter(|id| s.track(**id).is_none()).copied().collect();
        for id in gone {
            self.meters.remove(&id);
            if let Some(mut st) = self.strips.remove(&id) {
                for p in st.plugins.drain(..) {
                    cx.retire(p);
                }
                cx.retire(st.instrument.take());
            }
        }
        for t in &s.tracks {
            let ch = strip_channels(t, main_ch);
            let strip = self.strips.entry(t.id).or_insert_with(Strip::new);
            if strip.buf.len() != ch || strip.buf.first().map_or(0, Vec::len) != mb {
                strip.buf = vec![vec![0.0; mb]; ch];
            }
            while strip.plugins.len() > t.mixer.inserts.len() {
                let p = strip.plugins.pop().flatten();
                cx.retire(p);
            }
            strip.plugins.resize_with(t.mixer.inserts.len(), || None);
            for (idx, (slot, ins)) in strip.plugins.iter_mut().zip(t.mixer.inserts.iter()).enumerate() {
                match ins {
                    Some(i) if slot.as_ref().is_none_or(|p| p.id != i.plugin || p.ch != ch) || cx.waiting(t.id, idx) => {
                        let old = slot.take();
                        cx.retire(old);
                        *slot = cx.make(t.id, idx, i, ch);
                    }
                    None => {
                        let old = slot.take();
                        cx.retire(old);
                    }
                    _ => {}
                }
            }
            let want = t.instrument.as_ref();
            let ich = ch.max(2);
            if strip.instrument.as_ref().map(|p| (p.id.as_str(), p.ch)) != want.map(|i| (i.plugin.as_str(), ich))
                || (want.is_some() && cx.waiting(t.id, INSTRUMENT_SLOT))
            {
                let old = strip.instrument.take();
                cx.retire(old);
                strip.instrument = want.and_then(|i| cx.make(t.id, INSTRUMENT_SLOT, i, ich));
                strip.sample = None;
            }
            sync_sample(strip, want, s);
        }
        // Instances nobody claimed (the session changed again before they arrived).
        let leftovers: Vec<(TrackId, usize)> = cx.adopt.keys().copied().collect();
        for k in leftovers {
            if let Some(p) = cx.adopt.remove(&k) {
                cx.retired.push(p.plugin);
            }
        }
        if !self.external {
            self.retired.clear();
        }
        for b in &s.busses {
            let n = b.format.channels().clamp(1, MAX_CHANNELS);
            let bus = self.busses.entry(b.id).or_insert_with(|| BusBuf { fmt: b.format, buf: Vec::new() });
            bus.fmt = b.format;
            if bus.buf.len() != n {
                bus.buf = vec![vec![0.0; mb]; n];
            }
        }
        self.busses.retain(|id, _| s.bus(*id).is_some());
    }

    /// Re-syncs plugin instances and the routing order when the session's structure changed
    /// since the last sync (or instances were adopted). `render` calls it; a realtime host may
    /// also call it right after swapping sessions so slots are filled before playback starts.
    pub fn ensure_synced(&mut self, s: &Session) {
        let key = (structure_fingerprint(s), s.tracks.len(), s.busses.len());
        if key != self.synced {
            self.sync(s);
            self.order = processing_order(s);
            self.synced = key;
        }
    }

    /// Render `frames` (≤ max_block) samples starting at `pos` into `out` (≥ 2 channels).
    pub fn render(&mut self, s: &Session, pos: Samples, frames: usize, out: &mut [Vec<f32>]) {
        let frames = frames.min(self.max_block);
        self.begin_block(s, pos, frames);
        let blk = Block { pos, frames, any_solo: s.tracks.iter().any(|t| t.mixer.solo && !t.inactive) };
        let order = std::mem::take(&mut self.order);
        // Phase 1: independent strips (audio, instrument, MIDI) in parallel.
        let mut work = std::mem::take(&mut self.work);
        work.clear();
        for &i in &order {
            let Some(t) = s.tracks.get(i).filter(|t| is_live(t) && !is_aux(t)) else { continue };
            if let Some(st) = self.strips.remove(&t.id) {
                work.push((i, st));
            }
        }
        for (_, st) in work.iter_mut() {
            if st.scratch.len() < self.max_block {
                st.scratch.resize(self.max_block, 0.0);
            }
        }
        let busses = std::mem::take(&mut self.busses);
        let live_in = LiveInput { input: &self.input, monitor_only: self.monitor_only, recording: self.recording };
        run_strips(&mut work, |(i, st)| {
            if let Some(t) = s.tracks.get(*i) {
                process_strip(s, t, st, &busses, &live_in, blk);
            }
        });
        self.busses = busses;
        // Delay compensation: align every strip to the slowest. Direct-to-main outputs also wait
        // for the slowest aux return. Aux latency has to be read before those outputs are routed,
        // or the first block (and a one-block bounce probe) still sees zero.
        self.compensate_delay(s, &mut work, blk);
        self.work = work;
        self.render_auxes(s, &order, blk);
        self.order = order;
        self.render_masters(s, blk);
        self.write_output(s, blk, out);
    }

    /// Silence what a seek leaves ringing, sync to the session and clear the block's accumulators.
    fn begin_block(&mut self, s: &Session, pos: Samples, frames: usize) {
        if pos != self.last_end {
            // Seek: silence held notes and ringing plugins.
            for st in self.strips.values_mut() {
                if let Some(i) = &mut st.instrument {
                    i.plugin.all_notes_off();
                }
                st.held.clear();
                st.chase = true;
            }
        }
        self.last_end = pos.saturating_add(frames as i64);
        self.ensure_synced(s);
        for b in self.busses.values_mut() {
            for c in b.buf.iter_mut() {
                c.iter_mut().take(frames).for_each(|x| *x = 0.0);
            }
        }
        for c in &mut self.main {
            c.iter_mut().take(frames).for_each(|x| *x = 0.0);
        }
    }

    /// Delay compensation for the independent strips (`work`), then route them back into the engine.
    fn compensate_delay(&mut self, s: &Session, work: &mut Vec<(usize, Strip)>, blk: Block) {
        let Block { pos, frames, .. } = blk;
        let pdc = s.edit.delay_compensation;
        if pdc {
            for t in s.tracks.iter().filter(|t| is_aux(t) && !t.inactive) {
                if let Some(st) = self.strips.get_mut(&t.id) {
                    note_insert_latency(t, st, pos);
                }
            }
        }
        // The master faders' inserts delay the whole main mix, so their latency is part of the
        // mix's too (the faders run one after the other: it adds up).
        let mut lmax_m = 0usize;
        if pdc {
            for t in s.tracks.iter().filter(|t| t.kind == TrackKind::Master && !t.inactive) {
                if let Some(st) = self.strips.get_mut(&t.id) {
                    note_insert_latency(t, st, pos);
                    lmax_m = lmax_m.saturating_add(st.latency);
                }
            }
        }
        let lmax_t = if pdc { work.iter().map(|(_, st)| st.latency).max().unwrap_or(0) } else { 0 };
        let lmax_a =
            if pdc { self.strips.iter().filter(|(id, _)| s.track(**id).is_some_and(is_aux)).map(|(_, st)| st.latency).max().unwrap_or(0) } else { 0 };
        self.track_latency = lmax_t;
        self.latency = lmax_t + lmax_a + lmax_m;
        // An aux that returns onto a bus delays it by the slowest aux return, so the tracks routed
        // to that bus wait as well (as the ones routed to the main mix do) and the bus lines up.
        self.aux_latency = lmax_a;
        self.aux_fed.clear();
        if pdc {
            for t in s.tracks.iter().filter(|t| is_aux(t) && is_live(t)) {
                if let Route::Bus(b) = &t.mixer.output
                    && !self.aux_fed.contains(b)
                {
                    self.aux_fed.push(*b);
                }
            }
        }
        for (i, st) in work.iter_mut() {
            let Some(t) = s.tracks.get(*i) else { continue };
            let ch = st.buf.len();
            st.align.set(lmax_t.saturating_sub(st.latency), ch);
            st.align.process(&mut st.buf, frames);
            if t.mixer.sends.iter().flatten().any(|x| x.pre_fader) {
                st.align_pre.set(lmax_t.saturating_sub(st.latency), ch);
                st.align_pre.process(&mut st.pre, frames);
            }
            let waits = match &t.mixer.output {
                Route::Main => true,
                Route::Bus(b) => self.aux_fed.contains(b),
                _ => false,
            };
            st.out_extra.set(if waits { lmax_a } else { 0 }, ch);
        }
        for (i, st) in work.iter_mut() {
            if let Some(t) = s.tracks.get(*i) {
                self.route_strip(t, st, pos, frames);
            }
        }
        for (i, st) in work.drain(..) {
            if let Some(t) = s.tracks.get(i) {
                self.strips.insert(t.id, st);
            }
        }
    }

    /// Phase 2: auxes in dependency order (they read busses).
    fn render_auxes(&mut self, s: &Session, order: &[usize], blk: Block) {
        let Block { pos, frames, .. } = blk;
        let pdc = s.edit.delay_compensation;
        let lmax_a = self.aux_latency;
        for t in order.iter().filter_map(|&i| s.tracks.get(i)).filter(|t| is_live(t)).filter(|t| is_aux(t)) {
            let Some(mut st) = self.strips.remove(&t.id) else { continue };
            if st.scratch.len() < self.max_block {
                st.scratch.resize(self.max_block, 0.0);
            }
            let live_in = LiveInput { input: &self.input, monitor_only: self.monitor_only, recording: self.recording };
            process_strip(s, t, &mut st, &self.busses, &live_in, blk);
            let ch = st.buf.len();
            st.align.set(if pdc { lmax_a.saturating_sub(st.latency) } else { 0 }, ch);
            st.align.process(&mut st.buf, frames);
            st.out_extra.set(0, ch);
            self.route_strip(t, &mut st, pos, frames);
            self.strips.insert(t.id, st);
        }
    }

    /// Master faders process the main mix.
    fn render_masters(&mut self, s: &Session, blk: Block) {
        let Block { pos, frames, .. } = blk;
        // Master faders process the main mix.
        let transport = transport_at(s, pos, !self.monitor_only, self.recording);
        for t in s.tracks.iter().filter(|t| t.kind == TrackKind::Master && !t.inactive) {
            if let Some(strip) = self.strips.get_mut(&t.id) {
                for (dst, src) in strip.buf.iter_mut().zip(self.main.iter()) {
                    if let (Some(d), Some(sr)) = (dst.get_mut(..frames), src.get(..frames)) {
                        d.copy_from_slice(sr);
                    }
                }
                for (i, ins) in t.mixer.inserts.iter().enumerate() {
                    if let (Some(ins), Some(Some(slot))) = (ins, strip.plugins.get_mut(i))
                        && ins.active
                        && !ins.bypass
                        && !slot.failed
                    {
                        apply_params(slot, ins, t, i, pos);
                        slot.plugin.set_transport(&transport);
                        slot.plugin.process(&mut strip.buf, frames);
                        if contain_failure(slot, &mut strip.buf, frames) {
                            strip.failures.record(i);
                        }
                    }
                }
                // Match track faders: the next block's first sample must not ramp this one.
                let last = pos.saturating_add(frames.saturating_sub(1) as i64);
                let v0 = volume_db_at(t, pos);
                let v1 = volume_db_at(t, last);
                let ramp = GainRamp::new(v0, v1, frames.saturating_sub(1));
                for (ch, src) in strip.buf.iter().enumerate().take(self.main.len()) {
                    if let Some(m) = self.main.get_mut(ch) {
                        for i in 0..frames {
                            let g = ramp.at(i);
                            if let (Some(d), Some(x)) = (m.get_mut(i), src.get(i)) {
                                *d = x * g;
                            }
                        }
                    }
                }
                let m = self.meters.entry(t.id).or_default();
                m.measure(&self.main, frames, 0.0);
                m.plugin_failures = strip.failures;
            }
        }
    }

    /// Add the metronome and copy the main mix to `out`.
    fn write_output(&mut self, s: &Session, blk: Block, out: &mut [Vec<f32>]) {
        let Block { pos, frames, .. } = blk;
        // Metronome: summed after the master fader, so it reaches the speakers without passing
        // through any track, send or insert. Only the playback engine sets `metronome`.
        // Not while monitoring only (transport stopped): the position does not advance there, so the
        // start of the same beat would repeat every block as a buzz.
        if self.metronome && !self.monitor_only && s.edit.click && (!s.edit.flag("click.only_during_record") || self.recording) {
            click::render(s, pos, frames, &mut self.main);
        }
        // Output: min(out.len(), main channels) channels; any further output channels are silent.
        for (c, o) in out.iter_mut().enumerate() {
            let src = self.main.get(c);
            for i in 0..frames {
                let v = src.and_then(|m| m.get(i)).copied().unwrap_or(0.0);
                if let Some(d) = o.get_mut(i) {
                    *d = if v.is_finite() { v } else { 0.0 };
                }
            }
        }
        self.main_meter.measure(&self.main, frames, 0.0);
    }

    /// The delay-compensation latency written onto `bus`: the slowest track insert, plus the
    /// slowest aux return when an aux returns onto this bus.
    fn bus_latency(&self, bus: BusId) -> usize {
        self.track_latency + if self.aux_fed.contains(&bus) { self.aux_latency } else { 0 }
    }

    /// Copy one rendered block of `bus` into `out`. A missing bus stays silent.
    fn copy_bus(&self, bus: BusId, frames: usize, out: &mut [Vec<f32>]) {
        for o in out.iter_mut() {
            for x in o.iter_mut().take(frames) {
                *x = 0.0;
            }
        }
        let Some(src) = self.busses.get(&bus) else { return };
        for (o, c) in out.iter_mut().zip(src.buf.iter()) {
            for i in 0..frames {
                let v = c.get(i).copied().unwrap_or(0.0);
                if let Some(d) = o.get_mut(i) {
                    *d = if v.is_finite() { v } else { 0.0 };
                }
            }
        }
    }

    /// Send, meter and pan a processed strip into the busses / main mix.
    fn route_strip(&mut self, t: &Track, strip: &mut Strip, pos: Samples, frames: usize) {
        let m = self.meters.entry(t.id).or_default();
        m.measure(&strip.buf, frames, strip.gr);
        m.plugin_failures = strip.failures;
        let src_fmt = strip_format(t, strip.buf.len());
        let law = self.pan_law;
        for (i, snd) in t.mixer.sends.iter().enumerate() {
            let Some(snd) = snd else { continue };
            if snd.mute || strip.muted {
                continue;
            }
            let lvl = send_level(t, i, snd.level_db, pos);
            let gain = db_to_gain(lvl);
            let src = if snd.pre_fader { &strip.pre } else { &strip.buf };
            let Route::Bus(b) = &snd.target else { continue };
            let Some(dst) = self.busses.get_mut(b) else { continue };
            if gain <= 0.0 {
                continue;
            }
            // Mono sends use the send's own pan; stereo sends go straight to L/R.
            let stereo = if src.len() == 1 { [snd.pan, 0.0] } else { [-1.0, 1.0] };
            let surround = snd.surround.as_ref().or(if snd.follow_main_pan { t.mixer.surround.as_ref() } else { None });
            let m = build_matrix(src_fmt, src.len(), dst.fmt, dst.buf.len(), &PanSpec { stereo, surround }, law);
            apply_matrix(src, frames, &mut dst.buf, &m, gain);
        }
        let src: &[Vec<f32>] = match &t.mixer.output {
            Route::Main | Route::Bus(_) if strip.out_extra.len > 0 => {
                if strip.outbuf.len() != strip.buf.len() {
                    strip.outbuf = vec![Vec::new(); strip.buf.len()];
                }
                for (d, src) in strip.outbuf.iter_mut().zip(strip.buf.iter()) {
                    d.clear();
                    d.extend_from_slice(src.get(..frames).unwrap_or(&[]));
                }
                strip.out_extra.process(&mut strip.outbuf, frames);
                &strip.outbuf
            }
            _ => &strip.buf,
        };
        let (dst, dst_fmt) = match &t.mixer.output {
            Route::Main => (&mut self.main, self.main_fmt),
            Route::Bus(b) => match self.busses.get_mut(b) {
                Some(d) => (&mut d.buf, d.fmt),
                None => return,
            },
            _ => return,
        };
        let mut start = 0;
        while start < frames {
            let (at, end) = pan_segment(t, pos, start, frames);
            let spec = PanSpec { stereo: [pan_at(t, 0, at), pan_at(t, 1, at)], surround: t.mixer.surround.as_ref() };
            let m = build_matrix(src_fmt, src.len(), dst_fmt, dst.len(), &spec, law);
            apply_matrix_span(src, start..end, dst, &m, 1.0);
            start = end;
        }
    }
}

std::thread_local! {
    static AUDIO_THREAD: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

fn is_live(t: &Track) -> bool {
    !(t.kind == TrackKind::Master || t.inactive || t.kind == TrackKind::Vca)
}

fn is_aux(t: &Track) -> bool {
    matches!(t.kind, TrackKind::Aux | TrackKind::Folder)
}

/// Metronome: a short synthesized blip on every beat of the tempo map, accented on beat 1 of
/// each bar. Settings come from `setup.click_countoff` (`click.*` values).
mod click;

#[cfg(test)]
mod tests;
