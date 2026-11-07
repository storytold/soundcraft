//! A small pure-Rust FLAC encoder (RFC 9639): fixed blocksize, independent channels, and per
//! subframe the cheapest of CONSTANT, VERBATIM or FIXED (orders 0–4) prediction with
//! partitioned Rice-coded residuals. Decoding is symphonia's, after [`check_metadata`].

use crate::pcm::{Quantizer, be_u32, le_u32, validate_buffer};
use crate::{AudioBuffer, AudioError, AudioInfo, BitDepth, FileFormat, Result, SampleFormat};
use std::io::{Cursor, Seek, SeekFrom, Write};

const BLOCK_SIZE: usize = 4096;
const MAX_PARTITION_ORDER: u32 = 6;

struct BitWriter {
    out: Vec<u8>,
    acc: u64,
    nbits: u32,
}

impl BitWriter {
    fn new() -> Self {
        Self { out: Vec::new(), acc: 0, nbits: 0 }
    }

    /// Write the low `n` (≤ 32) bits of `v`, MSB first.
    fn write(&mut self, v: u64, n: u32) {
        if n == 0 {
            return;
        }
        let n = n.min(32);
        let mask = (1u64 << n) - 1;
        self.acc = (self.acc << n) | (v & mask);
        self.nbits += n;
        while self.nbits >= 8 {
            self.nbits -= 8;
            self.out.push((self.acc >> self.nbits) as u8);
        }
        self.acc &= (1u64 << self.nbits) - 1;
    }

    fn write_signed(&mut self, v: i64, n: u32) {
        self.write(v as u64, n);
    }

    /// `q` zero bits followed by a one bit.
    fn write_unary(&mut self, mut q: u64) {
        while q >= 32 {
            self.write(0, 32);
            q -= 32;
        }
        self.write(1, (q as u32) + 1);
    }

    fn align(&mut self) {
        if self.nbits > 0 {
            let pad = 8 - self.nbits;
            self.write(0, pad);
        }
    }

    fn into_bytes(mut self) -> Vec<u8> {
        self.align();
        self.out
    }
}

fn crc8(data: &[u8]) -> u8 {
    let mut crc: u8 = 0;
    for &b in data {
        crc ^= b;
        for _ in 0..8 {
            crc = if crc & 0x80 != 0 { (crc << 1) ^ 0x07 } else { crc << 1 };
        }
    }
    crc
}

fn crc16(data: &[u8]) -> u16 {
    let mut crc: u16 = 0;
    for &b in data {
        crc ^= u16::from(b) << 8;
        for _ in 0..8 {
            crc = if crc & 0x8000 != 0 { (crc << 1) ^ 0x8005 } else { crc << 1 };
        }
    }
    crc
}

/// FLAC's UTF-8-like variable-length integer coding (up to 36 bits).
fn write_utf8_number(w: &mut BitWriter, v: u64) {
    if v < 0x80 {
        w.write(v, 8);
        return;
    }
    // Payload bits available for an n-byte coding: 5n + 1 (n = 2..=6), 36 for n = 7.
    let n: u32 = (2..=7).find(|&n| if n == 7 { true } else { v < (1u64 << (5 * n + 1)) }).unwrap_or(7);
    let cont_bits = 6 * (n - 1);
    let lead_payload = if n == 7 { 0 } else { 7 - n };
    let prefix: u64 = (0xFFu64 << (8 - n)) & 0xFF;
    let lead = prefix | ((v >> cont_bits) & ((1u64 << lead_payload) - 1));
    w.write(lead, 8);
    for i in (0..n - 1).rev() {
        w.write(0x80 | ((v >> (6 * i)) & 0x3F), 8);
    }
}

fn sample_rate_code(r: u32) -> (u64, Option<(u64, u32)>) {
    match r {
        88_200 => (1, None),
        176_400 => (2, None),
        192_000 => (3, None),
        8_000 => (4, None),
        16_000 => (5, None),
        22_050 => (6, None),
        24_000 => (7, None),
        32_000 => (8, None),
        44_100 => (9, None),
        48_000 => (10, None),
        96_000 => (11, None),
        r if r % 1000 == 0 && r / 1000 <= 255 => (12, Some((u64::from(r / 1000), 8))),
        r if r <= 65_535 => (13, Some((u64::from(r), 16))),
        r if r % 10 == 0 && r / 10 <= 65_535 => (14, Some((u64::from(r / 10), 16))),
        _ => (0, None),
    }
}

fn bps_code(bps: u32) -> u64 {
    match bps {
        8 => 1,
        12 => 2,
        16 => 4,
        20 => 5,
        24 => 6,
        _ => 0, // from STREAMINFO
    }
}

fn zigzag(r: i64) -> u64 {
    ((r << 1) ^ (r >> 63)) as u64
}

/// Fixed-predictor residuals for `order`, or `None` when any residual leaves the i32 range.
fn fixed_residual(x: &[i64], order: usize) -> Option<Vec<i64>> {
    let mut res = Vec::with_capacity(x.len().saturating_sub(order));
    for n in order..x.len() {
        let g = |k: usize| x.get(n - k).copied().unwrap_or(0);
        let r = match order {
            0 => g(0),
            1 => g(0) - g(1),
            2 => g(0) - 2 * g(1) + g(2),
            3 => g(0) - 3 * g(1) + 3 * g(2) - g(3),
            _ => g(0) - 4 * g(1) + 6 * g(2) - 4 * g(3) + g(4),
        };
        if r < i64::from(i32::MIN) || r > i64::from(i32::MAX) {
            return None;
        }
        res.push(r);
    }
    Some(res)
}

/// Best Rice parameter and its cost in bits for one partition.
fn best_rice(part: &[u64]) -> (u32, u64) {
    if part.is_empty() {
        return (0, 0);
    }
    let sum: u64 = part.iter().sum();
    let mean = sum / part.len() as u64;
    let guess = if mean == 0 { 0 } else { 63 - mean.leading_zeros() };
    let mut best = (0u32, u64::MAX);
    for k in guess.saturating_sub(1)..=(guess + 1).min(30) {
        let cost = part.len() as u64 * u64::from(k + 1) + part.iter().map(|&u| u >> k).sum::<u64>();
        if cost < best.1 {
            best = (k, cost);
        }
    }
    best
}

/// Residual coding plan: partition order, per-partition parameters, total bits.
struct RicePlan {
    order: u32,
    params: Vec<u32>,
    bits: u64,
    wide: bool,
}

fn plan_residual(u: &[u64], block_len: usize, pred_order: usize) -> RicePlan {
    let mut best: Option<RicePlan> = None;
    for p in 0..=MAX_PARTITION_ORDER {
        let parts = 1usize << p;
        if !block_len.is_multiple_of(parts) || block_len / parts <= pred_order {
            break;
        }
        let per = block_len / parts;
        let mut params = Vec::with_capacity(parts);
        let mut bits = 0u64;
        let mut start = 0usize;
        for i in 0..parts {
            let len = if i == 0 { per - pred_order } else { per };
            let part = u.get(start..start + len).unwrap_or_default();
            start += len;
            let (k, cost) = best_rice(part);
            params.push(k);
            bits += cost;
        }
        let wide = params.iter().any(|&k| k > 14);
        bits += 6 + parts as u64 * if wide { 5 } else { 4 };
        if best.as_ref().is_none_or(|b| bits < b.bits) {
            best = Some(RicePlan { order: p, params, bits, wide });
        }
    }
    best.unwrap_or(RicePlan { order: 0, params: vec![0], bits: u64::MAX, wide: false })
}

fn write_residual(w: &mut BitWriter, u: &[u64], block_len: usize, pred_order: usize, plan: &RicePlan) {
    w.write(u64::from(plan.wide), 2);
    w.write(u64::from(plan.order), 4);
    let parts = 1usize << plan.order;
    let per = block_len / parts;
    let mut start = 0usize;
    for (i, &k) in plan.params.iter().enumerate() {
        let len = if i == 0 { per - pred_order } else { per };
        w.write(u64::from(k), if plan.wide { 5 } else { 4 });
        for &v in u.get(start..start + len).unwrap_or_default() {
            w.write_unary(v >> k);
            w.write(v, k);
        }
        start += len;
    }
}

fn write_subframe(w: &mut BitWriter, x: &[i64], bps: u32) {
    let n = x.len();
    let first = x.first().copied().unwrap_or(0);
    if x.iter().all(|&s| s == first) {
        w.write(0, 1);
        w.write(0, 6);
        w.write(0, 1);
        w.write_signed(first, bps);
        return;
    }
    let verbatim_bits = n as u64 * u64::from(bps);
    let mut best: Option<(usize, Vec<u64>, RicePlan)> = None;
    for order in 0..=4usize.min(n.saturating_sub(1)) {
        let Some(res) = fixed_residual(x, order) else { continue };
        let u: Vec<u64> = res.into_iter().map(zigzag).collect();
        let plan = plan_residual(&u, n, order);
        let bits = plan.bits.saturating_add(order as u64 * u64::from(bps));
        if bits < verbatim_bits && best.as_ref().is_none_or(|b| bits < b.2.bits.saturating_add(b.0 as u64 * u64::from(bps))) {
            best = Some((order, u, plan));
        }
    }
    match best {
        Some((order, u, plan)) => {
            w.write(0, 1);
            w.write(8 + order as u64, 6);
            w.write(0, 1);
            for &s in x.iter().take(order) {
                w.write_signed(s, bps);
            }
            write_residual(w, &u, n, order, &plan);
        }
        None => {
            w.write(0, 1);
            w.write(1, 6);
            w.write(0, 1);
            for &s in x {
                w.write_signed(s, bps);
            }
        }
    }
}

/// Checks that FLAC can hold the layout; returns the bits per sample.
fn check(channels: usize, sample_rate: u32, depth: BitDepth) -> Result<u32> {
    crate::pcm::validate_layout(channels, sample_rate)?;
    if channels > 8 {
        return Err(AudioError::Unsupported(format!("FLAC supports at most 8 channels, got {channels}")));
    }
    if sample_rate > 655_350 {
        return Err(AudioError::Unsupported(format!("FLAC sample rate {sample_rate} exceeds 655350 Hz")));
    }
    match depth {
        BitDepth::Int16 => Ok(16),
        BitDepth::Int24 => Ok(24),
        BitDepth::Int32 => Ok(32),
        BitDepth::Float32 => Err(AudioError::Unsupported("FLAC cannot store 32-bit float samples".into())),
    }
}

fn stream_info(channels: usize, sample_rate: u32, bps: u32, total: u64) -> Vec<u8> {
    let mut si = BitWriter::new();
    si.write(1, 1); // last metadata block
    si.write(0, 7); // STREAMINFO
    si.write(34, 24);
    si.write(BLOCK_SIZE as u64, 16);
    si.write(BLOCK_SIZE as u64, 16);
    si.write(0, 24);
    si.write(0, 24);
    si.write(u64::from(sample_rate), 20);
    si.write(channels as u64 - 1, 3);
    si.write(u64::from(bps - 1), 5);
    si.write(total >> 32, 4);
    si.write(total & 0xFFFF_FFFF, 32);
    for _ in 0..4 {
        si.write(0, 32); // MD5 unknown
    }
    si.into_bytes()
}

/// A FLAC stream written block by block; [`Writer::finish`] fills in the total sample count.
pub(crate) struct Writer<W> {
    out: W,
    start: u64,
    channels: usize,
    sample_rate: u32,
    bps: u32,
    quantizers: Vec<Quantizer>,
    pending: Vec<Vec<i64>>,
    frames: u64,
    frame_no: u64,
    /// Set when a frame write has failed: `write_frame` clears the pending samples before
    /// writing, so a failed frame is lost while `frames` still counts it; the writer refuses
    /// every later call.
    failed: bool,
}

impl<W: Write + Seek> Writer<W> {
    pub(crate) fn new(mut out: W, channels: usize, sample_rate: u32, depth: BitDepth, dither: bool) -> Result<Self> {
        let bps = check(channels, sample_rate, depth)?;
        let start = out.stream_position()?;
        out.write_all(b"fLaC")?;
        out.write_all(&stream_info(channels, sample_rate, bps, 0))?;
        Ok(Writer {
            out,
            start,
            channels,
            sample_rate,
            bps,
            quantizers: Quantizer::per_channel(bps, dither, channels),
            pending: (0..channels).map(|_| Vec::with_capacity(BLOCK_SIZE)).collect(),
            frames: 0,
            frame_no: 0,
            failed: false,
        })
    }

    /// The frame count after `frames` more, or an error past what FLAC can count (2^36 frames).
    pub(crate) fn check_room(&self, frames: u64) -> Result<u64> {
        let total = self.frames.saturating_add(frames);
        if total >= 1u64 << 36 {
            return Err(AudioError::TooLarge("too many frames for FLAC".into()));
        }
        Ok(total)
    }

    /// Appends `frames` frames of planar audio (missing samples are silence), unless they don't
    /// fit (see [`Writer::check_room`]).
    pub(crate) fn write(&mut self, block: &[Vec<f32>], frames: usize) -> Result<()> {
        if self.failed {
            return Err(crate::failed_write_error());
        }
        let total = self.check_room(frames as u64)?;
        let mut done = 0;
        while done < frames {
            let held = self.pending.first().map_or(0, Vec::len);
            let n = (BLOCK_SIZE - held).min(frames - done);
            for (c, (p, q)) in self.pending.iter_mut().zip(self.quantizers.iter_mut()).enumerate() {
                let src = block.get(c).and_then(|ch| ch.get(done..)).unwrap_or_default();
                p.extend((0..n).map(|i| i64::from(q.quantize(src.get(i).copied().unwrap_or(0.0)))));
            }
            done += n;
            if held + n == BLOCK_SIZE
                && let Err(e) = self.write_frame()
            {
                self.failed = true;
                return Err(e);
            }
        }
        self.frames = total;
        Ok(())
    }

    fn write_frame(&mut self) -> Result<()> {
        let len = self.pending.first().map_or(0, Vec::len);
        if len == 0 {
            return Ok(());
        }
        let (sr_code, sr_extra) = sample_rate_code(self.sample_rate);
        let mut w = BitWriter::new();
        w.write(0x3FFE, 14);
        w.write(0, 1);
        w.write(0, 1); // fixed blocksize stream
        w.write(7, 4); // 16-bit (blocksize-1) follows
        w.write(sr_code, 4);
        w.write(self.channels as u64 - 1, 4);
        w.write(bps_code(self.bps), 3);
        w.write(0, 1);
        write_utf8_number(&mut w, self.frame_no);
        w.write(len as u64 - 1, 16);
        if let Some((v, n)) = sr_extra {
            w.write(v, n);
        }
        let header = w.into_bytes();
        let mut w = BitWriter::new();
        for &b in &header {
            w.write(u64::from(b), 8);
        }
        w.write(u64::from(crc8(&header)), 8);
        for p in &mut self.pending {
            write_subframe(&mut w, p, self.bps);
            p.clear();
        }
        let mut frame = w.into_bytes();
        let crc = crc16(&frame);
        frame.extend_from_slice(&crc.to_be_bytes());
        self.out.write_all(&frame)?;
        self.frame_no += 1;
        Ok(())
    }

    pub(crate) fn finish(mut self) -> Result<W> {
        if self.failed {
            return Err(crate::failed_write_error());
        }
        self.write_frame()?;
        let info = stream_info(self.channels, self.sample_rate, self.bps, self.frames);
        self.out.seek(SeekFrom::Start(self.start.saturating_add(4)))?;
        self.out.write_all(&info)?;
        self.out.seek(SeekFrom::End(0))?;
        self.out.flush()?;
        Ok(self.out)
    }
}

pub(crate) fn encode(buf: &AudioBuffer, depth: BitDepth, dither: bool) -> Result<Vec<u8>> {
    validate_buffer(buf)?;
    let bps = check(buf.channels.len(), buf.sample_rate, depth)?;
    let frames = buf.frames();
    let mut out = Vec::new();
    out.try_reserve(frames.saturating_mul(buf.channels.len()).saturating_mul(bps as usize / 8) / 2)
        .map_err(|_| AudioError::TooLarge("cannot allocate FLAC output".into()))?;
    let mut w = Writer::new(Cursor::new(out), buf.channels.len(), buf.sample_rate, depth, dither)?;
    w.write(&buf.channels, frames)?;
    Ok(w.finish()?.into_inner())
}

/// Refuse the FLAC stream at `at` if a comment or picture field claims more bytes than its
/// metadata block holds: symphonia allocates each field's length before reading it.
pub(crate) fn check_metadata(b: &[u8], at: usize) -> Result<()> {
    let mut pos = at + 4;
    while let Some(head) = be_u32(b, pos) {
        let (body, len) = (pos + 4, (head & 0x00ff_ffff) as usize);
        let end = (body + len).min(b.len());
        let overruns = match (head >> 24) & 0x7f {
            4 => comments_overrun(b, body, end),
            // Picture: type, MIME type, description, four size fields, data.
            6 => overruns(b, body + 4, end, be_u32, [0, 0, 16]),
            _ => false,
        };
        if overruns {
            return Err(AudioError::Malformed("a FLAC metadata field is larger than its block".into()));
        }
        if head >> 31 == 1 {
            break;
        }
        pos = body + len;
    }
    Ok(())
}

/// The info of a FLAC stream with no audio frames at all: STREAMINFO declares 0 samples and
/// nothing follows the metadata blocks. Symphonia's reader fails on these, as it looks for a first
/// frame, so the caller answers them itself.
pub(crate) fn empty_stream_info(b: &[u8], at: usize) -> Option<AudioInfo> {
    let mut pos = at.checked_add(4)?;
    let mut first = true;
    loop {
        let head = be_u32(b, pos)?;
        let (body, len) = (pos.checked_add(4)?, (head & 0x00ff_ffff) as usize);
        let end = body.checked_add(len)?;
        if end > b.len() {
            return None;
        }
        if first && ((head >> 24) & 0x7f != 0 || len != 34) {
            return None;
        }
        first = false;
        if head >> 31 == 1 {
            if end != b.len() {
                return None;
            }
            break;
        }
        pos = end;
    }
    // STREAMINFO, from the sample rate on: 20 bits rate, 3 channels - 1, 5 bits per sample - 1, 36 bits total.
    let si = at.checked_add(4 + 4 + 10)?;
    let packed = u64::from_be_bytes(b.get(si..si.checked_add(8)?)?.try_into().ok()?);
    let sample_rate = (packed >> 44) as u32;
    let channels = ((packed >> 41) & 0x7) as u16 + 1;
    let bps = ((packed >> 36) & 0x1f) as u32 + 1;
    if packed & 0xf_ffff_ffff != 0 || sample_rate == 0 {
        return None;
    }
    let sample_format = match bps {
        1..=8 => SampleFormat::Int8,
        9..=16 => SampleFormat::Int16,
        17..=24 => SampleFormat::Int24,
        _ => SampleFormat::Int32,
    };
    Some(AudioInfo { format: FileFormat::Flac, sample_format, sample_rate, channels, frames: 0, bwf: None })
}

/// Whether a vendor string or comment in a (little-endian) Vorbis comment block claims more than
/// the block holds.
fn comments_overrun(b: &[u8], body: usize, end: usize) -> bool {
    let Some(vendor) = le_u32(b, body).filter(|_| body.saturating_add(4) <= end) else { return false };
    if vendor as usize > end - (body + 4) {
        return true;
    }
    let at = (body + 4).saturating_add(vendor as usize);
    let Some(count) = le_u32(b, at).filter(|_| at.saturating_add(4) <= end) else { return false };
    overruns(b, at + 4, end, le_u32, std::iter::repeat_n(0, count as usize))
}

/// Whether one of the length-prefixed fields from `at` claims more than `end` leaves; `gaps` are
/// the bytes skipped before each length, which `read` reads.
fn overruns(b: &[u8], mut at: usize, end: usize, read: fn(&[u8], usize) -> Option<u32>, gaps: impl IntoIterator<Item = usize>) -> bool {
    for gap in gaps {
        at = at.saturating_add(gap);
        let Some(n) = read(b, at).filter(|_| at.saturating_add(4) <= end) else { return false };
        at += 4;
        if n as usize > end - at {
            return true;
        }
        at += n as usize;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc_known_values() {
        // CRC-8/SMBUS and CRC-16/UMTS (FLAC's polynomials) of "123456789".
        assert_eq!(crc8(b"123456789"), 0xF4);
        assert_eq!(crc16(b"123456789"), 0xFEE8);
    }

    #[test]
    fn utf8_numbers() {
        let enc = |v| {
            let mut w = BitWriter::new();
            write_utf8_number(&mut w, v);
            w.into_bytes()
        };
        assert_eq!(enc(0x7F), vec![0x7F]);
        assert_eq!(enc(0x80), vec![0xC2, 0x80]);
        assert_eq!(enc(0x800), vec![0xE0, 0xA0, 0x80]);
    }
}
