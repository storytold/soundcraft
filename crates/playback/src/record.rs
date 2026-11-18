//! Audio input capture for recording.
//!
//! A [`Recorder`] opens the default input device. While armed, the input callback copies each
//! block into a preallocated lock-free ring (it never locks or allocates), and a writer thread
//! streams the ring into one WAV file per take, rewriting the header every second so a crash leaves
//! a playable file.

use std::fs::File;
use std::io::{Seek, SeekFrom, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use soundcraft_model::TrackId;

/// What [`Recorder::finish`] returns: the takes that hold audio and the first problem met.
pub type Takes = (Vec<(TrackId, PathBuf)>, Option<String>);

/// Frames each block of a [`LiveTake`] sums up.
pub const LIVE_BLOCK: usize = 128;
/// Blocks a live take keeps per channel at most (three hours at 48 kHz); its file records on.
const LIVE_BLOCKS: usize = 1 << 22;

/// A take while it records, summed up for drawing: per channel, the (min, max) of each
/// [`LIVE_BLOCK`] frames so far.
pub type LiveTake = Vec<Vec<(f32, f32)>>;

const RING_SECONDS: usize = 4;
/// How long a recording input may go without a callback before it counts as gone.
const STALL: Duration = Duration::from_secs(2);
const GONE: u8 = 1;
const BROKEN: u8 = 2;

/// Why the input can't go on.
#[derive(Debug, Clone, PartialEq)]
pub enum Trouble {
    /// The device went away or stopped sending audio, or its callback hit a bug.
    Device(String),
    /// Writing a take failed (a full disk, say).
    Disk(String),
}

/// Single-producer single-consumer FIFO of samples (input callback → writer thread). The slots
/// are atomics, so it needs no lock and no `unsafe`.
struct CaptureRing {
    slots: Box<[AtomicU32]>,
    mask: usize,
    head: AtomicUsize,
    tail: AtomicUsize,
}

impl CaptureRing {
    fn new(capacity: usize) -> CaptureRing {
        let n = capacity.max(2).next_power_of_two();
        CaptureRing { slots: (0..n).map(|_| AtomicU32::new(0)).collect(), mask: n - 1, head: AtomicUsize::new(0), tail: AtomicUsize::new(0) }
    }

    /// Queue up to `n` samples (`sample(i)` is the i-th); returns how many fit.
    fn push(&self, n: usize, sample: impl Fn(usize) -> f32) -> usize {
        let head = self.head.load(Ordering::Relaxed);
        let n = n.min(self.slots.len().saturating_sub(head.wrapping_sub(self.tail.load(Ordering::Acquire))));
        for i in 0..n {
            if let Some(slot) = self.slots.get(head.wrapping_add(i) & self.mask) {
                slot.store(sample(i).to_bits(), Ordering::Relaxed);
            }
        }
        self.head.store(head.wrapping_add(n), Ordering::Release);
        n
    }

    fn pop_all(&self, out: &mut Vec<f32>) {
        let (tail, head) = (self.tail.load(Ordering::Relaxed), self.head.load(Ordering::Acquire));
        out.extend(
            (0..head.wrapping_sub(tail))
                .map(|i| self.slots.get(tail.wrapping_add(i) & self.mask).map_or(0.0, |s| f32::from_bits(s.load(Ordering::Relaxed)))),
        );
        self.tail.store(head, Ordering::Release);
    }
}

/// Most frames the input callback converts and pushes at once; longer callbacks go in pieces.
const PUSH_FRAMES: usize = 8192;

/// An input sample, or silence for NaN and infinities, which would poison the take and the mix.
fn finite(x: f32) -> f32 {
    if x.is_finite() { x } else { 0.0 }
}

struct Shared {
    armed: AtomicBool,
    ring: CaptureRing,
    /// Samples lost while the ring was full.
    dropped: AtomicUsize,
    /// Input callbacks so far, to tell a device that stopped sending.
    beats: AtomicU64,
    /// Set from the input stream when it can't go on: `GONE` or `BROKEN`.
    fault: AtomicU8,
    /// The first failure writing a take, until it's reported.
    failed: Mutex<Option<String>>,
}

impl Shared {
    fn new(ring: CaptureRing) -> Shared {
        let (beats, fault, failed) = (AtomicU64::new(0), AtomicU8::new(0), Mutex::new(None));
        Shared { armed: AtomicBool::new(false), ring, dropped: AtomicUsize::new(0), beats, fault, failed }
    }

    /// Run the input callback's `work`. After a panic in it, the input is broken and does
    /// nothing more.
    fn guarded(&self, work: impl FnOnce()) {
        self.beats.fetch_add(1, Ordering::Relaxed);
        if self.fault.load(Ordering::Relaxed) != BROKEN && std::panic::catch_unwind(std::panic::AssertUnwindSafe(work)).is_err() {
            self.fault.store(BROKEN, Ordering::Relaxed);
        }
    }

    /// The input callback's work while armed. Samples that don't fit are owed (`owed`) and queued
    /// as silence before the next block, so the take keeps its timing.
    fn capture(&self, data: &[f32], owed: &mut usize) {
        *owed -= self.ring.push(*owed, |_| 0.0);
        let kept = if *owed == 0 { self.ring.push(data.len(), |i| data.get(i).copied().map_or(0.0, finite)) } else { 0 };
        *owed = owed.saturating_add(data.len() - kept);
        self.dropped.fetch_add(data.len() - kept, Ordering::Relaxed);
    }
}

pub struct Recorder {
    shared: Arc<Shared>,
    /// Live input for monitoring (always fed while the input stream runs).
    pub monitor: Arc<InputRing>,
    #[cfg(not(target_os = "freebsd"))]
    _stream: Option<cpal::Stream>,
    pub device_name: String,
    pub sample_rate: u32,
    pub channels: usize,
    /// The take writer, and what each track's take holds so far.
    writer: Option<(std::thread::JoinHandle<Takes>, Vec<(TrackId, Arc<Mutex<LiveTake>>)>)>,
    /// The callback count last seen, and since when.
    seen: (u64, Instant),
}

/// The input stream: device samples are converted to f32 in a scratch buffer allocated here (so
/// the callback never allocates), then feed the monitor ring and, while armed, the capture.
#[cfg(not(target_os = "freebsd"))]
fn build_input<T>(
    device: &cpal::Device,
    config: cpal::StreamConfig,
    shared: &Arc<Shared>,
    monitor: &Arc<InputRing>,
) -> Result<cpal::Stream, cpal::Error>
where
    T: cpal::SizedSample,
    f32: cpal::FromSample<T>,
{
    use cpal::Sample;
    use cpal::traits::DeviceTrait;
    let (sh, errors) = (Arc::clone(shared), Arc::clone(shared));
    let mon = Arc::clone(monitor);
    let mut scratch = vec![0.0f32; PUSH_FRAMES * usize::from(config.channels).max(1)];
    let chunk = scratch.len();
    let mut owed = 0;
    device.build_input_stream(
        config,
        move |data: &[T], _: &cpal::InputCallbackInfo| {
            crate::mark_audio_thread();
            sh.guarded(|| {
                for part in data.chunks(chunk) {
                    let Some(s) = scratch.get_mut(..part.len()) else { return };
                    for (o, x) in s.iter_mut().zip(part) {
                        *o = f32::from_sample(*x);
                    }
                    mon.push(s);
                    if sh.armed.load(Ordering::Acquire) {
                        sh.capture(s, &mut owed);
                    } else {
                        owed = 0;
                    }
                }
            });
        },
        move |e: cpal::Error| {
            // Possibly the audio thread (see `crate::mark_audio_thread`).
            crate::mark_audio_thread();
            // cpal 0.18 reports every operation through one error type; a device that went away
            // or a stream that must be rebuilt (`StreamInvalidated`) stops capture.
            // `DeviceChanged` (a rerouted default device) and an xrun are not faults.
            if crate::devices::stream_lost(&e) {
                let _ = errors.fault.compare_exchange(0, GONE, Ordering::Relaxed, Ordering::Relaxed);
            }
            log::warn!("input stream error: {e}");
        },
        None,
    )
}

/// The input configuration at `rate` when the device offers one: the best sample format first,
/// then the most channels (so a stereo interface is preferred over a mono one). `None` lets the
/// caller use the device's default input configuration (the mix resamples when the rates differ).
fn pick_input_config(configs: &[cpal::SupportedStreamConfigRange], rate: u32) -> Option<cpal::SupportedStreamConfig> {
    configs
        .iter()
        .filter_map(|c| {
            let rank = crate::devices::format_rank(c.sample_format())?;
            if !c.contains_rate(rate) {
                return None;
            }
            c.try_with_sample_rate(rate).map(|config| (rank, std::cmp::Reverse(c.channels()), config))
        })
        .min_by_key(|(rank, channels, _)| (*rank, *channels))
        .map(|(_, _, config)| config)
}

impl Recorder {
    /// Open the default input device. Errors when there is none (the caller shows a message).
    pub fn open() -> Result<Recorder, String> {
        Recorder::open_with(&crate::AudioConfig::default())
    }

    /// Open the input device `audio` chooses (else the default) at its chosen rate and buffer
    /// size when the device offers them, else at the device's defaults. Integer devices (ASIO
    /// drivers usually are) are converted to f32.
    #[cfg(not(target_os = "freebsd"))]
    pub fn open_with(audio: &crate::AudioConfig) -> Result<Recorder, String> {
        use cpal::traits::{DeviceTrait, StreamTrait};
        let host = crate::devices::host(audio);
        let device = crate::devices::device(&host, audio.input.as_deref(), false).ok_or("no audio input device")?;
        let name = crate::devices::device_name(&device).unwrap_or_else(|| "Input".into());
        // At the chosen rate when the device offers it, so input and output run together: the
        // best format first, then the most channels.
        let configs: Vec<cpal::SupportedStreamConfigRange> = device.supported_input_configs().map(|configs| configs.collect()).unwrap_or_default();
        let at_rate = audio.sample_rate.and_then(|r| pick_input_config(&configs, r));
        let cfg = match at_rate {
            Some(c) => c,
            None => {
                let d = device.default_input_config().map_err(|e| e.to_string())?;
                // cpal 0.18's default can name a format these callbacks do not decode (I24, F64);
                // open the same rate and channels in a format they do.
                if crate::devices::format_rank(d.sample_format()).is_some() {
                    d
                } else {
                    crate::devices::supported_format_config(&configs, d.sample_rate(), d.channels())
                        .ok_or_else(|| format!("unsupported input sample format {:?}", d.sample_format()))?
                }
            }
        };
        let format = cfg.sample_format();
        if crate::devices::format_rank(format).is_none() {
            return Err(format!("unsupported input sample format {format:?}"));
        }
        let mut config = cfg.config();
        config.buffer_size = crate::devices::buffer_size(audio.buffer_size, cfg.buffer_size());
        let channels = usize::from(config.channels).max(1);
        let ring = CaptureRing::new((config.sample_rate as usize).saturating_mul(channels * RING_SECONDS).min(1 << 25));
        let shared = Arc::new(Shared::new(ring));
        // The monitor ring is sized for the stream's callbacks before the stream starts; when the
        // host picks the buffer size, for the largest one Setup offers.
        let monitor_for = |buffer: cpal::BufferSize| {
            let callback = match buffer {
                cpal::BufferSize::Fixed(n) => n as usize,
                cpal::BufferSize::Default => InputRing::LARGEST_CALLBACK,
            };
            InputRing::sized(channels, config.sample_rate, callback)
        };
        let mut monitor = monitor_for(config.buffer_size);
        let build = |config: cpal::StreamConfig, monitor: &Arc<InputRing>| match format {
            cpal::SampleFormat::I32 => build_input::<i32>(&device, config, &shared, monitor),
            cpal::SampleFormat::I16 => build_input::<i16>(&device, config, &shared, monitor),
            _ => build_input::<f32>(&device, config, &shared, monitor),
        };
        // The chosen buffer size first; if the device refuses it, its default.
        let stream = match build(config, &monitor) {
            Ok(s) => s,
            Err(e) if config.buffer_size != cpal::BufferSize::Default => {
                log::warn!("cannot open the input with buffer {:?}: {e}; using the device default", config.buffer_size);
                config.buffer_size = cpal::BufferSize::Default;
                monitor = monitor_for(config.buffer_size);
                build(config, &monitor).map_err(|e| e.to_string())?
            }
            Err(e) => return Err(e.to_string()),
        };
        stream.play().map_err(|e| e.to_string())?;
        let seen = (0, Instant::now());
        Ok(Recorder { monitor, shared, _stream: Some(stream), device_name: name, sample_rate: config.sample_rate, channels, writer: None, seen })
    }

    #[cfg(target_os = "freebsd")]
    pub fn open_with(_audio: &crate::AudioConfig) -> Result<Recorder, String> {
        Err("recording is not supported on this platform yet".into())
    }

    /// Create each take's file and start recording into them: a track's take records the given
    /// input channels into a new file.
    pub fn arm(&mut self, takes: Vec<(TrackId, Vec<usize>, PathBuf)>) -> Result<(), String> {
        if self.writer.is_some() {
            return Err("already recording".into());
        }
        let takes =
            takes.into_iter().map(|(id, inputs, path)| TakeWriter::create(id, inputs, path, self.sample_rate)).collect::<Result<Vec<_>, _>>()?;
        let live = takes.iter().map(|t| (t.id, Arc::clone(&t.live))).collect();
        let ring = &self.shared.ring;
        ring.tail.store(ring.head.load(Ordering::Acquire), Ordering::Release);
        *self.shared.failed.lock().unwrap_or_else(PoisonError::into_inner) = None;
        self.shared.armed.store(true, Ordering::Release);
        let (shared, channels) = (Arc::clone(&self.shared), self.channels);
        match std::thread::Builder::new().name("take writer".into()).spawn(move || write_takes(&shared, takes, channels)) {
            Ok(w) => self.writer = Some((w, live)),
            Err(e) => {
                self.shared.armed.store(false, Ordering::Release);
                return Err(e.to_string());
            }
        }
        Ok(())
    }

    /// What the take recording onto `track` holds so far.
    pub fn live_take(&self, track: TrackId) -> Option<MutexGuard<'_, LiveTake>> {
        let (_, take) = self.writer.as_ref()?.1.iter().find(|(id, _)| *id == track)?;
        Some(take.lock().unwrap_or_else(PoisonError::into_inner))
    }

    /// Why the input can't go on, if it can't: the device went away or, while recording, stopped
    /// sending audio; its callback hit a bug; or writing a take failed (reported once).
    pub fn trouble(&mut self) -> Option<Trouble> {
        if let Some(e) = self.shared.failed.lock().unwrap_or_else(PoisonError::into_inner).take() {
            return Some(Trouble::Disk(e));
        }
        let name = &self.device_name;
        match self.shared.fault.load(Ordering::Relaxed) {
            GONE => return Some(Trouble::Device(format!("“{name}” is unavailable"))),
            BROKEN => return Some(Trouble::Device("the audio input hit an internal error (see the log)".into())),
            _ => {}
        }
        let beats = self.shared.beats.load(Ordering::Relaxed);
        if beats != self.seen.0 || !self.shared.armed.load(Ordering::Relaxed) {
            self.seen = (beats, Instant::now());
        }
        (self.seen.1.elapsed() >= STALL).then(|| Trouble::Device(format!("“{name}” stopped sending audio")))
    }

    /// Input the current take has lost to a slow disk so far, in milliseconds.
    pub fn lost_ms(&self) -> u64 {
        let frames = self.shared.dropped.load(Ordering::Relaxed) / self.channels.max(1);
        (frames as u64).saturating_mul(1000) / u64::from(self.sample_rate.max(1))
    }

    pub fn finish(&mut self) -> Takes {
        self.shared.armed.store(false, Ordering::Release);
        let (takes, problem) = self.writer.take().and_then(|(w, _)| w.join().ok()).unwrap_or_default();
        let lost = self.shared.dropped.swap(0, Ordering::Relaxed) / self.channels.max(1);
        let slow =
            (lost > 0).then(|| format!("{} ms of input lost: the disk was too slow", lost.saturating_mul(1000) / self.sample_rate.max(1) as usize));
        (takes, problem.or(slow))
    }
}

impl Drop for Recorder {
    fn drop(&mut self) {
        self.finish();
    }
}

fn write_takes(shared: &Shared, mut takes: Vec<TakeWriter>, channels: usize) -> Takes {
    let (mut block, mut header_written, mut reported) = (Vec::new(), Instant::now(), false);
    loop {
        let stopping = !shared.armed.load(Ordering::Acquire);
        // The ring holds interleaved samples and a pop can end mid-frame: write whole frames and
        // keep the remainder for the next pop, so channels never shift.
        shared.ring.pop_all(&mut block);
        let whole = block.len() - block.len() % channels.max(1);
        takes.iter_mut().for_each(|t| t.write(&block[..whole], channels));
        block.drain(..whole);
        if !reported && let Some(e) = takes.iter().find_map(|t| t.error.clone()) {
            *shared.failed.lock().unwrap_or_else(PoisonError::into_inner) = Some(e);
            reported = true;
        }
        if stopping {
            break;
        }
        if header_written.elapsed() >= Duration::from_secs(1) {
            header_written = Instant::now();
            takes.iter_mut().for_each(TakeWriter::write_header);
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let (mut done, mut problem) = (Vec::new(), None);
    for mut t in takes {
        t.write_header();
        if t.frames == 0 {
            let _ = std::fs::remove_file(&t.path);
        } else {
            done.push((t.id, t.path));
        }
        problem = problem.or(t.error);
    }
    (done, problem)
}

/// One take's file. After an error it stops writing; its header keeps covering what was written.
struct TakeWriter {
    id: TrackId,
    inputs: Vec<usize>,
    path: PathBuf,
    file: File,
    rate: u32,
    frames: u64,
    error: Option<String>,
    live: Arc<Mutex<LiveTake>>,
}

impl TakeWriter {
    fn create(id: TrackId, inputs: Vec<usize>, path: PathBuf, rate: u32) -> Result<TakeWriter, String> {
        let file = path.parent().map_or(Ok(()), std::fs::create_dir_all).and_then(|()| File::create_new(&path));
        let file = file.map_err(|e| format!("{}: {e}", path.display()))?;
        let live = Arc::new(Mutex::new(vec![Vec::new(); inputs.len()]));
        let mut take = TakeWriter { id, inputs, path, file, rate, frames: 0, error: None, live };
        take.write_header();
        take.error.take().map_or(Ok(take), Err)
    }

    /// Append this take's channels of `block` (interleaved, `channels` wide).
    fn write(&mut self, block: &[f32], channels: usize) {
        let channels = channels.max(1);
        if self.error.is_some() {
            return;
        }
        let bytes: Vec<u8> = block
            .chunks_exact(channels)
            .flat_map(|f| self.inputs.iter().flat_map(move |&i| f.get(i).copied().unwrap_or(0.0).to_le_bytes()))
            .collect();
        match self.file.write_all(&bytes) {
            Ok(()) => {
                self.summarize(block, channels);
                self.frames += (block.len() / channels) as u64;
            }
            Err(e) => self.fail(e),
        }
    }

    /// Add this take's channels of `block` to its live take.
    fn summarize(&self, block: &[f32], channels: usize) {
        let mut live = self.live.lock().unwrap_or_else(PoisonError::into_inner);
        for (f, n) in block.chunks_exact(channels).zip(self.frames..) {
            let at = (n / LIVE_BLOCK as u64) as usize;
            for (blocks, &i) in live.iter_mut().zip(&self.inputs) {
                let v = f.get(i).copied().unwrap_or(0.0);
                match blocks.get_mut(at) {
                    Some(b) => *b = (b.0.min(v), b.1.max(v)),
                    None if at < LIVE_BLOCKS => blocks.push((v, v)),
                    None => {}
                }
            }
        }
    }

    fn write_header(&mut self) {
        let header = match craft_audio::wav_float_header(self.rate, u16::try_from(self.inputs.len()).unwrap_or(u16::MAX), self.frames) {
            Ok(h) => h,
            Err(e) => return self.fail(e),
        };
        let f = &mut self.file;
        if let Err(e) = f.seek(SeekFrom::Start(0)).and_then(|_| f.write_all(&header)).and_then(|()| f.seek(SeekFrom::End(0))) {
            self.fail(e);
        }
    }

    fn fail(&mut self, e: impl std::fmt::Display) {
        self.error.get_or_insert_with(|| format!("{}: {e}", self.path.display()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decode(path: &PathBuf) -> Vec<Vec<f32>> {
        craft_audio::decode(&std::fs::read(path).unwrap(), None).unwrap().1.channels
    }

    #[test]
    fn a_full_ring_owes_the_lost_samples_as_silence() {
        let shared = Shared::new(CaptureRing::new(8));
        let (mut owed, mut out) = (0, Vec::new());
        shared.capture(&[1.0; 6], &mut owed);
        shared.capture(&[2.0; 4], &mut owed);
        shared.ring.pop_all(&mut out);
        shared.capture(&[3.0; 2], &mut owed);
        shared.ring.pop_all(&mut out);
        assert_eq!(out, [vec![1.0; 6], vec![2.0; 2], vec![0.0; 2], vec![3.0; 2]].concat());
        assert_eq!((owed, shared.dropped.load(Ordering::Relaxed)), (0, 2));
    }

    #[test]
    fn nan_and_infinite_input_records_as_silence() {
        let shared = Shared::new(CaptureRing::new(8));
        let (mut owed, mut out) = (0, Vec::new());
        shared.capture(&[0.5, f32::NAN, f32::INFINITY, f32::NEG_INFINITY, -0.5], &mut owed);
        shared.ring.pop_all(&mut out);
        assert_eq!(out, [0.5, 0.0, 0.0, 0.0, -0.5]);
    }

    /// Build a cpal configuration range for tests with a 64..1024-frame buffer.
    fn range(channels: u16, min: u32, max: u32, format: cpal::SampleFormat) -> cpal::SupportedStreamConfigRange {
        cpal::SupportedStreamConfigRange::new(channels, min, max, cpal::SupportedBufferSize::Range { min: 64, max: 1024 }, format)
    }

    /// At the chosen rate, input selection prefers the best supported format, then more channels; a rate the device
    /// does not offer returns `None` so the caller can fall back.
    #[test]
    fn input_config_prefers_the_best_format_then_more_channels_at_the_chosen_rate() {
        let configs = [
            range(1, 44_100, 192_000, cpal::SampleFormat::I32),
            range(2, 44_100, 96_000, cpal::SampleFormat::I16),
            range(2, 48_000, 96_000, cpal::SampleFormat::F32),
            range(8, 44_100, 48_000, cpal::SampleFormat::F32),
        ];
        let c = pick_input_config(&configs, 96_000).unwrap();
        assert_eq!((c.sample_rate(), c.channels(), c.sample_format()), (96_000, 2, cpal::SampleFormat::F32));
        let c = pick_input_config(&configs, 48_000).unwrap();
        assert_eq!((c.sample_rate(), c.channels(), c.sample_format()), (48_000, 8, cpal::SampleFormat::F32), "more channels win in the same format");
        assert!(pick_input_config(&configs, 192_001).is_none(), "an unsupported rate falls back to the device default");
    }

    fn recorder(shared: &Arc<Shared>) -> Recorder {
        Recorder {
            shared: Arc::clone(shared),
            monitor: InputRing::new(1, 48_000),
            #[cfg(not(target_os = "freebsd"))]
            _stream: None,
            device_name: "Mic In".into(),
            sample_rate: 48_000,
            channels: 1,
            writer: None,
            seen: (0, Instant::now()),
        }
    }

    /// A stalled armed input, a panicking callback and a lost device all surface as `Trouble`; a quiet input that is
    /// not recording does not.
    #[test]
    fn a_bug_a_stall_or_a_lost_device_is_trouble() {
        let shared = Arc::new(Shared::new(CaptureRing::new(64)));
        let mut r = recorder(&shared);
        shared.armed.store(true, Ordering::Release);
        assert_eq!(r.trouble(), None);
        r.seen.1 = Instant::now().checked_sub(STALL).unwrap();
        assert_eq!(r.trouble(), Some(Trouble::Device("“Mic In” stopped sending audio".into())));
        shared.guarded(|| {});
        assert_eq!(r.trouble(), None, "it sent again");
        shared.armed.store(false, Ordering::Release);
        r.seen.1 = Instant::now().checked_sub(STALL).unwrap();
        assert_eq!(r.trouble(), None, "a quiet input that isn't recording is fine");
        shared.guarded(|| std::panic::panic_any("a bug in the input"));
        let mut ran = false;
        shared.guarded(|| ran = true);
        assert!(!ran);
        assert_eq!(r.trouble(), Some(Trouble::Device("the audio input hit an internal error (see the log)".into())));
        let gone = Arc::new(Shared::new(CaptureRing::new(64)));
        gone.fault.store(GONE, Ordering::Relaxed);
        assert_eq!(recorder(&gone).trouble(), Some(Trouble::Device("“Mic In” is unavailable".into())));
    }

    #[test]
    fn a_failed_write_is_trouble_while_still_recording() {
        let dir = std::env::temp_dir().join(format!("soundcraft-failed-take-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut take = TakeWriter::create(TrackId(1), vec![0], dir.join("Vox_01.wav"), 48_000).unwrap();
        take.file = File::open(&take.path).unwrap();
        let shared = Arc::new(Shared::new(CaptureRing::new(1 << 12)));
        shared.armed.store(true, Ordering::Release);
        let mut r = recorder(&shared);
        let sh = Arc::clone(&shared);
        let writer = std::thread::spawn(move || write_takes(&sh, vec![take], 1));
        shared.capture(&[0.5; 480], &mut 0);
        let trouble = (0..300).find_map(|_| {
            std::thread::sleep(Duration::from_millis(10));
            r.trouble()
        });
        assert!(matches!(&trouble, Some(Trouble::Disk(e)) if e.contains("Vox_01.wav")), "{trouble:?}");
        assert_eq!(r.trouble(), None, "reported once");
        shared.armed.store(false, Ordering::Release);
        assert!(writer.join().unwrap().1.is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn takes_stream_into_their_files() {
        let dir = std::env::temp_dir().join(format!("soundcraft-takes-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let gtr = TakeWriter::create(TrackId(1), vec![1, 2], dir.join("Gtr_01.wav"), 48_000).unwrap();
        let mut vox = TakeWriter::create(TrackId(2), vec![0], dir.join("Vox_01.wav"), 48_000).unwrap();
        assert!(TakeWriter::create(TrackId(3), vec![0], dir.join("Vox_01.wav"), 48_000).is_err(), "never overwrites a file");
        vox.write(&[0.5; 300], 3);
        vox.write_header();
        vox.write(&[0.25; 150], 3);
        assert_eq!(decode(&dir.join("Vox_01.wav"))[0].len(), 100, "the header covers what it has seen");

        let shared = Arc::new(Shared::new(CaptureRing::new(1 << 16)));
        shared.armed.store(true, Ordering::Release);
        let sh = Arc::clone(&shared);
        let writer = std::thread::spawn(move || write_takes(&sh, vec![gtr, vox], 3));
        let input: Vec<f32> = (0..3 * 4_800).map(|i| i as f32 / 20_000.0).collect();
        let mut owed = 0;
        input.chunks(3 * 480).for_each(|block| shared.capture(block, &mut owed));
        shared.armed.store(false, Ordering::Release);
        let (done, problem) = writer.join().unwrap();
        let channel = |c: usize| input.iter().skip(c).step_by(3).copied().collect::<Vec<f32>>();
        assert_eq!((done.len(), problem), (2, None));
        assert_eq!(decode(&done[0].1), vec![channel(1), channel(2)]);
        assert_eq!(decode(&done[1].1), vec![[vec![0.5; 100], vec![0.25; 50], channel(0)].concat()]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_take_sums_itself_up_for_drawing_while_it_records() {
        let dir = std::env::temp_dir().join(format!("soundcraft-live-take-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut take = TakeWriter::create(TrackId(4), vec![2, 0], dir.join("Gtr_01.wav"), 48_000).unwrap();
        let input: Vec<f32> = (0..300).flat_map(|f| [f as f32, -1.0, -(f as f32)]).collect();
        take.write(&input[..3 * 100], 3);
        take.write(&input[3 * 100..], 3);
        let live = take.live.lock().unwrap().clone();
        assert_eq!(live, [[(-127.0, 0.0), (-255.0, -128.0), (-299.0, -256.0)], [(0.0, 127.0), (128.0, 255.0), (256.0, 299.0)]]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// The reading side of an [`InputRing`]: the read position and how well the streams keep step.
struct Reader {
    /// Fractional position (input frames) of the next output sample past the ring's tail.
    phase: f64,
    /// Reading has started: enough input is buffered. Cleared when the input runs dry.
    primed: bool,
    /// Extra frames kept in hand, grown each time the input ran dry.
    extra: usize,
    /// Fewest frames left after a read in the current half-second window, and its length so far.
    low: usize,
    window: usize,
    /// Largest reserve asked for in that window: what `low` is compared with.
    target: usize,
    /// Read speed correction (±0.5 %) that holds the reserve steady across two device clocks.
    trim: f64,
    /// The steady part of `trim`: the clock difference learned so far.
    drift: f64,
}

/// Live input from the input callback to the output callback, for input monitoring. The samples
/// cross lock-free; the reader's own state sits behind a lock only the output callback tries.
/// The reader keeps a small reserve and steers its speed to hold it; it also converts the rate.
pub struct InputRing {
    slots: Box<[AtomicU32]>,
    mask: usize,
    /// Samples written and samples read so far (wrapping).
    head: AtomicUsize,
    tail: AtomicUsize,
    pub channels: AtomicUsize,
    /// Input device sample rate in Hz (0 when unknown, which reads as "same as the session").
    rate: u32,
    /// Largest block the input callback delivered (frames).
    push_block: AtomicUsize,
    /// Times the input ran dry while being read (each is a gap in what is heard).
    underruns: AtomicUsize,
    /// The input found the ring full and dropped a block: what is buffered is stale.
    overflowed: AtomicBool,
    /// Where the samples written stood (`head`) when the last block was dropped.
    dropped_at: AtomicUsize,
    /// Peak of each input channel since [`InputRing::take_peaks`] (bits of a non-negative f32).
    peaks: Box<[AtomicU32]>,
    /// Only the reader locks this (`try_lock`, never contended in normal running).
    reader: Mutex<Reader>,
}

impl InputRing {
    /// Frames per callback planned for when the input's callback size is not known: the largest
    /// buffer size Setup offers.
    pub const LARGEST_CALLBACK: usize = 8192;

    /// A ring for an input whose callback size is not known.
    pub fn new(channels: usize, rate: u32) -> Arc<InputRing> {
        InputRing::sized(channels, rate, InputRing::LARGEST_CALLBACK)
    }

    /// A ring for an input delivering `callback_frames` frames per callback (at most 65 536 are
    /// planned for). Allocated whole here, before the stream starts.
    pub fn sized(channels: usize, rate: u32, callback_frames: usize) -> Arc<InputRing> {
        let ch = channels.max(1);
        // Half a second of input, and at least room for the reader's start (a read, the reserve
        // and the extra it may grow) with the next callbacks behind it: far more than the reader
        // keeps, room for a stalled reader.
        let half_second = if rate == 0 { 24_000 } else { rate as usize / 2 };
        let callbacks = callback_frames.clamp(1, 65_536).saturating_mul(8).saturating_add(8_192);
        let frames = half_second.max(callbacks).clamp(4_096, 1 << 20);
        let n = frames.saturating_mul(ch).next_power_of_two();
        Arc::new(InputRing {
            slots: (0..n).map(|_| AtomicU32::new(0)).collect(),
            mask: n - 1,
            head: AtomicUsize::new(0),
            tail: AtomicUsize::new(0),
            channels: AtomicUsize::new(ch),
            rate,
            push_block: AtomicUsize::new(0),
            underruns: AtomicUsize::new(0),
            overflowed: AtomicBool::new(false),
            dropped_at: AtomicUsize::new(0),
            peaks: (0..ch.min(64)).map(|_| AtomicU32::new(0)).collect(),
            reader: Mutex::new(Reader { phase: 0.0, primed: false, extra: 0, low: usize::MAX, window: 0, target: 0, trim: 0.0, drift: 0.0 }),
        })
    }

    /// Frames per callback the input device delivers (0 before the first).
    pub fn input_block(&self) -> usize {
        self.push_block.load(Ordering::Relaxed)
    }

    /// The peak level of each input channel since the last call (linear, 1.0 is full scale).
    pub fn take_peaks(&self) -> Vec<f32> {
        self.peaks.iter().map(|p| f32::from_bits(p.swap(0, Ordering::Relaxed))).collect()
    }

    /// How often the input ran dry while it was being listened to.
    pub fn underruns(&self) -> usize {
        self.underruns.load(Ordering::Relaxed)
    }

    /// Input callback: queue interleaved samples. A block that finds the ring full is dropped.
    pub fn push(&self, samples: &[f32]) {
        let ch = self.channels.load(Ordering::Relaxed).max(1);
        // Levels for the input meters, also of a block the full ring drops: the device still
        // receives it. Non-negative floats order like their bits.
        for (c, peak) in self.peaks.iter().enumerate() {
            let top = samples.iter().skip(c).step_by(ch).fold(0.0f32, |m, s| m.max(finite(*s).abs()));
            peak.fetch_max(top.to_bits(), Ordering::Relaxed);
        }
        // The block size goes before the samples: the reader that sees them (the release store
        // to `head` below) sizes its reserve by it.
        self.push_block.fetch_max(samples.len() / ch, Ordering::Relaxed);
        let head = self.head.load(Ordering::Relaxed);
        let free = self.slots.len().saturating_sub(head.wrapping_sub(self.tail.load(Ordering::Acquire)));
        if samples.len() > free {
            self.dropped_at.store(head, Ordering::Relaxed);
            self.overflowed.store(true, Ordering::Release);
            return;
        }
        for (i, s) in samples.iter().enumerate() {
            if let Some(slot) = self.slots.get(head.wrapping_add(i) & self.mask) {
                slot.store(finite(*s).to_bits(), Ordering::Relaxed);
            }
        }
        self.head.store(head.wrapping_add(samples.len()), Ordering::Release);
    }

    fn sample(&self, at: usize) -> f32 {
        self.slots.get(at & self.mask).map_or(0.0, |s| f32::from_bits(s.load(Ordering::Relaxed)))
    }

    /// Output callback: reading stops for a while. Drops what is buffered, so that the input
    /// from before the pause is not what is heard when reading resumes; the input that arrives
    /// during the pause is kept.
    pub fn pause(&self) {
        // A block dropped before now belongs to the input given up here, not to what comes
        // next: its notice is taken back. One the input callback raises this instant is told by
        // where it was dropped (see `pop_into`).
        self.overflowed.store(false, Ordering::Release);
        // Only the output callback moves `tail`, and this is the output callback.
        self.tail.store(self.head.load(Ordering::Acquire), Ordering::Release);
        self.resync();
    }

    /// Start afresh at the newest input the next time it is read (after a pause in reading).
    pub fn resync(&self) {
        if let Ok(mut r) = self.reader.try_lock() {
            r.primed = false;
        }
    }

    /// Output callback: fill `out` (planar, at `session_rate`) with `frames` frames, one read of
    /// an output callback that takes `callback` frames in all (in as many reads as it needs, with
    /// no input arriving between them). Silence until enough input is buffered, and where it
    /// runs dry.
    pub fn pop_into(&self, out: &mut [Vec<f32>], frames: usize, callback: usize, session_rate: u32) {
        let ch = self.channels.load(Ordering::Relaxed).max(1);
        for c in out.iter_mut() {
            c.iter_mut().take(frames).for_each(|x| *x = 0.0);
        }
        let Ok(mut r) = self.reader.try_lock() else { return };
        // Input frames consumed per output frame.
        let ratio = if self.rate == 0 || session_rate == 0 { 1.0 } else { f64::from(self.rate) / f64::from(session_rate) };
        // Before `head`: when a block was dropped, the `head` read next covers all the input
        // published before the drop, so none of it is kept and played next to fresh input.
        let overflowed = self.overflowed.swap(false, Ordering::Acquire);
        let head = self.head.load(Ordering::Acquire);
        let mut tail = self.tail.load(Ordering::Relaxed);
        // A block dropped where reading now starts was dropped for input that [`InputRing::pause`]
        // has given up since: what was written after it is fresh.
        let overflowed = overflowed && self.dropped_at.load(Ordering::Relaxed) != tail;
        let mut avail = head.wrapping_sub(tail) / ch;
        // What the whole output callback takes (not only this read of it), the input's own block,
        // and the reserve beyond the callback: a callback of the longer side and a little more,
        // so that a late input block is not a gap.
        let read = (frames as f64 * ratio).ceil() as usize + 2;
        let push = self.push_block.load(Ordering::Relaxed).max(1);
        // No more than the ring can hold, so that a ring filled by fresh input starts reading.
        let room = self.slots.len() / ch;
        // A callback too long for the ring (with an input block beside it) counts for what fits
        // twice, itself and its reserve, beside the input blocks around them: it is then heard
        // as well as the ring allows, where waiting for a start the ring cannot hold is never
        // heard. A callback the ring holds counts whole: reading that starts on less runs dry
        // at the end of every callback.
        let fits = room.saturating_sub(push.saturating_mul(2)).saturating_sub(r.extra).saturating_sub(64) / 2;
        let demand = ((frames.max(callback) as f64 * ratio).ceil() as usize).saturating_add(2);
        let need = if demand.saturating_add(push) <= room { demand } else { demand.min(fits).max(read) };
        // The ring is sized by the input's callback, not the output's. What is buffered before a
        // read (the start) or left after one (the reserve) leaves room for the input of one more
        // output callback and two blocks (one that comes early, one of headroom): a long output
        // callback would otherwise overflow the ring while it waits to start, drop it all and
        // wait again, and never be heard.
        let beside = room.saturating_sub(demand.max(push)).saturating_sub(push.saturating_mul(2));
        let reserve = need.max(push.saturating_add(2)).saturating_add(16).saturating_add(r.extra).min(beside);
        // Reading starts once the input covers this callback and the reserve, or as much of the
        // two as leaves that room; never less than the callback itself. What it then keeps is
        // the callback and the whole reserve, when that much is there.
        let start = need.saturating_add(reserve).min(beside).max(need);
        // The input side dropped a block: reading stood still and all of it is old. A ring that
        // is merely full holds fresh input that has not been read yet.
        if overflowed {
            tail = head;
            avail = 0;
            r.primed = false;
        }
        if !r.primed {
            // Wait for the start, then read from the newest input (with one more input block
            // when there is one).
            if avail < start {
                self.tail.store(tail, Ordering::Release);
                return;
            }
            let keep = need.saturating_add(reserve).saturating_add(push).min(need.saturating_add(beside)).min(avail);
            tail = head.wrapping_sub(keep * ch);
            avail = keep;
            *r = Reader { primed: true, extra: r.extra, phase: 0.0, low: usize::MAX, window: 0, target: 0, trim: r.drift, drift: r.drift };
        }
        let step = ratio * (1.0 + r.trim);
        for f in 0..frames {
            // Move past the input frames already read; keep the one at `phase` and the next.
            while r.phase >= 1.0 && avail >= 2 {
                tail = tail.wrapping_add(ch);
                avail -= 1;
                r.phase -= 1.0;
            }
            if r.phase >= 1.0 || avail < 2 {
                // Ran dry: a gap. Keep more in hand from now on (up to four input blocks more).
                r.primed = false;
                r.extra = r.extra.saturating_add(reserve / 2 + 1).min(push.saturating_mul(4).max(256));
                self.underruns.fetch_add(1, Ordering::Relaxed);
                break;
            }
            let fr = r.phase as f32;
            for c in 0..ch {
                let (a, b) = (self.sample(tail.wrapping_add(c)), self.sample(tail.wrapping_add(ch + c)));
                if let Some(x) = out.get_mut(c).and_then(|o| o.get_mut(f)) {
                    *x = a + (b - a) * fr;
                }
            }
            r.phase += step;
        }
        if r.primed {
            // Every half second, compare the least that was left after a read with the reserve:
            // the largest one the window asked for, so that a callback shorter than the others
            // does not make what the longer ones need look like a backlog.
            r.low = r.low.min(avail);
            r.window = r.window.saturating_add(frames);
            r.target = r.target.max(reserve);
            let window = (session_rate as usize / 2).max(1);
            if r.window >= window {
                let over = r.low as f64 - r.target as f64;
                if over > (push.max(need) + self.rate.max(1) as usize / 100) as f64 {
                    // Far behind (more than 10 ms late): drop the excess at once.
                    let skip = over as usize;
                    tail = tail.wrapping_add(skip.min(avail.saturating_sub(2)) * ch);
                    r.trim = r.drift;
                } else {
                    // The two clocks differ: learn by how much, and work the rest off quickly.
                    let rate = over / (window as f64 * ratio);
                    r.drift = (r.drift + 0.1 * rate).clamp(-0.005, 0.005);
                    r.trim = (r.drift + 0.5 * rate).clamp(-0.005, 0.005);
                }
                r.low = usize::MAX;
                r.window = 0;
                r.target = 0;
            }
        }
        self.tail.store(tail, Ordering::Release);
    }
}

#[cfg(test)]
mod ring_tests {
    use super::InputRing;

    fn sine(k: usize, rate: f64) -> f32 {
        (2.0 * std::f64::consts::PI * 1_000.0 * k as f64 / rate).sin() as f32
    }

    /// Runs input callbacks of `push` frames at `in_rate` (clock off by `drift`) against output
    /// callbacks of `pop` frames at 48 kHz, read as the output callback does: in blocks of at
    /// most 512 frames. Returns the output and how often the input ran dry.
    fn run(in_rate: u32, push: usize, pop: usize, seconds: f64, drift: f64) -> (Vec<f32>, usize) {
        run_ring(&InputRing::new(1, in_rate), in_rate, push, pop, seconds, drift)
    }

    /// [`run`] on a given mono ring.
    fn run_ring(ring: &InputRing, in_rate: u32, push: usize, pop: usize, seconds: f64, drift: f64) -> (Vec<f32>, usize) {
        let (mut out, mut block) = (Vec::new(), vec![vec![0.0f32; 512]]);
        let (mut pushed, mut k) = (0.0f64, 0usize);
        let total = (48_000.0 * seconds) as usize;
        while out.len() < total {
            // Input frames due by the end of this output block.
            let due = (out.len() + pop) as f64 * f64::from(in_rate) / 48_000.0 * (1.0 + drift);
            while pushed + push as f64 <= due {
                let data: Vec<f32> = (0..push).map(|i| sine(k + i, f64::from(in_rate))).collect();
                ring.push(&data);
                k += push;
                pushed += push as f64;
            }
            let mut left = pop;
            while left > 0 {
                let n = left.min(512);
                ring.pop_into(&mut block, n, pop, 48_000);
                out.extend_from_slice(&block[0][..n]);
                left -= n;
            }
        }
        (out, ring.underruns())
    }

    /// The largest jump between neighbouring samples: 0.131 at most for the sine, far more at a gap.
    fn roughness(out: &[f32]) -> f32 {
        let start = out.iter().position(|v| v.abs() > 0.5).unwrap_or(out.len());
        out.get(start..).unwrap_or_default().windows(2).map(|w| (w[1] - w[0]).abs()).fold(0.0, f32::max)
    }

    #[test]
    fn monitoring_is_continuous_for_small_and_large_buffers_on_one_clock() {
        for (rate, push, pop) in [(48_000, 64, 64), (48_000, 1024, 1024), (48_000, 256, 64), (48_000, 64, 512), (44_100, 441, 480), (96_000, 128, 64)]
        {
            let (out, dry) = run(rate, push, pop, 3.0, 0.0);
            assert_eq!(dry, 0, "{rate} Hz, {push} in, {pop} out");
            assert!(roughness(&out) < 0.14, "{rate} Hz, {push} in, {pop} out: {}", roughness(&out));
            assert!(out.iter().skip(48_000).any(|v| v.abs() > 0.9), "the input is heard");
        }
    }

    /// An output callback longer than a render block reads the ring several times with no input
    /// arriving in between: the reserve covers the whole callback, not one read of it. 8192
    /// frames out against 64-frame input blocks is the reviewed case.
    #[test]
    fn monitoring_is_continuous_for_output_callbacks_of_many_render_blocks() {
        for (rate, push, pop) in [(48_000, 64, 8192), (48_000, 256, 2048), (48_000, 64, 1024), (44_100, 128, 4096), (96_000, 64, 8192)] {
            let (out, dry) = run(rate, push, pop, 4.0, 0.0);
            assert_eq!(dry, 0, "{rate} Hz, {push} in, {pop} out");
            assert!(roughness(&out) < 0.14, "{rate} Hz, {push} in, {pop} out: {}", roughness(&out));
            assert!(out.iter().skip(96_000).any(|v| v.abs() > 0.9), "{rate} Hz, {push} in, {pop} out: the input is heard");
        }
    }

    /// An output callback that takes more input than the ring holds twice over (16 384 frames
    /// in one callback, the largest buffer played at double speed, from a ring of 32 768) is
    /// still heard.
    #[test]
    fn a_callback_too_long_for_the_ring_is_still_heard() {
        let ring = InputRing::sized(1, 48_000, 480);
        assert_eq!(ring.slots.len(), 32_768);
        let (out, _) = run_ring(&ring, 48_000, 480, 16_384, 4.0, 0.0);
        assert!(out.iter().any(|v| v.abs() > 0.9), "the input is never heard");
    }

    /// The ring is sized by the input's callback (64 frames: 32 768 slots), and an output
    /// callback of 12 288 frames asks for a start of two callbacks: with the input of the third
    /// that is more than the ring holds. The start leaves room for it, and the input is heard.
    #[test]
    fn an_output_callback_far_longer_than_the_inputs_is_heard() {
        let ring = InputRing::sized(1, 48_000, 64);
        assert_eq!(ring.slots.len(), 32_768);
        let (out, dry) = run_ring(&ring, 48_000, 64, 12_288, 4.0, 0.0);
        assert!(out.iter().any(|v| v.abs() > 0.9), "the input is never heard");
        assert_eq!(dry, 0);
        assert!(roughness(&out) < 0.14, "{}", roughness(&out));
    }

    /// An output callback of half the ring (16 384 frames, 32 768 slots): the limit on the start
    /// does not go below the callback itself, or every callback would run dry at its end.
    #[test]
    fn an_output_callback_of_half_the_ring_does_not_run_dry() {
        let ring = InputRing::sized(1, 48_000, 64);
        assert_eq!(ring.slots.len(), 32_768);
        let (out, dry) = run_ring(&ring, 48_000, 64, 16_384, 4.0, 0.0);
        assert!(out.iter().any(|v| v.abs() > 0.9), "the input is never heard");
        assert_eq!(dry, 0);
        assert!(roughness(&out) < 0.14, "{}", roughness(&out));
    }

    /// A block dropped before a pause says the input from before it is stale. That input is
    /// given up at the pause; the input since is fresh and is heard. A block dropped during the
    /// pause still means what is buffered is stale.
    #[test]
    fn a_block_dropped_before_a_pause_does_not_drop_the_input_since() {
        let ring = InputRing::new(1, 48_000);
        let mut out = vec![vec![0.0f32; 64]];
        for _ in 0..ring.slots.len() / 64 + 2 {
            ring.push(&[0.5; 64]);
        }
        ring.pause();
        for _ in 0..4 {
            ring.push(&[-0.5; 64]);
        }
        ring.pop_into(&mut out, 64, 64, 48_000);
        assert!(out[0].iter().all(|v| *v == -0.5), "the input since the pause is dropped: {:?}", out[0]);
        ring.pause();
        for _ in 0..ring.slots.len() / 64 + 2 {
            ring.push(&[0.25; 64]);
        }
        ring.pop_into(&mut out, 64, 64, 48_000);
        assert!(out[0].iter().all(|v| *v == 0.0), "input left to overflow during the pause is stale");
    }

    /// Input from before a pause in reading is not heard after it: one new block is not enough
    /// to start on, and the old input is not used to make up the rest.
    #[test]
    fn a_pause_drops_the_input_from_before_it() {
        let ring = InputRing::new(1, 48_000);
        let mut out = vec![vec![0.0f32; 64]];
        for _ in 0..4 {
            ring.push(&[0.5; 64]);
        }
        ring.pop_into(&mut out, 64, 64, 48_000);
        assert!(out[0].iter().all(|v| *v == 0.5), "the input is heard");
        ring.pause();
        ring.push(&[-0.5; 64]);
        ring.pop_into(&mut out, 64, 64, 48_000);
        assert!(out[0].iter().all(|v| *v == 0.0), "input from before the pause: {:?}", out[0]);
        for _ in 0..3 {
            ring.push(&[-0.5; 64]);
        }
        ring.pop_into(&mut out, 64, 64, 48_000);
        assert!(out[0].iter().all(|v| *v == -0.5), "the input since the pause is heard");
    }

    /// A few callbacks shorter than the others, the last of them closing the half-second window,
    /// do not make what the long ones keep in hand look like a backlog to be skipped. 48 kHz
    /// input into a 16 kHz session is the reviewed case.
    #[test]
    fn short_callbacks_among_long_ones_do_not_skip_input() {
        let ring = InputRing::new(1, 48_000);
        let (mut out, mut block) = (Vec::new(), vec![vec![0.0f32; 512]]);
        let mut k = 0usize;
        // 14 callbacks of 512 frames, then 64-frame ones across the end of the window (8000).
        for pop in std::iter::repeat_n(512usize, 14).chain(std::iter::repeat_n(64, 40)) {
            // Three input frames for each output frame: a ramp, so that a skip shows as a jump.
            for _ in 0..pop * 3 / 64 {
                let data: Vec<f32> = (k..k + 64).map(|i| i as f32).collect();
                ring.push(&data);
                k += 64;
            }
            ring.pop_into(&mut block, pop, pop, 16_000);
            out.extend_from_slice(&block[0][..pop]);
        }
        let start = out.iter().position(|v| *v != 0.0).unwrap_or(out.len());
        let heard = out.get(start..).unwrap_or_default();
        assert!(heard.len() > 8_000, "the input is heard: {}", heard.len());
        let jump = heard.windows(2).map(|w| w[1] - w[0]).fold(0.0, f32::max);
        assert!(jump < 3.5, "input was skipped: a jump of {jump} frames");
        assert_eq!(ring.underruns(), 0);
    }

    /// The input meters show a block that the full ring dropped: the device still receives it.
    #[test]
    fn a_full_ring_still_meters_the_input() {
        let ring = InputRing::new(1, 48_000);
        for _ in 0..ring.slots.len() / 128 + 10 {
            ring.push(&[0.0; 128]);
        }
        assert_eq!(ring.take_peaks(), vec![0.0]);
        ring.push(&[0.5; 128]);
        assert_eq!(ring.take_peaks(), vec![0.5], "nobody listens, the ring is full, and the input has signal");
    }

    #[test]
    fn monitoring_follows_an_input_clock_that_runs_fast_or_slow() {
        for drift in [0.0002, -0.0002, 0.001, -0.001] {
            let (out, dry) = run(48_000, 64, 64, 20.0, drift);
            assert_eq!(dry, 0, "drift {drift}");
            assert!(roughness(&out) < 0.14, "drift {drift}: {}", roughness(&out));
        }
    }

    #[test]
    fn the_reserve_is_about_one_block_of_each_side_not_the_whole_ring() {
        // A reader that comes late finds half a second waiting: it must start at the newest input.
        let ring = InputRing::new(1, 48_000);
        let ramp: Vec<f32> = (0..20_000).map(|i| i as f32).collect();
        for block in ramp.chunks(64) {
            ring.push(block);
        }
        let mut out = vec![vec![0.0f32; 64]];
        ring.pop_into(&mut out, 64, 64, 48_000);
        // 64 + 2 to read, that and 16 more in reserve, 64 for the next input block.
        assert_eq!(out[0][0], (20_000 - 212) as f32);
        assert_eq!(out[0][63], (20_000 - 212 + 63) as f32);
    }

    #[test]
    fn a_ring_left_full_starts_afresh_and_an_empty_one_is_silent() {
        let ring = InputRing::new(2, 48_000);
        let mut out = vec![vec![1.0f32; 64], vec![1.0f32; 64]];
        ring.pop_into(&mut out, 64, 64, 48_000);
        assert!(out.iter().flatten().all(|v| *v == 0.0));
        assert_eq!(ring.underruns(), 0, "waiting for input is not a gap");
        ring.push(&[0.1, -0.7, -0.3, 0.2]);
        assert_eq!((ring.take_peaks(), ring.take_peaks()), (vec![0.3, 0.7], vec![0.0, 0.0]), "a level per input channel");
        // Nobody reads until the ring overflows: old input must not be played back afterwards.
        for _ in 0..ring.slots.len() / 128 + 10 {
            ring.push(&[0.25; 128]);
        }
        ring.pop_into(&mut out, 64, 64, 48_000);
        assert!(out.iter().flatten().all(|v| *v == 0.0), "the stale input is dropped");
        for _ in 0..4 {
            ring.push(&[0.5, -0.5].repeat(64));
        }
        ring.pop_into(&mut out, 64, 64, 48_000);
        assert!(out[0].iter().all(|v| *v == 0.5) && out[1].iter().all(|v| *v == -0.5));
    }

    /// Input callbacks as large as Setup offers (8192 frames) at low device rates, where one
    /// callback is a large part of the ring or all of it: the input is heard, without gaps.
    /// Mono 16 kHz with 8192-frame callbacks into a 48 kHz session reading 512 frames is the
    /// reviewed case (eight callbacks, 48 reads between them).
    #[test]
    fn monitoring_is_heard_with_callbacks_as_large_as_the_ring() {
        for (rate, push) in [(16_000, 8192), (16_000, 4096), (22_050, 8192), (8_000, 4096), (8_000, 8192)] {
            let (out, dry) = run(rate, push, 512, 8.0 * 8192.0 / 16_000.0, 0.0);
            assert!(out.iter().any(|v| v.abs() > 0.9), "{rate} Hz, {push} in: the input is never heard");
            assert_eq!(dry, 0, "{rate} Hz, {push} in");
            assert!(roughness(&out) < 0.14, "{rate} Hz, {push} in: {}", roughness(&out));
        }
    }

    /// One callback that exactly fills the ring is fresh input, not the stale overflow of a
    /// reader that stood still: it is played, from its start.
    #[test]
    fn a_callback_that_fills_the_ring_is_fresh_not_stale() {
        let ring = InputRing::new(1, 16_000);
        let frames = ring.slots.len();
        let ramp: Vec<f32> = (1..=frames).map(|i| i as f32).collect();
        ring.push(&ramp);
        let mut out = vec![vec![0.0f32; 512]];
        let mut heard = Vec::new();
        for _ in 0..48 {
            ring.pop_into(&mut out, 512, 512, 48_000);
            heard.extend(out[0].iter().copied().filter(|v| *v != 0.0));
        }
        assert!(!heard.is_empty(), "a full first callback is dropped as stale");
        assert!(heard.windows(2).all(|w| w[1] >= w[0]), "played in order");
    }

    /// Whatever the device rate, channel count and callback size Setup can choose, a ring sized
    /// for that callback has room for the reader's start (this read and its reserve) plus the
    /// next input callback.
    #[test]
    fn the_ring_holds_the_readers_start_and_the_next_callback() {
        for rate in [8_000u32, 11_025, 16_000, 22_050, 44_100, 48_000, 96_000, 192_000] {
            for ch in [1usize, 2, 8] {
                for push in crate::devices::STANDARD_BUFFERS.iter().map(|b| *b as usize) {
                    holds_the_start(&InputRing::sized(ch, rate, push), rate, ch, push);
                }
            }
        }
    }

    /// A ring for an input whose callback size is not known holds the same for the largest
    /// callback Setup offers.
    #[test]
    fn the_default_ring_holds_the_largest_callback() {
        for rate in [8_000u32, 11_025, 16_000, 22_050, 44_100, 48_000, 96_000, 192_000] {
            for ch in [1usize, 2, 8] {
                holds_the_start(&InputRing::new(ch, rate), rate, ch, InputRing::LARGEST_CALLBACK);
            }
        }
    }

    fn holds_the_start(ring: &InputRing, rate: u32, ch: usize, push: usize) {
        let need = (512.0 * f64::from(rate) / 48_000.0).ceil() as usize + 2;
        let reserve = need.max(push + 2) + 16;
        let room = ring.slots.len() / ch;
        assert!(
            need + reserve + push < room,
            "{rate} Hz, {ch} ch, {push}-frame callbacks: start {} + callback {push} >= ring {room}",
            need + reserve
        );
    }

    /// NaN and infinities from the device monitor as silence.
    #[test]
    fn monitor_silences_nan_and_infinite_input() {
        let ring = InputRing::new(1, 48_000);
        // Enough for the reader's reserve, so that it reads.
        for _ in 0..4 {
            ring.push(&[0.5, f32::NAN, f32::INFINITY, -0.5].repeat(16));
        }
        let mut out = vec![vec![1.0f32; 64]];
        ring.pop_into(&mut out, 64, 64, 48_000);
        assert!(out[0].iter().all(|v| v.is_finite() && v.abs() <= 0.5), "{:?}", out[0]);
        assert!(out[0].iter().any(|v| *v > 0.25) && out[0].iter().any(|v| *v < -0.25), "{:?}", out[0]);
        assert!(ring.take_peaks().iter().all(|p| *p == 0.5));
    }
}
