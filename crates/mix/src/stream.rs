use craft_audio::{AudioBuffer, CodecId, Container, DecodeOptions, Decoder};
use soundcraft_model::SourceAudio;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError, RwLock, Weak};

/// Frames per block a streamed source is read and cached in.
pub const STREAM_BLOCK: usize = 1 << 15;
/// Most blocks one streamed source keeps in memory.
pub const STREAM_CACHE_BLOCKS: usize = 32;
/// Frames summed up by one (min, max) pair of a streamed source's [`StreamedAudio::overview`].
pub const OVERVIEW_FRAMES: usize = 256;
const MAX_STREAM_CHANNELS: u16 = 64;
const AHEAD: u64 = 6;
const CUED: u64 = 2;
const HINTS: usize = 4;
const IDLE: u64 = u64::MAX;
const WORKER_PERIOD: std::time::Duration = std::time::Duration::from_millis(4);

type Block = Arc<Vec<Vec<f32>>>;

struct Cache {
    blocks: Vec<(u64, Block, u64)>,
    tick: u64,
}

impl Cache {
    fn find(&self, idx: u64) -> Option<usize> {
        self.blocks.iter().position(|b| b.0 == idx)
    }

    fn touch(&mut self, at: usize) {
        self.tick = self.tick.wrapping_add(1);
        if let Some(b) = self.blocks.get_mut(at) {
            b.2 = self.tick;
        }
    }

    fn insert(&mut self, idx: u64, block: Block) -> Option<Block> {
        if let Some(at) = self.find(idx) {
            self.touch(at);
            return None;
        }
        let evicted = if self.blocks.len() >= STREAM_CACHE_BLOCKS {
            let oldest = self.blocks.iter().enumerate().min_by_key(|(_, b)| b.2).map(|(i, _)| i);
            oldest.map(|i| self.blocks.swap_remove(i).1)
        } else {
            None
        };
        self.tick = self.tick.wrapping_add(1);
        self.blocks.push((idx, block, self.tick));
        evicted
    }
}

/// A PCM file played from disk: blocks are read on demand into a bounded cache, and a worker
/// thread reads ahead of what the audio thread plays, so the audio thread never waits on a file.
pub struct StreamedAudio {
    path: PathBuf,
    ext: Option<String>,
    channels: usize,
    frames: u64,
    reader: Mutex<Option<Decoder<'static>>>,
    cache: Mutex<Cache>,
    hints: [AtomicU64; HINTS],
    next_hint: AtomicUsize,
    misses: AtomicU64,
    overview: Vec<Vec<(f32, f32)>>,
}

impl std::fmt::Debug for StreamedAudio {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StreamedAudio")
            .field("path", &self.path)
            .field("channels", &self.channels)
            .field("frames", &self.frames)
            .finish_non_exhaustive()
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

impl StreamedAudio {
    /// Frames per channel.
    pub fn frames(&self) -> u64 {
        self.frames
    }

    pub fn channels(&self) -> usize {
        self.channels
    }

    /// The file the audio is read from.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Per channel, the (min, max) of each run of [`OVERVIEW_FRAMES`] frames.
    pub fn overview(&self) -> &[Vec<(f32, f32)>] {
        &self.overview
    }

    /// Bytes of decoded samples the cache holds now.
    pub fn cached_bytes(&self) -> usize {
        lock(&self.cache).blocks.iter().map(|(_, b, _)| b.iter().map(|c| c.len() * size_of::<f32>()).sum::<usize>()).sum()
    }

    /// Bytes the overview holds.
    pub fn overview_bytes(&self) -> usize {
        self.overview.iter().map(|c| c.len() * size_of::<(f32, f32)>()).sum()
    }

    /// Samples the audio thread wanted before they were read: each played as silence.
    pub fn misses(&self) -> u64 {
        self.misses.load(Ordering::Relaxed)
    }

    fn open_decoder(&self) -> Result<Decoder<'static>, String> {
        let file = std::fs::File::open(&self.path).map_err(|e| e.to_string())?;
        Decoder::open(file, self.ext.as_deref(), &DecodeOptions::default()).map_err(|e| e.to_string())
    }

    fn read_block(&self, idx: u64) -> Option<Block> {
        let start = idx.checked_mul(STREAM_BLOCK as u64).filter(|&s| s < self.frames)?;
        let len = usize::try_from((self.frames - start).min(STREAM_BLOCK as u64)).ok()?;
        let mut reader = lock(&self.reader);
        if reader.is_none() {
            *reader = self.open_decoder().ok();
        }
        let dec = reader.as_mut()?;
        let read = read_frames(dec, start, len, self.channels);
        if read.is_none() {
            *reader = None;
        }
        read.map(Arc::new)
    }

    fn cached(&self, idx: u64) -> Option<Block> {
        let mut cache = lock(&self.cache);
        let at = cache.find(idx)?;
        cache.touch(at);
        cache.blocks.get(at).map(|b| Arc::clone(&b.1))
    }

    fn store(&self, idx: u64, block: Block) {
        let evicted = lock(&self.cache).insert(idx, block);
        drop(evicted);
    }

    fn load(&self, idx: u64) -> Option<Block> {
        if let Some(b) = self.cached(idx) {
            return Some(b);
        }
        let b = self.read_block(idx)?;
        self.store(idx, Arc::clone(&b));
        Some(b)
    }

    fn hint(&self, frame: u64) {
        let slot = 1 + self.next_hint.fetch_add(1, Ordering::Relaxed) % (HINTS - 1);
        if let Some(h) = self.hints.get(slot) {
            h.store(frame, Ordering::Relaxed);
        }
    }

    fn wanted(&self) -> Vec<u64> {
        let blocks = self.frames.div_ceil(STREAM_BLOCK as u64);
        let mut want = Vec::with_capacity(STREAM_CACHE_BLOCKS);
        for (n, h) in self.hints.iter().enumerate().skip(1).chain(self.hints.iter().enumerate().take(1)) {
            let frame = h.load(Ordering::Relaxed);
            if frame == IDLE {
                continue;
            }
            let at = frame / STREAM_BLOCK as u64;
            let ahead = if n == 0 { CUED } else { AHEAD };
            let behind = if n == 0 { None } else { at.checked_sub(1) };
            for idx in (at..=at.saturating_add(ahead)).chain(behind) {
                if idx < blocks && !want.contains(&idx) && want.len() < STREAM_CACHE_BLOCKS {
                    want.push(idx);
                }
            }
        }
        want
    }

    fn prefetch(&self) {
        let want = self.wanted();
        let missing: Vec<u64> = {
            let mut cache = lock(&self.cache);
            want.into_iter()
                .filter(|&idx| match cache.find(idx) {
                    Some(at) => {
                        cache.touch(at);
                        false
                    }
                    None => true,
                })
                .collect()
        };
        for idx in missing {
            if let Some(b) = self.read_block(idx) {
                self.store(idx, b);
            }
        }
    }

    /// Reads the block at `frame` now and keeps the blocks from there cached while the stream
    /// plays elsewhere: where playback starts, or a loop starts again.
    pub fn cue(&self, frame: u64) {
        if let Some(h) = self.hints.first() {
            h.store(frame, Ordering::Relaxed);
        }
        let _ = self.load(frame / STREAM_BLOCK as u64);
    }

    fn reader(&self, live: bool) -> Reader<'_> {
        if live {
            match self.cache.try_lock() {
                Ok(guard) => Reader::Live { st: self, cache: guard, last: usize::MAX },
                Err(_) => Reader::Busy(self),
            }
        } else {
            Reader::Blocking { st: self, cur: None }
        }
    }

    /// The whole file decoded into memory.
    pub fn decode_whole(&self) -> Result<AudioBuffer, String> {
        let len = usize::try_from(self.frames).map_err(|e| e.to_string())?;
        let mut dec = self.open_decoder()?;
        let channels = read_frames(&mut dec, 0, len, self.channels).ok_or_else(|| format!("{}: could not be read", self.path.display()))?;
        let rate = dec.info().sample_rate;
        Ok(AudioBuffer { sample_rate: rate, channels })
    }

    /// `len` frames from `start`, read from the file.
    pub fn decode_span(&self, start: u64, len: usize) -> Result<Vec<Vec<f32>>, String> {
        let start = start.min(self.frames);
        let len = len.min(usize::try_from(self.frames - start).unwrap_or(usize::MAX));
        let mut dec = self.open_decoder()?;
        read_frames(&mut dec, start, len, self.channels).ok_or_else(|| format!("{}: could not be read", self.path.display()))
    }
}

fn read_frames(dec: &mut Decoder<'static>, start: u64, len: usize, channels: usize) -> Option<Vec<Vec<f32>>> {
    if dec.seek(start).ok()? != start {
        return None;
    }
    let mut out: Vec<Vec<f32>> = (0..channels).map(|_| Vec::with_capacity(len)).collect();
    let mut have = 0;
    while have < len {
        let Some(b) = dec.next_block().ok()? else { break };
        let take = b.frames().min(len - have);
        for (o, c) in out.iter_mut().zip(b.channels.iter()) {
            o.extend_from_slice(c.get(..take).unwrap_or(c));
        }
        have += take;
    }
    (have == len && out.iter().all(|c| c.len() == len)).then_some(out)
}

enum Reader<'a> {
    Live { st: &'a StreamedAudio, cache: MutexGuard<'a, Cache>, last: usize },
    Busy(&'a StreamedAudio),
    Blocking { st: &'a StreamedAudio, cur: Option<(u64, Block)> },
}

impl Reader<'_> {
    fn at(&mut self, ch: usize, x: i64) -> f32 {
        let Ok(x) = u64::try_from(x) else { return 0.0 };
        let idx = x / STREAM_BLOCK as u64;
        let off = (x % STREAM_BLOCK as u64) as usize;
        let sample = |b: &Block| b.get(ch).and_then(|c| c.get(off)).copied().unwrap_or(0.0);
        match self {
            Reader::Live { st, cache, last } => {
                if x >= st.frames {
                    return 0.0;
                }
                if cache.blocks.get(*last).is_none_or(|b| b.0 != idx) {
                    match cache.find(idx) {
                        Some(at) => {
                            cache.touch(at);
                            *last = at;
                        }
                        None => {
                            st.misses.fetch_add(1, Ordering::Relaxed);
                            return 0.0;
                        }
                    }
                }
                cache.blocks.get(*last).map_or(0.0, |b| sample(&b.1))
            }
            Reader::Busy(st) => {
                if x < st.frames {
                    st.misses.fetch_add(1, Ordering::Relaxed);
                }
                0.0
            }
            Reader::Blocking { st, cur } => {
                if x >= st.frames {
                    return 0.0;
                }
                if cur.as_ref().is_none_or(|c| c.0 != idx) {
                    *cur = st.load(idx).map(|b| (idx, b));
                }
                cur.as_ref().map_or(0.0, |c| sample(&c.1))
            }
        }
    }
}

type Streams = Vec<(Weak<SourceAudio>, Arc<StreamedAudio>)>;

static STREAMS: RwLock<Streams> = RwLock::new(Vec::new());
static WORKER: OnceLock<bool> = OnceLock::new();

fn is_stub(audio: &SourceAudio) -> bool {
    !audio.buffer.channels.is_empty() && audio.buffer.channels.iter().all(Vec::is_empty)
}

fn find(streams: &Streams, audio: &SourceAudio) -> Option<Arc<StreamedAudio>> {
    streams.iter().find(|(w, _)| std::ptr::eq(w.as_ptr(), audio) && w.strong_count() > 0).map(|(_, s)| Arc::clone(s))
}

/// The disk stream behind `audio`, when it is played from its file rather than from memory.
pub fn streamed(audio: &SourceAudio) -> Option<Arc<StreamedAudio>> {
    if !is_stub(audio) {
        return None;
    }
    find(&STREAMS.read().unwrap_or_else(PoisonError::into_inner), audio)
}

fn with_streamed<R>(audio: &SourceAudio, f: impl FnOnce(&StreamedAudio) -> R) -> Option<R> {
    if !is_stub(audio) {
        return None;
    }
    let streams = if crate::on_audio_thread() { STREAMS.try_read().ok()? } else { STREAMS.read().unwrap_or_else(PoisonError::into_inner) };
    let st = streams.iter().find(|(w, _)| std::ptr::eq(w.as_ptr(), audio) && w.strong_count() > 0).map(|(_, s)| s)?;
    Some(f(st))
}

/// Frames per channel of `audio`, streamed or in memory.
pub fn source_frames(audio: &SourceAudio) -> usize {
    match streamed(audio) {
        Some(st) => usize::try_from(st.frames()).unwrap_or(usize::MAX),
        None => audio.frames(),
    }
}

/// `audio` with all of its samples in memory: the same audio when it is, else its file decoded
/// whole. For edits and exports that need every sample at once.
pub fn whole(audio: &Arc<SourceAudio>) -> Result<Arc<SourceAudio>, String> {
    match streamed(audio) {
        Some(st) => st.decode_whole().map(|b| Arc::new(SourceAudio::new(b))),
        None => Ok(Arc::clone(audio)),
    }
}

/// `len` frames of every channel of `audio` from frame `start`, shorter at the end of the audio.
pub fn source_span(audio: &SourceAudio, start: usize, len: usize) -> Result<Vec<Vec<f32>>, String> {
    match streamed(audio) {
        Some(st) => st.decode_span(start as u64, len),
        None => Ok(audio.buffer.channels.iter().map(|c| c.iter().skip(start).take(len).copied().collect()).collect()),
    }
}

/// Cues the stream of every audio clip that plays within a few seconds of `pos`
/// ([`StreamedAudio::cue`]): called where playback starts, before it does.
pub fn cue_streams(s: &soundcraft_model::Session, pos: soundcraft_time::Samples) {
    let Ok(streams) = STREAMS.read() else { return };
    if streams.is_empty() {
        return;
    }
    for t in &s.tracks {
        for c in t.clips() {
            let soundcraft_model::ClipContent::Audio { source, offset } = c.content else { continue };
            if pos >= c.end() || c.start > pos.saturating_add((AHEAD * STREAM_BLOCK as u64) as i64) {
                continue;
            }
            let Some(st) = s.pool.get(source).filter(|a| is_stub(a)).and_then(|a| find(&streams, a)) else { continue };
            let stretch = if c.stretch.is_finite() && c.stretch > 0.0 { c.stretch } else { 1.0 };
            let rel = pos.saturating_sub(c.start).max(0) as f64 / stretch;
            st.cue((offset as f64 + rel).max(0.0) as u64);
        }
    }
}

fn worker() {
    loop {
        let live: Vec<Arc<StreamedAudio>> =
            STREAMS.read().unwrap_or_else(PoisonError::into_inner).iter().filter(|(w, _)| w.strong_count() > 0).map(|(_, s)| Arc::clone(s)).collect();
        for st in &live {
            st.prefetch();
        }
        drop(live);
        std::thread::sleep(WORKER_PERIOD);
    }
}

/// Opens the PCM WAV or AIFF file at `path` to be played from disk, when it holds at least
/// `min_samples` samples (frames × channels) at `rate`. `Ok(None)` for any other file: decode
/// it whole instead. The returned audio holds no samples; its waveform overview is built here,
/// in one pass over the file.
pub fn open_streamed(path: &Path, rate: u32, min_samples: u64) -> Result<Option<Arc<SourceAudio>>, String> {
    let ext = path.extension().and_then(OsStr::to_str).map(str::to_ascii_lowercase);
    if !std::fs::metadata(path).map_err(|e| e.to_string())?.is_file() {
        return Ok(None);
    }
    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut dec = Decoder::open(file, ext.as_deref(), &DecodeOptions::default()).map_err(|e| e.to_string())?;
    let info = dec.info().clone();
    let pcm = matches!(info.container, Container::Wav | Container::Aiff) && info.codec == CodecId::Pcm;
    let big = info.frames.saturating_mul(u64::from(info.channels)) >= min_samples;
    if !pcm || !big || !dec.length_known() || info.sample_rate != rate || info.channels == 0 || info.channels > MAX_STREAM_CHANNELS {
        return Ok(None);
    }
    let channels = usize::from(info.channels);
    let mut overview: Vec<Vec<(f32, f32)>> = (0..channels).map(|_| Vec::new()).collect();
    let mut run = vec![(f32::INFINITY, f32::NEG_INFINITY); channels];
    let (mut frames, mut filled) = (0u64, 0usize);
    while let Some(b) = dec.next_block().map_err(|e| e.to_string())? {
        if b.channels.len() != channels {
            return Ok(None);
        }
        for i in 0..b.frames() {
            for ((r, c), o) in run.iter_mut().zip(&b.channels).zip(overview.iter_mut()) {
                let v = c.get(i).copied().unwrap_or(0.0);
                *r = (r.0.min(v), r.1.max(v));
                if filled + 1 == OVERVIEW_FRAMES {
                    o.push(*r);
                    *r = (f32::INFINITY, f32::NEG_INFINITY);
                }
            }
            filled = (filled + 1) % OVERVIEW_FRAMES;
        }
        frames = frames.saturating_add(b.frames() as u64);
    }
    if filled > 0 {
        for (r, o) in run.iter().zip(overview.iter_mut()) {
            o.push(*r);
        }
    }
    let stub = Arc::new(SourceAudio { buffer: AudioBuffer { sample_rate: rate, channels: vec![Vec::new(); channels] }, peaks: Vec::new() });
    let st = Arc::new(StreamedAudio {
        path: path.to_path_buf(),
        ext,
        channels,
        frames,
        reader: Mutex::new(Some(dec)),
        cache: Mutex::new(Cache { blocks: Vec::with_capacity(STREAM_CACHE_BLOCKS + 1), tick: 0 }),
        hints: std::array::from_fn(|_| AtomicU64::new(IDLE)),
        next_hint: AtomicUsize::new(0),
        misses: AtomicU64::new(0),
        overview,
    });
    {
        let mut streams = STREAMS.write().unwrap_or_else(PoisonError::into_inner);
        streams.retain(|(w, _)| w.strong_count() > 0);
        streams.push((Arc::downgrade(&stub), st));
    }
    WORKER.get_or_init(|| std::thread::Builder::new().name("soundcraft-stream".into()).spawn(worker).is_ok());
    Ok(Some(stub))
}

pub(crate) fn render_streamed(audio: &SourceAudio, each: impl FnOnce(&mut dyn FnMut(usize, i64) -> f32), first: i64) -> bool {
    with_streamed(audio, |st| {
        let live = crate::on_audio_thread();
        if live {
            st.hint(u64::try_from(first).unwrap_or(0));
        }
        let mut r = st.reader(live);
        each(&mut |ch, x| r.at(ch, x));
    })
    .is_some()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{render_clips, render_range};
    use craft_audio::{BitDepth, EncodeFormat, EncodeOptions};
    use soundcraft_model::{ChannelFormat, Clip, Session, SourceId, TrackId, TrackKind};
    use soundcraft_time::Range;

    const FRAMES: usize = 3 * STREAM_BLOCK + 777;

    struct Scratch(PathBuf);

    impl Scratch {
        fn new(tag: &str) -> Scratch {
            let dir = std::env::temp_dir().join(format!("soundcraft-mix-stream-{tag}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Scratch(dir)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn noise(seed: u32, n: usize) -> Vec<f32> {
        let mut x = seed.wrapping_mul(2_654_435_761).max(1);
        (0..n)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 17;
                x ^= x << 5;
                (x as f32 / u32::MAX as f32) * 1.8 - 0.9
            })
            .collect()
    }

    fn write_wav(dir: &Path, depth: BitDepth, ext: &str, format: EncodeFormat) -> (PathBuf, Arc<SourceAudio>) {
        let buf = AudioBuffer { sample_rate: 48_000, channels: vec![noise(1, FRAMES), noise(2, FRAMES)] };
        let bytes = craft_audio::encode(&buf, &EncodeOptions::new(format).with_bit_depth(depth)).unwrap();
        let path = dir.join(format!("take.{ext}"));
        std::fs::write(&path, &bytes).unwrap();
        let (_, decoded) = craft_audio::decode(&bytes, Some(ext)).unwrap();
        (path, Arc::new(SourceAudio::new(decoded)))
    }

    fn session(audio: Arc<SourceAudio>) -> (Session, TrackId) {
        let mut s = Session::default();
        let t = s.add_track(TrackKind::Audio, ChannelFormat::Stereo, None);
        s.pool.insert(SourceId(7), audio);
        let mut clips = vec![
            Clip::audio(s.new_clip_id(), "a", SourceId(7), 1_000, 0, (2 * STREAM_BLOCK) as i64),
            Clip::audio(s.new_clip_id(), "b", SourceId(7), 2 * STREAM_BLOCK as i64 - 50, 3 * STREAM_BLOCK as i64, FRAMES as i64),
            Clip::audio(s.new_clip_id(), "c", SourceId(7), 500, 6 * STREAM_BLOCK as i64, 40_000),
        ];
        clips[0].fade_in.len = 4_000;
        clips[0].fade_out.len = 9_000;
        clips[0].gain_db = -3.0;
        clips[2].stretch = 0.73;
        if let Some(pl) = s.track_mut(t).and_then(|t| t.playlist_mut()) {
            pl.clips = clips;
        }
        (s, t)
    }

    fn bound(st: &StreamedAudio) -> usize {
        STREAM_CACHE_BLOCKS * STREAM_BLOCK * st.channels() * size_of::<f32>()
    }

    #[test]
    fn a_streamed_file_renders_sample_for_sample_what_its_decoded_audio_does() {
        let dir = Scratch::new("exact");
        for (depth, ext, format) in
            [(BitDepth::Int24, "wav", EncodeFormat::Wav), (BitDepth::Int16, "aif", EncodeFormat::Aiff), (BitDepth::Float32, "wav", EncodeFormat::Wav)]
        {
            let (path, decoded) = write_wav(&dir.0, depth, ext, format);
            let stub = open_streamed(&path, 48_000, 0).unwrap().expect("a PCM file at the session rate streams");
            assert!(stub.buffer.channels.iter().all(Vec::is_empty), "a streamed source holds no samples");
            let st = streamed(&stub).unwrap();
            assert_eq!((st.frames(), st.channels()), (FRAMES as u64, 2));
            assert_eq!(source_frames(&stub), FRAMES);
            let (full, t) = session(decoded.clone());
            let (disk, _) = session(Arc::clone(&stub));
            let range = Range::new(0, 10 * STREAM_BLOCK as i64);
            let want = render_clips(&full, t, range);
            assert!(want[0].iter().any(|v| v.abs() > 0.1));
            assert_eq!(render_clips(&disk, t, range), want, "{ext} {depth:?}: render_clips");
            assert_eq!(render_range(&disk, range, 512), render_range(&full, range, 512), "{ext} {depth:?}: bounce");
            assert!(st.cached_bytes() <= bound(&st), "{} cached, more than {}", st.cached_bytes(), bound(&st));
            assert_eq!(whole(&stub).unwrap().buffer.channels, decoded.buffer.channels);
            assert_eq!(source_span(&stub, 5, 70_000).unwrap(), source_span(&decoded, 5, 70_000).unwrap());
            let over = &st.overview()[1];
            assert_eq!(over.len(), FRAMES.div_ceil(OVERVIEW_FRAMES));
            let last = decoded.buffer.channels[1].chunks(OVERVIEW_FRAMES).last().unwrap();
            let lo = last.iter().copied().fold(f32::INFINITY, f32::min);
            assert_eq!(over.last().copied(), Some((lo, last.iter().copied().fold(f32::NEG_INFINITY, f32::max))));
        }
    }

    #[test]
    fn short_compressed_or_resampled_files_are_not_streamed() {
        let dir = Scratch::new("refused");
        let (wav, _) = write_wav(&dir.0, BitDepth::Int24, "wav", EncodeFormat::Wav);
        assert!(open_streamed(&wav, 44_100, 0).unwrap().is_none(), "another rate is converted in memory");
        assert!(open_streamed(&wav, 48_000, 2 * FRAMES as u64 + 1).unwrap().is_none(), "below the threshold");
        let (flac, _) = write_wav(&dir.0, BitDepth::Int24, "flac", EncodeFormat::Flac);
        assert!(open_streamed(&flac, 48_000, 0).unwrap().is_none(), "compressed files decode whole");
        let mem = Arc::new(SourceAudio::new(AudioBuffer { sample_rate: 48_000, channels: vec![vec![0.5; 10]] }));
        assert!(streamed(&mem).is_none());
        assert!(Arc::ptr_eq(&whole(&mem).unwrap(), &mem));
    }

    #[test]
    fn the_audio_thread_plays_only_cached_blocks_and_never_reads_the_file() {
        let dir = Scratch::new("live");
        let (path, decoded) = write_wav(&dir.0, BitDepth::Int24, "wav", EncodeFormat::Wav);
        let stub = open_streamed(&path, 48_000, 0).unwrap().unwrap();
        let st = streamed(&stub).unwrap();
        let (disk, t) = session(Arc::clone(&stub));
        let (full, _) = session(decoded);
        let at = STREAM_BLOCK as i64 + 100;
        let block = (at as u64 + 1_000) / STREAM_BLOCK as u64;
        let range = Range::new(at, at + 1024);
        let want = render_clips(&full, t, range);
        assert!(want[0].iter().any(|v| v.abs() > 0.1));
        let live = |s: Session| {
            std::thread::spawn(move || {
                crate::mark_audio_thread();
                render_clips(&s, t, range)
            })
            .join()
            .unwrap()
        };
        lock(&st.cache).blocks.clear();
        let worker_period = WORKER_PERIOD;
        {
            let _hold = lock(&st.reader);
            let cold = live(disk.clone());
            assert!(cold.iter().flatten().all(|&v| v == 0.0), "nothing cached: silence, not a read on the audio thread");
            assert!(st.misses() > 0);
        }
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while lock(&st.cache).find(block).is_none() && std::time::Instant::now() < deadline {
            std::thread::sleep(worker_period);
        }
        assert_eq!(live(disk.clone()), want, "the reader thread fetched what the audio thread asked for");
        lock(&st.cache).blocks.clear();
        cue_streams(&disk, at);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while lock(&st.cache).find(block).is_none() && std::time::Instant::now() < deadline {
            std::thread::sleep(worker_period);
        }
        assert_eq!(live(disk), want, "a cue fetches blocks before playback asks");
        assert!(st.cached_bytes() <= bound(&st));
    }
}
