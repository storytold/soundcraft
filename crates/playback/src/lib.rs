//! Realtime playback through the system audio device (cpal: CoreAudio, WASAPI, ALSA/Pulse/
//! PipeWire via ALSA, JACK, WebAudio).
//!
//! The UI thread owns a [`Player`]; the audio callback owns the [`MixEngine`] and a snapshot of
//! the session (`Arc<Session>`) that the UI swaps whenever the document changes. Commands go to
//! the callback through a channel; position and meters come back through atomics and a
//! `try_lock`ed snapshot, so the callback never blocks. When no device is available the player
//! falls back to a silent clock thread so the transport, meters and automation still run.
//!
//! Surround: the device is opened with as many output channels as it offers, up to the width of
//! the session's main format. Main channels (SMPTE/WAV order) go to device channels in order; when
//! the device has fewer channels than the main mix, surround is folded down to stereo (ITU-R
//! BS.775). A mono device receives the average of the stereo channels, before output clipping.
//!
//! Plugins and threads: hosted third-party plugins (CLAP/VST3) are created, given their stored
//! state and prepared on the thread that calls [`Player::update_session`] (the UI thread), and
//! travel to the audio thread together with the session snapshot that needs them; the mix
//! engine adopts them (see `soundcraft_mix`'s crate docs). Instances the engine retires, and the
//! session snapshots the callback replaces, come back on return channels and are dropped on the UI
//! thread ([`Player::idle`]). Plugin editors are driven, and plugin states for saving read
//! ([`Player::capture_states`]), on the UI thread
//! through handles taken from each instance before it leaves ([`Player::open_editor`]).
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod devices;
pub mod record;

pub use devices::AudioConfig;

use soundcraft_dsp::{Plugin, PluginEditor, PluginStateReader};
use soundcraft_mix::{MAX_CHANNELS, MixEngine, PreparedInstance, StripMeter, itu_stereo_fold, main_channels};
use soundcraft_model::{Session, TrackId};
use soundcraft_time::{Range, Samples};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU32, Ordering};
use std::sync::mpsc::{Receiver, Sender, SyncSender, channel, sync_channel};
use std::sync::{Arc, Mutex, PoisonError};

const BLOCK: usize = 512;

/// Room for replaced snapshots on their way back; `update_session` empties it before each send.
const OLD_SESSIONS: usize = 16;

/// The realtime audio thread marker lives in the mix engine, which carries it into the strips it
/// renders on worker threads; the cpal callbacks here set it.
pub use soundcraft_mix::{mark_audio_thread, on_audio_thread};

/// Meter values for the UI: the highest peak (and gain reduction) of every block rendered since
/// the UI last took them with [`Player::take_meters`]. The audio thread renders blocks faster than
/// the UI draws (about 94 blocks a second at 512 frames and 48 kHz against 20-60 polls), so
/// keeping only the latest block would miss most transients.
#[derive(Debug, Clone, Default)]
pub struct MeterSnapshot {
    pub tracks: HashMap<TrackId, StripMeter>,
    pub main: StripMeter,
    /// Something was written since the UI last took the values.
    fresh: bool,
}

impl MeterSnapshot {
    /// Fold one rendered block's meters in (max). Allocation-free once the tracks are known.
    fn accumulate(&mut self, main: &StripMeter, tracks: &HashMap<TrackId, StripMeter>) {
        self.main.accumulate(main);
        for (k, v) in tracks {
            match self.tracks.get_mut(k) {
                Some(d) => d.accumulate(v),
                None => {
                    self.tracks.insert(*k, v.clone());
                }
            }
        }
        self.fresh = true;
    }

    /// Start a new accumulation period, keeping every entry and its storage.
    fn reset_levels(&mut self) {
        self.main.reset_levels();
        self.tracks.values_mut().for_each(StripMeter::reset_levels);
        self.fresh = false;
    }

    /// Nothing is playing or monitored: every meter reads silence. Levels are zeroed in place,
    /// so starting again does not allocate the tracks' entries anew.
    fn silence(&mut self) {
        self.reset_levels();
        self.fresh = true;
    }

    /// Drop the entries of deleted tracks.
    fn forget(&mut self, gone: &[TrackId]) {
        for id in gone {
            self.tracks.remove(id);
        }
    }
}

/// Plugin states read from the live instances: `(track, slot or INSTRUMENT_SLOT, state)`.
pub type PluginStates = Vec<(TrackId, usize, Vec<u8>)>;

/// A parameter edit made in a plugin's own editor: `(track, slot, param id, value)`.
pub type EditorEdit = (TrackId, usize, String, f32);

enum Cmd {
    /// A new snapshot plus the third-party instances it needs (created on the UI thread).
    Session(Arc<Session>, Vec<PreparedInstance>),
    /// Where the callback sends the snapshots it replaces (freeing one takes thousands of frees).
    OldSessions(SyncSender<Arc<Session>>),
    Input(Arc<record::InputRing>),
    Recording(bool),
    Play {
        from: Samples,
        end: Option<Samples>,
        looped: Option<Range>,
    },
    Stop,
    Locate(Samples),
    /// Tracks the latest session no longer has (worked out on the UI thread): their meters go.
    Forget(Vec<TrackId>),
}

struct Shared {
    position: AtomicI64,
    playing: AtomicBool,
    /// Speed × 1000.
    speed: AtomicU32,
    meters: Mutex<MeterSnapshot>,
}

/// What stopped the audio output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputTrouble {
    /// A block panicked; the output stays silent.
    Bug,
    /// The device went away, or the stream was invalidated and must be rebuilt.
    Gone,
    /// The device stopped asking for audio while playing.
    Stalled,
}

/// How the audio output is doing, for the UI.
#[derive(Default)]
struct Fault {
    /// A block panicked: output stays silent from then on.
    failed: AtomicBool,
    /// The device went away or the stream was invalidated: only a reconnect can restore audio.
    gone: AtomicBool,
    /// Blocks so far, to tell a device that stopped asking for audio.
    beats: std::sync::atomic::AtomicU64,
    /// The UI has been told.
    reported: AtomicBool,
}

impl Fault {
    /// What stopped the output, once.
    fn trouble(&self, stalled: bool) -> Option<OutputTrouble> {
        let trouble = if self.failed.load(Ordering::Relaxed) {
            OutputTrouble::Bug
        } else if self.gone.load(Ordering::Relaxed) {
            OutputTrouble::Gone
        } else if stalled {
            OutputTrouble::Stalled
        } else {
            return None;
        };
        (!self.reported.swap(true, Ordering::Relaxed)).then_some(trouble)
    }
}

/// How long a playing output may go without a block before it counts as stalled.
#[cfg(not(target_arch = "wasm32"))]
const STALL: std::time::Duration = std::time::Duration::from_secs(2);

/// True once the block count `beats` has stood still for `STALL` while playing; `seen` is the
/// count last seen and since when.
#[cfg(not(target_arch = "wasm32"))]
fn stalled(seen: &mut Option<(u64, std::time::Instant)>, beats: u64, playing: bool) -> bool {
    match *seen {
        Some((b, since)) if playing && b == beats => since.elapsed() >= STALL,
        _ => {
            *seen = Some((beats, std::time::Instant::now()));
            false
        }
    }
}

/// Fill one block with `fill`. If it panics, the block and every later one are silent and the
/// transport stops, rather than the panic taking the app down.
fn guarded_block(fault: &Fault, shared: &Shared, out: &mut [f32], fill: impl FnOnce(&mut [f32])) {
    fault.beats.fetch_add(1, Ordering::Relaxed);
    if fault.failed.load(Ordering::Relaxed) || std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| fill(out))).is_err() {
        fault.failed.store(true, Ordering::Relaxed);
        shared.playing.store(false, Ordering::Relaxed);
        out.fill(0.0);
    }
}

/// State owned by the audio callback.
struct AudioState {
    rx: Receiver<Cmd>,
    shared: Arc<Shared>,
    session: Arc<Session>,
    mix: MixEngine,
    pos: Samples,
    playing: bool,
    end: Option<Samples>,
    looped: Option<Range>,
    /// Rendered frames waiting to be consumed (planar, main channels) and the fractional read index.
    pending: Vec<Vec<f32>>,
    /// Stereo fold-down gains per main channel (used when the device has fewer channels).
    fold: [(f32, f32); MAX_CHANNELS],
    pending_len: usize,
    read: f64,
    device_rate: f64,
    input: Option<Arc<record::InputRing>>,
    recording: bool,
    /// Retired plugin instances go back to the UI thread to be dropped there.
    garbage: Sender<Vec<Box<dyn Plugin>>>,
    old_sessions: Option<SyncSender<Arc<Session>>>,
    /// Meters of blocks rendered while the UI held the shared snapshot (`try_lock` failed), so
    /// their peaks reach it with the next block instead of being lost.
    unsent: MeterSnapshot,
    /// Deleted tracks still to drop from the shared snapshot (it was locked when they went).
    forget_shared: Vec<TrackId>,
}

fn new_engine(sr: f32) -> MixEngine {
    let mut m = MixEngine::new(sr, BLOCK);
    m.set_external_instances(true);
    m.metronome = true;
    m
}

impl AudioState {
    fn new(rx: Receiver<Cmd>, shared: Arc<Shared>, session: Arc<Session>, device_rate: f64, garbage: Sender<Vec<Box<dyn Plugin>>>) -> Self {
        let sr = session.sample_rate.as_f64() as f32;
        let nch = main_channels(&session);
        AudioState {
            garbage,
            old_sessions: None,
            fold: fold_gains(&session),
            rx,
            shared,
            mix: new_engine(sr),
            session,
            pos: 0,
            playing: false,
            end: None,
            looped: None,
            pending: vec![vec![0.0; BLOCK]; nch],
            pending_len: 0,
            read: 0.0,
            device_rate,
            input: None,
            recording: false,
            unsent: MeterSnapshot::default(),
            forget_shared: Vec::new(),
        }
    }

    fn drain(&mut self) {
        let mut changed = false;
        while let Ok(c) = self.rx.try_recv() {
            match c {
                Cmd::Input(r) => self.input = Some(r),
                Cmd::Forget(gone) => {
                    self.unsent.forget(&gone);
                    if self.forget_shared.is_empty() {
                        self.forget_shared = gone;
                    } else {
                        self.forget_shared.extend_from_slice(&gone);
                    }
                    self.forget_meters();
                }
                Cmd::Recording(r) => self.recording = r,
                Cmd::Session(s, instances) => {
                    changed = true;
                    if (s.sample_rate.as_f64() - self.session.sample_rate.as_f64()).abs() > 0.5 {
                        let mut old = std::mem::replace(&mut self.mix, new_engine(s.sample_rate.as_f64() as f32));
                        let _ = self.garbage.send(old.drain_plugins());
                    }
                    self.mix.adopt(instances);
                    // The main format changed (a document edit, not steady state): resize.
                    let nch = main_channels(&s);
                    if self.pending.len() != nch {
                        self.pending = vec![vec![0.0; BLOCK]; nch];
                        self.pending_len = 0;
                        self.read = 0.0;
                    }
                    self.fold = fold_gains(&s);
                    let old = std::mem::replace(&mut self.session, s);
                    if let Some(tx) = &self.old_sessions {
                        let _ = tx.try_send(old);
                    }
                }
                Cmd::OldSessions(tx) => self.old_sessions = Some(tx),
                Cmd::Play { from, end, looped } => {
                    self.pos = from;
                    self.end = end;
                    self.looped = looped.filter(|r| r.len() > 64);
                    self.playing = true;
                    self.pending_len = 0;
                    self.read = 0.0;
                    self.mix.reset();
                }
                Cmd::Stop => {
                    self.playing = false;
                    self.end = None;
                    self.looped = None;
                    self.mix.reset();
                }
                Cmd::Locate(at) => {
                    self.pos = at;
                    self.pending_len = 0;
                    self.read = 0.0;
                    self.mix.reset();
                }
            }
        }
        if changed {
            // Place adopted instances now (not only once playback starts), so editors see them;
            // `sync` creates no third-party plugins here.
            self.mix.ensure_synced(&self.session);
        }
        self.send_garbage();
        self.shared.playing.store(self.playing, Ordering::Relaxed);
    }

    fn send_garbage(&mut self) {
        let g = self.mix.take_retired();
        if !g.is_empty() {
            let _ = self.garbage.send(g);
        }
    }

    /// Render the next block at the session rate into `pending`.
    fn render_next(&mut self, quantum: usize) {
        let mut n = quantum.clamp(1, BLOCK);
        if let Some(l) = self.looped
            && self.pos >= l.end
        {
            self.pos = l.start;
        }
        if let Some(l) = self.looped {
            n = n.min(usize::try_from((l.end - self.pos).max(1)).unwrap_or(BLOCK));
        } else if let Some(e) = self.end {
            if self.pos >= e {
                self.playing = false;
                self.end = None;
                self.shared.playing.store(false, Ordering::Relaxed);
                n = 0;
            } else {
                n = n.min(usize::try_from(e - self.pos).unwrap_or(BLOCK));
            }
        }
        if n == 0 {
            for c in &mut self.pending {
                c.iter_mut().for_each(|x| *x = 0.0);
            }
            self.pending_len = BLOCK;
            return;
        }
        if let Some(ring) = &self.input {
            let ch = ring.channels.load(Ordering::Relaxed).max(1);
            if self.mix.input.len() != ch {
                self.mix.input = vec![vec![0.0; BLOCK]; ch];
            }
            ring.pop_into(&mut self.mix.input, n, self.session.sample_rate.hz());
        }
        self.mix.monitor_only = !self.playing;
        self.mix.recording = self.recording;
        self.mix.render(&self.session, self.pos, n, &mut self.pending);
        self.send_garbage();
        self.pending_len = n;
        self.publish_meters();
        if self.playing {
            self.pos += n as i64;
            self.shared.position.store(self.pos, Ordering::Relaxed);
        }
    }

    /// Drop deleted tracks' meters from the shared snapshot, unless the UI holds it right now.
    fn forget_meters(&mut self) {
        if !self.forget_shared.is_empty()
            && let Ok(mut m) = self.shared.meters.try_lock()
        {
            m.forget(&self.forget_shared);
            self.forget_shared.clear();
        }
    }

    /// Hand this block's meters to the UI without blocking: they are folded (max) into what the
    /// UI has not taken yet. If the UI holds the snapshot right now, they wait in `unsent`.
    fn publish_meters(&mut self) {
        self.unsent.accumulate(&self.mix.main_meter, &self.mix.meters);
        self.forget_meters();
        if let Ok(mut m) = self.shared.meters.try_lock() {
            m.accumulate(&self.unsent.main, &self.unsent.tracks);
            self.unsent.reset_levels();
        }
    }

    /// Fill an interleaved device buffer.
    fn fill(&mut self, out: &mut [f32], channels: usize) {
        self.drain();
        let channels = channels.max(1);
        let monitoring = self.input.is_some() && self.session.tracks.iter().any(|t| t.mixer.input_monitor || t.mixer.record_arm);
        if !self.playing && !monitoring {
            out.iter_mut().for_each(|x| *x = 0.0);
            self.unsent.silence();
            self.forget_meters();
            if let Ok(mut m) = self.shared.meters.try_lock() {
                m.silence();
            }
            return;
        }
        let speed = f64::from(self.shared.speed.load(Ordering::Relaxed)) / 1000.0;
        let step = self.session.sample_rate.as_f64() / self.device_rate.max(1.0) * speed.clamp(0.1, 4.0);
        let callback_frames = out.len() / channels;
        let quantum = ((callback_frames as f64 * step).ceil() as usize).clamp(1, BLOCK);
        for frame in out.chunks_mut(channels) {
            while self.read >= self.pending_len as f64 {
                self.read -= self.pending_len as f64;
                if !self.playing && !monitoring {
                    break;
                }
                self.render_next(quantum);
                if self.pending_len == 0 {
                    break;
                }
            }
            let i0 = self.read.floor() as usize;
            let fr = (self.read - i0 as f64) as f32;
            let sounding = self.playing || monitoring;
            let nch = self.pending.len();
            let sample = |c: usize| {
                let ch = self.pending.get(c);
                let a = ch.and_then(|v| v.get(i0)).copied().unwrap_or(0.0);
                // `pending` is BLOCK long; only `pending_len` frames are valid. Hold the last one at the edge.
                let b = if i0 + 1 < self.pending_len { ch.and_then(|v| v.get(i0 + 1)).copied().unwrap_or(a) } else { a };
                a * (1.0 - fr) + b * fr
            };
            write_frame(frame, channels, nch, sounding, &self.fold, sample);
            self.read += step;
        }
    }
}

/// Write one device frame from `nch` rendered channels: fold surround to stereo (a mono device
/// averages the two sides), or copy the channels straight through. `sample(c)` is channel `c`.
fn write_frame(frame: &mut [f32], channels: usize, nch: usize, sounding: bool, fold: &[(f32, f32)], sample: impl Fn(usize) -> f32) {
    if channels < nch {
        // Fold surround to stereo first; a mono device averages the two sides.
        let (mut l, mut r) = (0.0f32, 0.0f32);
        if sounding {
            if nch == 2 {
                (l, r) = (sample(0), sample(1));
            } else {
                for (c, (gl, gr)) in fold.iter().enumerate().take(nch) {
                    let v = sample(c);
                    l += v * gl;
                    r += v * gr;
                }
            }
        }
        for (c, o) in frame.iter_mut().enumerate() {
            *o = match c {
                0 if channels == 1 => out_sample(0.5 * l + 0.5 * r),
                0 => out_sample(l),
                1 => out_sample(r),
                _ => 0.0,
            };
        }
    } else {
        for (c, o) in frame.iter_mut().enumerate() {
            *o = if c < nch.max(1) && sounding { out_sample(sample(c)) } else { 0.0 };
        }
    }
}

/// Clamp to the device range, mapping NaN and infinities to silence.
fn out_sample(v: f32) -> f32 {
    if v.is_finite() { v.clamp(-1.0, 1.0) } else { 0.0 }
}

/// UI-thread bookkeeping of the third-party instances the audio thread holds.
struct Side {
    /// What was last created for each slot: (plugin id, channels, created ok).
    shadow: HashMap<(TrackId, usize), (String, usize, bool)>,
    /// Session sample rate the instances were prepared for.
    rate: f32,
    /// Editor handles of the live instances.
    editors: HashMap<(TrackId, usize), Box<dyn PluginEditor>>,
    /// State readers of the live instances.
    states: BTreeMap<(TrackId, usize), Box<dyn PluginStateReader>>,
    /// Instances that refused their stored state: saves keep that state until the plugin's editor
    /// is opened.
    refused: HashSet<(TrackId, usize)>,
    garbage: Receiver<Vec<Box<dyn Plugin>>>,
    old_sessions: Option<Receiver<Arc<Session>>>,
    /// The tracks of the session last sent, to tell the audio thread which ones went.
    tracks: HashSet<TrackId>,
}

impl Side {
    fn new(rate: f32, garbage: Receiver<Vec<Box<dyn Plugin>>>) -> Side {
        Side {
            shadow: HashMap::new(),
            rate,
            editors: HashMap::new(),
            states: BTreeMap::new(),
            refused: HashSet::new(),
            garbage,
            old_sessions: None,
            tracks: HashSet::new(),
        }
    }

    /// Note `s`'s tracks; the ones the previous session had and `s` doesn't.
    fn gone_tracks(&mut self, s: &Session) -> Vec<TrackId> {
        let now: HashSet<TrackId> = s.tracks.iter().map(|t| t.id).collect();
        let gone = self.tracks.difference(&now).copied().collect();
        self.tracks = now;
        gone
    }

    /// Drops retired instances (closing their editors first, inside the plugin's drop) and
    /// replaced session snapshots.
    fn collect_garbage(&mut self) {
        while let Ok(g) = self.garbage.try_recv() {
            for p in g {
                log::debug!("plugin instance released: {}", p.info().name);
                drop(p);
            }
        }
        if let Some(rx) = &self.old_sessions {
            rx.try_iter().for_each(drop);
        }
    }

    fn return_old_sessions(&mut self, tx: &Sender<Cmd>) {
        let (otx, orx) = sync_channel(OLD_SESSIONS);
        self.old_sessions = Some(orx);
        let _ = tx.send(Cmd::OldSessions(otx));
    }

    fn forget(&mut self, key: (TrackId, usize)) {
        self.shadow.remove(&key);
        self.states.remove(&key);
        self.refused.remove(&key);
        if let Some(mut e) = self.editors.remove(&key) {
            e.close();
        }
    }
}

/// The transport and audio device.
pub struct Player {
    shared: Arc<Shared>,
    fault: Arc<Fault>,
    /// The block count last seen while playing, and since when.
    #[cfg(not(target_arch = "wasm32"))]
    watch: Mutex<Option<(u64, std::time::Instant)>>,
    tx: Sender<Cmd>,
    side: Mutex<Side>,
    #[cfg(not(target_arch = "wasm32"))]
    _stream: Option<cpal::Stream>,
    #[cfg(target_arch = "wasm32")]
    _stream: Option<cpal::Stream>,
    pub device_name: String,
    pub device_rate: u32,
    /// Output channels the device was opened with (0 for the silent clock).
    pub device_channels: u16,
    /// The device buffer in frames when one was chosen (`None`: the device default).
    pub device_buffer: Option<u32>,
    /// True when no audio device could be opened (silent clock).
    pub silent: bool,
}

impl Player {
    /// Open the default output device. Never fails: without a device it runs a silent clock.
    pub fn new(session: Arc<Session>) -> Player {
        Player::with_config(session, &AudioConfig::default())
    }

    /// Open the output device `cfg` chooses (system, device, rate, buffer size), falling back to the
    /// defaults for whatever is unavailable. Never fails: without a device it runs a silent clock.
    pub fn with_config(session: Arc<Session>, cfg: &AudioConfig) -> Player {
        let p = Player::open(Arc::clone(&session), cfg);
        p.side().return_old_sessions(&p.tx);
        // Hand the initial session's third-party plugins over.
        p.update_session(session);
        p
    }

    /// What stopped the audio output, once: a bug in the engine, or the device went away or
    /// stopped asking for audio while playing.
    pub fn trouble(&self) -> Option<OutputTrouble> {
        #[cfg(not(target_arch = "wasm32"))]
        let stalled = {
            let playing = self.shared.playing.load(Ordering::Relaxed) && !self.silent;
            stalled(&mut self.watch.lock().unwrap_or_else(PoisonError::into_inner), self.fault.beats.load(Ordering::Relaxed), playing)
        };
        #[cfg(target_arch = "wasm32")]
        let stalled = false;
        self.fault.trouble(stalled)
    }

    fn side(&self) -> std::sync::MutexGuard<'_, Side> {
        self.side.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Open the output device for `session`, trying the requested buffer size before the device default, then build
    /// the shared player state. If no configuration starts or the device cannot open, fall back to a silent clock at
    /// the session rate.
    fn open(session: Arc<Session>, cfg: &AudioConfig) -> Player {
        let shared = Arc::new(Shared {
            position: AtomicI64::new(0),
            playing: AtomicBool::new(false),
            speed: AtomicU32::new(1000),
            meters: Mutex::new(MeterSnapshot::default()),
        });
        let want = session.sample_rate.hz();
        let want_ch = u16::try_from(main_channels(&session)).unwrap_or(2);
        let side_rate = session.sample_rate.as_f64() as f32;
        let new_side = |garbage: Receiver<Vec<Box<dyn Plugin>>>| {
            let mut side = Side::new(side_rate, garbage);
            side.gone_tracks(&session);
            Mutex::new(side)
        };
        match open_device(want, want_ch, cfg) {
            Ok((device, config, format, name)) => {
                // The chosen buffer size first; if the device refuses it, its default.
                let mut attempts = vec![config];
                if config.buffer_size != cpal::BufferSize::Default {
                    attempts.push(cpal::StreamConfig { buffer_size: cpal::BufferSize::Default, ..config });
                }
                let opened = first_started(
                    attempts,
                    |config: &cpal::StreamConfig| -> Result<Opened, cpal::Error> {
                        let rate = config.sample_rate;
                        let channels = usize::from(config.channels);
                        let (tx, rx) = channel();
                        let (gtx, grx) = channel();
                        let state = AudioState::new(rx, Arc::clone(&shared), Arc::clone(&session), f64::from(rate), gtx);
                        let fault = Arc::new(Fault::default());
                        let stream = match format {
                            cpal::SampleFormat::I32 => build_output::<i32>(&device, *config, state, channels, &fault, &shared),
                            cpal::SampleFormat::I16 => build_output::<i16>(&device, *config, state, channels, &fault, &shared),
                            _ => build_output::<f32>(&device, *config, state, channels, &fault, &shared),
                        }?;
                        Ok(Opened { stream, tx, grx, fault })
                    },
                    |opened| start_stream(&opened.stream),
                );
                match opened {
                    Some((config, Opened { stream, tx, grx, fault })) => {
                        let rate = config.sample_rate;
                        let channels = usize::from(config.channels);
                        let buffer = match config.buffer_size {
                            cpal::BufferSize::Fixed(n) => Some(n),
                            cpal::BufferSize::Default => None,
                        };
                        log::info!(
                            "audio device: {name} @ {rate} Hz, {channels} ch, {format}, buffer {}",
                            buffer.map_or("default".to_string(), |n| n.to_string())
                        );
                        Player {
                            shared,
                            fault,
                            #[cfg(not(target_arch = "wasm32"))]
                            watch: Mutex::new(None),
                            tx,
                            side: new_side(grx),
                            _stream: Some(stream),
                            device_name: name,
                            device_rate: rate,
                            device_channels: config.channels,
                            device_buffer: buffer,
                            silent: false,
                        }
                    }
                    None => {
                        log::warn!("no usable audio stream; using a silent clock");
                        let (tx, rx) = channel();
                        let (gtx, grx) = channel();
                        Player::silent_clock(shared, tx, rx, want, new_side(grx), gtx)
                    }
                }
            }
            Err(e) => {
                log::warn!("no audio output: {e}; using a silent clock");
                let (tx, rx) = channel();
                let (gtx, grx) = channel();
                Player::silent_clock_with(shared, tx, rx, Arc::clone(&session), new_side(grx), gtx)
            }
        }
    }

    fn silent_clock(
        shared: Arc<Shared>,
        tx: Sender<Cmd>,
        rx: Receiver<Cmd>,
        rate: u32,
        side: Mutex<Side>,
        gtx: Sender<Vec<Box<dyn Plugin>>>,
    ) -> Player {
        let s = Arc::new(Session::new("Untitled", soundcraft_time::SampleRate::new(rate).unwrap_or_default()));
        Player::silent_clock_with(shared, tx, rx, s, side, gtx)
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn silent_clock_with(
        shared: Arc<Shared>,
        tx: Sender<Cmd>,
        rx: Receiver<Cmd>,
        session: Arc<Session>,
        side: Mutex<Side>,
        gtx: Sender<Vec<Box<dyn Plugin>>>,
    ) -> Player {
        let rate = session.sample_rate.hz();
        let mut state = AudioState::new(rx, Arc::clone(&shared), session, f64::from(rate), gtx);
        let fault = Arc::new(Fault::default());
        let failed = Arc::clone(&fault);
        let spawn = std::thread::Builder::new().name("soundcraft-clock".into()).spawn(move || {
            let mut buf = vec![0.0f32; 4096];
            let mut last = std::time::Instant::now();
            loop {
                std::thread::sleep(std::time::Duration::from_millis(10));
                let now = std::time::Instant::now();
                let frames = ((now - last).as_secs_f64() * state.device_rate) as usize;
                last = now;
                let n = (frames * 2).min(buf.len());
                if let Some(b) = buf.get_mut(..n) {
                    let shared = Arc::clone(&state.shared);
                    guarded_block(&failed, &shared, b, |b| state.fill(b, 2));
                }
                if Arc::strong_count(&state.shared) <= 1 {
                    break;
                }
            }
        });
        if let Err(e) = spawn {
            log::warn!("clock thread failed: {e}");
        }
        Player {
            shared,
            fault,
            watch: Mutex::new(None),
            tx,
            side,
            _stream: None,
            device_name: "No audio device (silent)".into(),
            device_rate: rate,
            device_channels: 0,
            device_buffer: None,
            silent: true,
        }
    }

    #[cfg(target_arch = "wasm32")]
    fn silent_clock_with(
        shared: Arc<Shared>,
        tx: Sender<Cmd>,
        _rx: Receiver<Cmd>,
        session: Arc<Session>,
        side: Mutex<Side>,
        _gtx: Sender<Vec<Box<dyn Plugin>>>,
    ) -> Player {
        Player {
            shared,
            fault: Arc::default(),
            tx,
            side,
            _stream: None,
            device_name: "No audio device".into(),
            device_rate: session.sample_rate.hz(),
            device_channels: 0,
            device_buffer: None,
            silent: true,
        }
    }

    /// Sends a new session snapshot to the audio thread, with freshly created (state-restored,
    /// prepared) instances of any third-party plugin it newly needs. Call on the UI thread.
    pub fn update_session(&self, s: Arc<Session>) {
        let (instances, gone) = {
            let mut side = self.side();
            (side.prepare(&s), side.gone_tracks(&s))
        };
        let _ = self.tx.send(Cmd::Session(s, instances));
        if !gone.is_empty() {
            let _ = self.tx.send(Cmd::Forget(gone));
        }
    }

    /// Reads the state of every live third-party plugin, for saving the session.
    pub fn capture_states(&self) -> PluginStates {
        self.side().capture_states()
    }

    /// The plugins that said their state changed since the last call, so the session can keep
    /// changes made only in plugin editors.
    pub fn changed_states(&self) -> Vec<(TrackId, usize)> {
        self.side().changed_states()
    }

    /// Reads the state of the plugins in `slots`.
    pub fn capture_states_of(&self, slots: &[(TrackId, usize)]) -> PluginStates {
        self.side().capture_states_of(slots)
    }

    /// Per-UI-frame housekeeping: drops retired instances and services plugin editors. Returns
    /// the parameter edits made in open editors since the last call.
    pub fn idle(&self) -> Vec<EditorEdit> {
        let mut side = self.side();
        side.collect_garbage();
        let mut out = Vec::new();
        for (&(t, slot), e) in side.editors.iter_mut() {
            for (id, v) in e.idle() {
                out.push((t, slot, id, v));
            }
        }
        out
    }

    /// True when the plugin in this slot has its own editor.
    pub fn has_editor(&self, track: TrackId, slot: usize) -> bool {
        self.side().editors.contains_key(&(track, slot))
    }

    /// Opens the plugin's own editor window. Main thread only.
    pub fn open_editor(&self, track: TrackId, slot: usize) -> Result<(), String> {
        self.side().open_editor((track, slot))
    }

    pub fn close_editor(&self, track: TrackId, slot: usize) {
        if let Some(e) = self.side().editors.get_mut(&(track, slot)) {
            e.close();
        }
    }

    /// Closes every plugin editor window (main thread). Call before replacing the player: its
    /// instances may be dropped on the audio thread, where editor windows cannot be closed.
    pub fn close_editors(&self) {
        for e in self.side().editors.values_mut() {
            e.close();
        }
    }

    pub fn editor_open(&self, track: TrackId, slot: usize) -> bool {
        self.side().editors.get(&(track, slot)).is_some_and(|e| e.is_open())
    }

    pub fn any_editor_open(&self) -> bool {
        self.side().editors.values().any(|e| e.is_open())
    }

    /// Shows a parameter change made elsewhere in the plugin's open editor.
    pub fn editor_set_param(&self, track: TrackId, slot: usize, id: &str, value: f32) {
        if let Some(e) = self.side().editors.get_mut(&(track, slot)) {
            e.set_param(id, value);
        }
    }

    /// The plugin's own text for a parameter value when SoundCraft's format would be wrong for it
    /// (see `PluginEditor::value_text`). Main thread only.
    pub fn editor_value_text(&self, track: TrackId, slot: usize, id: &str, value: f32) -> Option<String> {
        self.side().editors.get_mut(&(track, slot))?.value_text(id, value)
    }

    /// Start playback at `from`, stopping at `end`, or looping `looped`.
    pub fn play(&self, from: Samples, end: Option<Samples>, looped: Option<Range>) {
        self.shared.position.store(from, Ordering::Relaxed);
        self.shared.playing.store(true, Ordering::Relaxed);
        let _ = self.tx.send(Cmd::Play { from, end, looped });
    }

    /// Feed live input (from a `record::Recorder`) into the mix for input monitoring.
    pub fn set_input(&self, ring: Arc<record::InputRing>) {
        let _ = self.tx.send(Cmd::Input(ring));
    }

    pub fn set_recording(&self, on: bool) {
        let _ = self.tx.send(Cmd::Recording(on));
    }

    pub fn stop(&self) {
        self.shared.playing.store(false, Ordering::Relaxed);
        let _ = self.tx.send(Cmd::Stop);
    }

    pub fn locate(&self, at: Samples) {
        self.shared.position.store(at, Ordering::Relaxed);
        let _ = self.tx.send(Cmd::Locate(at));
    }

    pub fn set_speed(&self, speed: f32) {
        let v = if speed.is_finite() { (speed.clamp(0.1, 4.0) * 1000.0) as u32 } else { 1000 };
        self.shared.speed.store(v, Ordering::Relaxed);
    }

    /// Playback rate last requested, 1.0 at full speed.
    pub fn speed(&self) -> f32 {
        self.shared.speed.load(Ordering::Relaxed) as f32 / 1000.0
    }

    pub fn position(&self) -> Samples {
        self.shared.position.load(Ordering::Relaxed)
    }

    pub fn is_playing(&self) -> bool {
        self.shared.playing.load(Ordering::Relaxed)
    }

    /// A copy of the meters accumulated since the last [`Player::take_meters`], leaving them in
    /// place (for inspection; the UI takes them instead).
    pub fn meters(&self) -> MeterSnapshot {
        self.shared.meters.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    /// Take the meters accumulated since the last call into reusable UI storage and start a new
    /// period: `out` gets the highest peak of every block rendered in between. If no block was
    /// rendered since (a large device buffer renders several blocks per callback), `out` keeps
    /// the previous values rather than dropping to silence. Call it from one place only (two
    /// readers would split the peaks between them). Once track/channel counts are stable this
    /// performs no allocation. The audio thread only ever `try_lock`s, so holding the lock here
    /// never blocks it; blocks rendered meanwhile reach the next call.
    pub fn take_meters(&self, out: &mut MeterSnapshot) {
        let mut source = self.shared.meters.lock().unwrap_or_else(PoisonError::into_inner);
        take_meter_snapshot(&mut source, out);
    }
}

fn take_meter_snapshot(source: &mut MeterSnapshot, out: &mut MeterSnapshot) {
    if !source.fresh {
        // No block since the last take: keep the readings, but not those of deleted tracks.
        out.tracks.retain(|id, _| source.tracks.contains_key(id));
        return;
    }
    copy_meter_snapshot(source, out);
    source.reset_levels();
}

fn copy_meter_snapshot(source: &MeterSnapshot, out: &mut MeterSnapshot) {
    out.tracks.retain(|id, _| source.tracks.contains_key(id));
    for (id, meter) in &source.tracks {
        out.tracks.entry(*id).or_default().copy_from(meter);
    }
    out.main.copy_from(&source.main);
}

impl Side {
    fn capture_states(&mut self) -> PluginStates {
        self.states.iter_mut().filter(|(k, _)| !self.refused.contains(*k)).filter_map(|(&(t, slot), r)| Some((t, slot, r.save_state()?))).collect()
    }

    fn changed_states(&mut self) -> Vec<(TrackId, usize)> {
        self.editors.iter_mut().filter_map(|(k, e)| e.state_changed().then_some(*k)).collect()
    }

    fn capture_states_of(&mut self, slots: &[(TrackId, usize)]) -> PluginStates {
        self.states
            .iter_mut()
            .filter(|(k, _)| !self.refused.contains(*k) && slots.contains(k))
            .filter_map(|(&(t, slot), r)| Some((t, slot, r.save_state()?)))
            .collect()
    }

    /// A plugin that marks its state changed while it loads or restores it hasn't changed.
    fn add_editor(&mut self, key: (TrackId, usize), mut e: Box<dyn PluginEditor>) {
        let _ = e.state_changed();
        self.editors.insert(key, e);
    }

    fn open_editor(&mut self, key: (TrackId, usize)) -> Result<(), String> {
        let e = self.editors.get_mut(&key).ok_or_else(|| "this plugin has no editor (or it is not loaded)".to_string())?;
        e.open()?;
        self.refused.remove(&key);
        Ok(())
    }

    /// Creates the third-party instances `s` needs that the audio thread does not have yet.
    fn prepare(&mut self, s: &Session) -> Vec<PreparedInstance> {
        self.collect_garbage();
        let sr = s.sample_rate.as_f64() as f32;
        if (sr - self.rate).abs() > 0.5 {
            // The audio thread rebuilds its engine: everything is created anew.
            let keys: Vec<(TrackId, usize)> = self.shadow.keys().copied().collect();
            for k in keys {
                self.forget(k);
            }
            self.rate = sr;
        }
        let specs = soundcraft_mix::instance_specs(s);
        let stale: Vec<(TrackId, usize)> = self.shadow.keys().filter(|k| !specs.iter().any(|x| (x.track, x.slot) == **k)).copied().collect();
        for k in stale {
            self.forget(k);
        }
        let mut out = Vec::new();
        for spec in specs {
            let key = (spec.track, spec.slot);
            if self.shadow.get(&key).is_some_and(|(id, ch, _)| *id == spec.id && *ch == spec.ch) {
                continue;
            }
            self.forget(key);
            let made = soundcraft_mix::create_instance(&spec.id, spec.state.as_deref(), sr, BLOCK, spec.ch);
            log::debug!("plugin instance for {:?} slot {}: {} ({} ch, created: {})", spec.track, spec.slot, spec.id, spec.ch, made.is_some());
            self.shadow.insert(key, (spec.id.clone(), spec.ch, made.is_some()));
            if let Some(soundcraft_mix::NewInstance { mut plugin, refused_state }) = made {
                if let Some(e) = plugin.editor() {
                    self.add_editor(key, e);
                }
                if let Some(r) = plugin.state_reader() {
                    self.states.insert(key, r);
                }
                if refused_state {
                    self.refused.insert(key);
                }
                out.push(PreparedInstance { track: spec.track, slot: spec.slot, id: spec.id, ch: spec.ch, plugin });
            }
        }
        out
    }
}

/// ITU stereo fold-down gains for each main channel of a session.
fn fold_gains(s: &Session) -> [(f32, f32); MAX_CHANNELS] {
    let mut g = [(0.0, 0.0); MAX_CHANNELS];
    let f = s.main_format();
    let sp = f.speakers();
    if sp.len() == f.channels() {
        for (o, x) in g.iter_mut().zip(sp.iter()) {
            *o = itu_stereo_fold(*x);
        }
    } else {
        // Ambisonics: W only, at -3 dB per side.
        g[0] = (std::f32::consts::FRAC_1_SQRT_2, std::f32::consts::FRAC_1_SQRT_2);
    }
    g
}

/// Start a freshly built output stream. `false` when the device refuses to start it, so the caller
/// treats it like a stream that failed to build and tries the next configuration.
fn start_stream(stream: &impl cpal::traits::StreamTrait) -> bool {
    match stream.play() {
        Ok(()) => true,
        Err(e) => {
            log::warn!("audio stream failed to start: {e}");
            false
        }
    }
}

/// A built output stream and the channels and fault flag its callback shares with the player.
struct Opened {
    stream: cpal::Stream,
    tx: Sender<Cmd>,
    grx: Receiver<Vec<Box<dyn Plugin>>>,
    fault: Arc<Fault>,
}

/// Build and start the attempts in order; the first that builds and starts wins. A build error or
/// a stream that will not start is logged, and the next attempt is tried. `None` when none works.
fn first_started<A: std::fmt::Debug, S, E: std::fmt::Display>(
    attempts: impl IntoIterator<Item = A>,
    mut build: impl FnMut(&A) -> Result<S, E>,
    start: impl Fn(&S) -> bool,
) -> Option<(A, S)> {
    for attempt in attempts {
        match build(&attempt) {
            Ok(built) if start(&built) => return Some((attempt, built)),
            // `start` has already logged why the stream would not start.
            Ok(_) => {}
            Err(e) => log::warn!("cannot open audio stream ({attempt:?}): {e}"),
        }
    }
    None
}

/// The output stream: the mix renders f32 into a scratch buffer allocated here (so the callback
/// never allocates), converted to the device's sample type.
fn build_output<T>(
    device: &cpal::Device,
    config: cpal::StreamConfig,
    mut state: AudioState,
    channels: usize,
    fault: &Arc<Fault>,
    shared: &Arc<Shared>,
) -> Result<cpal::Stream, cpal::Error>
where
    T: cpal::SizedSample + cpal::FromSample<f32>,
{
    use cpal::traits::DeviceTrait;
    let frame = channels.max(1);
    let mut scratch = vec![0.0f32; 8192 * frame];
    let chunk = scratch.len();
    let (failed, guard, gone) = (Arc::clone(fault), Arc::clone(shared), Arc::clone(fault));
    device.build_output_stream(
        config,
        move |data: &mut [T], _: &cpal::OutputCallbackInfo| {
            mark_audio_thread();
            for out in data.chunks_mut(chunk) {
                let Some(s) = scratch.get_mut(..out.len()) else { return };
                guarded_block(&failed, &guard, s, |d| state.fill(d, channels));
                for (o, x) in out.iter_mut().zip(s.iter()) {
                    *o = T::from_sample(*x);
                }
            }
        },
        // cpal may call this on the audio thread (on ALSA in a loop while the device stays
        // broken): marked, so the logger hands the record off instead of writing it.
        move |e: cpal::Error| {
            mark_audio_thread();
            // cpal 0.18 uses one error type for every operation. A device that went away, or a
            // stream that must be rebuilt (`StreamInvalidated`), stops playback until the user
            // reconnects; a rerouted default device (`DeviceChanged`) keeps playing on the new
            // route, and an xrun is transient, so neither counts as losing the stream.
            if devices::stream_lost(&e) {
                gone.gone.store(true, Ordering::Relaxed);
            }
            log::warn!("audio stream error: {e}");
        },
        None,
    )
}

/// Open the output device `cfg` chooses (else the default) at the chosen rate (else the session
/// rate) if possible, in the best sample format it offers (f32, else 32- or 16-bit integers),
/// with as many channels as it offers up to `want_ch` (at least stereo when available), and the
/// chosen buffer size when the device allows it.
fn open_device(want_rate: u32, want_ch: u16, cfg: &AudioConfig) -> Result<(cpal::Device, cpal::StreamConfig, cpal::SampleFormat, String), String> {
    use cpal::traits::DeviceTrait;
    let host = devices::host(cfg);
    let device = devices::device(&host, cfg.output.as_deref(), true).ok_or_else(|| "no output device".to_string())?;
    let name = devices::device_name(&device).unwrap_or_else(|| "Audio device".into());
    // Prefer a config at the chosen (else session) rate; else the device default (we resample).
    let want_ch = want_ch.max(2);
    let rates: Vec<u32> = cfg.sample_rate.into_iter().chain(Some(want_rate)).collect();
    let configs: Vec<cpal::SupportedStreamConfigRange> = device.supported_output_configs().map(|configs| configs.collect()).unwrap_or_default();
    let (config, format) = match pick_output_config(&configs, &rates, want_ch) {
        Some((mut c, buffers, format)) => {
            c.buffer_size = devices::buffer_size(cfg.buffer_size, &buffers);
            (c, format)
        }
        None => {
            let d = device.default_output_config().map_err(|e| e.to_string())?;
            let format = d.sample_format();
            // cpal 0.18's default can name a format these callbacks do not decode (I24, F64);
            // open the same rate and channels in a format they do, rather than lose output.
            let c = if devices::format_rank(format).is_some() {
                d
            } else {
                // Same rate and channels first; else any usable channel count at the default
                // rate; else the session rate (the mix resamples and folds channels). A device
                // whose only decoded format is wider or at another rate would otherwise open no
                // stream at all.
                devices::supported_format_config(&configs, d.sample_rate(), d.channels())
                    .or_else(|| devices::supported_format_config(&configs, want_rate, want_ch))
                    .ok_or_else(|| format!("unsupported device sample format {format:?}"))?
            };
            let mut stream_config = c.config();
            stream_config.buffer_size = devices::buffer_size(cfg.buffer_size, c.buffer_size());
            (stream_config, c.sample_format())
        }
    };
    Ok((device, config, format, name))
}

/// Pick the output configuration for the first requested rate the device offers (the configured
/// rate first, then the session rate), preferring the best sample format and then the most
/// channels from stereo up to `want_ch`. `None` when the device offers none of the requested
/// rates in a usable format; the caller then uses the device's default output configuration,
/// which the mix resamples to the session rate.
fn pick_output_config(
    configs: &[cpal::SupportedStreamConfigRange],
    rates: &[u32],
    want_ch: u16,
) -> Option<(cpal::StreamConfig, cpal::SupportedBufferSize, cpal::SampleFormat)> {
    for &rate in rates {
        let mut best: Option<(cpal::StreamConfig, cpal::SupportedBufferSize, cpal::SampleFormat, usize, u16)> = None;
        for c in configs {
            let Some(rank) = devices::format_rank(c.sample_format()) else { continue };
            if !c.contains_rate(rate) || c.channels() < 2 || c.channels() > want_ch {
                continue;
            }
            let Some(config) = c.try_with_sample_rate(rate) else { continue };
            // Better: a preferred format first, then more channels (ties keep the earlier range).
            let better = (rank, std::cmp::Reverse(c.channels()));
            if best.as_ref().is_none_or(|x| better < (x.3, std::cmp::Reverse(x.4))) {
                best = Some((config.config(), *c.buffer_size(), c.sample_format(), rank, c.channels()));
            }
        }
        if let Some((config, buffers, format, _, _)) = best {
            return Some((config, buffers, format));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A stream that builds; `starts` says whether the device lets it play (it can refuse after a
    /// reconnect).
    struct Fake {
        starts: bool,
    }

    impl cpal::traits::StreamTrait for Fake {
        /// Play reports the device's refusal when `starts` is false.
        fn play(&self) -> Result<(), cpal::Error> {
            if self.starts { Ok(()) } else { Err(cpal::Error::new(cpal::ErrorKind::DeviceNotAvailable)) }
        }
        /// Pause always succeeds on the fake stream.
        fn pause(&self) -> Result<(), cpal::Error> {
            Ok(())
        }
        /// The fake stream has no clock and no fixed buffer; neither is used by these tests.
        fn buffer_size(&self) -> Result<cpal::FrameCount, cpal::Error> {
            Ok(0)
        }
        /// The fake stream has no usable clock; these tests do not read it.
        fn now(&self) -> cpal::StreamInstant {
            cpal::StreamInstant::from_nanos(0)
        }
    }

    #[test]
    fn a_stream_that_will_not_start_is_reported_as_failed() {
        assert!(!start_stream(&Fake { starts: false }));
        assert!(start_stream(&Fake { starts: true }));
    }

    #[test]
    fn the_first_config_that_starts_wins_and_later_ones_are_not_tried() {
        let mut tried = Vec::new();
        let won = first_started(
            [1u32, 2, 3],
            |&n| {
                tried.push(n);
                Ok::<_, String>(Fake { starts: n != 1 })
            },
            start_stream,
        );
        assert_eq!(won.map(|(n, _)| n), Some(2));
        assert_eq!(tried, vec![1, 2]);
    }

    #[test]
    fn a_config_that_fails_to_build_falls_through_to_the_next() {
        let won =
            first_started([1u32, 2], |&n| if n == 1 { Err("no such buffer size".to_string()) } else { Ok(Fake { starts: true }) }, start_stream);
        assert_eq!(won.map(|(n, _)| n), Some(2));
    }

    /// Build a cpal configuration range for tests with a 64..1024-frame buffer.
    fn range(channels: u16, min: u32, max: u32, format: cpal::SampleFormat) -> cpal::SupportedStreamConfigRange {
        cpal::SupportedStreamConfigRange::new(channels, min, max, cpal::SupportedBufferSize::Range { min: 64, max: 1024 }, format)
    }

    /// Rates are tried in order, so a stale configured rate falls through to the session rate; when no requested rate
    /// is offered the caller uses the device default and resamples.
    #[test]
    fn output_config_uses_the_first_requested_rate_the_device_offers() {
        let configs = [
            range(2, 44_100, 48_000, cpal::SampleFormat::I16),
            range(2, 44_100, 96_000, cpal::SampleFormat::F32),
            range(8, 44_100, 96_000, cpal::SampleFormat::F32),
        ];
        let (config, buffers, format) = pick_output_config(&configs, &[96_000, 48_000], 8).unwrap();
        assert_eq!((config.sample_rate, config.channels, config.buffer_size), (96_000, 8, cpal::BufferSize::Default));
        assert_eq!(format, cpal::SampleFormat::F32);
        assert_eq!(buffers, cpal::SupportedBufferSize::Range { min: 64, max: 1024 });
        // 192 kHz is not offered: the session's 48 kHz is tried next.
        let (config, _, format) = pick_output_config(&configs, &[192_000, 48_000], 8).unwrap();
        assert_eq!((config.sample_rate, config.channels, format), (48_000, 8, cpal::SampleFormat::F32));
        // No requested rate is offered: the caller uses the device's default config and resamples.
        assert!(pick_output_config(&configs, &[192_000], 8).is_none());
    }

    /// At a supported rate, a preferred sample format wins over more channels; output stays between stereo and the
    /// requested channel cap, and mono-only or wider devices take the default path.
    #[test]
    fn output_config_prefers_format_over_channels_and_needs_stereo_within_the_cap() {
        let configs = [range(8, 44_100, 48_000, cpal::SampleFormat::I16), range(2, 44_100, 48_000, cpal::SampleFormat::F32)];
        let (config, _, format) = pick_output_config(&configs, &[48_000], 8).unwrap();
        assert_eq!((config.channels, format), (2, cpal::SampleFormat::F32), "a preferred format wins over more channels");
        let (config, _, _) = pick_output_config(&configs, &[48_000], 2).unwrap();
        assert_eq!(config.channels, 2, "the channel cap applies");
        let mono = [range(1, 44_100, 48_000, cpal::SampleFormat::F32)];
        assert!(pick_output_config(&mono, &[48_000], 8).is_none(), "a mono-only device takes the default config path");
        let surround = [range(6, 44_100, 48_000, cpal::SampleFormat::F32)];
        assert!(pick_output_config(&surround, &[48_000], 2).is_none(), "a wider device is not narrowed here");
        assert_eq!(pick_output_config(&surround, &[48_000], 6).map(|(c, _, _)| c.channels), Some(6));
    }

    #[test]
    fn no_config_that_starts_yields_none_so_the_caller_falls_back_to_a_silent_clock() {
        let mut tried = 0;
        let won = first_started(
            [1u32, 2],
            |_| {
                tried += 1;
                Ok::<_, String>(Fake { starts: false })
            },
            start_stream,
        );
        assert!(won.is_none());
        assert_eq!(tried, 2);
    }

    #[test]
    fn reusable_meter_snapshot_updates_and_drops_stale_tracks() {
        let keep = TrackId(1);
        let stale = TrackId(2);
        let mut source = MeterSnapshot::default();
        source.tracks.insert(keep, StripMeter { peak: [0.5, 0.25], peaks: vec![0.5, 0.25], gain_reduction_db: 3.0, ..StripMeter::default() });
        source.main.peaks = vec![0.75, 0.5];
        let mut out = MeterSnapshot::default();
        out.tracks.insert(stale, StripMeter::default());
        copy_meter_snapshot(&source, &mut out);
        assert_eq!(out.tracks.len(), 1);
        assert_eq!(out.tracks[&keep], source.tracks[&keep]);
        assert_eq!(out.main, source.main);
    }

    /// A mono track playing a quiet signal with one short spike in the block starting at
    /// `spike_at`, and an audio thread ready to play it.
    fn spike_session(spike_at: usize) -> (Arc<Shared>, AudioState, std::sync::mpsc::Sender<Cmd>, TrackId) {
        use soundcraft_model::{ChannelFormat, Clip, SourceAudio, SourceId, TrackKind};
        let mut s = soundcraft_model::Session::default();
        let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, None);
        let mut x = vec![0.01f32; 48_000];
        x[spike_at..spike_at + 32].fill(0.9);
        s.pool.insert(SourceId(5), Arc::new(SourceAudio::new(soundcraft_audio_io::AudioBuffer { sample_rate: 48_000, channels: vec![x] })));
        let id = s.new_clip_id();
        s.track_mut(t).unwrap().playlist_mut().unwrap().clips.push(Clip::audio(id, "x", SourceId(5), 0, 0, 48_000));
        let shared = Arc::new(Shared {
            position: AtomicI64::new(0),
            playing: AtomicBool::new(false),
            speed: AtomicU32::new(1000),
            meters: Mutex::new(MeterSnapshot::default()),
        });
        let (tx, rx) = channel();
        let st = AudioState::new(rx, Arc::clone(&shared), Arc::new(s), 48_000.0, channel().0);
        tx.send(Cmd::Play { from: 0, end: None, looped: None }).unwrap();
        (shared, st, tx, t)
    }

    #[test]
    fn a_spike_in_one_block_among_quiet_ones_reaches_the_ui() {
        // Eight 512-frame blocks per callback, as between two UI polls; the spike is in the 3rd.
        let (shared, mut st, _tx, t) = spike_session(2 * 512 + 100);
        let mut out = vec![0.0f32; 2 * 8 * 512];
        st.fill(&mut out, 2);
        // The last block alone (what the UI used to see) is quiet.
        assert!(st.mix.meters[&t].peak[0] < 0.05);
        let mut ui = MeterSnapshot::default();
        take_meter_snapshot(&mut shared.meters.lock().unwrap(), &mut ui);
        assert!(ui.tracks[&t].peak[0] > 0.85, "{:?}", ui.tracks[&t]);
        assert!(ui.tracks[&t].peaks[0] > 0.85);
        assert!(ui.main.peak[0] > 0.5 && ui.main.peaks.iter().all(|p| *p > 0.5), "{:?}", ui.main);
        // Taking starts a new period: the next quiet blocks read quiet.
        st.fill(&mut out, 2);
        take_meter_snapshot(&mut shared.meters.lock().unwrap(), &mut ui);
        assert!(ui.tracks[&t].peak[0] < 0.05 && ui.main.peak[0] < 0.05);
        // No block since the last take: the reading stays rather than dropping to silence.
        take_meter_snapshot(&mut shared.meters.lock().unwrap(), &mut ui);
        assert!(ui.tracks[&t].peak[0] > 0.005);
    }

    #[test]
    fn a_spike_rendered_while_the_ui_holds_the_meters_arrives_later() {
        let (shared, mut st, _tx, t) = spike_session(100);
        let mut out = vec![0.0f32; 2 * 512];
        {
            let _held = shared.meters.lock().unwrap();
            st.fill(&mut out, 2);
        }
        st.fill(&mut out, 2);
        let mut ui = MeterSnapshot::default();
        take_meter_snapshot(&mut shared.meters.lock().unwrap(), &mut ui);
        assert!(ui.tracks[&t].peak[0] > 0.85, "{:?}", ui.tracks[&t]);
    }

    #[test]
    fn stopping_zeroes_meters_in_place_and_removed_tracks_are_pruned() {
        let (shared, mut st, tx, t) = spike_session(100);
        let mut out = vec![0.0f32; 2 * 512];
        st.fill(&mut out, 2);
        tx.send(Cmd::Stop).unwrap();
        st.fill(&mut out, 2);
        {
            let m = shared.meters.lock().unwrap();
            let e = m.tracks.get(&t).expect("kept while stopped, so playing again does not allocate it");
            assert!(e.peak == [0.0; 2] && e.peaks.iter().all(|p| *p == 0.0) && e.peaks.len() == 1, "{e:?}");
            assert!(m.fresh, "the UI sees the silence");
        }
        // The track is deleted: the UI side works out which tracks went and says so.
        let mut side = Side::new(48_000.0, channel().1);
        side.gone_tracks(&st.session);
        let empty = soundcraft_model::Session::default();
        let gone = side.gone_tracks(&empty);
        assert_eq!(gone, vec![t]);
        tx.send(Cmd::Session(Arc::new(empty), Vec::new())).unwrap();
        tx.send(Cmd::Forget(gone)).unwrap();
        tx.send(Cmd::Play { from: 0, end: None, looped: None }).unwrap();
        st.fill(&mut out, 2);
        assert!(!shared.meters.lock().unwrap().tracks.contains_key(&t));
        assert!(!st.unsent.tracks.contains_key(&t) && !st.mix.meters.contains_key(&t));
    }

    #[test]
    fn a_take_without_new_blocks_still_drops_deleted_tracks() {
        let gone = TrackId(7);
        let mut source = MeterSnapshot::default();
        source.accumulate(&StripMeter::default(), &HashMap::from([(gone, StripMeter::default())]));
        let mut ui = MeterSnapshot::default();
        take_meter_snapshot(&mut source, &mut ui);
        source.forget(&[gone]);
        take_meter_snapshot(&mut source, &mut ui);
        assert!(ui.tracks.is_empty());
    }

    #[test]
    fn a_channel_count_change_takes_the_new_stereo_peaks() {
        let mut acc = StripMeter { peak: [0.9, 0.9], peaks: vec![0.9], ..StripMeter::default() };
        acc.accumulate(&StripMeter { peak: [0.9, 0.0], peaks: vec![0.9, 0.0], ..StripMeter::default() });
        assert_eq!(acc.peak, [0.9, 0.0]);
    }

    #[test]
    fn gain_reduction_keeps_its_maximum_until_taken() {
        let mut acc = StripMeter::default();
        for gr in [1.0, 9.0, 2.0, f32::NAN] {
            acc.accumulate(&StripMeter { peak: [0.1, 0.1], peaks: vec![0.1], gain_reduction_db: gr, ..StripMeter::default() });
        }
        assert_eq!(acc.gain_reduction_db, 9.0);
        acc.reset_levels();
        assert_eq!((acc.gain_reduction_db, acc.peak, acc.peaks.len()), (0.0, [0.0; 2], 1));
    }

    #[test]
    fn a_block_that_panics_goes_silent_and_is_reported_once() {
        let shared = Shared {
            position: AtomicI64::new(0),
            playing: AtomicBool::new(true),
            speed: AtomicU32::new(1000),
            meters: Mutex::new(MeterSnapshot::default()),
        };
        let fault = Fault::default();
        let mut out = [0.5f32; 8];
        guarded_block(&fault, &shared, &mut out, |o| o.fill(0.25));
        assert_eq!((out, fault.trouble(false)), ([0.25; 8], None));
        guarded_block(&fault, &shared, &mut out, |_| std::panic::panic_any("a bug in the mix"));
        guarded_block(&fault, &shared, &mut out, |o| o.fill(0.25));
        assert_eq!(out, [0.0; 8]);
        assert!(!shared.playing.load(Ordering::Relaxed));
        assert_eq!((fault.trouble(false), fault.trouble(false)), (Some(OutputTrouble::Bug), None));
        assert_eq!(fault.beats.load(Ordering::Relaxed), 3);
    }

    #[test]
    fn a_lost_or_stalled_device_is_reported_once() {
        let ago = |seen: &mut Option<(u64, std::time::Instant)>| {
            if let Some(s) = seen {
                s.1 = std::time::Instant::now().checked_sub(STALL).unwrap();
            }
        };
        let mut seen = None;
        assert!(!stalled(&mut seen, 5, true));
        ago(&mut seen);
        assert!(stalled(&mut seen, 5, true));
        assert!(!stalled(&mut seen, 6, true), "it asked again");
        ago(&mut seen);
        assert!(!stalled(&mut seen, 6, false), "stopped, so nothing to ask for");
        assert_eq!(Fault::default().trouble(true), Some(OutputTrouble::Stalled));
        let fault = Fault::default();
        fault.gone.store(true, Ordering::Relaxed);
        assert_eq!((fault.trouble(false), fault.trouble(true)), (Some(OutputTrouble::Gone), None));
    }

    #[test]
    fn audio_state_plays_and_stops_at_end() {
        let s = Arc::new(soundcraft_model::Session::default());
        let shared = Arc::new(Shared {
            position: AtomicI64::new(0),
            playing: AtomicBool::new(false),
            speed: AtomicU32::new(1000),
            meters: Mutex::new(MeterSnapshot::default()),
        });
        let (tx, rx) = channel();
        let mut st = AudioState::new(rx, Arc::clone(&shared), s, 48_000.0, channel().0);
        tx.send(Cmd::Play { from: 0, end: Some(1000), looped: None }).unwrap();
        let mut out = vec![0.0f32; 4096];
        st.fill(&mut out, 2);
        assert!(shared.position.load(Ordering::Relaxed) >= 1000);
        assert!(!shared.playing.load(Ordering::Relaxed));
    }

    #[test]
    fn short_device_callbacks_render_short_mix_blocks() {
        let s = Arc::new(soundcraft_model::Session::default());
        let shared = Arc::new(Shared {
            position: AtomicI64::new(0),
            playing: AtomicBool::new(false),
            speed: AtomicU32::new(1000),
            meters: Mutex::new(MeterSnapshot::default()),
        });
        let (tx, rx) = channel();
        let mut st = AudioState::new(rx, Arc::clone(&shared), s, 48_000.0, channel().0);
        tx.send(Cmd::Play { from: 0, end: None, looped: None }).unwrap();
        let mut out = vec![0.0f32; 2 * 64];
        st.fill(&mut out, 2);
        assert_eq!(shared.position.load(Ordering::Relaxed), 64);
    }

    fn audio_output_state(format: soundcraft_model::ChannelFormat, channels: Vec<Vec<f32>>) -> AudioState {
        use soundcraft_model::{Clip, SourceAudio, SourceId, TrackKind};
        let mut s = Session::default();
        s.edit.click = false;
        s.outputs[0].format = format;
        let t = s.add_track(TrackKind::Audio, format, None);
        let frames = channels[0].len();
        s.pool.insert(SourceId(5), Arc::new(SourceAudio::new(soundcraft_audio_io::AudioBuffer { sample_rate: 48_000, channels })));
        let id = s.new_clip_id();
        s.track_mut(t).unwrap().playlist_mut().unwrap().clips.push(Clip::audio(id, "output", SourceId(5), 0, 0, frames as i64));
        let shared = Arc::new(Shared {
            position: AtomicI64::new(0),
            playing: AtomicBool::new(false),
            speed: AtomicU32::new(1000),
            meters: Mutex::new(MeterSnapshot::default()),
        });
        let (tx, rx) = channel();
        let st = AudioState::new(rx, shared, Arc::new(s), 48_000.0, channel().0);
        tx.send(Cmd::Play { from: 0, end: None, looped: None }).unwrap();
        st
    }

    #[test]
    fn mono_device_averages_stereo_before_clipping() {
        use soundcraft_model::ChannelFormat;
        // Right only, left only, identical, opposite polarity, and levels that must be averaged
        // before clipping either side to the device's range.
        let left = [0.0, 0.8, 0.8, 0.8, 2.0, -2.0, 2.0];
        let right = [0.8, 0.0, 0.8, -0.8, -0.5, 0.5, 2.0];
        let expected = [0.4, 0.4, 0.8, 0.0, 0.75, -0.75, 1.0];
        for callback in [1, 37, 512] {
            let channels = [left, right].map(|pattern| (0..2048).map(|i| pattern[i % pattern.len()]).collect());
            let mut st = audio_output_state(ChannelFormat::Stereo, channels.into());
            let mut out = vec![0.0; callback];
            for block in 0..3 {
                st.fill(&mut out, 1);
                for (i, actual) in out.iter().enumerate() {
                    let at = block * callback + i;
                    assert!((actual - expected[at % expected.len()]).abs() < 1e-6, "callback {callback}, sample {at}: {actual}");
                }
            }
        }
    }

    #[test]
    fn resampling_at_a_short_block_edge_does_not_mix_in_stale_frames() {
        use soundcraft_model::ChannelFormat;
        // DC for the first long block, then silence. The device runs slower than the session, so
        // reads land between frames; a short block leaves the long block's DC past `pending_len`.
        let mut st = audio_output_state(ChannelFormat::Stereo, vec![vec![0.8; BLOCK]; 2]);
        st.device_rate = 44_100.0;
        let mut long = vec![0.0; 2 * 470];
        st.fill(&mut long, 2);
        assert_eq!(st.pending_len, BLOCK);
        let mut short = [0.0; 2 * 8];
        for call in 0..20 {
            st.fill(&mut short, 2);
            assert!(st.pending_len < BLOCK);
            // The first frame of the first short call still reads the DC block's last frame.
            let from = if call == 0 { 2 } else { 0 };
            assert!(short[from..].iter().all(|x| x.abs() < 1e-6), "call {call}: {short:?}");
        }
    }

    #[test]
    fn mono_device_preserves_centered_mono_level_and_includes_surround_right_channels() {
        use soundcraft_model::ChannelFormat;
        let h = std::f32::consts::FRAC_1_SQRT_2;
        let mut mono = audio_output_state(ChannelFormat::Mono, vec![vec![0.8; 512]]);
        let mut out = [0.0; 128];
        mono.fill(&mut out, 1);
        let centered = 0.8 * soundcraft_dsp::db_to_gain(-3.0);
        assert!(out.iter().all(|x| (*x - centered).abs() < 1e-6), "a centered mono track keeps its rendered level under the -3 dB pan law");

        // Independently exercise each SMPTE 5.1 channel: L, R, C, LFE, Ls, Rs.
        let channels = (0..6).map(|c| (0..512).map(|i| if i % 6 == c { 0.8 } else { 0.0 }).collect()).collect();
        let mut surround = audio_output_state(ChannelFormat::Surround51, channels);
        surround.fill(&mut out, 1);
        let expected = [0.4, 0.4, 0.8 * h, 0.0, 0.4 * h, 0.4 * h];
        for (i, actual) in out.iter().enumerate() {
            assert!((actual - expected[i % expected.len()]).abs() < 1e-6, "surround sample {i}: {actual}");
        }
    }

    #[test]
    fn mono_device_contains_overflow_from_opposite_surround_sides() {
        use soundcraft_model::ChannelFormat;
        let channels = [f32::MAX, -f32::MAX, 0.0, 0.0, f32::MAX, -f32::MAX].map(|v| vec![v; 128]);
        let mut st = audio_output_state(ChannelFormat::Surround51, channels.into());
        let mut out = [0.5; 128];
        st.fill(&mut out, 1);
        assert!(out.iter().all(|x| *x == 0.0), "overflow in the stereo fold-down must not send NaN to a mono device");
    }

    #[test]
    fn interpolation_overflow_between_huge_samples_never_reaches_the_device() {
        use soundcraft_model::ChannelFormat;
        let alternating: Vec<f32> = (0..512).map(|i| if i % 2 == 0 { -3.4e38 } else { 3.4e38 }).collect();
        for (format, nch, device) in [(ChannelFormat::Stereo, 2, 2), (ChannelFormat::Stereo, 2, 8), (ChannelFormat::Mono, 1, 1)] {
            let mut st = audio_output_state(format, vec![alternating.clone(); nch]);
            let mut out = vec![0.5; 64 * device];
            st.fill(&mut out, device);
            assert!(out.iter().all(|x| x.is_finite() && (-1.0..=1.0).contains(x)), "{format:?} on {device} channels: {out:?}");
        }
    }

    #[test]
    fn surround_main_maps_to_device_channels_or_folds_down() {
        use soundcraft_model::{ChannelFormat, Clip, SourceAudio, SourceId, TrackKind};
        let mut s = soundcraft_model::Session::default();
        s.outputs[0].format = ChannelFormat::Surround51;
        let t = s.add_track(TrackKind::Audio, ChannelFormat::Surround51, None);
        let chans: Vec<Vec<f32>> = (0..6).map(|c| vec![0.1 * (c + 1) as f32; 48_000]).collect();
        s.pool.insert(SourceId(5), Arc::new(SourceAudio::new(soundcraft_audio_io::AudioBuffer { sample_rate: 48_000, channels: chans })));
        let id = s.new_clip_id();
        s.track_mut(t).unwrap().playlist_mut().unwrap().clips.push(Clip::audio(id, "x", SourceId(5), 0, 0, 48_000));
        let s = Arc::new(s);
        let shared = Arc::new(Shared {
            position: AtomicI64::new(0),
            playing: AtomicBool::new(false),
            speed: AtomicU32::new(1000),
            meters: Mutex::new(MeterSnapshot::default()),
        });
        let (tx, rx) = channel();
        let mut st = AudioState::new(rx, Arc::clone(&shared), Arc::clone(&s), 48_000.0, channel().0);
        tx.send(Cmd::Play { from: 0, end: None, looped: None }).unwrap();
        // An 8-channel device: channels in order, the rest silent.
        let mut out = vec![0.0f32; 8 * 256];
        st.fill(&mut out, 8);
        let f = &out[8 * 100..8 * 101];
        for c in 0..6 {
            assert!((f[c] - 0.1 * (c + 1) as f32).abs() < 1e-5, "{f:?}");
        }
        assert!(f[6] == 0.0 && f[7] == 0.0);
        // A stereo device: ITU fold-down.
        let mut out = vec![0.0f32; 2 * 256];
        st.fill(&mut out, 2);
        let h = std::f32::consts::FRAC_1_SQRT_2;
        let l = (0.1 + h * 0.3 + h * 0.5f32).min(1.0);
        assert!((out[200] - l).abs() < 1e-4, "{} vs {l}", out[200]);
        assert_eq!(shared.meters.lock().unwrap().main.peaks.len(), 6);
    }

    #[test]
    fn instances_arrive_with_the_session_and_leave_through_the_return_channel() {
        use soundcraft_model::{ChannelFormat, Insert, TrackKind};
        let mut s = soundcraft_model::Session::default();
        let t = s.add_track(TrackKind::Audio, ChannelFormat::Stereo, None);
        s.track_mut(t).unwrap().mixer.inserts[1] = Some(Insert::new("clap:test.fake"));
        let s = Arc::new(s);
        let shared = Arc::new(Shared {
            position: AtomicI64::new(0),
            playing: AtomicBool::new(false),
            speed: AtomicU32::new(1000),
            meters: Mutex::new(MeterSnapshot::default()),
        });
        let (tx, rx) = channel();
        let (gtx, grx) = channel();
        let mut st = AudioState::new(rx, Arc::clone(&shared), Arc::new(soundcraft_model::Session::default()), 48_000.0, gtx);
        let plugin = soundcraft_dsp::create("gain").unwrap();
        tx.send(Cmd::Session(Arc::clone(&s), vec![PreparedInstance { track: t, slot: 1, id: "clap:test.fake".into(), ch: 2, plugin }])).unwrap();
        let mut out = vec![0.0f32; 256];
        st.fill(&mut out, 2);
        assert!(grx.try_recv().is_err(), "nothing retired yet");
        // A session without the insert retires the instance to the return channel.
        tx.send(Cmd::Session(Arc::new(soundcraft_model::Session::default()), Vec::new())).unwrap();
        st.fill(&mut out, 2);
        assert_eq!(grx.try_recv().unwrap().len(), 1);
        // A sample-rate change rebuilds the engine without dropping plugins on this thread.
        tx.send(Cmd::Session(
            Arc::clone(&s),
            vec![PreparedInstance { track: t, slot: 1, id: "clap:test.fake".into(), ch: 2, plugin: soundcraft_dsp::create("gain").unwrap() }],
        ))
        .unwrap();
        st.fill(&mut out, 2);
        let mut s44 = (*s).clone();
        s44.sample_rate = soundcraft_time::SampleRate::new(44_100).unwrap();
        tx.send(Cmd::Session(Arc::new(s44), Vec::new())).unwrap();
        st.fill(&mut out, 2);
        assert_eq!(grx.try_recv().unwrap().len(), 1);
    }

    #[test]
    fn plugin_states_are_read_without_the_audio_thread() {
        let mut side = Side::new(48_000.0, channel().1);
        let (a, b) = (TrackId(1), TrackId(2));
        side.states.insert((b, 0), Box::new(|| Some(b"b0".to_vec())));
        side.states.insert((a, soundcraft_mix::INSTRUMENT_SLOT), Box::new(|| Some(b"synth".to_vec())));
        side.states.insert((a, 3), Box::new(|| Some(b"a3".to_vec())));
        side.states.insert((a, 4), Box::new(|| None));
        let want = vec![(a, 3, b"a3".to_vec()), (a, soundcraft_mix::INSTRUMENT_SLOT, b"synth".to_vec()), (b, 0, b"b0".to_vec())];
        assert_eq!(side.capture_states(), want);
        side.forget((a, 3));
        assert_eq!(side.capture_states(), want[1..]);
    }

    struct Window;

    impl PluginEditor for Window {
        fn open(&mut self) -> Result<(), String> {
            Ok(())
        }
        fn close(&mut self) {}
        fn is_open(&self) -> bool {
            false
        }
        fn idle(&mut self) -> Vec<(String, f32)> {
            Vec::new()
        }
    }

    #[test]
    fn a_refused_state_is_kept_until_the_editor_opens() {
        let mut side = Side::new(48_000.0, channel().1);
        let k = (TrackId(1), 0);
        side.states.insert(k, Box::new(|| Some(b"defaults".to_vec())));
        side.editors.insert(k, Box::new(Window));
        side.refused.insert(k);
        assert!(side.capture_states().is_empty());
        side.open_editor(k).unwrap();
        assert_eq!(side.capture_states(), vec![(TrackId(1), 0, b"defaults".to_vec())]);
    }

    /// An editor whose plugin said its state changed (CLAP `state.mark_dirty`).
    struct Dirty(bool);

    impl PluginEditor for Dirty {
        fn open(&mut self) -> Result<(), String> {
            Ok(())
        }
        fn close(&mut self) {}
        fn is_open(&self) -> bool {
            true
        }
        fn idle(&mut self) -> Vec<(String, f32)> {
            Vec::new()
        }
        fn state_changed(&mut self) -> bool {
            std::mem::take(&mut self.0)
        }
    }

    #[test]
    fn changed_states_are_reported_once_and_read_for_the_session() {
        let mut side = Side::new(48_000.0, channel().1);
        let (a, b) = (TrackId(1), TrackId(2));
        let synth = (a, soundcraft_mix::INSTRUMENT_SLOT);
        side.states.insert((a, 0), Box::new(|| Some(b"a0".to_vec())));
        side.states.insert((a, 1), Box::new(|| Some(b"a1".to_vec())));
        side.states.insert(synth, Box::new(|| Some(b"synth".to_vec())));
        side.states.insert((b, 0), Box::new(|| Some(b"b0".to_vec())));
        side.editors.insert((a, 0), Box::new(Dirty(true)));
        side.editors.insert((a, 1), Box::new(Dirty(false)));
        side.editors.insert((b, 0), Box::new(Dirty(true)));
        side.refused.insert((b, 0));
        let mut changed = side.changed_states();
        changed.sort();
        assert_eq!(changed, vec![(a, 0), (b, 0)]);
        assert!(side.changed_states().is_empty(), "each change is reported once");
        let want = vec![(a, 0, b"a0".to_vec()), (a, soundcraft_mix::INSTRUMENT_SLOT, b"synth".to_vec())];
        assert_eq!(side.capture_states_of(&[(a, 0), synth, (b, 0)]), want);
    }

    #[test]
    fn a_state_marked_changed_while_loading_is_not_a_change() {
        let mut side = Side::new(48_000.0, channel().1);
        let k = (TrackId(1), 0);
        side.states.insert(k, Box::new(|| Some(b"loaded".to_vec())));
        side.add_editor(k, Box::new(Dirty(true)));
        assert!(side.changed_states().is_empty());
    }

    #[test]
    fn side_creates_each_missing_instance_once() {
        use soundcraft_model::{ChannelFormat, Insert, TrackKind};
        let mut side = Side::new(48_000.0, channel().1);
        let mut s = soundcraft_model::Session::default();
        let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, None);
        s.track_mut(t).unwrap().mixer.inserts[0] = Some(Insert::new("clap:does.not.exist"));
        s.track_mut(t).unwrap().mixer.inserts[1] = Some(Insert::new("gain"));
        assert!(side.prepare(&s).is_empty(), "the plugin is not installed; built-ins stay with the engine");
        assert_eq!(side.shadow.get(&(t, 0)), Some(&("clap:does.not.exist".to_string(), 1, false)));
        assert!(side.prepare(&s).is_empty());
        assert_eq!(side.shadow.len(), 1, "failures are remembered, not retried every update");
        s.track_mut(t).unwrap().mixer.inserts[0] = None;
        side.prepare(&s);
        assert!(side.shadow.is_empty());
    }

    #[test]
    fn replaced_sessions_are_freed_on_the_ui_thread() {
        let shared = Arc::new(Shared {
            position: AtomicI64::new(0),
            playing: AtomicBool::new(false),
            speed: AtomicU32::new(1000),
            meters: Mutex::new(MeterSnapshot::default()),
        });
        let (tx, rx) = channel();
        let (gtx, grx) = channel();
        let mut st = AudioState::new(rx, shared, Arc::new(soundcraft_model::Session::default()), 48_000.0, gtx);
        let mut side = Side::new(48_000.0, grx);
        side.return_old_sessions(&tx);
        let mut out = vec![0.0f32; 256];
        let mut watched = Vec::new();
        for _ in 0..3 * OLD_SESSIONS {
            let s = Arc::new(soundcraft_model::Session::default());
            watched.push(Arc::downgrade(&s));
            side.collect_garbage();
            tx.send(Cmd::Session(s, Vec::new())).unwrap();
            st.fill(&mut out, 2);
            let replaced = watched.len().checked_sub(2).and_then(|i| watched.get(i));
            assert!(replaced.is_none_or(|w| w.strong_count() == 1), "freed on the audio thread");
        }
        side.collect_garbage();
        assert_eq!(watched.iter().filter(|w| w.strong_count() > 0).count(), 1, "only the live snapshot is left");
    }

    #[test]
    fn looping_wraps() {
        let s = Arc::new(soundcraft_model::Session::default());
        let shared = Arc::new(Shared {
            position: AtomicI64::new(0),
            playing: AtomicBool::new(false),
            speed: AtomicU32::new(1000),
            meters: Mutex::new(MeterSnapshot::default()),
        });
        let (tx, rx) = channel();
        let mut st = AudioState::new(rx, Arc::clone(&shared), s, 48_000.0, channel().0);
        tx.send(Cmd::Play { from: 0, end: None, looped: Some(Range::new(0, 1000)) }).unwrap();
        let mut out = vec![0.0f32; 20_000];
        st.fill(&mut out, 2);
        let p = shared.position.load(Ordering::Relaxed);
        assert!(p <= 1000, "position {p} should stay inside the loop");
        assert!(shared.playing.load(Ordering::Relaxed));
    }

    #[test]
    fn locate_while_playing_drops_the_old_position_audio() {
        use soundcraft_model::{ChannelFormat, Clip, Insert, SourceAudio, SourceId, TrackKind};
        for (plugin, params) in [("compressor", &[("lookahead", 5.0), ("ratio", 1.0)][..]), ("room_reverb", &[][..])] {
            let mut s = soundcraft_model::Session::default();
            s.edit.click = false;
            let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, None);
            s.pool.insert(
                SourceId(5),
                Arc::new(SourceAudio::new(soundcraft_audio_io::AudioBuffer { sample_rate: 48_000, channels: vec![vec![0.5; 4800]] })),
            );
            let id = s.new_clip_id();
            let tr = s.track_mut(t).unwrap();
            tr.playlist_mut().unwrap().clips.push(Clip::audio(id, "x", SourceId(5), 0, 0, 4800));
            let mut ins = Insert::new(plugin);
            for (k, v) in params {
                ins.params.insert((*k).into(), *v);
            }
            tr.mixer.inserts[0] = Some(ins);
            let shared = Arc::new(Shared {
                position: AtomicI64::new(0),
                playing: AtomicBool::new(false),
                speed: AtomicU32::new(1000),
                meters: Mutex::new(MeterSnapshot::default()),
            });
            let (tx, rx) = channel();
            let mut st = AudioState::new(rx, Arc::clone(&shared), Arc::new(s), 48_000.0, channel().0);
            tx.send(Cmd::Play { from: 0, end: None, looped: None }).unwrap();
            let mut out = vec![0.0f32; 2 * 512];
            let mut before = 0.0f32;
            for _ in 0..9 {
                st.fill(&mut out, 2);
                before = out.iter().fold(before, |m, x| m.max(x.abs()));
            }
            assert!(before > 0.1, "{plugin}: the clip plays before the locate");
            tx.send(Cmd::Locate(1_000_000)).unwrap();
            for _ in 0..8 {
                st.fill(&mut out, 2);
                assert!(out.iter().all(|x| x.abs() < 1e-6), "{plugin}: audio from before the locate");
            }
        }
    }

    #[test]
    fn armed_tracks_are_heard_and_metered_after_playback_stops() {
        use soundcraft_model::{ChannelFormat, TrackKind};
        let mut s = soundcraft_model::Session::default();
        let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, None);
        s.track_mut(t).unwrap().mixer.record_arm = true;
        let shared = Arc::new(Shared {
            position: AtomicI64::new(0),
            playing: AtomicBool::new(false),
            speed: AtomicU32::new(1000),
            meters: Mutex::new(MeterSnapshot::default()),
        });
        let (tx, rx) = channel();
        let mut st = AudioState::new(rx, Arc::clone(&shared), Arc::new(s), 48_000.0, channel().0);
        let ring = record::InputRing::new(1, 48_000);
        tx.send(Cmd::Input(Arc::clone(&ring))).unwrap();
        tx.send(Cmd::Play { from: 0, end: Some(1000), looped: None }).unwrap();
        let mut out = vec![0.0f32; 4096];
        st.fill(&mut out, 2);
        assert!(!shared.playing.load(Ordering::Relaxed));
        ring.push(&[0.5; 4096]);
        st.fill(&mut out, 2);
        assert!(out.iter().any(|x| x.abs() > 0.1), "the input is heard");
        assert!(shared.meters.lock().unwrap().tracks.get(&t).is_some_and(|m| m.peak[0] > 0.1), "and metered");
    }
}
