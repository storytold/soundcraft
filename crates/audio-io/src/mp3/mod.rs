//! A small pure-Rust MP3 encoder (ISO/IEC 11172-3 MPEG-1 Layer III): constant bitrate, long
//! blocks, joint (mid/side) stereo, a bit reservoir, scalefactor noise shaping against a simple
//! masking estimate, and an `Info` header with the frame and byte counts. Other session rates are
//! resampled to 44.1 or 48 kHz. Decoding is symphonia's.
#![allow(clippy::needless_range_loop)]

mod analysis;
mod huffman;
mod quantize;
mod resample;
mod tables;

use crate::pcm::{clamp_float, validate_layout};
use crate::{AudioBuffer, AudioError, Result};
use analysis::{Analysis, Bank, GRANULE};
use quantize::{Granule, Quantizer, Work};
use resample::Resampler;
use std::collections::VecDeque;
use std::io::{Cursor, Seek, SeekFrom, Write};

/// Bitrates (kbps) by header index 1–14.
pub(crate) const BITRATES: [u32; 14] = [32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320];
/// The bitrate used when none is given.
pub(crate) const DEFAULT_KBPS: u32 = 320;
/// Sample rates by header index.
const RATES: [u32; 3] = [44_100, 48_000, 32_000];
/// Samples per channel in a frame.
const FRAME: usize = 2 * GRANULE;
/// Silence appended at the end so the decoder's delay doesn't cut the last samples off.
const TAIL: usize = GRANULE + 529;
/// The largest `main_data_begin` (bytes of earlier frames a frame's data may start in).
const MAX_RESERVOIR: u64 = 511;
/// Level applied before the filterbank so a decoder reproduces the input level (the MDCT and the
/// decoder's IMDCT are unnormalised, together a gain of 9).
const INPUT_GAIN: f32 = 1.0 / 9.0;

/// Bits written most significant first.
pub(super) struct Bits {
    out: Vec<u8>,
    acc: u64,
    n: u32,
}

impl Bits {
    fn new() -> Self {
        Bits { out: Vec::new(), acc: 0, n: 0 }
    }

    /// Writes the low `n` (at most 32) bits of `v`.
    pub(super) fn put(&mut self, v: u32, n: u32) {
        if n == 0 {
            return;
        }
        let n = n.min(32);
        self.acc = (self.acc << n) | (u64::from(v) & ((1u64 << n) - 1));
        self.n += n;
        while self.n >= 8 {
            self.n -= 8;
            self.out.push((self.acc >> self.n) as u8);
        }
        self.acc &= (1u64 << self.n) - 1;
    }

    fn bits(&self) -> u64 {
        self.out.len() as u64 * 8 + u64::from(self.n)
    }

    /// The bytes, the last one padded with zero bits.
    fn into_bytes(mut self) -> Vec<u8> {
        if self.n > 0 {
            self.out.push((self.acc << (8 - self.n)) as u8);
        }
        self.out
    }
}

/// The header index of `kbps`, or an error naming the bitrates MP3 has.
fn bitrate_index(kbps: u32) -> Result<u32> {
    BITRATES.iter().position(|&b| b == kbps).map(|i| i as u32 + 1).ok_or_else(|| {
        let list: Vec<String> = BITRATES.iter().map(u32::to_string).collect();
        AudioError::Unsupported(format!("MP3 bitrate {kbps} kbps (use one of {})", list.join(", ")))
    })
}

/// The MPEG-1 rate a session rate is written at: itself when MP3 has it, else 44.1 kHz for the
/// 44.1 kHz family and 48 kHz for everything else.
fn mp3_rate(sample_rate: u32) -> u32 {
    if RATES.contains(&sample_rate) {
        sample_rate
    } else if sample_rate.is_multiple_of(11_025) {
        44_100
    } else {
        48_000
    }
}

/// The lowpass for `kbps` per channel: the fewer the bits, the less treble is worth coding.
fn lowpass_hz(kbps_per_channel: u32, sample_rate: u32) -> f32 {
    const CURVE: [(f32, f32); 12] = [
        (16.0, 5_000.0),
        (24.0, 7_000.0),
        (32.0, 9_500.0),
        (40.0, 11_500.0),
        (48.0, 13_500.0),
        (56.0, 15_000.0),
        (64.0, 16_000.0),
        (80.0, 17_000.0),
        (96.0, 18_000.0),
        (112.0, 19_000.0),
        (128.0, 19_500.0),
        (160.0, 20_000.0),
    ];
    let k = kbps_per_channel as f32;
    let hz = CURVE
        .windows(2)
        .find(|w| k <= w[1].0)
        .map_or(CURVE[CURVE.len() - 1].1, |w| if k <= w[0].0 { w[0].1 } else { w[0].1 + (w[1].1 - w[0].1) * (k - w[0].0) / (w[1].0 - w[0].0) });
    hz.min(sample_rate as f32 * 0.5 * 0.95)
}

/// A frame waiting for later frames' main data to fill the free end of its slot.
struct Pending {
    bytes: Vec<u8>,
    /// Where the main-data slot starts in `bytes`, and in the stream of all slots.
    slot_at: usize,
    slot_start: u64,
}

/// The frames written so far whose slots may still receive main data (the bit reservoir).
#[derive(Default)]
struct Reservoir {
    pending: VecDeque<Pending>,
    /// Where the next main data goes, and the end of all slots so far.
    fill: u64,
    end: u64,
}

impl Reservoir {
    /// Bytes of free slot the next frame's main data can begin in.
    fn free(&mut self) -> u64 {
        // Free space more than `main_data_begin` can reach back to stays unused.
        self.fill = self.fill.max(self.end.saturating_sub(MAX_RESERVOIR));
        self.end - self.fill
    }

    /// Adds a frame (header and side info) with a slot of `slot` bytes, and writes `main` from the
    /// start of the free space. `main` must fit in [`Reservoir::free`] plus `slot`.
    fn push(&mut self, head: Vec<u8>, slot: usize, main: &[u8]) {
        let slot_at = head.len();
        let mut bytes = head;
        bytes.resize(slot_at + slot, 0);
        self.pending.push_back(Pending { bytes, slot_at, slot_start: self.end });
        self.end += slot as u64;
        let mut at = self.fill;
        let mut rest = main;
        for p in &mut self.pending {
            let slot_end = p.slot_start + (p.bytes.len() - p.slot_at) as u64;
            if rest.is_empty() || at >= slot_end {
                continue;
            }
            let from = p.slot_at + (at - p.slot_start) as usize;
            let n = rest.len().min(p.bytes.len() - from);
            if let (Some(dst), Some(src)) = (p.bytes.get_mut(from..from + n), rest.get(..n)) {
                dst.copy_from_slice(src);
            }
            rest = rest.get(n..).unwrap_or_default();
            at += n as u64;
        }
        self.fill = at;
    }

    /// Writes the frames no later main data can reach (all of them when `all`).
    fn flush<W: Write>(&mut self, out: &mut W, all: bool) -> Result<()> {
        while let Some(p) = self.pending.front() {
            let slot_end = p.slot_start + (p.bytes.len() - p.slot_at) as u64;
            if !all && slot_end > self.fill.max(self.end.saturating_sub(MAX_RESERVOIR)) {
                break;
            }
            if let Some(p) = self.pending.pop_front() {
                out.write_all(&p.bytes)?;
            }
        }
        Ok(())
    }
}

/// The MP3 encoder proper, at an MPEG-1 rate.
struct Encoder {
    channels: usize,
    sample_rate: u32,
    rate_index: u32,
    bitrate_index: u32,
    kbps: u32,
    bank: Box<Bank>,
    analysis: Vec<Box<Analysis>>,
    quant: Quantizer,
    work: Box<Work>,
    /// The quantized granules of the frame being built, `[granule][channel]`.
    granules: [[Box<Granule>; 2]; 2],
    /// Remainder of the padding accumulator (frames are 144 × bitrate / rate bytes, plus one now and
    /// then at 44.1 kHz).
    padding: u64,
    reservoir: Reservoir,
    /// Audio frames written (not counting the `Info` frame).
    frames: u64,
}

impl Encoder {
    fn new(channels: usize, sample_rate: u32, kbps: u32) -> Result<Self> {
        let rate_index = RATES.iter().position(|&r| r == sample_rate).ok_or_else(|| AudioError::Unsupported(format!("MP3 at {sample_rate} Hz")))?;
        let sfb = tables::SFB_LONG[rate_index];
        Ok(Encoder {
            channels,
            sample_rate,
            rate_index: rate_index as u32,
            bitrate_index: bitrate_index(kbps)?,
            kbps,
            bank: Bank::new(),
            analysis: (0..channels).map(|_| Analysis::new()).collect(),
            quant: Quantizer::new(sfb, sample_rate, lowpass_hz(kbps / channels as u32, sample_rate)),
            work: Work::new(),
            granules: [[Granule::new(), Granule::new()], [Granule::new(), Granule::new()]],
            padding: 0,
            reservoir: Reservoir::default(),
            frames: 0,
        })
    }

    fn side_info_len(&self) -> usize {
        if self.channels == 1 { 17 } else { 32 }
    }

    /// The length of the next frame in bytes, and whether it is padded.
    fn next_frame_len(&mut self) -> (usize, bool) {
        let num = 144_000 * u64::from(self.kbps);
        let rate = u64::from(self.sample_rate);
        self.padding += num % rate;
        let pad = self.padding >= rate;
        if pad {
            self.padding -= rate;
        }
        ((num / rate) as usize + usize::from(pad), pad)
    }

    /// The four header bytes.
    fn header(&self, pad: bool, mid_side: bool) -> Vec<u8> {
        let mut h = Bits::new();
        h.put(0x7FF, 11); // sync
        h.put(0b11, 2); // MPEG-1
        h.put(0b01, 2); // Layer III
        h.put(1, 1); // no CRC
        h.put(self.bitrate_index, 4);
        h.put(self.rate_index, 2);
        h.put(u32::from(pad), 1);
        h.put(0, 1); // private
        h.put(if self.channels == 1 { 0b11 } else { 0b01 }, 2); // mono or joint stereo
        h.put(if mid_side { 0b10 } else { 0 }, 2);
        h.put(0, 1); // copyright
        h.put(0, 1); // original
        h.put(0, 2); // no emphasis
        h.into_bytes()
    }

    /// The `Info` frame that heads the file; [`Encoder::info_counts`] fills in its counts.
    fn info_frame(&mut self) -> Vec<u8> {
        let (len, pad) = self.next_frame_len();
        let mut f = self.header(pad, false);
        f.resize(4 + self.side_info_len(), 0);
        f.extend_from_slice(b"Info");
        f.extend_from_slice(&3u32.to_be_bytes()); // frame and byte counts follow
        f.resize(len, 0);
        f
    }

    /// Where the `Info` frame's counts start.
    fn info_counts(&self) -> u64 {
        (4 + self.side_info_len() + 8) as u64
    }

    /// Encodes one frame of `FRAME` samples per channel and queues it.
    fn frame(&mut self, pcm: &[Vec<f32>]) {
        let (len, pad) = self.next_frame_len();
        let side = self.side_info_len();
        let slot = len - 4 - side;
        let free = self.reservoir.free();

        let mut xr = [[[0.0f32; GRANULE]; 2]; 2];
        let mut input = [0.0f32; GRANULE];
        for gr in 0..2 {
            for ch in 0..self.channels {
                for (i, x) in input.iter_mut().enumerate() {
                    *x = pcm.get(ch).and_then(|c| c.get(gr * GRANULE + i)).copied().unwrap_or(0.0) * INPUT_GAIN;
                }
                self.analysis[ch].granule(&self.bank, &input, &mut xr[gr][ch]);
            }
        }
        // Mid/side unless the channels are mostly out of phase.
        let mid_side = self.channels == 2 && {
            let (mut mid, mut side) = (0.0f32, 0.0f32);
            for g in &xr {
                for (l, r) in g[0].iter().zip(&g[1]) {
                    mid += (l + r) * (l + r);
                    side += (l - r) * (l - r);
                }
            }
            side <= mid
        };
        if mid_side {
            for g in &mut xr {
                let [l, r] = g;
                for (l, r) in l.iter_mut().zip(r.iter_mut()) {
                    let (m, s) = ((*l + *r) * std::f32::consts::FRAC_1_SQRT_2, (*l - *r) * std::f32::consts::FRAC_1_SQRT_2);
                    *l = m;
                    *r = s;
                }
            }
        }

        // Each granule gets its share of the frame plus half of what the reservoir holds then, and
        // spends at least what the reservoir couldn't keep for later.
        let mean = (slot * 8 / 2) as i64;
        let mut avail = ((free as usize + slot) * 8) as i64;
        for gr in 0..2 {
            let masks: Vec<_> = (0..self.channels).map(|ch| self.quant.mask(&mut xr[gr][ch])).collect();
            let later = mean * (1 - gr as i64);
            let mut budget = (mean + (avail - later - mean).max(0) / 2).min(avail);
            let mut floor = (avail - later - MAX_RESERVOIR as i64 * 8).max(0);
            let total_pe: f32 = masks.iter().map(|m| m.pe.max(1.0)).sum();
            for ch in 0..self.channels {
                let share = if ch + 1 == self.channels { 1.0 } else { (masks[ch].pe.max(1.0) / total_pe).clamp(0.2, 0.8) };
                let max = ((budget as f32 * share) as i64).clamp(0, 4095) as u32;
                let min = ((floor as f32 * share) as i64).clamp(0, i64::from(max)) as u32;
                let g = &mut self.granules[gr][ch];
                self.quant.quantize(&xr[gr][ch], &masks[ch], min, max, &mut self.work, g);
                budget -= i64::from(g.bits());
                floor -= i64::from(g.bits());
                avail -= i64::from(g.bits());
            }
        }

        let mut main = Bits::new();
        for gr in 0..2 {
            for ch in 0..self.channels {
                let g = &self.granules[gr][ch];
                let (s1, s2) = tables::SLEN[g.scalefac_compress as usize];
                for (b, &sf) in g.sf.iter().enumerate() {
                    main.put(u32::from(sf), if b < 11 { s1 } else { s2 });
                }
                huffman::write(&mut main, &g.ix, &g.neg, &g.coding, &self.quant.sfb);
            }
        }
        debug_assert_eq!(main.bits(), self.granules.iter().flat_map(|g| g[..self.channels].iter()).map(|g| u64::from(g.bits())).sum::<u64>());

        let mut head = Bits::new();
        head.put(free as u32, 9); // main_data_begin
        head.put(0, if self.channels == 1 { 5 } else { 3 }); // private bits
        head.put(0, 4 * self.channels as u32); // scfsi
        for gr in 0..2 {
            for ch in 0..self.channels {
                let g = &self.granules[gr][ch];
                head.put(g.bits(), 12);
                head.put(g.coding.big_values as u32, 9);
                head.put(g.global_gain, 8);
                head.put(g.scalefac_compress, 4);
                head.put(0, 1); // long blocks
                for t in g.coding.table_select {
                    head.put(t, 5);
                }
                head.put(g.coding.region0_count, 4);
                head.put(g.coding.region1_count, 3);
                head.put(0, 1); // preflag
                head.put(0, 1); // scalefac_scale
                head.put(u32::from(g.coding.count1_b), 1);
            }
        }
        let mut bytes = self.header(pad, mid_side);
        bytes.extend(head.into_bytes());
        self.reservoir.push(bytes, slot, &main.into_bytes());
        self.frames += 1;
    }
}

/// An MP3 file written block by block; [`Writer::finish`] fills in the `Info` frame's counts.
pub(crate) struct Writer<W> {
    out: W,
    start: u64,
    enc: Box<Encoder>,
    from_rate: u32,
    resampler: Option<Resampler>,
    /// Input not yet encoded, per channel, at the MP3 rate.
    pcm: Vec<Vec<f32>>,
    /// Input frames received.
    received: u64,
}

impl<W: Write + Seek> Writer<W> {
    /// Starts a constant-bitrate MP3 of `kbps` (one of [`BITRATES`]; `None` is
    /// [`DEFAULT_KBPS`]). MP3 holds one or two channels.
    pub(crate) fn new(mut out: W, channels: usize, sample_rate: u32, kbps: Option<u32>) -> Result<Self> {
        validate_layout(channels, sample_rate)?;
        if channels > 2 {
            return Err(AudioError::Unsupported(format!("MP3 holds mono or stereo, not {channels} channels")));
        }
        let rate = mp3_rate(sample_rate);
        let mut enc = Box::new(Encoder::new(channels, rate, kbps.unwrap_or(DEFAULT_KBPS))?);
        let start = out.stream_position()?;
        out.write_all(&enc.info_frame())?;
        Ok(Writer {
            out,
            start,
            enc,
            from_rate: sample_rate,
            resampler: (rate != sample_rate).then(|| Resampler::new(channels, sample_rate, rate)),
            pcm: vec![Vec::with_capacity(2 * FRAME); channels],
            received: 0,
        })
    }

    /// The frame count after `frames` more, or an error past what the `Info` frame can count
    /// (a 4 GiB file).
    pub(crate) fn check_room(&self, frames: u64) -> Result<u64> {
        let total = self.received.saturating_add(frames);
        let at_rate = (u128::from(total) * u128::from(self.enc.sample_rate)).div_ceil(u128::from(self.from_rate));
        let mp3_frames = at_rate / FRAME as u128 + 3;
        let bytes = mp3_frames * (144_000 * u128::from(self.enc.kbps) / u128::from(self.enc.sample_rate) + 1);
        if bytes > u128::from(u32::MAX) {
            return Err(AudioError::TooLarge("too long for an MP3 file".into()));
        }
        Ok(total)
    }

    /// Appends `frames` frames of planar audio (missing samples are silence), unless they don't
    /// fit (see [`Writer::check_room`]).
    pub(crate) fn write(&mut self, block: &[Vec<f32>], frames: usize) -> Result<()> {
        let total = self.check_room(frames as u64)?;
        let clamped: Vec<Vec<f32>> = (0..self.enc.channels)
            .map(|c| {
                let src = block.get(c).map_or(&[][..], |b| b.get(..frames.min(b.len())).unwrap_or_default());
                let mut v: Vec<f32> = src.iter().map(|&x| clamp_float(x)).collect();
                v.resize(frames, 0.0);
                v
            })
            .collect();
        match &mut self.resampler {
            Some(r) => r.push(&clamped, frames, &mut self.pcm),
            None => {
                for (p, c) in self.pcm.iter_mut().zip(clamped) {
                    p.extend(c);
                }
            }
        }
        self.received = total;
        self.encode_ready()?;
        self.enc.reservoir.flush(&mut self.out, false)
    }

    /// Encodes every whole frame of pending input.
    fn encode_ready(&mut self) -> Result<()> {
        let mut at = 0;
        while self.pcm.first().is_some_and(|p| p.len() >= at + FRAME) {
            let frame: Vec<Vec<f32>> = self.pcm.iter().map(|p| p.get(at..at + FRAME).unwrap_or_default().to_vec()).collect();
            self.enc.frame(&frame);
            at += FRAME;
        }
        for p in &mut self.pcm {
            p.drain(..at.min(p.len()));
        }
        Ok(())
    }

    /// Encodes the rest, completes the `Info` frame and hands back `out`, positioned at the end
    /// of the file.
    pub(crate) fn finish(mut self) -> Result<W> {
        if let Some(r) = &mut self.resampler {
            r.finish(&mut self.pcm);
        }
        let held = self.pcm.first().map_or(0, Vec::len) + TAIL;
        let padded = held.div_ceil(FRAME) * FRAME;
        for p in &mut self.pcm {
            p.resize(padded, 0.0);
        }
        self.encode_ready()?;
        self.enc.reservoir.flush(&mut self.out, true)?;
        let end = self.out.stream_position()?;
        let frames = u32::try_from(self.enc.frames).map_err(|_| AudioError::TooLarge("too many MP3 frames".into()))?;
        let bytes = u32::try_from(end.saturating_sub(self.start)).map_err(|_| AudioError::TooLarge("too long for an MP3 file".into()))?;
        self.out.seek(SeekFrom::Start(self.start + self.enc.info_counts()))?;
        self.out.write_all(&frames.to_be_bytes())?;
        self.out.write_all(&bytes.to_be_bytes())?;
        self.out.seek(SeekFrom::End(0))?;
        self.out.flush()?;
        Ok(self.out)
    }
}

pub(crate) fn encode(buf: &AudioBuffer, kbps: Option<u32>) -> Result<Vec<u8>> {
    crate::pcm::validate_buffer(buf)?;
    let frames = buf.frames();
    let mut w = Writer::new(Cursor::new(Vec::new()), buf.channels.len(), buf.sample_rate, kbps)?;
    w.write(&buf.channels, frames)?;
    Ok(w.finish()?.into_inner())
}
