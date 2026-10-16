//! Native AIFF / AIFF-C reader and writer (Apple "Audio Interchange File Format" 1.3 and the
//! AIFF-C draft: big-endian IFF chunks, 80-bit IEEE extended sample rate).

use crate::pcm::{PcmKind, Quantizer, be_u16, be_u32, deinterleave, sample_format, tag, validate_buffer, validate_layout, write_interleaved};
use crate::{AudioBuffer, AudioError, AudioInfo, BitDepth, FileFormat, Result, SampleFormat};
use std::io::{Cursor, Seek, SeekFrom, Write};

/// AIFF-C version 1 timestamp for the FVER chunk.
const AIFC_VERSION_1: u32 = 0xA280_5140;
const MAX_SAMPLE_RATE: f64 = 10_000_000.0;

pub(crate) struct AiffLayout {
    pub info: AudioInfo,
    pub kind: PcmKind,
    pub data_start: usize,
    pub data_len: usize,
}

pub(crate) enum AiffParse {
    Native(AiffLayout),
    /// Compressed AIFF-C (ulaw, alaw, ima4, ...): handed to symphonia.
    Foreign,
}

/// True when the bytes start like an AIFF or AIFF-C file.
pub(crate) fn is_aiff(b: &[u8]) -> bool {
    tag(b, 0).is_some_and(|t| &t == b"FORM") && tag(b, 8).is_some_and(|t| &t == b"AIFF" || &t == b"AIFC")
}

/// Decode an 80-bit IEEE 754 extended-precision float.
pub(crate) fn extended_to_f64(b: &[u8]) -> Option<f64> {
    let se = be_u16(b, 0)?;
    let hi = u64::from(be_u32(b, 2)?);
    let lo = u64::from(be_u32(b, 6)?);
    let mant = (hi << 32) | lo;
    if mant == 0 {
        return Some(0.0);
    }
    let exp = i32::from(se & 0x7FFF);
    if exp == 0x7FFF {
        return None; // inf / NaN
    }
    let v = mant as f64 * 2f64.powi(exp - 16383 - 63);
    Some(if se & 0x8000 != 0 { -v } else { v })
}

/// Encode a positive integer as an 80-bit IEEE 754 extended float (exact).
pub(crate) fn u32_to_extended(r: u32) -> [u8; 10] {
    let mut out = [0u8; 10];
    if r == 0 {
        return out;
    }
    let e = 31 - r.leading_zeros();
    let exp = (16383 + e) as u16;
    let mant = u64::from(r) << (63 - e);
    out[..2].copy_from_slice(&exp.to_be_bytes());
    out[2..].copy_from_slice(&mant.to_be_bytes());
    out
}

pub(crate) fn parse(b: &[u8]) -> Result<AiffParse> {
    if !is_aiff(b) {
        return Err(AudioError::Malformed("not an AIFF/AIFF-C file".into()));
    }
    let is_aifc = tag(b, 8).is_some_and(|t| &t == b"AIFC");
    let mut comm: Option<(i16, u32, i16, f64, [u8; 4])> = None;
    let mut ssnd: Option<(usize, usize)> = None;

    let mut pos: usize = 12;
    let mut chunks = 0usize;
    while let (Some(id), Some(size32)) = (tag(b, pos), be_u32(b, pos + 4)) {
        chunks += 1;
        if chunks > 100_000 {
            break;
        }
        let body = pos.checked_add(8).ok_or_else(|| AudioError::Malformed("chunk offset overflow".into()))?;
        let remaining = b.len().saturating_sub(body);
        let declared = usize::try_from(size32).unwrap_or(usize::MAX);
        let len = declared.min(remaining);
        let c = b.get(body..body + len).unwrap_or_default();
        match &id {
            b"COMM" => {
                let short = || AudioError::Malformed("COMM chunk too short".into());
                let channels = be_u16(c, 0).ok_or_else(short)? as i16;
                let frames = be_u32(c, 2).ok_or_else(short)?;
                let bits = be_u16(c, 6).ok_or_else(short)? as i16;
                let rate = extended_to_f64(c.get(8..18).ok_or_else(short)?).ok_or_else(|| AudioError::Malformed("invalid sample rate".into()))?;
                let compression = if is_aifc { tag(c, 18).ok_or_else(short)? } else { *b"NONE" };
                comm = Some((channels, frames, bits, rate, compression));
            }
            b"SSND" if ssnd.is_none() => {
                let offset = usize::try_from(be_u32(c, 0).unwrap_or(0)).unwrap_or(usize::MAX);
                let start = body.saturating_add(8).saturating_add(offset).min(b.len());
                let data_len = len.saturating_sub(8).saturating_sub(offset);
                ssnd = Some((start, data_len));
            }
            _ => {}
        }
        if declared > remaining {
            break;
        }
        pos = match body.checked_add(declared).and_then(|p| p.checked_add(declared & 1)) {
            Some(p) => p,
            None => break,
        };
    }

    let (channels, frames, bits, rate, compression) = comm.ok_or_else(|| AudioError::Malformed("AIFF has no COMM chunk".into()))?;
    if channels <= 0 {
        return Err(AudioError::Malformed("AIFF declares zero or negative channels".into()));
    }
    if !(1.0..=MAX_SAMPLE_RATE).contains(&rate) {
        return Err(AudioError::Malformed(format!("AIFF sample rate {rate} out of range")));
    }
    let sample_rate = rate.round() as u32;
    let int_kind = |big_endian: bool| -> Result<(PcmKind, SampleFormat)> {
        if !(1..=32).contains(&bits) {
            return Err(AudioError::Unsupported(format!("AIFF sample size {bits}")));
        }
        let bytes = (bits as usize).div_ceil(8);
        let sf = match bits {
            1..=8 => SampleFormat::Int8,
            9..=16 => SampleFormat::Int16,
            17..=24 => SampleFormat::Int24,
            _ => SampleFormat::Int32,
        };
        Ok((PcmKind::Int { bytes, big_endian, unsigned: false }, sf))
    };
    let (kind, sample_format) = match &compression {
        b"NONE" | b"twos" => int_kind(true)?,
        b"sowt" => int_kind(false)?,
        b"fl32" | b"FL32" => (PcmKind::Float { bytes: 4, big_endian: true }, SampleFormat::Float32),
        b"fl64" | b"FL64" => (PcmKind::Float { bytes: 8, big_endian: true }, SampleFormat::Float64),
        _ => return Ok(AiffParse::Foreign),
    };
    let (data_start, data_len) = ssnd.unwrap_or((b.len(), 0));
    let block = kind.bytes().saturating_mul(channels as usize).max(1);
    let frames = u64::from(frames).min((data_len / block) as u64);
    Ok(AiffParse::Native(AiffLayout {
        info: AudioInfo { format: FileFormat::Aiff, sample_format, sample_rate, channels: channels as u16, frames, bwf: None },
        kind,
        data_start,
        data_len,
    }))
}

pub(crate) fn decode(b: &[u8], layout: &AiffLayout) -> Result<AudioBuffer> {
    let end = layout.data_start.saturating_add(layout.data_len).min(b.len());
    let data = b.get(layout.data_start..end).unwrap_or_default();
    let channels = deinterleave(data, layout.kind, usize::from(layout.info.channels), layout.info.frames)?;
    Ok(AudioBuffer { sample_rate: layout.info.sample_rate, channels })
}

fn push_chunk(out: &mut Vec<u8>, id: &[u8; 4], body: &[u8]) {
    out.extend_from_slice(id);
    out.extend_from_slice(&u32::try_from(body.len()).unwrap_or(u32::MAX).to_be_bytes());
    out.extend_from_slice(body);
    if body.len() % 2 == 1 {
        out.push(0);
    }
}

/// Encode AIFF (integer depths) or AIFF-C `fl32` (Float32).
pub(crate) fn encode(buf: &AudioBuffer, depth: BitDepth, dither: bool) -> Result<Vec<u8>> {
    validate_buffer(buf)?;
    let frames = buf.frames();
    let head_len = header(buf.channels.len(), buf.sample_rate, depth, frames as u64)?.len();
    let data_len = usize::from(sample_format(depth).0 / 8).saturating_mul(buf.channels.len()).saturating_mul(frames);
    let total = head_len.saturating_add(data_len).saturating_add(1);
    let mut out = Vec::new();
    out.try_reserve_exact(total).map_err(|_| AudioError::TooLarge(format!("cannot allocate {total} bytes for AIFF output")))?;
    let mut w = Writer::new(Cursor::new(out), buf.channels.len(), buf.sample_rate, depth, dither)?;
    w.write(&buf.channels, frames)?;
    Ok(w.finish()?.into_inner())
}

/// An AIFF written block by block: a placeholder header first, then the real one (the same
/// length) written over it by [`Writer::finish`].
pub(crate) struct Writer<W> {
    out: W,
    start: u64,
    channels: usize,
    sample_rate: u32,
    depth: BitDepth,
    quantizers: Vec<Quantizer>,
    frames: u64,
    bytes: Vec<u8>,
    /// Set when a block write has failed: the sink may hold a prefix of that block while
    /// `frames` does not count it, so the writer refuses every later call.
    failed: bool,
}

impl<W: Write + Seek> Writer<W> {
    pub(crate) fn new(mut out: W, channels: usize, sample_rate: u32, depth: BitDepth, dither: bool) -> Result<Self> {
        validate_layout(channels, sample_rate)?;
        let start = out.stream_position()?;
        out.write_all(&header(channels, sample_rate, depth, 0)?)?;
        let quantizers = Quantizer::per_channel(u32::from(sample_format(depth).0), dither, channels);
        Ok(Writer { out, start, channels, sample_rate, depth, quantizers, frames: 0, bytes: Vec::new(), failed: false })
    }

    /// The frame count after `frames` more, or an error when AIFF can't hold that many (4 GiB).
    pub(crate) fn check_room(&self, frames: u64) -> Result<u64> {
        let total = self.frames.saturating_add(frames);
        header(self.channels, self.sample_rate, self.depth, total)?;
        Ok(total)
    }

    /// Appends `frames` frames of planar audio (missing samples are silence), unless they don't
    /// fit (see [`Writer::check_room`]).
    pub(crate) fn write(&mut self, block: &[Vec<f32>], frames: usize) -> Result<()> {
        if self.failed {
            return Err(crate::failed_write_error());
        }
        let total = self.check_room(frames as u64)?;
        if let Err(e) = write_interleaved(&mut self.out, &mut self.bytes, block, frames, &mut self.quantizers, sample_format(self.depth), true) {
            self.failed = true;
            return Err(AudioError::from(e));
        }
        self.frames = total;
        Ok(())
    }

    pub(crate) fn finish(mut self) -> Result<W> {
        if self.failed {
            return Err(crate::failed_write_error());
        }
        let data_len = u64::from(sample_format(self.depth).0 / 8).saturating_mul(self.channels as u64).saturating_mul(self.frames);
        if data_len % 2 == 1 {
            self.out.write_all(&[0])?;
        }
        let head = header(self.channels, self.sample_rate, self.depth, self.frames)?;
        self.out.seek(SeekFrom::Start(self.start))?;
        self.out.write_all(&head)?;
        self.out.seek(SeekFrom::End(0))?;
        self.out.flush()?;
        Ok(self.out)
    }
}

/// Everything before the sample data of an AIFF holding `frames` frames; its length doesn't
/// depend on `frames`.
fn header(channels: usize, sample_rate: u32, depth: BitDepth, frames: u64) -> Result<Vec<u8>> {
    let channels_i16 = i16::try_from(channels).map_err(|_| AudioError::Encode("too many channels for AIFF".into()))?;
    let frames_u32 = u32::try_from(frames).map_err(|_| AudioError::TooLarge("too many frames for AIFF".into()))?;
    let (bits, is_float) = sample_format(depth);
    let data_len = u64::from(bits / 8).saturating_mul(channels as u64).saturating_mul(frames);

    let mut comm = Vec::with_capacity(40);
    comm.extend_from_slice(&channels_i16.to_be_bytes());
    comm.extend_from_slice(&frames_u32.to_be_bytes());
    comm.extend_from_slice(&bits.to_be_bytes());
    comm.extend_from_slice(&u32_to_extended(sample_rate));
    if is_float {
        comm.extend_from_slice(b"fl32");
        let name = b"32-bit float";
        comm.push(name.len() as u8);
        comm.extend_from_slice(name);
        if (name.len() + 1) % 2 == 1 {
            comm.push(0);
        }
    }

    let mut head = Vec::with_capacity(128);
    head.extend_from_slice(b"FORM");
    head.extend_from_slice(&[0, 0, 0, 0]); // patched below
    head.extend_from_slice(if is_float { b"AIFC" } else { b"AIFF" });
    if is_float {
        push_chunk(&mut head, b"FVER", &AIFC_VERSION_1.to_be_bytes());
    }
    push_chunk(&mut head, b"COMM", &comm);
    head.extend_from_slice(b"SSND");
    let ssnd_len = u32::try_from(data_len.saturating_add(8)).map_err(|_| AudioError::TooLarge("AIFF data exceeds 4 GiB".into()))?;
    head.extend_from_slice(&ssnd_len.to_be_bytes());
    head.extend_from_slice(&[0u8; 8]); // offset, blockSize

    let total = (head.len() as u64).saturating_add(data_len).saturating_add(data_len & 1);
    let form_len = u32::try_from(total - 8).map_err(|_| AudioError::TooLarge("AIFF file exceeds 4 GiB".into()))?;
    if let Some(slot) = head.get_mut(4..8) {
        slot.copy_from_slice(&form_len.to_be_bytes());
    }
    Ok(head)
}
