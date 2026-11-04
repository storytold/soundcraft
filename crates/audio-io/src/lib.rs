//! SoundCraft audio file I/O.
//!
//! Bytes in, bytes out (no filesystem access, so the crate also builds for `wasm32`):
//!
//! * [`detect_format`] / [`probe`] / [`decode`] read WAV (PCM 8/16/24/32, float 32/64,
//!   `WAVE_FORMAT_EXTENSIBLE`, RF64/BW64, BWF `bext`) and AIFF/AIFF-C (big-endian, `sowt`,
//!   `fl32`/`fl64`) with native readers, and everything else (FLAC, MP3, Ogg Vorbis, AAC/ALAC in
//!   MP4, CAF, Matroska, ADPCM/A-law/µ-law WAV, ...) through `symphonia`.
//! * [`encode`] writes WAV/BWF (RF64 above 4 GiB), AIFF / AIFF-C `fl32`, and FLAC (built-in
//!   encoder), with optional TPDF dither.
//! * [`wav_float_header`] heads a float WAV written while it grows (recording).
//! * [`peaks`] builds multi-resolution min/max overviews for waveform drawing.
//! * [`FileEncoder`] writes the same formats block by block into any `Write + Seek`, so a file
//!   of any length never has to fit in memory.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

mod aiff;
mod buffer;
mod caf;
mod flac;
mod mp4;
mod pcm;
pub mod peaks;
mod symph;
mod wav;

pub use buffer::AudioBuffer;
use std::io::{Seek, Write};

/// Errors from probing, decoding and encoding.
#[derive(Debug, thiserror::Error)]
pub enum AudioError {
    /// The format, codec or option is valid but not supported.
    #[error("unsupported audio: {0}")]
    Unsupported(String),
    /// The input is corrupt or not what it claims to be.
    #[error("malformed audio: {0}")]
    Malformed(String),
    /// The input (or requested output) exceeds a size limit.
    #[error("audio too large: {0}")]
    TooLarge(String),
    /// Encoding failed (e.g. an empty buffer).
    #[error("audio encode failed: {0}")]
    Encode(String),
    /// Writing the output failed.
    #[error("audio write failed: {0}")]
    Io(#[from] std::io::Error),
}

/// Result alias for this crate.
pub type Result<T> = std::result::Result<T, AudioError>;

/// Container / codec family of an audio file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum FileFormat {
    Wav,
    Aiff,
    Flac,
    Mp3,
    Ogg,
    Aac,
    Alac,
    Caf,
    /// Any other container `symphonia` reads: Matroska and WebM by name.
    Other,
}

impl FileFormat {
    /// The formats [`decode`] reads, found by extension. Contents decide when they disagree.
    pub const READ: &'static [FileFormat] =
        &[FileFormat::Wav, FileFormat::Aiff, FileFormat::Flac, FileFormat::Mp3, FileFormat::Ogg, FileFormat::Aac, FileFormat::Caf, FileFormat::Other];
    /// The formats [`encode`] writes.
    pub const WRITE: &'static [FileFormat] = &[FileFormat::Wav, FileFormat::Aiff, FileFormat::Flac];

    /// The extensions that name this format, lower case and without the dot; the first is the one
    /// a new file gets. ALAC shares AAC's MP4 container, so a `.m4a` name reads as AAC until the
    /// contents say otherwise.
    pub fn extensions(self) -> &'static [&'static str] {
        match self {
            FileFormat::Wav => &["wav", "wave", "bwf", "rf64"],
            FileFormat::Aiff => &["aif", "aiff", "aifc"],
            FileFormat::Flac => &["flac"],
            FileFormat::Mp3 => &["mp3", "mp2", "mp1"],
            FileFormat::Ogg => &["ogg", "oga"],
            FileFormat::Aac => &["m4a", "m4b", "aac", "mp4", "adts"],
            FileFormat::Alac => &["m4a"],
            FileFormat::Caf => &["caf"],
            FileFormat::Other => &["mkv", "webm"],
        }
    }

    /// The name people know the format by, as menus show it.
    pub fn name(self) -> &'static str {
        match self {
            FileFormat::Wav => "WAV",
            FileFormat::Aiff => "AIFF",
            FileFormat::Flac => "FLAC",
            FileFormat::Mp3 => "MP3",
            FileFormat::Ogg => "Ogg Vorbis",
            FileFormat::Aac => "AAC",
            FileFormat::Alac => "ALAC",
            FileFormat::Caf => "CAF",
            FileFormat::Other => "Other",
        }
    }

    /// The media type a file of this format is served or downloaded as.
    pub fn mime_type(self) -> &'static str {
        match self {
            FileFormat::Wav => "audio/wav",
            FileFormat::Aiff => "audio/aiff",
            FileFormat::Flac => "audio/flac",
            FileFormat::Mp3 => "audio/mpeg",
            FileFormat::Ogg => "audio/ogg",
            FileFormat::Aac | FileFormat::Alac => "audio/mp4",
            FileFormat::Caf => "audio/x-caf",
            FileFormat::Other => "application/octet-stream",
        }
    }

    /// The extension a new file of this format gets.
    pub fn default_extension(self) -> &'static str {
        self.extensions().first().copied().unwrap_or("bin")
    }

    /// The readable format an extension or format name names (`wav`, `.AIFF`; ASCII
    /// case-insensitive), or `None` when nothing reads it.
    pub fn from_extension(ext: &str) -> Option<FileFormat> {
        let ext = ext.trim_start_matches('.');
        Self::READ.iter().copied().find(|f| f.extensions().iter().any(|x| x.eq_ignore_ascii_case(ext)))
    }

    /// The readable format `path`'s extension names, or `None` without a known extension.
    pub fn from_path(path: impl AsRef<std::path::Path>) -> Option<FileFormat> {
        path.as_ref().extension().and_then(std::ffi::OsStr::to_str).and_then(Self::from_extension)
    }

    /// The extensions [`encode`] writes this format under: the names whose readers expect what it
    /// writes. RF64 and AIFF-C names are left out because `encode` writes plain RIFF and AIFF
    /// headers for them.
    pub fn write_extensions(self) -> &'static [&'static str] {
        match self {
            FileFormat::Wav => &["wav", "wave", "bwf"],
            FileFormat::Aiff => &["aif", "aiff"],
            FileFormat::Flac => &["flac"],
            _ => &[],
        }
    }

    /// The format [`encode`] writes for an extension or format name (ASCII case-insensitive), or
    /// `None` when it writes none.
    pub fn from_write_extension(ext: &str) -> Option<FileFormat> {
        let ext = ext.trim_start_matches('.');
        Self::WRITE.iter().copied().find(|f| f.write_extensions().iter().any(|x| x.eq_ignore_ascii_case(ext)))
    }

    /// Every extension of [`FileFormat::READ`], in order: what file pickers and browsers filter by.
    pub fn readable_extensions() -> Vec<&'static str> {
        Self::READ.iter().flat_map(|f| f.extensions().iter().copied()).collect()
    }

    /// Every extension of [`FileFormat::WRITE`], in order: what save dialogs, help and errors list.
    pub fn writable_extensions() -> Vec<&'static str> {
        Self::WRITE.iter().flat_map(|f| f.write_extensions().iter().copied()).collect()
    }
}

/// Stored sample representation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum SampleFormat {
    Int8,
    Int16,
    Int24,
    Int32,
    Float32,
    Float64,
    /// A lossy codec (MP3, AAC, Vorbis, ...): no fixed bit depth.
    Compressed,
}

/// Broadcast Wave (`bext`) metadata.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct BwfInfo {
    pub description: String,
    pub originator: String,
    pub originator_reference: String,
    /// `yyyy-mm-dd`.
    pub origination_date: String,
    /// `hh:mm:ss`.
    pub origination_time: String,
    /// Sample count since midnight of the first sample (the timestamp Pro Tools spots to).
    pub time_reference: u64,
}

/// What [`probe`] learns about a file without decoding all of it.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct AudioInfo {
    pub format: FileFormat,
    pub sample_format: SampleFormat,
    pub sample_rate: u32,
    pub channels: u16,
    pub frames: u64,
    pub bwf: Option<BwfInfo>,
}

impl AudioInfo {
    /// Duration in seconds.
    pub fn duration_secs(&self) -> f64 {
        if self.sample_rate == 0 { 0.0 } else { self.frames as f64 / f64::from(self.sample_rate) }
    }
}

/// Bit depth for [`encode`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum BitDepth {
    Int16,
    Int24,
    Int32,
    /// Preserve finite samples, including values above full scale; map NaN to 0 and infinities to ±1.
    Float32,
}

/// Options for [`encode`].
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct EncodeOptions {
    /// `Wav`, `Aiff` or `Flac`; anything else is [`AudioError::Unsupported`].
    pub format: FileFormat,
    pub bit_depth: BitDepth,
    /// Add TPDF dither when quantizing to an integer depth.
    pub dither: bool,
    /// Write a BWF `bext` chunk (WAV only; ignored for other formats).
    pub bwf: Option<BwfInfo>,
}

impl Default for EncodeOptions {
    fn default() -> Self {
        Self { format: FileFormat::Wav, bit_depth: BitDepth::Int24, dither: false, bwf: None }
    }
}

/// Size of an ID3v2 tag at the start of `b`, if there is one. The 10-byte footer flag (`0x10`)
/// exists only in ID3v2.4; in v2.2/v2.3 that bit is reserved and must not extend the skip.
fn id3v2_len(b: &[u8]) -> Option<usize> {
    if b.get(..3)? != b"ID3" {
        return None;
    }
    let version = *b.get(3)?;
    let flags = *b.get(5)?;
    let size = b.get(6..10)?.iter().fold(0usize, |acc, &x| (acc << 7) | usize::from(x & 0x7F));
    let footer = version == 4 && flags & 0x10 != 0;
    Some(10 + size + if footer { 10 } else { 0 })
}

fn detect_by_magic(b: &[u8], depth: u32) -> Option<FileFormat> {
    let head = |n: usize| b.get(..n);
    if wav::is_wav(b) {
        return Some(FileFormat::Wav);
    }
    if aiff::is_aiff(b) {
        return Some(FileFormat::Aiff);
    }
    match head(4)? {
        b"fLaC" => return Some(FileFormat::Flac),
        b"OggS" => return Some(FileFormat::Ogg),
        b"caff" => return Some(FileFormat::Caf),
        [0x1A, 0x45, 0xDF, 0xA3] => return Some(FileFormat::Other),
        _ => {}
    }
    if b.get(4..8) == Some(b"ftyp") {
        // MP4 / M4A: the sample entry inside `stsd` names the codec.
        let found = b.windows(4).position(|w| w == b"stsd");
        let codec = found.and_then(|p| b.get(p.checked_add(16)?..p.checked_add(20)?));
        return Some(if codec == Some(b"alac") { FileFormat::Alac } else { FileFormat::Aac });
    }
    if let Some(len) = id3v2_len(b) {
        if depth == 0
            && let Some(f) = b.get(len..).and_then(|rest| detect_by_magic(rest, 1))
        {
            return Some(f);
        }
        return Some(FileFormat::Mp3);
    }
    let (b0, b1) = (*b.first()?, *b.get(1)?);
    if b0 == 0xFF && b1 & 0xE0 == 0xE0 {
        let layer = (b1 >> 1) & 0x3;
        if b1 & 0xF0 == 0xF0 && layer == 0 {
            return Some(FileFormat::Aac); // ADTS
        }
        if layer != 0 {
            return Some(FileFormat::Mp3);
        }
    }
    None
}

/// Identify a file by its magic bytes, falling back to the extension hint (`"wav"`, `".flac"`).
pub fn detect_format(bytes: &[u8], ext_hint: Option<&str>) -> FileFormat {
    detect_by_magic(bytes, 0).unwrap_or_else(|| ext_hint.and_then(FileFormat::from_extension).unwrap_or(FileFormat::Other))
}

/// Read a file's format, sample rate, channel count, length and BWF metadata.
pub fn probe(bytes: &[u8], ext_hint: Option<&str>) -> Result<AudioInfo> {
    let format = detect_format(bytes, ext_hint);
    match format {
        FileFormat::Wav if wav::is_wav(bytes) => match wav::parse(bytes)? {
            wav::WavParse::Native(l) => Ok(l.info),
            wav::WavParse::Foreign { bwf } => {
                let mut info = symph::probe(bytes, format)?;
                info.bwf = bwf;
                Ok(info)
            }
        },
        FileFormat::Aiff if aiff::is_aiff(bytes) => match aiff::parse(bytes)? {
            aiff::AiffParse::Native(l) => Ok(l.info),
            aiff::AiffParse::Foreign => symph::probe(bytes, format),
        },
        _ => symph::probe(bytes, format),
    }
}

/// Decode a whole file to planar `f32`.
pub fn decode(bytes: &[u8], ext_hint: Option<&str>) -> Result<(AudioInfo, AudioBuffer)> {
    let format = detect_format(bytes, ext_hint);
    match format {
        FileFormat::Wav if wav::is_wav(bytes) => match wav::parse(bytes)? {
            wav::WavParse::Native(l) => {
                let buf = wav::decode(bytes, &l)?;
                let mut info = l.info;
                info.frames = buf.frames() as u64;
                Ok((info, buf))
            }
            wav::WavParse::Foreign { bwf } => {
                let (mut info, buf) = symph::decode(bytes, format)?;
                info.bwf = bwf;
                Ok((info, buf))
            }
        },
        FileFormat::Aiff if aiff::is_aiff(bytes) => match aiff::parse(bytes)? {
            aiff::AiffParse::Native(l) => {
                let buf = aiff::decode(bytes, &l)?;
                let mut info = l.info;
                info.frames = buf.frames() as u64;
                Ok((info, buf))
            }
            aiff::AiffParse::Foreign => symph::decode(bytes, format),
        },
        _ => symph::decode(bytes, format),
    }
}

/// Encode `buf` as WAV/BWF, AIFF/AIFF-C or FLAC. Samples are clamped to [-1, 1].
pub fn encode(buf: &AudioBuffer, opts: &EncodeOptions) -> Result<Vec<u8>> {
    match opts.format {
        FileFormat::Wav => wav::encode(buf, opts.bit_depth, opts.dither, opts.bwf.as_ref(), false, None),
        FileFormat::Aiff => aiff::encode(buf, opts.bit_depth, opts.dither),
        FileFormat::Flac => flac::encode(buf, opts.bit_depth, opts.dither),
        other => Err(AudioError::Unsupported(format!("encoding {other:?} is not supported"))),
    }
}

/// Encode a WAV file in the RF64 layout regardless of size (normally used only above 4 GiB).
pub fn encode_wav_rf64(buf: &AudioBuffer, opts: &EncodeOptions) -> Result<Vec<u8>> {
    wav::encode(buf, opts.bit_depth, opts.dither, opts.bwf.as_ref(), true, None)
}

/// Like [`encode`], with an explicit WAV speaker mask (`dwChannelMask`, e.g. 0x60F) for layouts
/// a channel count alone cannot identify (7.0 vs 6.1, 5.1.4 vs 7.1.2, Ambisonics = 0). AIFF and
/// FLAC have fixed channel orders and ignore it.
pub fn encode_with_channel_mask(buf: &AudioBuffer, opts: &EncodeOptions, mask: u32) -> Result<Vec<u8>> {
    match opts.format {
        FileFormat::Wav => wav::encode(buf, opts.bit_depth, opts.dither, opts.bwf.as_ref(), false, Some(mask)),
        _ => encode(buf, opts),
    }
}

/// The error a stream encoder returns for every call after one of its writes has failed: the
/// sink may already hold a prefix of the failed block, so the encoder's counters can no longer
/// describe the bytes on the sink, and later writes or `finish` must not claim success.
pub(crate) fn failed_write_error() -> AudioError {
    AudioError::Encode("the stream encoder already failed on an earlier write; its output is incomplete".into())
}

/// Writes WAV/BWF (RF64 above 4 GiB), AIFF / AIFF-C `fl32` or FLAC block by block, producing
/// the same bytes [`encode_with_channel_mask`] would for the whole buffer.
pub struct FileEncoder<W: Write + Seek>(Writer<W>);

enum Writer<W> {
    Wav(wav::Writer<W>),
    Aiff(aiff::Writer<W>),
    Flac(flac::Writer<W>),
}

impl<W: Write + Seek> FileEncoder<W> {
    /// Starts a file at the current position of `out`. `mask` overrides the WAV speaker mask
    /// derived from the channel count.
    pub fn new(out: W, opts: &EncodeOptions, channels: usize, sample_rate: u32, mask: Option<u32>) -> Result<Self> {
        let (depth, dither) = (opts.bit_depth, opts.dither);
        Ok(FileEncoder(match opts.format {
            FileFormat::Wav => Writer::Wav(wav::Writer::new(
                out,
                channels,
                sample_rate,
                depth,
                dither,
                wav::HeaderExtras { bwf: opts.bwf.as_ref(), mask, force_rf64: false },
            )?),
            FileFormat::Aiff => Writer::Aiff(aiff::Writer::new(out, channels, sample_rate, depth, dither)?),
            FileFormat::Flac => Writer::Flac(flac::Writer::new(out, channels, sample_rate, depth, dither)?),
            other => return Err(AudioError::Unsupported(format!("encoding {other:?} is not supported"))),
        }))
    }

    /// Fails, without writing anything, when `frames` more frames would not fit the format (an
    /// AIFF holds 4 GiB, a FLAC 2^36 frames).
    pub fn check_room(&self, frames: u64) -> Result<()> {
        match &self.0 {
            Writer::Wav(w) => w.check_room(frames),
            Writer::Aiff(w) => w.check_room(frames),
            Writer::Flac(w) => w.check_room(frames),
        }
        .map(drop)
    }

    /// Appends the first `frames` frames of `block`, one `Vec` per channel (missing samples are
    /// silence), unless they don't fit (see [`FileEncoder::check_room`]). Once a write has
    /// failed, the encoder is unusable: the sink may hold a prefix of the failed block, so
    /// every later `write` and `finish` fails instead of completing an inconsistent file.
    pub fn write(&mut self, block: &[Vec<f32>], frames: usize) -> Result<()> {
        match &mut self.0 {
            Writer::Wav(w) => w.write(block, frames),
            Writer::Aiff(w) => w.write(block, frames),
            Writer::Flac(w) => w.write(block, frames),
        }
    }

    /// Completes the headers and hands back `out`, positioned at the end of the file.
    pub fn finish(self) -> Result<W> {
        match self.0 {
            Writer::Wav(w) => w.finish(),
            Writer::Aiff(w) => w.finish(),
            Writer::Flac(w) => w.finish(),
        }
    }
}

/// The WAVE_FORMAT_EXTENSIBLE speaker mask of a WAV file, if it has one.
pub fn wav_channel_mask(bytes: &[u8]) -> Option<u32> {
    wav::read_channel_mask(bytes)
}

/// The header of a 32-bit float WAV holding `frames` frames (little-endian samples follow it). Its
/// length doesn't depend on `frames`, so a recorder can rewrite it in place as the file grows; it
/// switches to RF64 above 4 GiB.
pub fn wav_float_header(channels: usize, sample_rate: u32, frames: u64) -> Result<Vec<u8>> {
    if channels == 0 || sample_rate == 0 {
        return Err(AudioError::Encode("a WAV needs channels and a sample rate".into()));
    }
    wav::header(channels, sample_rate, (32, true), None, None, frames, false)
}
