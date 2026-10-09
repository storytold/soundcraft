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
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

mod aiff;
mod buffer;
mod flac;
mod pcm;
pub mod peaks;
mod symph;
mod wav;

pub use buffer::AudioBuffer;

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
    Other,
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

/// Size of an ID3v2 tag at the start of `b`, if there is one.
fn id3v2_len(b: &[u8]) -> Option<usize> {
    if b.get(..3)? != b"ID3" {
        return None;
    }
    let flags = *b.get(5)?;
    let size = b.get(6..10)?.iter().fold(0usize, |acc, &x| (acc << 7) | usize::from(x & 0x7F));
    Some(10 + size + if flags & 0x10 != 0 { 10 } else { 0 })
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

fn detect_by_ext(ext: &str) -> FileFormat {
    match ext.trim_start_matches('.').to_ascii_lowercase().as_str() {
        "wav" | "wave" | "bwf" | "rf64" => FileFormat::Wav,
        "aif" | "aiff" | "aifc" => FileFormat::Aiff,
        "flac" => FileFormat::Flac,
        "mp3" | "mp2" | "mp1" => FileFormat::Mp3,
        "ogg" | "oga" => FileFormat::Ogg,
        "aac" | "m4a" | "mp4" | "adts" => FileFormat::Aac,
        "caf" => FileFormat::Caf,
        _ => FileFormat::Other,
    }
}

/// Identify a file by its magic bytes, falling back to the extension hint (`"wav"`, `".flac"`).
pub fn detect_format(bytes: &[u8], ext_hint: Option<&str>) -> FileFormat {
    detect_by_magic(bytes, 0).unwrap_or_else(|| ext_hint.map(detect_by_ext).unwrap_or(FileFormat::Other))
}

/// Read a file's format, sample rate, channel count, length and BWF metadata.
pub fn probe(bytes: &[u8], ext_hint: Option<&str>) -> Result<AudioInfo> {
    let format = detect_format(bytes, ext_hint);
    match format {
        FileFormat::Wav if wav::is_wav(bytes) => match wav::parse(bytes)? {
            wav::WavParse::Native(l) => Ok(l.info),
            wav::WavParse::Foreign { bwf } => {
                let mut info = symph::probe(bytes, Some("wav"), format)?;
                info.bwf = bwf;
                Ok(info)
            }
        },
        FileFormat::Aiff if aiff::is_aiff(bytes) => match aiff::parse(bytes)? {
            aiff::AiffParse::Native(l) => Ok(l.info),
            aiff::AiffParse::Foreign => symph::probe(bytes, Some("aiff"), format),
        },
        _ => symph::probe(bytes, ext_hint, format),
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
                let (mut info, buf) = symph::decode(bytes, Some("wav"), format)?;
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
            aiff::AiffParse::Foreign => symph::decode(bytes, Some("aiff"), format),
        },
        _ => symph::decode(bytes, ext_hint, format),
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

/// Formats [`encode`] can write, as the extensions that name them.
pub const ENCODE_EXTENSIONS: &str = "wav, aif, aiff, flac";

/// The format [`encode`] writes for a file extension or format name (`wav`, `aiff`, `flac`, case-insensitive), or `None` when it cannot write it.
pub fn encode_format_for(name: &str) -> Option<FileFormat> {
    match name.trim_start_matches('.').to_ascii_lowercase().as_str() {
        "wav" | "wave" | "bwf" => Some(FileFormat::Wav),
        "aif" | "aiff" => Some(FileFormat::Aiff),
        "flac" => Some(FileFormat::Flac),
        _ => None,
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
