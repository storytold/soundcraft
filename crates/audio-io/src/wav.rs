//! Native WAV / BWF / RF64 reader and writer (RIFF chunk layout per the Microsoft multimedia
//! specification, `bext` per EBU Tech 3285, `ds64` per EBU Tech 3306).

use crate::pcm::{PcmKind, Quantizer, clamp_float, deinterleave, le_u16, le_u32, le_u64, tag, validate_buffer};
use std::io::{Read, Seek, SeekFrom};
use crate::{AudioBuffer, AudioError, AudioInfo, BitDepth, BwfInfo, FileFormat, Result, SampleFormat};

const WAVE_FORMAT_PCM: u16 = 1;
const WAVE_FORMAT_IEEE_FLOAT: u16 = 3;
const WAVE_FORMAT_EXTENSIBLE: u16 = 0xFFFE;
/// Tail of the KSDATAFORMAT_SUBTYPE_* GUIDs (bytes 2..16); bytes 0..2 hold the format tag.
const GUID_TAIL: [u8; 14] = [0x00, 0x00, 0x00, 0x00, 0x10, 0x00, 0x80, 0x00, 0x00, 0xAA, 0x00, 0x38, 0x9B, 0x71];
const BEXT_MIN_LEN: usize = 602;

/// Parsed WAV layout (no sample data decoded yet).
pub(crate) struct WavLayout {
    pub info: AudioInfo,
    pub kind: PcmKind,
    pub data_start: usize,
    pub data_len: usize,
}

/// A bounded, file-backed PCM WAV reader. It keeps only the requested interleaved window in
/// memory; compressed WAV variants continue to use the existing decoder path.
pub struct DiskWavReader {
    file: std::fs::File,
    layout: WavLayout,
    block: usize,
}

impl DiskWavReader {
    /// Open a native PCM/float WAV without decoding the recording into one large allocation.
    pub fn open(path: impl AsRef<std::path::Path>) -> Result<Self> {
        let mut file = std::fs::File::open(path).map_err(|e| AudioError::Malformed(e.to_string()))?;
        let len = file.metadata().map_err(|e| AudioError::Malformed(e.to_string()))?.len();
        let header_len = usize::try_from(len.min(1024 * 1024)).map_err(|_| AudioError::TooLarge("WAV header is too large".into()))?;
        let mut header = vec![0; header_len];
        file.read_exact(&mut header).map_err(|e| AudioError::Malformed(e.to_string()))?;
        let WavParse::Native(mut layout) = parse(&header)? else {
            return Err(AudioError::Unsupported("disk streaming currently supports native PCM/float WAV only".into()));
        };
        let available = len.saturating_sub(layout.data_start as u64);
        layout.data_len = usize::try_from(available).unwrap_or(usize::MAX);
        let block = layout.kind.bytes().checked_mul(usize::from(layout.info.channels)).ok_or_else(|| AudioError::Malformed("WAV block size overflow".into()))?;
        layout.info.frames = (layout.data_len / block) as u64;
        Ok(Self { file, layout, block })
    }

    pub fn info(&self) -> &AudioInfo { &self.layout.info }

    /// Read at most `frames` starting at `start`, returning planar samples.
    pub fn read_frames(&mut self, start: u64, frames: usize) -> Result<AudioBuffer> {
        let start_frame = start.min(self.layout.info.frames);
        let count = frames.min((self.layout.info.frames - start_frame) as usize);
        let bytes = count.checked_mul(self.block).ok_or_else(|| AudioError::TooLarge("requested audio window is too large".into()))?;
        let offset = (self.layout.data_start as u64).saturating_add(start_frame.saturating_mul(self.block as u64));
        self.file.seek(SeekFrom::Start(offset)).map_err(|e| AudioError::Malformed(e.to_string()))?;
        let mut data = vec![0; bytes];
        self.file.read_exact(&mut data).map_err(|e| AudioError::Malformed(e.to_string()))?;
        let channels = deinterleave(&data, self.layout.kind, usize::from(self.layout.info.channels), count as u64)?;
        Ok(AudioBuffer { sample_rate: self.layout.info.sample_rate, channels })
    }
}

/// Outcome of parsing a WAV header.
pub(crate) enum WavParse {
    /// A PCM / float layout we decode natively.
    Native(WavLayout),
    /// A valid WAV whose codec we hand to symphonia (ADPCM, A-law, ...).
    Foreign { bwf: Option<BwfInfo> },
}

struct Fmt {
    tag: u16,
    channels: u16,
    sample_rate: u32,
    block_align: u16,
    bits: u16,
    valid_bits: u16,
}

fn parse_fmt(c: &[u8]) -> Result<Fmt> {
    let short = || AudioError::Malformed("fmt chunk too short".into());
    let mut fmt = Fmt {
        tag: le_u16(c, 0).ok_or_else(short)?,
        channels: le_u16(c, 2).ok_or_else(short)?,
        sample_rate: le_u32(c, 4).ok_or_else(short)?,
        block_align: le_u16(c, 12).ok_or_else(short)?,
        bits: le_u16(c, 14).unwrap_or(0),
        valid_bits: 0,
    };
    fmt.valid_bits = fmt.bits;
    if fmt.tag == WAVE_FORMAT_EXTENSIBLE {
        // cbSize(2) validBits(2) channelMask(4) subFormat GUID(16)
        if let (Some(valid), Some(sub)) = (le_u16(c, 18), le_u16(c, 24)) {
            if valid != 0 {
                fmt.valid_bits = valid;
            }
            fmt.tag = sub;
        } else {
            return Err(AudioError::Malformed("WAVE_FORMAT_EXTENSIBLE fmt chunk too short".into()));
        }
    }
    Ok(fmt)
}

/// Read a NUL-padded ASCII field.
fn fixed_str(b: &[u8]) -> String {
    let end = b.iter().position(|&c| c == 0).unwrap_or(b.len());
    String::from_utf8_lossy(b.get(..end).unwrap_or_default()).trim_end().to_string()
}

fn parse_bext(c: &[u8]) -> Option<BwfInfo> {
    if c.len() < 346 {
        return None;
    }
    let lo = u64::from(le_u32(c, 338)?);
    let hi = u64::from(le_u32(c, 342)?);
    Some(BwfInfo {
        description: fixed_str(c.get(0..256)?),
        originator: fixed_str(c.get(256..288)?),
        originator_reference: fixed_str(c.get(288..320)?),
        origination_date: fixed_str(c.get(320..330)?),
        origination_time: fixed_str(c.get(330..338)?),
        time_reference: (hi << 32) | lo,
    })
}

/// True when the bytes start like a RIFF/RF64/BW64 WAVE file.
pub(crate) fn is_wav(b: &[u8]) -> bool {
    matches!(tag(b, 0), Some(t) if &t == b"RIFF" || &t == b"RF64" || &t == b"BW64") && tag(b, 8).is_some_and(|t| &t == b"WAVE")
}

pub(crate) fn parse(b: &[u8]) -> Result<WavParse> {
    if !is_wav(b) {
        return Err(AudioError::Malformed("not a RIFF/RF64 WAVE file".into()));
    }
    let is_rf64 = tag(b, 0).is_some_and(|t| &t != b"RIFF");
    let mut ds64_data: Option<u64> = None;
    let mut fmt: Option<Fmt> = None;
    let mut bwf: Option<BwfInfo> = None;
    let mut data: Option<(usize, usize)> = None;

    let mut pos: usize = 12;
    // Bound the number of chunks we look at so a file of tiny chunks can't spin forever.
    let mut chunks = 0usize;
    while let (Some(id), Some(size32)) = (tag(b, pos), le_u32(b, pos + 4)) {
        chunks += 1;
        if chunks > 100_000 {
            break;
        }
        let body = pos.checked_add(8).ok_or_else(|| AudioError::Malformed("chunk offset overflow".into()))?;
        let remaining = b.len().saturating_sub(body);
        let mut size = u64::from(size32);
        if &id == b"data" && is_rf64 && size32 == u32::MAX {
            size = ds64_data.unwrap_or(remaining as u64);
        }
        let declared = usize::try_from(size).unwrap_or(usize::MAX);
        if &id == b"data" {
            // Lenient: a truncated recording (or a streaming writer's 0 / 0xFFFFFFFF size) still
            // yields whatever audio is actually present.
            let len = if size32 == 0 && !is_rf64 { remaining } else { declared.min(remaining) };
            if data.is_none() {
                data = Some((body, len));
            }
            if declared > remaining {
                break;
            }
        } else {
            if declared > remaining {
                // A truncated non-audio chunk at the end of the file: stop scanning.
                break;
            }
            let c = b.get(body..body + declared).unwrap_or_default();
            match &id {
                b"fmt " => fmt = Some(parse_fmt(c)?),
                b"bext" => bwf = parse_bext(c),
                b"ds64" => ds64_data = le_u64(c, 8),
                _ => {}
            }
        }
        let padded = declared.saturating_add(declared & 1);
        pos = match body.checked_add(padded) {
            Some(p) => p,
            None => break,
        };
    }

    let fmt = fmt.ok_or_else(|| AudioError::Malformed("WAV has no fmt chunk".into()))?;
    if fmt.channels == 0 {
        return Err(AudioError::Malformed("WAV declares zero channels".into()));
    }
    if fmt.sample_rate == 0 {
        return Err(AudioError::Malformed("WAV declares a zero sample rate".into()));
    }
    let channels = usize::from(fmt.channels);
    let (kind, sample_format) = match (fmt.tag, fmt.bits) {
        (WAVE_FORMAT_PCM, bits @ 1..=32) => {
            let bytes = usize::from(bits.div_ceil(8));
            let sf = match fmt.valid_bits.min(bits) {
                0..=8 => SampleFormat::Int8,
                9..=16 => SampleFormat::Int16,
                17..=24 => SampleFormat::Int24,
                _ => SampleFormat::Int32,
            };
            (PcmKind::Int { bytes, big_endian: false, unsigned: bytes == 1 }, sf)
        }
        (WAVE_FORMAT_IEEE_FLOAT, 32) => (PcmKind::Float { bytes: 4, big_endian: false }, SampleFormat::Float32),
        (WAVE_FORMAT_IEEE_FLOAT, 64) => (PcmKind::Float { bytes: 8, big_endian: false }, SampleFormat::Float64),
        (WAVE_FORMAT_PCM | WAVE_FORMAT_IEEE_FLOAT, bits) => {
            return Err(AudioError::Unsupported(format!("WAV format {} with {bits} bits per sample", fmt.tag)));
        }
        _ => return Ok(WavParse::Foreign { bwf }),
    };
    let block = kind.bytes().checked_mul(channels).ok_or_else(|| AudioError::Malformed("block size overflow".into()))?;
    if usize::from(fmt.block_align) != block {
        log::debug!("WAV block_align {} disagrees with computed {block}; using computed", fmt.block_align);
    }
    let (data_start, data_len) = data.ok_or_else(|| AudioError::Malformed("WAV has no data chunk".into()))?;
    let frames = (data_len / block) as u64;
    Ok(WavParse::Native(WavLayout {
        info: AudioInfo { format: FileFormat::Wav, sample_format, sample_rate: fmt.sample_rate, channels: fmt.channels, frames, bwf },
        kind,
        data_start,
        data_len,
    }))
}

pub(crate) fn decode(b: &[u8], layout: &WavLayout) -> Result<AudioBuffer> {
    let end = layout.data_start.saturating_add(layout.data_len).min(b.len());
    let data = b.get(layout.data_start..end).unwrap_or_default();
    let channels = deinterleave(data, layout.kind, usize::from(layout.info.channels), layout.info.frames)?;
    Ok(AudioBuffer { sample_rate: layout.info.sample_rate, channels })
}

/// Default speaker mask for a channel count (callers that know the layout pass their own):
/// mono, stereo, LCR, quad, 5.0, 5.1, 6.1, 7.1 (sides + rears), 7.1.4.
fn channel_mask(channels: usize) -> u32 {
    match channels {
        1 => 0x4,
        2 => 0x3,
        3 => 0x7,
        4 => 0x33,
        5 => 0x37,
        6 => 0x3F,
        7 => 0x13F,
        8 => 0x63F,
        // L R C LFE Lrs Rrs Lss Rss Ltf Rtf Ltr Rtr (7.1.4).
        12 => 0x2_D63F,
        n if n <= 18 => (1u32 << n) - 1,
        _ => 0,
    }
}

/// The `dwChannelMask` of a WAVE_FORMAT_EXTENSIBLE file (None for plain PCM/float headers).
pub(crate) fn read_channel_mask(b: &[u8]) -> Option<u32> {
    if !is_wav(b) {
        return None;
    }
    let mut pos = 12usize;
    for _ in 0..10_000 {
        let (id, size) = (tag(b, pos)?, usize::try_from(le_u32(b, pos + 4)?).ok()?);
        if &id == b"fmt " {
            return (le_u16(b, pos + 8)? == WAVE_FORMAT_EXTENSIBLE).then(|| le_u32(b, pos + 8 + 20)).flatten();
        }
        pos = pos.checked_add(8)?.checked_add(size)?.checked_add(size & 1)?;
    }
    None
}

fn put_fixed_str(out: &mut Vec<u8>, s: &str, len: usize) {
    let bytes = s.as_bytes();
    let n = bytes.len().min(len);
    out.extend_from_slice(bytes.get(..n).unwrap_or_default());
    out.resize(out.len() + (len - n), 0);
}

fn bext_chunk(bwf: &BwfInfo) -> Vec<u8> {
    let mut c = Vec::with_capacity(BEXT_MIN_LEN);
    put_fixed_str(&mut c, &bwf.description, 256);
    put_fixed_str(&mut c, &bwf.originator, 32);
    put_fixed_str(&mut c, &bwf.originator_reference, 32);
    put_fixed_str(&mut c, &bwf.origination_date, 10);
    put_fixed_str(&mut c, &bwf.origination_time, 8);
    c.extend_from_slice(&((bwf.time_reference & 0xFFFF_FFFF) as u32).to_le_bytes());
    c.extend_from_slice(&((bwf.time_reference >> 32) as u32).to_le_bytes());
    c.extend_from_slice(&1u16.to_le_bytes()); // version 1
    c.resize(BEXT_MIN_LEN, 0); // UMID, loudness, reserved; no coding history
    c
}

fn push_chunk(out: &mut Vec<u8>, id: &[u8; 4], body: &[u8]) {
    out.extend_from_slice(id);
    out.extend_from_slice(&u32::try_from(body.len()).unwrap_or(u32::MAX).to_le_bytes());
    out.extend_from_slice(body);
    if body.len() % 2 == 1 {
        out.push(0);
    }
}

/// Encode a WAV file. `force_rf64` writes the RF64 layout even for small files (for tests);
/// it is used automatically when the file would exceed 4 GiB.
/// `mask` overrides the speaker mask derived from the channel count.
pub(crate) fn encode(
    buf: &AudioBuffer,
    depth: BitDepth,
    dither: bool,
    bwf: Option<&BwfInfo>,
    force_rf64: bool,
    mask: Option<u32>,
) -> Result<Vec<u8>> {
    validate_buffer(buf)?;
    let channels = buf.channels.len();
    let frames = buf.frames();
    let (bits, is_float): (u16, bool) = match depth {
        BitDepth::Int16 => (16, false),
        BitDepth::Int24 => (24, false),
        BitDepth::Int32 => (32, false),
        BitDepth::Float32 => (32, true),
    };
    let bytes = usize::from(bits / 8);
    let block = bytes.checked_mul(channels).ok_or_else(|| AudioError::TooLarge("block size overflow".into()))?;
    let block_align = u16::try_from(block).map_err(|_| AudioError::Encode("too many channels for WAV".into()))?;
    let data_len = block.checked_mul(frames).ok_or_else(|| AudioError::TooLarge("data size overflow".into()))?;
    let extensible = channels > 2 || bits > 16 || mask.is_some_and(|m| m != channel_mask(channels));
    let fmt_tag = if is_float { WAVE_FORMAT_IEEE_FLOAT } else { WAVE_FORMAT_PCM };

    let mut fmt = Vec::with_capacity(40);
    fmt.extend_from_slice(&(if extensible { WAVE_FORMAT_EXTENSIBLE } else { fmt_tag }).to_le_bytes());
    fmt.extend_from_slice(&(channels as u16).to_le_bytes());
    fmt.extend_from_slice(&buf.sample_rate.to_le_bytes());
    fmt.extend_from_slice(&buf.sample_rate.saturating_mul(u32::from(block_align)).to_le_bytes());
    fmt.extend_from_slice(&block_align.to_le_bytes());
    fmt.extend_from_slice(&bits.to_le_bytes());
    if extensible {
        fmt.extend_from_slice(&22u16.to_le_bytes());
        fmt.extend_from_slice(&bits.to_le_bytes());
        fmt.extend_from_slice(&mask.unwrap_or_else(|| channel_mask(channels)).to_le_bytes());
        fmt.extend_from_slice(&fmt_tag.to_le_bytes());
        fmt.extend_from_slice(&GUID_TAIL);
    }

    let mut head = Vec::with_capacity(1024);
    let rf64_body_len = 28usize;
    // Reserve room for ds64 (as JUNK when not needed, so a later writer could upgrade in place).
    let mut meta = Vec::new();
    if let Some(bwf) = bwf {
        push_chunk(&mut meta, b"bext", &bext_chunk(bwf));
    }
    push_chunk(&mut meta, b"fmt ", &fmt);
    if is_float {
        push_chunk(&mut meta, b"fact", &u32::try_from(frames).unwrap_or(u32::MAX).to_le_bytes());
    }
    let total = 4 + (8 + rf64_body_len) + meta.len() + 8 + data_len + (data_len & 1);
    let rf64 = force_rf64 || u32::try_from(total).is_err();

    if rf64 {
        head.extend_from_slice(b"RF64");
        head.extend_from_slice(&u32::MAX.to_le_bytes());
        head.extend_from_slice(b"WAVE");
        let mut ds64 = Vec::with_capacity(rf64_body_len);
        ds64.extend_from_slice(&(total as u64).to_le_bytes());
        ds64.extend_from_slice(&(data_len as u64).to_le_bytes());
        ds64.extend_from_slice(&(frames as u64).to_le_bytes());
        ds64.extend_from_slice(&0u32.to_le_bytes());
        push_chunk(&mut head, b"ds64", &ds64);
    } else {
        head.extend_from_slice(b"RIFF");
        head.extend_from_slice(&(total as u32).to_le_bytes());
        head.extend_from_slice(b"WAVE");
        push_chunk(&mut head, b"JUNK", &[0u8; 28]);
    }
    head.extend_from_slice(&meta);
    head.extend_from_slice(b"data");
    head.extend_from_slice(&(if rf64 { u32::MAX } else { data_len as u32 }).to_le_bytes());

    let mut out = Vec::new();
    out.try_reserve_exact(head.len().saturating_add(data_len).saturating_add(1))
        .map_err(|_| AudioError::TooLarge(format!("cannot allocate {data_len} bytes for WAV output")))?;
    out.extend_from_slice(&head);
    let mut qs = Quantizer::per_channel(u32::from(bits), dither, channels);
    for f in 0..frames {
        for (ch, q) in buf.channels.iter().zip(qs.iter_mut()) {
            let s = ch.get(f).copied().unwrap_or(0.0);
            if is_float {
                out.extend_from_slice(&clamp_float(s).to_le_bytes());
            } else {
                let v = q.quantize(s).to_le_bytes();
                out.extend_from_slice(v.get(..bytes).unwrap_or_default());
            }
        }
    }
    if data_len % 2 == 1 {
        out.push(0);
    }
    Ok(out)
}
